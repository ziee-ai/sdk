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
}

impl Default for ShardOptions {
    fn default() -> Self {
        Self {
            batch: 2000,
            pause: std::time::Duration::from_millis(50),
            limit: None,
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
        // Passes until one moves nothing: a directory being renamed out of
        // while it is read may skip entries, and a concurrent writer may add
        // flat ones — the next pass picks both up.
        loop {
            let mut moved_this_pass = 0u64;
            let mut flat_seen = 0u64;
            // Conflicts and skips are a property of what is in the directory,
            // so each pass recounts them (the last pass's numbers are the
            // report's).
            let mut conflicts = 0u64;
            let mut skipped = 0u64;
            let mut entries = match fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(report),
                Err(e) => return Err(e),
            };
            while let Some(entry) = entries.next_entry().await? {
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
                    fs::create_dir_all(&leaf).await?;
                    leaves.insert(leaf);
                }
                match fs::symlink_metadata(&to).await {
                    Ok(_) => {
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
                                Err(e) => return Err(e),
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
                    Err(e) => return Err(e),
                }
                match fs::rename(&from, &to).await {
                    Ok(()) => {
                        report.moved += 1;
                        moved_this_pass += 1;
                        flat_seen -= 1;
                    }
                    // Deleted (or moved) under us — nothing left to move.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => flat_seen -= 1,
                    Err(e) => return Err(e),
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
            if moved_this_pass == 0 || opts.limit.is_some_and(|l| report.moved >= l) {
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

        let opts = ShardOptions { batch: 3, pause: std::time::Duration::ZERO, limit: Some(4) };
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
}
