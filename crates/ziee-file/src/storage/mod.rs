// File storage abstraction

pub mod filesystem;
pub mod manager;

use ziee_core::AppError;
use async_trait::async_trait;
use std::path::PathBuf;
use uuid::Uuid;

/// Storage result type
pub type StorageResult<T> = Result<T, AppError>;

/// File storage operations
#[async_trait]
pub trait FileStorage: Send + Sync {
    /// Save original file
    async fn save_original(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
        data: &[u8],
    ) -> StorageResult<PathBuf>;

    /// Save text page
    async fn save_text_page(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        text: &str,
    ) -> StorageResult<PathBuf>;

    /// Save per-page citation geometry (JSON) — a derivative like the text page.
    async fn save_geometry_page(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        geometry_json: &str,
    ) -> StorageResult<PathBuf>;

    /// Load per-page citation geometry (JSON).
    async fn load_geometry_page(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
    ) -> StorageResult<String>;

    /// Save image (page or thumbnail)
    async fn save_image(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        is_thumbnail: bool,
        data: &[u8],
    ) -> StorageResult<PathBuf>;

    /// Get original file path
    fn get_original_path(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> PathBuf;

    /// Get text page path
    fn get_text_path(&self, user_id: Uuid, file_id: Uuid, page_num: u32) -> PathBuf;

    /// Get image path
    fn get_image_path(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        is_thumbnail: bool,
    ) -> PathBuf;

    /// Load original file
    async fn load_original(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> StorageResult<Vec<u8>>;

    /// Load text page
    async fn load_text_page(&self, user_id: Uuid, file_id: Uuid, page_num: u32) -> StorageResult<String>;

    /// Load preview image (high quality, 2000px)
    async fn load_preview(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
    ) -> StorageResult<Vec<u8>>;

    /// Load thumbnail (300px, always from first page)
    async fn load_thumbnail(&self, user_id: Uuid, file_id: Uuid) -> StorageResult<Vec<u8>>;

    /// Where an original's bytes are on disk RIGHT NOW, or `None` when it is
    /// not stored. Differs from [`Self::get_original_path`] (where a NEW write
    /// goes) for a store that is migrating between layouts: an implementation
    /// that keeps a legacy location answers it here too, so readers find an
    /// object wherever it currently is. A symlink is never returned.
    async fn resolve_original_path(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> Option<PathBuf> {
        let path = self.get_original_path(user_id, file_id, extension);
        match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) if meta.file_type().is_file() => Some(path),
            _ => None,
        }
    }

    /// Delete exactly ONE original — `<file_id>.<extension>` — from every
    /// location this storage may hold it at. `Ok(true)` when something was
    /// removed, `Ok(false)` when it was already absent (idempotent), `Err` when
    /// an unlink failed and the object may still be on disk.
    async fn delete_original(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> std::io::Result<bool> {
        let path = self.get_original_path(user_id, file_id, extension);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(std::io::Error::new(
                e.kind(),
                format!("remove {}: {e}", path.display()),
            )),
        }
    }

    /// Resolve, then open, an original — retrying the resolve ONCE if the
    /// first open answers `NotFound`.
    ///
    /// Closes a TOCTOU window a plain `resolve_original_path` + `File::open`
    /// call pair leaves open: a relocator (the sharding mover, or any future
    /// one) can rename the object between the resolve and the open, so the
    /// open answers `NotFound` for an object that is very much still on
    /// disk, one path over. Re-resolving picks up wherever the object landed
    /// and opens that. `None` only when the object is genuinely absent (or a
    /// symlink was refused) after the retry.
    ///
    /// Never follows a symlink: `resolve_original_path` already refuses one,
    /// and this never opens a path that function did not just hand back.
    async fn open_original(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> Option<(PathBuf, tokio::fs::File)> {
        for attempt in 0..2u8 {
            let path = self
                .resolve_original_path(user_id, file_id, extension)
                .await?;
            match tokio::fs::File::open(&path).await {
                Ok(file) => return Some((path, file)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && attempt == 0 => continue,
                Err(_) => return None,
            }
        }
        None
    }

    /// Delete all files for a file_id
    async fn delete_all(&self, user_id: Uuid, file_id: Uuid) -> StorageResult<()>;

    /// Remove every on-disk directory scoped to a user across all storage
    /// subdirs. Called on user delete so the per-user dirs (and any remaining
    /// blobs) don't linger as filesystem orphans after the `files` rows
    /// cascade-delete.
    async fn delete_user_dirs(&self, user_id: Uuid) -> StorageResult<()>;

    /// Calculate SHA-256 checksum
    fn calculate_checksum(&self, data: &[u8]) -> String;
}
