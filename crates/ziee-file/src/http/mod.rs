//! The file module's HTTP/aide surface — the mountable, store-generic routes
//! bundle.
//!
//! Chunk `ziee-file-http` completes the file extraction (deferred by `ziee-file`
//! to preserve OpenAPI byte-identity): it moves the **store-generic** file HTTP
//! handlers (list / get / preview / raw / thumbnail / text / text-rects / delete /
//! download / download-token / version reads / restore) out of ziee's
//! `modules/file/{handlers,routes}` into a mountable [`file_routes`] bundle,
//! generic over the app's injected
//! [`ziee_framework::permissions::IdentityResolver`] (fixed to this crate's
//! `ziee_auth::{User, Group}` wire types, since every file response is owner-
//! scoped by `user.id`). A second app mounts working file endpoints instead of
//! wiring its own.
//!
//! What STAYS ziee-side (processing / domain coupled, not the reusable store):
//! - `upload_file` — the `ProcessingManager` producer + quota + RAG.
//! - `export_file` — pandoc conversion.
//! - `append_version` — `commit_new_version` (the `ProcessingManager` producer).
//! - `download_with_token` — re-verifies identity BY user-id from the token
//!   claim (`get_by_id` + active + permission-union), which does not fit the
//!   request-`Parts`-based [`ziee_framework::permissions::IdentityResolver`]
//!   cleanly; it keeps using ziee's own re-check.
//! - the conversation `deliverables` routes — a chat/domain surface.
//!
//! Gated behind the default-on `routes` cargo feature (which turns on the
//! aide/axum/ziee-framework/ziee-auth + jsonwebtoken deps); the store ENGINE
//! compiles without it.

pub mod context;
pub mod handlers;
pub mod routes;

pub use context::{DownloadTokenSigner, FileContext};
pub use handlers::download::{
    content_disposition, FILE_CONTENT_CACHE_CONTROL, FILE_HEAD_CACHE_CONTROL,
};
pub use routes::file_routes;
