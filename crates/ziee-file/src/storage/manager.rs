// Global file storage manager

use super::{filesystem::FilesystemStorage, FileStorage};
use std::sync::Arc;

/// Global file storage instance.
///
/// Stored as a leaked `&'static` behind an `RwLock` so that
/// `get_file_storage()` keeps returning the CURRENT storage with zero
/// per-call allocation, while still allowing the global to be OVERWRITTEN.
///
/// Production calls `init_file_storage` exactly once at boot, so the leak is
/// a single storage for the process lifetime. The integration test binary
/// calls it once per test setup (many times per process) — each re-init
/// leaks one small `Arc`, which is bounded and acceptable for a test
/// process. Crucially, re-init now actually SWAPS the active storage (the
/// old `OnceCell::set` kept the first storage silently, so every later
/// in-process file-storage test operated on a different test's temp dir →
/// spurious "File storage not initialized" / wrong-directory failures).
static FILE_STORAGE: std::sync::RwLock<Option<&'static Arc<dyn FileStorage>>> =
    std::sync::RwLock::new(None);

/// Initialize (or re-initialize) file storage.
///
/// Overwrites any previously-installed storage. In a non-test build a second
/// call is logged as a warning (it signals a second bootstrap path in
/// production), but the overwrite still happens.
pub fn init_file_storage(base_path: impl AsRef<std::path::Path>) -> Arc<dyn FileStorage> {
    let storage = Arc::new(FilesystemStorage::new(base_path)) as Arc<dyn FileStorage>;
    let leaked: &'static Arc<dyn FileStorage> = Box::leak(Box::new(storage.clone()));
    let mut guard = FILE_STORAGE.write().unwrap_or_else(|e| e.into_inner());
    #[cfg(not(test))]
    if guard.is_some() {
        tracing::warn!(
            "init_file_storage called more than once in this process; \
             overwriting the active storage. In production this signals a \
             second bootstrap path — investigate."
        );
    }
    *guard = Some(leaked);
    storage
}

/// Get file storage instance
pub fn get_file_storage() -> Arc<dyn FileStorage> {
    let guard = FILE_STORAGE.read().unwrap_or_else(|e| e.into_inner());
    let storage: &'static Arc<dyn FileStorage> =
        guard.expect("File storage not initialized");
    Arc::clone(storage)
}
