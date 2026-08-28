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

/// Filesystem-based file storage
#[derive(Debug, Clone)]
pub struct FilesystemStorage {
    base_path: PathBuf,
}

impl FilesystemStorage {
    /// Create new filesystem storage
    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
        }
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
        // SECURITY: the extension flows from user input on upload. Without
        // sanitization, a value like 'x/../../<victim_uuid>/y.pdf' lets
        // create_dir_all + fs::write escape the per-user originals
        // directory and overwrite (or shadow) another user's file. Same
        // primitive on the read path. Closes 05-file F-03 (High).
        //
        // Allow only ASCII alphanumeric extensions (matches every legit
        // mime-type-derived extension). Anything else is replaced with
        // 'bin', which keeps the file storable but isolated.
        let safe_ext: String = if extension
            .chars()
            .all(|c| c.is_ascii_alphanumeric())
            && !extension.is_empty()
            && extension.len() <= 16
        {
            extension.to_ascii_lowercase()
        } else {
            "bin".to_string()
        };
        self.get_user_path(user_id, "originals")
            .join(format!("{}.{}", file_id, safe_ext))
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

    async fn load_original(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> StorageResult<Vec<u8>> {
        let path = self.get_original_path(user_id, file_id, extension);
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
                // Delete files matching pattern
                if let Ok(mut entries) = fs::read_dir(&path).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        if let Some(name) = entry.file_name().to_str()
                            && name.starts_with(&file_id.to_string()) {
                                let _ = fs::remove_file(entry.path()).await;
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
}
