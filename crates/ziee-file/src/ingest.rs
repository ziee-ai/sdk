//! Store-side ingest: persist raw bytes (+ pre-computed derivatives) as a
//! durable, versioned `File`.
//!
//! Two entry points:
//! - [`store_processed`] — the pure STORE half: given already-processed
//!   derivatives, save the original + derivative blobs, create the
//!   `files`/`file_versions` rows (rolling the blobs back on a DB failure),
//!   and fire the owner-scoped change event. Names no processing engine.
//! - [`ingest_bytes`] — the orchestration convenience: run the injected
//!   [`FileProcessor`] over the bytes, then `store_processed`. The app's
//!   `ProcessingManager` is reached only through the [`FileProcessor`] seam.
//!
//! The former `workflow_run_id` linkage is NOT here — that file↔run association
//! is an app-domain concern (ziee's `file_workflow_runs` join table), applied by
//! the caller after this returns.

use uuid::Uuid;

use crate::models::{File, FileCreateData, ProcessingResult};
use crate::repository::FileRepository;
use crate::seams::{FileEvents, FileProcessor};
use crate::storage::manager::get_file_storage;
use crate::utils::extension_of;
use ziee_core::AppError;

/// Persist `bytes` + `processed` derivatives as a new durable file owned by
/// `user_id`. Stores the original + every derivative (text pages, per-page
/// geometry, thumbnail, preview images), creates the `files`/`file_versions`
/// rows, and emits a cross-device change event. On a DB failure the already-
/// written blobs are rolled back. Returns the head `File`.
#[allow(clippy::too_many_arguments)]
pub async fn store_processed(
    repo: &FileRepository,
    events: &dyn FileEvents,
    user_id: Uuid,
    bytes: &[u8],
    filename: &str,
    mime_type: Option<String>,
    created_by: &str,
    source_message_id: Option<Uuid>,
    processed: &ProcessingResult,
) -> Result<File, AppError> {
    // Canonical extension (rsplit + lowercase) — MUST match how the download/
    // read paths derive the blob key (Path::extension would mis-key dotfiles).
    let ext = extension_of(filename);

    let file_id = Uuid::new_v4();
    let storage = get_file_storage();
    storage
        .save_original(user_id, file_id, &ext, bytes)
        .await
        .map_err(AppError::internal_with_id)?;

    // Derivative writes are best-effort (the original + DB row are the source
    // of truth) but a failure is logged so a dropped page/thumbnail is
    // traceable instead of silently vanishing.
    for (n, text) in processed.text_pages.iter().enumerate() {
        if let Err(e) = storage
            .save_text_page(user_id, file_id, (n + 1) as u32, text)
            .await
        {
            tracing::warn!(
                "store_processed: failed to save text page {} for {}: {}",
                n + 1,
                file_id,
                e
            );
        }
    }
    // Per-page citation geometry (PDF only; aligned 1:1 with text pages).
    for (n, geom) in processed.geometry_pages.iter().enumerate() {
        if let Err(e) = storage
            .save_geometry_page(user_id, file_id, (n + 1) as u32, geom)
            .await
        {
            tracing::warn!(
                "store_processed: failed to save geometry page {} for {}: {}",
                n + 1,
                file_id,
                e
            );
        }
    }
    if let Some(thumb) = processed.thumbnails.first() {
        if let Err(e) = storage.save_image(user_id, file_id, 1, true, thumb).await {
            tracing::warn!(
                "store_processed: failed to save thumbnail for {}: {}",
                file_id,
                e
            );
        }
    }
    for (n, img) in processed.images.iter().enumerate() {
        if let Err(e) = storage
            .save_image(user_id, file_id, (n + 1) as u32, false, img)
            .await
        {
            tracing::warn!(
                "store_processed: failed to save preview image {} for {}: {}",
                n + 1,
                file_id,
                e
            );
        }
    }

    let checksum = storage.calculate_checksum(bytes);
    let file = match repo
        .create(FileCreateData {
            id: file_id,
            user_id,
            filename: filename.to_string(),
            file_size: bytes.len() as i64,
            mime_type: mime_type.clone(),
            checksum: Some(checksum),
            has_thumbnail: !processed.thumbnails.is_empty(),
            preview_page_count: processed.images.len() as i32,
            text_page_count: processed.text_pages.len() as i32,
            processing_metadata: serde_json::to_value(&processed.metadata).unwrap_or_default(),
            source_message_id,
            created_by: created_by.to_string(),
        })
        .await
    {
        Ok(f) => f,
        Err(e) => {
            // The original + derivatives were already written to the file store
            // above; the DB row failed, so roll the blobs back to avoid orphaned
            // storage that no file_id row will ever reference.
            if let Err(cleanup_err) = storage.delete_all(user_id, file_id).await {
                tracing::warn!(
                    "store_processed: failed to clean up orphaned storage for {} after DB error: {}",
                    file_id,
                    cleanup_err
                );
            }
            return Err(e);
        }
    };

    events.on_file_changed(user_id, file_id, None);

    Ok(file)
}

/// Run the injected [`FileProcessor`] over `bytes`, then [`store_processed`].
/// A processing failure is non-fatal (the raw original is still stored) but is
/// logged. Returns the head `File`.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_bytes(
    repo: &FileRepository,
    processor: &dyn FileProcessor,
    events: &dyn FileEvents,
    user_id: Uuid,
    bytes: &[u8],
    filename: &str,
    mime_hint: Option<String>,
    created_by: &str,
    source_message_id: Option<Uuid>,
) -> Result<File, AppError> {
    let ext = extension_of(filename);
    let mime_type = mime_hint.or_else(|| mime_guess::from_ext(&ext).first().map(|m| m.to_string()));
    let mime_type_str = mime_type.as_deref().unwrap_or("application/octet-stream");

    let processed = processor
        .process(bytes, mime_type_str)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                "ingest_bytes: processing failed for {} ({}): {}; storing original only",
                filename,
                mime_type_str,
                e
            );
            Default::default()
        });

    store_processed(
        repo,
        events,
        user_id,
        bytes,
        filename,
        mime_type,
        created_by,
        source_message_id,
        &processed,
    )
    .await
}
