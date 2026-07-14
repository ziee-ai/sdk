//! `ziee-file` — the generic, domain-agnostic multi-derivative blob STORE.
//!
//! Moved from ziee's `modules/file` (chunk `ziee-file`): the STORE half of the
//! file module — bytes + versions + derivatives + CRUD-repository + upload-
//! security validators — with ZERO domain coupling. The domain PROCESSING
//! (`ProcessingManager`, pdfium/pandoc), the chat/project bridges, LLM provider
//! routing, RAG, deliverables, and `available_files` all STAY in ziee.
//!
//! What moved:
//! - [`models`] — `File` / `FileVersion` / `FileCreateData` /
//!   `FileVersionCreateData` (schemars keys preserved → OpenAPI byte-identical),
//!   plus the pure-data `ProcessingMetadata` / `ProcessingResult`.
//! - [`storage`] — the `FileStorage` trait, `FilesystemStorage` impl, and the
//!   global `get/init_file_storage` manager. Moved verbatim (error type
//!   `crate::common::AppError` → [`ziee_core::AppError`]).
//! - [`repository`] — `FileRepository` (`query!`/`query_as!` over
//!   `files`/`file_versions`). The `workflow_run_id` column + its two helpers
//!   were REMOVED (the file↔run link is ziee's app-side `file_workflow_runs`
//!   join table — the store stays domain-agnostic).
//! - [`types`] — `FileListResponse`, the download-token / pagination DTOs.
//! - [`utils`] — `extension_of` (blob-key) + `magic` (MIME sniff) + `zipbomb`.
//! - [`permissions`] — the `files::*` permission keys
//!   ([`ziee_identity::PermissionCheck`]).
//! - [`seams`] — the injected [`seams::FileProcessor`] / [`seams::FileEvents`].
//! - [`ingest`] — the STORE-half `store_processed` + the `ingest_bytes`
//!   orchestration convenience (both drive the seams).
//! - `migrations/` — the base `files`/`file_versions` schema (the app globs
//!   `sdk/crates/*/migrations/` into its merged set). The file FKs
//!   (`users`/self) + the `Users`-group permission grant + the new
//!   `file_workflow_runs` join table stay ziee-side (domain FKs).
//!
//! ziee keeps a thin re-export shim (`modules::file` → `pub use ziee_file::…`)
//! so its ~59 store consumers compile unchanged, plus its still-local
//! processing/handlers/routes/ingest-orchestration.

pub mod ingest;
pub mod models;
pub mod permissions;
pub mod repository;
pub mod seams;
pub mod storage;
pub mod types;
pub mod utils;

// Re-export the repository for the app's global `Repos.file` aggregator.
pub use repository::FileRepository;
pub use storage::{
    filesystem::FilesystemStorage,
    manager::{get_file_storage, init_file_storage},
    FileStorage,
};

/// The file store's base migrations (the `files`/`file_versions` tables +
/// indexes), embedded at build time. The app globs `sdk/crates/*/migrations/`
/// into ONE version-sorted merged set for BOTH its runtime `migrate` and its
/// build-DB provisioner; this crate's own `build.rs` provisions a file-only
/// build DB from exactly this set for its `query!` verification. Original
/// filename + version preserved so the merged `_sqlx_migrations` history is
/// unchanged.
pub static FILE_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
