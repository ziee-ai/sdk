// File routes (store-generic subset — chunk `ziee-file-http`).
//
// `file_routes::<R>()` is the mountable bundle of the store-generic file
// endpoints, generic over the app's injected `IdentityResolver` (fixed to this
// crate's `ziee_auth::{User, Group}` wire types). ziee merges it into its own
// `file_router()`, which additionally registers the routes that STAYED ziee-side
// (upload / export / download-with-token / the version-append POST / the
// conversation deliverables). Route paths + operationIds are byte-identical to
// the pre-move single router, and the emitted OpenAPI is order-independent
// (paths + schemas + operationIds are sorted at TS emit / compared canonically),
// so splitting the router across two crates does not move the spec.

use aide::axum::routing::{delete_with, get_with, post_with};
use aide::axum::ApiRouter;

use super::handlers::*;
use ziee_framework::permissions::IdentityResolver;
use ziee_auth::{Group, User};

/// The store-generic file management routes — mounted (merged) by the app.
pub fn file_routes<R: IdentityResolver<User = User, Group = Group>>() -> ApiRouter {
    ApiRouter::new()
        // List files
        .api_route("/files", get_with(list_files::<R>, list_files_docs))
        // Binary endpoints (must come BEFORE /files/{file_id} to avoid route conflicts)
        .api_route("/files/{file_id}/preview", get_with(get_preview::<R>, get_preview_docs))
        .api_route("/files/{file_id}/raw", get_with(get_raw::<R>, get_raw_docs))
        .api_route("/files/{file_id}/thumbnail", get_with(get_thumbnail::<R>, get_thumbnail_docs))
        .api_route("/files/{file_id}/text", get_with(get_text_content::<R>, get_text_content_docs))
        .api_route("/files/{file_id}/text-rects", get_with(get_text_rects::<R>, get_text_rects_docs))
        .api_route("/files/{file_id}/download", get_with(download_file::<R>, download_file_docs))
        // Version endpoints (also before /files/{file_id}). The GET (list) is
        // store-generic and lives here; the POST (append_version) is
        // processing-coupled and stays ziee-side — the two method routers merge
        // on the same path.
        .api_route(
            "/files/{file_id}/versions",
            get_with(list_versions::<R>, list_versions_docs),
        )
        .api_route("/files/{file_id}/head", get_with(get_head_version::<R>, get_head_version_docs))
        .api_route("/files/{file_id}/versions/{version}", get_with(get_version::<R>, get_version_docs))
        .api_route("/files/{file_id}/versions/{version}/download", get_with(download_version::<R>, download_version_docs))
        .api_route("/files/{file_id}/versions/{version}/preview", get_with(preview_version::<R>, preview_version_docs))
        .api_route("/files/{file_id}/versions/{version}/text", get_with(text_version::<R>, text_version_docs))
        .api_route("/files/{file_id}/restore", post_with(restore_version::<R>, restore_version_docs))
        // Get file metadata
        .api_route("/files/{file_id}", get_with(get_file::<R>, get_file_docs))
        // Download token generation
        .api_route(
            "/files/{file_id}/download-token",
            post_with(generate_download_token::<R>, generate_download_token_docs),
        )
        // Delete
        .api_route(
            "/files/{file_id}",
            delete_with(delete_file::<R>, delete_file_docs),
        )
}
