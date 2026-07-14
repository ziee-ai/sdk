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
