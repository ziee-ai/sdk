//! Injected seams that sever the file STORE from the app's file PROCESSING and
//! realtime-sync/RAG subsystems.
//!
//! The store persists bytes + derivative blobs + version rows; it does NOT know
//! how to PRODUCE derivatives (that is the app's `ProcessingManager`) nor how to
//! notify other devices / trigger RAG re-indexing (that is the app's `sync` +
//! `file_rag`). Two traits invert those dependencies:
//!
//! - [`FileProcessor`] — the app implements it with `ProcessingManager`; the
//!   store's `ingest_bytes` calls it to turn raw bytes into a
//!   [`crate::models::ProcessingResult`]. The store never names the extraction
//!   engine.
//! - [`FileEvents`] — the app wires `on_file_changed`/`on_file_deleted` to
//!   `sync::publish` (`SyncEntity::File`, owner-scoped) and `on_committed` to
//!   `file_rag::ingest::spawn_{index,reindex}`. The store never names
//!   `SyncEntity` or `file_rag`.

use async_trait::async_trait;
use uuid::Uuid;

use crate::models::ProcessingResult;
use ziee_core::AppError;

/// Produce derivative bytes (text pages / geometry / thumbnails / preview
/// images / metadata) from a file's raw bytes. Implemented app-side by
/// `ProcessingManager`; the store calls it before persisting.
#[async_trait]
pub trait FileProcessor: Send + Sync {
    /// Run the extraction pipeline. Returning `Err` lets the store degrade
    /// gracefully (store the raw original only) while logging the failure.
    async fn process(&self, bytes: &[u8], mime_type: &str) -> Result<ProcessingResult, AppError>;
}

/// Post-persist notifications. The store fires these after a durable change so
/// the app can push cross-device sync + kick off background RAG indexing —
/// without the store naming the concrete `SyncEntity`/`file_rag` symbols.
pub trait FileEvents: Send + Sync {
    /// A file's head/version set changed (append / restore / new file). Wired
    /// app-side to `sync::publish_file_changed_with_origin` (owner-scoped
    /// `SyncEntity::File` update). `origin` is the originating SSE connection
    /// (self-echo skip) when known.
    fn on_file_changed(&self, user_id: Uuid, file_id: Uuid, origin: Option<Uuid>);

    /// A file was deleted. Wired app-side to
    /// `sync::publish_file_deleted_with_origin`.
    fn on_file_deleted(&self, user_id: Uuid, file_id: Uuid, origin: Option<Uuid>);

    /// A new head was committed (new file or new version). Wired app-side to
    /// `file_rag::ingest::spawn_{index,reindex}` for background RAG indexing.
    /// `is_new` distinguishes a brand-new file (`spawn_index`) from a new
    /// version of an existing file (`spawn_reindex`). Default no-op so callers
    /// that don't need RAG (e.g. plain ingest) can ignore it.
    fn on_committed(&self, _user_id: Uuid, _file_id: Uuid, _is_new: bool) {}
}
