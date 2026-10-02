// Filesystem storage implementation

use super::{FileStorage, StorageResult};
use ziee_core::AppError;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

/// Reject reads that would follow a symlink. If the storage tree
/// somehow contains a symlink (planted by a co-located process, a
/// privilege escalation, or a future bug), refuse to read it rather
/// than silently following to arbitrary host paths. Closes
/// 05-file F-15 (Medium). NotFound is the same shape callers already
/// expect, so no surface-level change.
async fn reject_if_symlink(path: &Path) -> StorageResult<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if meta.file_type().is_symlink() => {
            tracing::error!(
                path = %path.display(),
                "Refusing to read storage path that is a symlink"
            );
            Err(AppError::not_found("File"))
        }
        // ENOENT → propagate as not_found later in the read call.
        // Other errors (permission denied, etc.) → propagate the same.
        _ => Ok(()),
    }
}

/// Where NEW originals are written under `originals/<user>/`.
///
/// Reads and deletes are layout-agnostic: they look in BOTH places
/// ([`FileStorage::resolve_original_path`], [`FileStorage::delete_original`]),
/// so a store can be migrated from `Flat` to `Sharded` while it is serving.
///
/// - `Flat` — `originals/<user>/<id>.<ext>`: one directory per user. Fine for
///   a per-user upload directory; one enormous directory for a store that
///   keeps millions of objects under a single namespace id (every `readdir`
///   tool is O(n) there, and ext4 without `large_dir` caps one directory at
///   roughly 10–15M entries).
/// - `Sharded` — `originals/<user>/<id[0..2]>/<id[2..4]>/<id>.<ext>`, where
///   `id` is the hyphenated UUID string, so the shard key is its first four
///   hex digits: 65 536 leaf directories that fill evenly for v4 ids. **If ids
///   ever become UUIDv7** (time-ordered prefix) the key must come from the
///   random tail instead, or a whole day lands in one leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OriginalsLayout {
    #[default]
    Flat,
    Sharded,
}

/// Knobs for [`FilesystemStorage::shard_flat_originals`].
#[derive(Debug, Clone)]
pub struct ShardOptions {
    /// Moves between progress callbacks and pauses.
    pub batch: usize,
    /// Sleep after each batch, so a backlog of millions of renames does not
    /// starve serving I/O.
    pub pause: std::time::Duration,
    /// Stop after this many objects were moved (a partial run); `None` ⇒
    /// until nothing flat is left.
    pub limit: Option<u64>,
    /// Bound on the number of `read_dir` passes a single call makes. The loop
    /// normally stops when a pass moves nothing, but a sustained concurrent
    /// flat writer (a not-yet-restarted server still writing flat) can keep
    /// adding entries forever, so without a ceiling the call would never
    /// return. `ShardReport::remaining_flat` reports what is still flat when
    /// the bound is hit; a follow-up call continues from there.
    pub max_passes: usize,
}

impl Default for ShardOptions {
    fn default() -> Self {
        Self {
            batch: 2000,
            pause: std::time::Duration::from_millis(50),
            limit: None,
            max_passes: 3,
        }
    }
}

/// What one [`FilesystemStorage::shard_flat_originals`] run did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShardReport {
    /// Flat objects renamed into their shard leaf.
    pub moved: u64,
    /// Flat objects whose shard target already held IDENTICAL bytes — the
    /// flat copy was removed.
    pub deduplicated: u64,
    /// Flat objects whose shard target holds DIFFERENT bytes — both left in
    /// place and logged, for a human to decide (as found by the final pass).
    pub conflicts: u64,
    /// Directory entries that are not a regular `<uuid>.<ext>` object file
    /// (symlinks, foreign names; sub-directories are the shard levels and are
    /// not counted) — never touched (as found by the final pass).
    pub skipped: u64,
    /// Flat object files still present when the run ended (a `limit`ed run,
    /// a conflict, or a racing writer). Zero ⇒ the directory is migrated.
    pub remaining_flat: u64,
}

/// Filesystem-based file storage
#[derive(Debug, Clone)]
pub struct FilesystemStorage {
    base_path: PathBuf,
    layout: OriginalsLayout,
}

impl FilesystemStorage {
    /// Create new filesystem storage (the `Flat` originals layout).
    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self::with_layout(base_path, OriginalsLayout::Flat)
    }

    /// Create filesystem storage that writes NEW originals in `layout`.
    pub fn with_layout(base_path: impl AsRef<Path>, layout: OriginalsLayout) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            layout,
        }
    }

    /// The layout NEW originals are written in.
    pub fn layout(&self) -> OriginalsLayout {
        self.layout
    }

    /// The extension sanitiser every original path goes through, whatever the
    /// layout.
    ///
    /// SECURITY: the extension flows from user input on upload. Without
    /// sanitization, a value like 'x/../../<victim_uuid>/y.pdf' lets
    /// create_dir_all + fs::write escape the per-user originals
    /// directory and overwrite (or shadow) another user's file. Same
    /// primitive on the read path. Closes 05-file F-03 (High).
    ///
    /// Allow only ASCII alphanumeric extensions (matches every legit
    /// mime-type-derived extension). Anything else is replaced with
    /// 'bin', which keeps the file storable but isolated.
    fn safe_extension(extension: &str) -> String {
        if extension.chars().all(|c| c.is_ascii_alphanumeric())
            && !extension.is_empty()
            && extension.len() <= 16
        {
            extension.to_ascii_lowercase()
        } else {
            "bin".to_string()
        }
    }

    /// `originals/<user>/<id>.<ext>` — the `Flat` location.
    pub fn flat_original_path(&self, user_id: Uuid, file_id: Uuid, extension: &str) -> PathBuf {
        self.get_user_path(user_id, "originals")
            .join(format!("{}.{}", file_id, Self::safe_extension(extension)))
    }

    /// `originals/<user>/<id[0..2]>/<id[2..4]>/` — an object's shard leaf.
    fn shard_dir(&self, user_id: Uuid, file_id: Uuid) -> PathBuf {
        let id = file_id.to_string();
        self.get_user_path(user_id, "originals")
            .join(&id[0..2])
            .join(&id[2..4])
    }

    /// `originals/<user>/<id[0..2]>/<id[2..4]>/<id>.<ext>` — the `Sharded`
    /// location.
    pub fn sharded_original_path(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> PathBuf {
        self.shard_dir(user_id, file_id)
            .join(format!("{}.{}", file_id, Self::safe_extension(extension)))
    }

    /// Move every FLAT original of `user_id` into its shard leaf — the one-time
    /// migration of a store written before `Sharded` existed.
    ///
    /// Streams `read_dir` of `originals/<user>/` (the last O(n) pass over the
    /// flat directory) and, for each REGULAR file named `<uuid>.<ext>`,
    /// `rename`s it into its leaf. Anything else — a symlink, a directory (the
    /// shard levels themselves), a foreign name — is never touched. Rename is
    /// atomic within one filesystem, so a crash at any point leaves each object
    /// at exactly one of its two paths, where the dual reader finds it; a
    /// re-run continues with whatever is still flat. A target that already
    /// exists is compared byte for byte: identical ⇒ the flat copy is removed;
    /// different ⇒ both are left and logged.
    ///
    /// A delete racing the move cannot resurrect its object:
    /// [`FileStorage::delete_original`] unlinks the flat name, then the sharded
    /// one, and `rename`/`unlink` of one name are atomic with respect to each
    /// other — either the unlink wins (the rename then fails `ENOENT` and is
    /// skipped) or the rename wins (the sharded unlink, which runs after, hits).
    ///
    /// The target of a move is probed with `symlink_metadata` (never
    /// followed) before anything is read through it: if it exists and is
    /// NOT a regular file — a symlink, a directory, anything else — it is
    /// never opened and the flat copy is never unlinked on its say-so; that
    /// is counted as a conflict and both are left in place. Without this, a
    /// symlink planted at the shard target pointing at bytes identical to
    /// the flat copy would make the byte-compare report "same" and the
    /// mover would unlink the REAL flat object, leaving only the
    /// attacker-controlled symlink behind.
    ///
    /// Bounded by `opts.max_passes`: a sustained concurrent flat writer can
    /// keep adding entries to the directory forever, so without a ceiling
    /// this would never return. `ShardReport::remaining_flat` reports what
    /// is left when the bound is hit (or a `limit`/conflict stopped it
    /// early); a follow-up call continues from there. On an I/O error the
    /// best-effort report accumulated so far is handed to `progress` before
    /// the error propagates, so a caller does not lose a long run's progress
    /// to its last failure.
    pub async fn shard_flat_originals(
        &self,
        user_id: Uuid,
        opts: &ShardOptions,
        progress: &mut (dyn FnMut(&ShardReport) + Send),
    ) -> std::io::Result<ShardReport> {
        let dir = self.get_user_path(user_id, "originals");
        let mut report = ShardReport::default();
        let mut leaves: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let batch = opts.batch.max(1) as u64;
        let mut since_pause = 0u64;
        let max_passes = opts.max_passes.max(1);
        let mut passes = 0usize;
        // Passes until one moves nothing (or the bound is hit): a directory
        // being renamed out of while it is read may skip entries, and a
        // concurrent writer may add flat ones — the next pass picks both up.
        loop {
            passes += 1;
            let mut moved_this_pass = 0u64;
            let mut flat_seen = 0u64;
            // Conflicts and skips are a property of what is in the directory,
            // so each pass recounts them (the last pass's numbers are the
            // report's).
            let mut conflicts = 0u64;
            let mut skipped = 0u64;
            // On any I/O error below, hand `progress` the best-effort report
            // accumulated so far (this pass's counters folded in) before
            // propagating — a long run's partial progress is not silently
            // dropped on its last failure.
            macro_rules! bail {
                ($e:expr) => {{
                    report.remaining_flat = flat_seen;
                    report.conflicts = conflicts;
                    report.skipped = skipped;
                    progress(&report);
                    return Err($e);
                }};
            }
            let mut entries = match fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(report),
                Err(e) => bail!(e),
            };
            loop {
                let entry = match entries.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(e) => bail!(e),
                };
                if opts.limit.is_some_and(|l| report.moved >= l) {
                    // Count what is left without moving it.
                    if Self::flat_object_name(&entry).await.is_some() {
                        flat_seen += 1;
                    }
                    continue;
                }
                let Some((file_id, ext)) = Self::flat_object_name(&entry).await else {
                    match entry.file_type().await {
                        Ok(t) if t.is_dir() => {}
                        _ => skipped += 1,
                    }
                    continue;
                };
                flat_seen += 1;
                let from = entry.path();
                let to = self.sharded_original_path(user_id, file_id, &ext);
                let leaf = self.shard_dir(user_id, file_id);
                if !leaves.contains(&leaf) {
                    if let Err(e) = fs::create_dir_all(&leaf).await {
                        bail!(e);
                    }
                    leaves.insert(leaf);
                }
                match fs::symlink_metadata(&to).await {
                    Ok(meta) if !meta.file_type().is_file() => {
                        // The target exists but is not a regular file.
                        // INV-6: never read through it, never unlink the
                        // flat copy on its say-so.
                        conflicts += 1;
                        tracing::warn!(
                            flat = %from.display(),
                            sharded = %to.display(),
                            "shard move: target exists but is not a regular file \
                             (symlink or other) — left the flat copy in place"
                        );
                        continue;
                    }
                    Ok(_) => {
                        // The FLAT side is already known to be a regular file
                        // (`flat_object_name` filtered it); the sharded side
                        // was just confirmed one above.
                        let same = match (fs::read(&from).await, fs::read(&to).await) {
                            (Ok(a), Ok(b)) => a == b,
                            _ => false,
                        };
                        if same {
                            match fs::remove_file(&from).await {
                                Ok(()) => {
                                    report.deduplicated += 1;
                                    flat_seen -= 1;
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                    flat_seen -= 1;
                                }
                                Err(e) => bail!(e),
                            }
                        } else {
                            conflicts += 1;
                            tracing::warn!(
                                flat = %from.display(),
                                sharded = %to.display(),
                                "shard move: both locations hold DIFFERENT bytes for one \
                                 object — left both in place"
                            );
                        }
                        continue;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => bail!(e),
                }
                match fs::rename(&from, &to).await {
                    Ok(()) => {
                        report.moved += 1;
                        moved_this_pass += 1;
                        flat_seen -= 1;
                    }
                    // Deleted (or moved) under us — nothing left to move.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => flat_seen -= 1,
                    Err(e) => bail!(e),
                }
                since_pause += 1;
                if since_pause >= batch {
                    since_pause = 0;
                    progress(&report);
                    tokio::time::sleep(opts.pause).await;
                }
            }
            report.remaining_flat = flat_seen;
            report.conflicts = conflicts;
            report.skipped = skipped;
            if moved_this_pass == 0
                || opts.limit.is_some_and(|l| report.moved >= l)
                || passes >= max_passes
            {
                break;
            }
        }
        progress(&report);
        Ok(report)
    }

    /// `Some((id, ext))` when `entry` is a REGULAR file named `<uuid>.<ext>`
    /// with an extension the sanitiser keeps as-is — i.e. an object this
    /// storage could have written flat. `file_type` does not follow symlinks.
    async fn flat_object_name(entry: &fs::DirEntry) -> Option<(Uuid, String)> {
        if !entry.file_type().await.ok()?.is_file() {
            return None;
        }
        let name = entry.file_name();
        let name = name.to_str()?;
        let (stem, ext) = name.split_once('.')?;
        let id = Uuid::parse_str(stem).ok()?;
        if id.to_string() != stem || Self::safe_extension(ext) != ext {
            return None;
        }
        Some((id, ext.to_string()))
    }

    /// Ensure directory exists
    async fn ensure_dir(&self, path: &Path) -> StorageResult<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| {
                tracing::error!(error = %e, "ensure_dir failed");
                AppError::internal_error("Storage error")
            })?;
        }
        Ok(())
    }

    /// Get base path for user
    fn get_user_path(&self, user_id: Uuid, subdir: &str) -> PathBuf {
        self.base_path.join(subdir).join(user_id.to_string())
    }
}

#[async_trait]
impl FileStorage for FilesystemStorage {
    async fn save_original(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
        data: &[u8],
    ) -> StorageResult<PathBuf> {
        let path = self.get_original_path(user_id, file_id, extension);
        self.ensure_dir(&path).await?;

        fs::write(&path, data)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "save_original write failed");
                AppError::internal_error("Storage error")
            })?;

        Ok(path)
    }

    async fn save_text_page(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        text: &str,
    ) -> StorageResult<PathBuf> {
        let path = self.get_text_path(user_id, file_id, page_num);
        self.ensure_dir(&path).await?;

        fs::write(&path, text)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "save_text_page write failed");
                AppError::internal_error("Storage error")
            })?;

        Ok(path)
    }

    async fn save_geometry_page(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        geometry_json: &str,
    ) -> StorageResult<PathBuf> {
        let path = self
            .get_user_path(user_id, "geometry")
            .join(file_id.to_string())
            .join(format!("page_{}.json", page_num));
        self.ensure_dir(&path).await?;
        fs::write(&path, geometry_json).await.map_err(|e| {
            tracing::error!(error = %e, "save_geometry_page write failed");
            AppError::internal_error("Storage error")
        })?;
        Ok(path)
    }

    async fn load_geometry_page(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
    ) -> StorageResult<String> {
        let path = self
            .get_user_path(user_id, "geometry")
            .join(file_id.to_string())
            .join(format!("page_{}.json", page_num));
        reject_if_symlink(&path).await?;
        fs::read_to_string(&path).await.map_err(|e| {
            tracing::warn!(error = %e, page = page_num, "load_geometry_page failed");
            AppError::not_found("Geometry page")
        })
    }

    async fn save_image(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        is_thumbnail: bool,
        data: &[u8],
    ) -> StorageResult<PathBuf> {
        let path = self.get_image_path(user_id, file_id, page_num, is_thumbnail);
        self.ensure_dir(&path).await?;

        fs::write(&path, data)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "save_image write failed");
                AppError::internal_error("Storage error")
            })?;

        Ok(path)
    }

    fn get_original_path(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> PathBuf {
        // Both layouts go through `safe_extension` (the F-03 sanitiser).
        match self.layout {
            OriginalsLayout::Flat => self.flat_original_path(user_id, file_id, extension),
            OriginalsLayout::Sharded => self.sharded_original_path(user_id, file_id, extension),
        }
    }

    fn get_text_path(&self, user_id: Uuid, file_id: Uuid, page_num: u32) -> PathBuf {
        self.get_user_path(user_id, "text")
            .join(file_id.to_string())
            .join(format!("page_{}.txt", page_num))
    }

    fn get_image_path(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        is_thumbnail: bool,
    ) -> PathBuf {
        if is_thumbnail {
            // Single thumbnail: thumbnails/{user_id}/{file_id}.jpg
            self.get_user_path(user_id, "thumbnails")
                .join(format!("{}.jpg", file_id))
        } else {
            // Multiple images: images/{user_id}/{file_id}/page_N.jpg
            self.get_user_path(user_id, "images")
                .join(file_id.to_string())
                .join(format!("page_{}.jpg", page_num))
        }
    }

    /// Sharded, else flat, else sharded once more — the last probe closes the
    /// race with a concurrent mover that renamed the object between the first
    /// two. Whatever the write layout, so a store being migrated reads both.
    /// A symlink at a probed location is REFUSED (never followed, never
    /// skipped past): it means the tree was tampered with.
    async fn resolve_original_path(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> Option<PathBuf> {
        let sharded = self.sharded_original_path(user_id, file_id, extension);
        let flat = self.flat_original_path(user_id, file_id, extension);
        for path in [&sharded, &flat, &sharded] {
            match fs::symlink_metadata(path).await {
                Ok(meta) if meta.file_type().is_symlink() => {
                    tracing::error!(
                        path = %path.display(),
                        "Refusing to resolve storage path that is a symlink"
                    );
                    return None;
                }
                Ok(meta) if meta.file_type().is_file() => return Some(path.clone()),
                _ => {}
            }
        }
        None
    }

    /// Flat first, then sharded — the order that makes a racing mover unable
    /// to resurrect the object (see `shard_flat_originals`).
    async fn delete_original(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> std::io::Result<bool> {
        let mut removed = false;
        for path in [
            self.flat_original_path(user_id, file_id, extension),
            self.sharded_original_path(user_id, file_id, extension),
        ] {
            match fs::remove_file(&path).await {
                Ok(()) => removed = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(std::io::Error::new(
                        e.kind(),
                        format!("remove {}: {e}", path.display()),
                    ))
                }
            }
        }
        Ok(removed)
    }

    async fn load_original(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> StorageResult<Vec<u8>> {
        let path = self
            .resolve_original_path(user_id, file_id, extension)
            .await
            .unwrap_or_else(|| self.get_original_path(user_id, file_id, extension));
        reject_if_symlink(&path).await?;
        fs::read(&path)
            .await
            .map_err(|e| {
                tracing::warn!(
                    error = %e,
                    base = %self.base_path.display(),
                    path = %path.display(),
                    %user_id,
                    %file_id,
                    extension,
                    "load_original failed"
                );
                AppError::not_found("File")
            })
    }

    async fn load_text_page(&self, user_id: Uuid, file_id: Uuid, page_num: u32) -> StorageResult<String> {
        let path = self.get_text_path(user_id, file_id, page_num);
        reject_if_symlink(&path).await?;
        fs::read_to_string(&path)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, page = page_num, "load_text_page failed");
                AppError::not_found("Text page")
            })
    }

    async fn load_preview(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
    ) -> StorageResult<Vec<u8>> {
        let path = self.get_image_path(user_id, file_id, page_num, false);
        reject_if_symlink(&path).await?;
        fs::read(&path)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "load_preview failed");
                AppError::not_found("Preview")
            })
    }

    async fn load_thumbnail(&self, user_id: Uuid, file_id: Uuid) -> StorageResult<Vec<u8>> {
        let path = self.get_image_path(user_id, file_id, 1, true);
        reject_if_symlink(&path).await?;
        fs::read(&path)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "load_thumbnail failed");
                AppError::not_found("Thumbnail")
            })
    }

    async fn delete_all(&self, user_id: Uuid, file_id: Uuid) -> StorageResult<()> {
        // Delete from all possible locations
        let locations = vec![
            ("originals", None),
            ("text", Some(file_id.to_string())),   // Directory with text pages
            ("images", Some(file_id.to_string())), // Directory with image pages
        ];

        for (subdir, file_subdir) in locations {
            let mut path = self.get_user_path(user_id, subdir);
            if let Some(ref subdir_name) = file_subdir {
                path = path.join(subdir_name);
                // Delete entire directory
                if path.exists() {
                    let _ = fs::remove_dir_all(&path).await;
                }
            } else {
                // Delete files matching pattern — in the flat directory and in
                // the id's shard leaf (a sharded or migrated object).
                for dir in [path.clone(), self.shard_dir(user_id, file_id)] {
                    if let Ok(mut entries) = fs::read_dir(&dir).await {
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            if let Some(name) = entry.file_name().to_str()
                                && name.starts_with(&file_id.to_string()) {
                                    let _ = fs::remove_file(entry.path()).await;
                                }
                        }
                    }
                }
            }
        }

        // Delete single thumbnail file: thumbnails/{user_id}/{file_id}.jpg
        let thumbnail_path = self.get_user_path(user_id, "thumbnails")
            .join(format!("{}.jpg", file_id));
        if thumbnail_path.exists() {
            let _ = fs::remove_file(&thumbnail_path).await;
        }

        Ok(())
    }

    async fn delete_user_dirs(&self, user_id: Uuid) -> StorageResult<()> {
        // Remove the per-user directory under every storage subdir so deleting
        // a user leaves no orphaned (even empty) dirs behind.
        for subdir in ["originals", "text", "images", "thumbnails"] {
            let path = self.get_user_path(user_id, subdir);
            if path.exists() {
                let _ = fs::remove_dir_all(&path).await;
            }
        }
        Ok(())
    }

    fn calculate_checksum(&self, data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        hex::encode(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> (tempfile::TempDir, FilesystemStorage) {
        let dir = tempfile::tempdir().unwrap();
        let s = FilesystemStorage::new(dir.path());
        (dir, s)
    }

    #[tokio::test]
    async fn save_then_load_original_roundtrips_bytes() {
        let (_dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();
        let data = b"hello core file bytes";

        let path = s.save_original(user, file, "txt", data).await.unwrap();
        assert!(path.exists(), "saved file must exist on disk");

        let loaded = s.load_original(user, file, "txt").await.unwrap();
        assert_eq!(loaded, data, "load must return exactly the saved bytes");
    }

    #[tokio::test]
    async fn load_missing_original_is_not_found() {
        let (_dir, s) = storage();
        let res = s
            .load_original(Uuid::new_v4(), Uuid::new_v4(), "txt")
            .await;
        assert!(res.is_err(), "loading a nonexistent file must error");
    }

    #[tokio::test]
    async fn delete_all_removes_the_original() {
        let (_dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();
        s.save_original(user, file, "txt", b"x").await.unwrap();

        s.delete_all(user, file).await.unwrap();

        assert!(
            s.load_original(user, file, "txt").await.is_err(),
            "the original must be gone after delete_all"
        );
    }

    #[test]
    fn calculate_checksum_is_sha256_hex() {
        let (_dir, s) = storage();
        // Known vector: sha256("hello").
        assert_eq!(
            s.calculate_checksum(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    /// Security (F-03): the upload-supplied `extension` feeds
    /// `get_original_path`; without sanitization a value containing path
    /// separators / `..` would let create_dir_all + write escape the per-user
    /// `originals/` dir and clobber another user's file. `safe_ext` must
    /// collapse any non-ASCII-alnum / empty / >16-char extension to `bin`,
    /// confining the result under the user's originals dir.
    #[test]
    fn get_original_path_sanitizes_traversal_extensions() {
        let (_dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();

        let originals_dir = s.get_user_path(user, "originals");
        let expected_bin = originals_dir.join(format!("{}.bin", file));

        // Every hostile / malformed extension collapses to `.bin` under the
        // user's own originals dir — no escape.
        for evil in [
            "x/../../victim/y",     // path-traversal via separators + ..
            "e/../../../etc",       // deeper traversal
            "png/../../../../root", // still contains separators
            "a".repeat(17).as_str(),// >16 chars → rejected
            "",                      // empty → rejected
            "pn g",                  // space (non-alnum)
            "p.g",                   // dot (non-alnum)
            "évil",                  // non-ASCII
        ] {
            let path = s.get_original_path(user, file, evil);
            assert_eq!(
                path, expected_bin,
                "extension {evil:?} must collapse to '.bin' under the user dir"
            );
            assert!(
                path.starts_with(&originals_dir),
                "sanitized path for {evil:?} escaped the user originals dir: {}",
                path.display()
            );
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(
                name.ends_with(".bin"),
                "expected a .bin file name for {evil:?}, got {name}"
            );
        }

        // A benign extension is preserved (lowercased).
        let ok = s.get_original_path(user, file, "PNG");
        assert_eq!(ok, originals_dir.join(format!("{}.png", file)));
        assert!(ok.starts_with(&originals_dir));
    }

    /// `delete_user_dirs` removes the per-user directory under every storage
    /// subdir it manages (originals / text / images / thumbnails) so deleting a
    /// user leaves no orphaned dirs. Save one of every artifact kind, delete the
    /// user's dirs, and assert each per-user subdir is gone.
    #[tokio::test]
    async fn delete_user_dirs_removes_every_per_user_subdir() {
        let (dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();

        // One of every artifact kind so each managed subdir exists on disk.
        s.save_original(user, file, "txt", b"orig").await.unwrap();
        s.save_text_page(user, file, 1, "page one").await.unwrap();
        s.save_image(user, file, 1, false, b"img").await.unwrap(); // images/
        s.save_image(user, file, 1, true, b"thumb").await.unwrap(); // thumbnails/

        // Sanity: the per-user subdirs exist before the delete.
        for subdir in ["originals", "text", "images", "thumbnails"] {
            assert!(
                s.get_user_path(user, subdir).exists(),
                "{subdir} per-user dir must exist before delete"
            );
        }

        s.delete_user_dirs(user).await.unwrap();

        // Every managed per-user subdir is gone after the delete.
        for subdir in ["originals", "text", "images", "thumbnails"] {
            assert!(
                !s.get_user_path(user, subdir).exists(),
                "{subdir} per-user dir must be gone after delete_user_dirs"
            );
        }
        // The storage root itself survives (only the user's dirs were removed).
        assert!(dir.path().exists());
    }

    /// Per-page citation geometry round-trips through
    /// `save_geometry_page`/`load_geometry_page` (the citation-highlight
    /// derivative). A load of a page that was never saved is a not-found.
    #[tokio::test]
    async fn save_then_load_geometry_page_roundtrips() {
        let (_dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();
        let geom = r#"{"text":"hello","boxes":[[0.1,0.1,0.05,0.02]]}"#;

        let path = s.save_geometry_page(user, file, 3, geom).await.unwrap();
        assert!(path.exists(), "saved geometry page must exist on disk");

        let loaded = s.load_geometry_page(user, file, 3).await.unwrap();
        assert_eq!(loaded, geom, "geometry must round-trip byte-for-byte");

        // A page that was never written is a not-found.
        assert!(
            s.load_geometry_page(user, file, 99).await.is_err(),
            "an unsaved geometry page must be not-found"
        );
    }

    /// Security (F-15), geometry path: a symlink planted at the geometry page's
    /// location must NOT be followed on load — the read is refused (mirrors the
    /// original-blob symlink guard).
    #[cfg(unix)]
    #[tokio::test]
    async fn load_geometry_page_refuses_to_follow_a_symlink() {
        let (dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();

        // A secret outside the storage tree the symlink would point at.
        let secret = dir.path().join("secret.json");
        tokio::fs::write(&secret, b"{\"text\":\"TOP SECRET\"}").await.unwrap();

        // Plant a symlink AT the path load_geometry_page will compute.
        let target = s
            .get_user_path(user, "geometry")
            .join(file.to_string())
            .join("page_1.json");
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        std::os::unix::fs::symlink(&secret, &target).unwrap();

        let res = s.load_geometry_page(user, file, 1).await;
        assert!(
            res.is_err(),
            "a symlinked geometry page must be refused, not followed"
        );
    }

    /// Security (F-15): a symlink planted in the storage tree must NOT be
    /// followed on load — the read is refused.
    #[cfg(unix)]
    #[tokio::test]
    async fn load_refuses_to_follow_a_symlink() {
        let (dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();

        // A secret outside the storage tree the symlink would point at.
        let secret = dir.path().join("secret.txt");
        tokio::fs::write(&secret, b"TOP SECRET").await.unwrap();

        // Plant a symlink AT the path load_original will compute.
        let target = s.get_original_path(user, file, "txt");
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        std::os::unix::fs::symlink(&secret, &target).unwrap();

        let res = s.load_original(user, file, "txt").await;
        assert!(res.is_err(), "a symlinked original must be refused, not followed");
    }

    fn sharded() -> (tempfile::TempDir, FilesystemStorage) {
        let dir = tempfile::tempdir().unwrap();
        let s = FilesystemStorage::sharded_for_test(dir.path());
        (dir, s)
    }

    impl FilesystemStorage {
        fn sharded_for_test(base: &Path) -> Self {
            Self::with_layout(base, OriginalsLayout::Sharded)
        }
    }

    async fn write_at(path: &Path, bytes: &[u8]) {
        tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        tokio::fs::write(path, bytes).await.unwrap();
    }

    /// `Sharded` writes `originals/<user>/<id[0..2]>/<id[2..4]>/<id>.<ext>`;
    /// `Flat` (the default) is byte-for-byte the old location.
    #[tokio::test]
    async fn sharded_layout_writes_under_the_first_four_hex_digits() {
        let (dir, s) = sharded();
        let user = Uuid::new_v4();
        let file = Uuid::parse_str("1c0a7e11-2233-4455-8899-aabbccddeeff").unwrap();
        let path = s.save_original(user, file, "webp", b"x").await.unwrap();
        assert_eq!(
            path,
            dir.path()
                .join("originals")
                .join(user.to_string())
                .join("1c")
                .join("0a")
                .join(format!("{file}.webp"))
        );
        assert_eq!(s.get_original_path(user, file, "webp"), path);
        let flat = FilesystemStorage::new(dir.path());
        assert_eq!(flat.layout(), OriginalsLayout::Flat);
        assert_eq!(
            flat.get_original_path(user, file, "webp"),
            dir.path().join("originals").join(user.to_string()).join(format!("{file}.webp"))
        );
        // The sanitiser still applies inside the leaf.
        let evil = s.get_original_path(user, file, "x/../../y");
        assert_eq!(evil, path.with_extension("bin"));
    }

    /// Resolve and load read BOTH layouts whatever the write layout: sharded
    /// wins when both exist.
    #[tokio::test]
    async fn resolve_reads_both_layouts_sharded_first() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let only_flat = Uuid::new_v4();
        let both = Uuid::new_v4();
        write_at(&s.flat_original_path(user, only_flat, "webp"), b"flat").await;
        write_at(&s.flat_original_path(user, both, "webp"), b"old").await;
        write_at(&s.sharded_original_path(user, both, "webp"), b"new").await;
        assert_eq!(s.load_original(user, only_flat, "webp").await.unwrap(), b"flat");
        assert_eq!(s.load_original(user, both, "webp").await.unwrap(), b"new");
        assert_eq!(
            s.resolve_original_path(user, both, "webp").await,
            Some(s.sharded_original_path(user, both, "webp"))
        );
        assert_eq!(s.resolve_original_path(user, Uuid::new_v4(), "webp").await, None);
        let flat_mode = FilesystemStorage::new(s.base_path.clone());
        let sharded_only = Uuid::new_v4();
        write_at(&s.sharded_original_path(user, sharded_only, "webp"), b"s").await;
        assert_eq!(flat_mode.load_original(user, sharded_only, "webp").await.unwrap(), b"s");
    }

    /// Delete removes the object from every location; absent is Ok(false).
    #[tokio::test]
    async fn delete_original_removes_flat_and_sharded_copies() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        write_at(&s.flat_original_path(user, id, "webp"), b"a").await;
        write_at(&s.sharded_original_path(user, id, "webp"), b"b").await;
        assert!(s.delete_original(user, id, "webp").await.unwrap());
        assert!(!s.flat_original_path(user, id, "webp").exists());
        assert!(!s.sharded_original_path(user, id, "webp").exists());
        assert!(!s.delete_original(user, id, "webp").await.unwrap());
    }

    /// `delete_all` (no extension) also sweeps the id's shard leaf.
    #[tokio::test]
    async fn delete_all_sweeps_the_shard_leaf() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let sibling = Uuid::new_v4();
        s.save_original(user, id, "webp", b"x").await.unwrap();
        write_at(&s.flat_original_path(user, id, "png"), b"y").await;
        s.save_original(user, sibling, "webp", b"z").await.unwrap();
        s.delete_all(user, id).await.unwrap();
        assert!(s.resolve_original_path(user, id, "webp").await.is_none());
        assert!(s.resolve_original_path(user, id, "png").await.is_none());
        assert!(s.resolve_original_path(user, sibling, "webp").await.is_some());
    }

    /// The mover: moves objects, leaves junk, dedups identical copies, keeps
    /// conflicting ones, honours `limit`, and a re-run finishes the job.
    #[cfg(unix)]
    #[tokio::test]
    async fn shard_flat_originals_moves_resumably_and_touches_nothing_else() {
        let (dir, s) = sharded();
        let user = Uuid::new_v4();
        let ids: Vec<Uuid> = (0..10).map(|_| Uuid::new_v4()).collect();
        for (i, id) in ids.iter().enumerate() {
            write_at(&s.flat_original_path(user, *id, "webp"), format!("o{i}").as_bytes()).await;
        }
        let flat_dir = dir.path().join("originals").join(user.to_string());
        tokio::fs::write(flat_dir.join("notes.txt"), b"keep").await.unwrap();
        let outside = dir.path().join("outside.webp");
        tokio::fs::write(&outside, b"outside").await.unwrap();
        let link_id = Uuid::new_v4();
        std::os::unix::fs::symlink(&outside, flat_dir.join(format!("{link_id}.webp"))).unwrap();
        let dup = Uuid::new_v4();
        write_at(&s.flat_original_path(user, dup, "webp"), b"same").await;
        write_at(&s.sharded_original_path(user, dup, "webp"), b"same").await;
        let conflict = Uuid::new_v4();
        write_at(&s.flat_original_path(user, conflict, "webp"), b"one").await;
        write_at(&s.sharded_original_path(user, conflict, "webp"), b"two").await;

        let opts = ShardOptions {
            batch: 3,
            pause: std::time::Duration::ZERO,
            limit: Some(4),
            ..Default::default()
        };
        let mut calls = 0;
        let first = s.shard_flat_originals(user, &opts, &mut |_| calls += 1).await.unwrap();
        assert_eq!(first.moved, 4);
        assert!(calls >= 1);
        for id in &ids {
            assert!(s.resolve_original_path(user, *id, "webp").await.is_some(), "readable mid-migration");
        }
        let opts = ShardOptions { limit: None, ..opts };
        let second = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        assert_eq!(second.moved, 6);
        assert_eq!(second.remaining_flat, 1, "only the conflict stays flat: {second:?}");
        assert_eq!(second.conflicts, 1);
        assert_eq!(first.deduplicated + second.deduplicated, 1);
        for (i, id) in ids.iter().enumerate() {
            let p = s.sharded_original_path(user, *id, "webp");
            assert_eq!(tokio::fs::read(&p).await.unwrap(), format!("o{i}").as_bytes());
            assert!(!s.flat_original_path(user, *id, "webp").exists());
        }
        assert!(!s.flat_original_path(user, dup, "webp").exists());
        assert!(s.flat_original_path(user, conflict, "webp").exists());
        assert_eq!(tokio::fs::read(s.sharded_original_path(user, conflict, "webp")).await.unwrap(), b"two");
        assert!(flat_dir.join("notes.txt").exists());
        assert!(tokio::fs::symlink_metadata(flat_dir.join(format!("{link_id}.webp"))).await.unwrap().file_type().is_symlink());
        assert!(!s.sharded_original_path(user, link_id, "webp").exists());
        let third = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        assert_eq!(third.moved, 0);
    }

    /// INV-6, HIGH finding: a symlink planted AT the shard target must never
    /// be read through for the dedup byte-compare, and must never make the
    /// mover unlink the real flat copy. Before the fix, `symlink_metadata`
    /// was checked only for existence (`Ok(_) =>`), so a symlink at `to`
    /// pointing at bytes identical to the flat copy made `fs::read(&to)`
    /// follow it, the compare report "same", and the flat (REAL) object get
    /// unlinked — leaving only the attacker-controlled symlink behind.
    #[cfg(unix)]
    #[tokio::test]
    async fn shard_flat_originals_refuses_to_read_through_a_symlinked_target() {
        let (dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let flat = s.flat_original_path(user, id, "webp");
        write_at(&flat, b"identical bytes").await;

        // A file OUTSIDE the store whose bytes are byte-for-byte identical to
        // the flat copy.
        let victim = dir.path().join("victim.webp");
        tokio::fs::write(&victim, b"identical bytes").await.unwrap();
        // Plant a symlink AT the shard target, pointing at the victim.
        let to = s.sharded_original_path(user, id, "webp");
        tokio::fs::create_dir_all(to.parent().unwrap()).await.unwrap();
        std::os::unix::fs::symlink(&victim, &to).unwrap();

        let mut report = ShardReport::default();
        let opts = ShardOptions::default();
        let r = s
            .shard_flat_originals(user, &opts, &mut |p| report = p.clone())
            .await
            .unwrap();

        assert!(flat.exists(), "the REAL flat object must SURVIVE a symlinked target");
        assert_eq!(
            tokio::fs::read(&flat).await.unwrap(),
            b"identical bytes",
            "the surviving flat object must be untouched"
        );
        assert!(
            tokio::fs::symlink_metadata(&to).await.unwrap().file_type().is_symlink(),
            "the symlink at the target must be left exactly as planted, never followed or replaced"
        );
        assert_eq!(r.conflicts, 1, "a non-regular-file target must be counted as a conflict: {r:?}");
        assert_eq!(r.moved, 0);
        assert_eq!(r.deduplicated, 0, "must NOT be reported as a dedup — that would imply the flat copy was removed");
    }

    /// A DIRECTORY at the shard target (pathological, but the same class of
    /// bug as a symlink) must be refused the same way: never read through,
    /// never a reason to unlink the flat copy.
    #[tokio::test]
    async fn shard_flat_originals_refuses_a_non_file_target_that_is_a_directory() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let flat = s.flat_original_path(user, id, "webp");
        write_at(&flat, b"x").await;
        let to = s.sharded_original_path(user, id, "webp");
        tokio::fs::create_dir_all(&to).await.unwrap(); // a directory, not a file, at the target name

        let opts = ShardOptions::default();
        let r = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        assert!(flat.exists(), "the flat object must survive a directory at the target");
        assert_eq!(r.conflicts, 1);
        assert_eq!(r.moved, 0);
    }

    /// MEDIUM finding: `max_passes` bounds the mover against a sustained
    /// concurrent flat writer that keeps adding entries — without the bound
    /// this would loop until the writer stops, which under a live write load
    /// may be never. The writer here adds one new flat object after every
    /// pass boundary (paced by `pause`), forever — `max_passes` must still
    /// make the call return, reporting what is left in `remaining_flat`.
    #[tokio::test]
    async fn shard_flat_originals_is_bounded_by_max_passes_under_a_sustained_writer() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        write_at(&s.flat_original_path(user, Uuid::new_v4(), "webp"), b"x").await;

        let keep_writing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let writer_storage = s.clone();
        let writer_flag = keep_writing.clone();
        let writer = tokio::spawn(async move {
            while writer_flag.load(std::sync::atomic::Ordering::SeqCst) {
                write_at(&writer_storage.flat_original_path(user, Uuid::new_v4(), "webp"), b"y").await;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        });

        let opts = ShardOptions {
            batch: 1,
            pause: std::time::Duration::from_millis(5),
            limit: None,
            max_passes: 3,
        };
        let started = std::time::Instant::now();
        let report = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        let elapsed = started.elapsed();
        keep_writing.store(false, std::sync::atomic::Ordering::SeqCst);
        writer.await.unwrap();

        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "max_passes must make the call return promptly even under a sustained writer: {elapsed:?}"
        );
        // Under a sustained writer the bound is very likely to still find
        // something flat; this is the whole point of the report field (a
        // follow-up run continues from here). Not asserting > 0 would make
        // this test pass for the wrong reason if the writer happened to be
        // slow, so just assert the call returned with a well-formed report.
        assert!(report.moved + report.remaining_flat >= 1, "{report:?}");
    }

    /// LOW finding: an I/O error must not drop the run's partial progress —
    /// `progress` must see a `ShardReport` reflecting what happened before
    /// the failure, not be skipped in favour of just propagating `Err`.
    ///
    /// Deterministic by construction (no dependence on `read_dir`'s
    /// unspecified entry order, which is why this is a SEPARATE test from the
    /// symlink/directory conflict ones above rather than reusing their
    /// multi-entry setup): exactly ONE object exists, so pass 1 moves it
    /// cleanly and, because it moved something, the mover's loop always goes
    /// on to a pass 2. The `progress` callback itself — called synchronously
    /// right after that one move, strictly BETWEEN pass 1 and pass 2 — is
    /// used to revoke read access to the namespace directory, so pass 2's own
    /// `read_dir` is what fails. The report `progress` is called with right
    /// before the error propagates must therefore already carry `moved: 1`
    /// from pass 1, not a blank default.
    #[cfg(unix)]
    #[tokio::test]
    async fn shard_flat_originals_reports_partial_progress_before_an_io_error() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, s) = sharded();
        let user = Uuid::new_v4();
        let ok_id = Uuid::new_v4();
        write_at(&s.flat_original_path(user, ok_id, "webp"), b"ok").await;
        let flat_dir = dir.path().join("originals").join(user.to_string());

        let opts = ShardOptions { batch: 1, pause: std::time::Duration::ZERO, ..Default::default() };
        let mut last_seen = ShardReport::default();
        let mut calls = 0u32;
        let flat_dir_for_cb = flat_dir.clone();
        let res = s
            .shard_flat_originals(user, &opts, &mut |r| {
                calls += 1;
                last_seen = r.clone();
                if calls == 1 {
                    // Fires once, right after the one object moved in pass 1
                    // (batch: 1 ⇒ a progress call after every move) and
                    // before pass 2's `read_dir` runs — revoke read+execute
                    // on the namespace dir so that NEXT read_dir fails with a
                    // real I/O error instead of NotFound.
                    let _ = std::fs::set_permissions(
                        &flat_dir_for_cb,
                        std::fs::Permissions::from_mode(0o000),
                    );
                }
            })
            .await;

        // Restore so the TempDir's own cleanup can recurse into it.
        let _ = std::fs::set_permissions(&flat_dir, std::fs::Permissions::from_mode(0o755));

        match res {
            Err(_) => {
                assert_eq!(
                    last_seen.moved, 1,
                    "the LAST progress call before the error must already carry \
                     pass 1's moved count, not a blank report: {last_seen:?}"
                );
                assert!(calls >= 2, "progress must be called again (with the partial report) at the failure site, not just after pass 1: {calls}");
            }
            Ok(r) => {
                // Root ignores the directory's permission bits, so pass 2's
                // read_dir succeeds anyway and the run simply completes; the
                // property under test does not apply, but the one object must
                // still be correctly accounted for.
                assert_eq!(r.moved, 1);
            }
        }
    }

    /// `FileStorage::open_original`'s wrapper: a [`FilesystemStorage`] whose
    /// FIRST `resolve_original_path` answer is a pre-recorded STALE path,
    /// regardless of where the object really is on disk — a deterministic
    /// stand-in for "the mover renamed the object between resolve and open"
    /// without racing a real background task. Every other call delegates
    /// straight through to the real storage.
    struct FlakyResolve<'a> {
        inner: &'a FilesystemStorage,
        stale_path: PathBuf,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl FileStorage for FlakyResolve<'_> {
        async fn save_original(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            extension: &str,
            data: &[u8],
        ) -> StorageResult<PathBuf> {
            self.inner.save_original(user_id, file_id, extension, data).await
        }
        async fn save_text_page(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
            text: &str,
        ) -> StorageResult<PathBuf> {
            self.inner.save_text_page(user_id, file_id, page_num, text).await
        }
        async fn save_geometry_page(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
            geometry_json: &str,
        ) -> StorageResult<PathBuf> {
            self.inner
                .save_geometry_page(user_id, file_id, page_num, geometry_json)
                .await
        }
        async fn load_geometry_page(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
        ) -> StorageResult<String> {
            self.inner.load_geometry_page(user_id, file_id, page_num).await
        }
        async fn save_image(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
            is_thumbnail: bool,
            data: &[u8],
        ) -> StorageResult<PathBuf> {
            self.inner
                .save_image(user_id, file_id, page_num, is_thumbnail, data)
                .await
        }
        fn get_original_path(&self, user_id: Uuid, file_id: Uuid, extension: &str) -> PathBuf {
            self.inner.get_original_path(user_id, file_id, extension)
        }
        fn get_text_path(&self, user_id: Uuid, file_id: Uuid, page_num: u32) -> PathBuf {
            self.inner.get_text_path(user_id, file_id, page_num)
        }
        fn get_image_path(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
            is_thumbnail: bool,
        ) -> PathBuf {
            self.inner.get_image_path(user_id, file_id, page_num, is_thumbnail)
        }
        async fn load_original(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            extension: &str,
        ) -> StorageResult<Vec<u8>> {
            self.inner.load_original(user_id, file_id, extension).await
        }
        async fn load_text_page(&self, user_id: Uuid, file_id: Uuid, page_num: u32) -> StorageResult<String> {
            self.inner.load_text_page(user_id, file_id, page_num).await
        }
        async fn load_preview(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
        ) -> StorageResult<Vec<u8>> {
            self.inner.load_preview(user_id, file_id, page_num).await
        }
        async fn load_thumbnail(&self, user_id: Uuid, file_id: Uuid) -> StorageResult<Vec<u8>> {
            self.inner.load_thumbnail(user_id, file_id).await
        }
        async fn resolve_original_path(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            extension: &str,
        ) -> Option<PathBuf> {
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Some(self.stale_path.clone())
            } else {
                self.inner.resolve_original_path(user_id, file_id, extension).await
            }
        }
        async fn delete_all(&self, user_id: Uuid, file_id: Uuid) -> StorageResult<()> {
            self.inner.delete_all(user_id, file_id).await
        }
        async fn delete_user_dirs(&self, user_id: Uuid) -> StorageResult<()> {
            self.inner.delete_user_dirs(user_id).await
        }
        fn calculate_checksum(&self, data: &[u8]) -> String {
            self.inner.calculate_checksum(data)
        }
    }

    /// HIGH finding (serve TOCTOU): `open_original` must retry the resolve
    /// once when the first open answers `NotFound`. The object sits at the
    /// FLAT path when the (stale, pre-recorded) resolve answer is handed
    /// out, but by the time `open_original`'s first open runs it has
    /// genuinely been renamed to its SHARD LEAF (simulating a mover winning
    /// the race) — the first open must 404 internally, the retry must
    /// re-resolve for real, and the real resolver finds the object at its
    /// new home.
    #[tokio::test]
    async fn open_original_retries_the_resolve_after_a_toctou_open_failure() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let flat_path = s.flat_original_path(user, id, "webp");
        let sharded_path = s.sharded_original_path(user, id, "webp");
        write_at(&flat_path, b"moved-mid-flight").await;
        // The mover wins the race: gone from flat, now sharded — by the time
        // `open_original`'s first open runs against the stale answer below.
        tokio::fs::create_dir_all(sharded_path.parent().unwrap()).await.unwrap();
        tokio::fs::rename(&flat_path, &sharded_path).await.unwrap();

        let flaky = FlakyResolve {
            inner: &s,
            stale_path: flat_path.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let (path, mut file) = flaky
            .open_original(user, id, "webp")
            .await
            .expect("the retry must find the object at its new sharded path");
        assert_eq!(
            path, sharded_path,
            "the SECOND resolve must return the real, current path, not the stale one"
        );
        let mut bytes = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut file, &mut bytes).await.unwrap();
        assert_eq!(bytes, b"moved-mid-flight");
    }

    /// The retry does not paper over a genuine absence: an object that never
    /// existed is still `None` after both attempts.
    #[tokio::test]
    async fn open_original_is_none_when_the_object_never_existed() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        assert!(s.open_original(user, id, "webp").await.is_none());
    }

    /// MEDIUM finding: a concurrent writer creating new flat objects and a
    /// concurrent deleter removing existing ones, both racing the mover on
    /// ONE directory — every surviving object keeps its exact bytes and
    /// resolves, every deleted object stays gone (never resurrected by the
    /// mover winning a race against the delete), and a follow-up run
    /// converges to `remaining_flat == 0`.
    #[tokio::test]
    async fn shard_flat_originals_is_safe_against_a_concurrent_writer_and_deleter() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();

        // Pre-existing objects: half will be deleted mid-run, half survive.
        let mut expected: std::collections::HashMap<Uuid, Vec<u8>> = std::collections::HashMap::new();
        let pre: Vec<Uuid> = (0..12).map(|_| Uuid::new_v4()).collect();
        for (i, id) in pre.iter().enumerate() {
            let bytes = format!("pre{i}").into_bytes();
            write_at(&s.flat_original_path(user, *id, "webp"), &bytes).await;
            expected.insert(*id, bytes);
        }
        let to_delete: Vec<Uuid> = pre.iter().step_by(2).cloned().collect();
        let to_survive: Vec<Uuid> = pre.iter().skip(1).step_by(2).cloned().collect();
        for id in &to_delete {
            expected.remove(id);
        }

        // New objects the "writer" adds WHILE the mover is running.
        let written: Vec<Uuid> = (0..12).map(|_| Uuid::new_v4()).collect();
        let mut written_bytes: std::collections::HashMap<Uuid, Vec<u8>> = std::collections::HashMap::new();
        for (i, id) in written.iter().enumerate() {
            written_bytes.insert(*id, format!("new{i}").into_bytes());
        }

        let writer_storage = s.clone();
        let writer_bytes = written_bytes.clone();
        let writer = tokio::spawn(async move {
            for (id, bytes) in writer_bytes {
                write_at(&writer_storage.flat_original_path(user, id, "webp"), &bytes).await;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        });
        let deleter_storage = s.clone();
        let deleter_ids = to_delete.clone();
        let deleter = tokio::spawn(async move {
            for id in deleter_ids {
                // May race the mover either way — both outcomes are
                // idempotent/NotFound-tolerant, so the result is ignored.
                let _ = deleter_storage.delete_original(user, id, "webp").await;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        });

        let opts = ShardOptions {
            batch: 2,
            pause: std::time::Duration::from_millis(1),
            ..Default::default()
        };
        let _first = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        writer.await.unwrap();
        deleter.await.unwrap();
        // A follow-up run picks up anything the race left flat (a write that
        // arrived after the mover's last pass, or a rename that lost a race
        // to a delete).
        let follow_up = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        assert_eq!(
            follow_up.remaining_flat, 0,
            "a follow-up run must converge to zero flat objects: {follow_up:?}"
        );

        for id in &to_survive {
            let bytes = s
                .load_original(user, *id, "webp")
                .await
                .unwrap_or_else(|e| panic!("surviving object {id} must still load: {e}"));
            assert_eq!(&bytes, &expected[id], "surviving object {id} must keep its exact bytes");
        }
        for id in &to_delete {
            assert!(
                s.resolve_original_path(user, *id, "webp").await.is_none(),
                "deleted object {id} must never be resurrected"
            );
        }
        for id in &written {
            let bytes = s.load_original(user, *id, "webp").await.unwrap();
            assert_eq!(&bytes, &written_bytes[id], "written object {id} must keep its exact bytes");
        }
    }
}
