// File version handlers (store-generic subset — chunk `ziee-file-http`) — list
// versions, read a pinned version's metadata/bytes, and restore (append-only) a
// prior version as the new head.
//
// `append_version` STAYED ziee-side: it appends from user text via
// `commit_new_version`, which runs the `ProcessingManager` producer (text
// extraction / thumbnails) — a processing-coupled path the store does not carry.

use aide::transform::TransformOperation;
use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use super::super::context::FileContext;
use super::download::{content_disposition, FILE_CONTENT_CACHE_CONTROL};
use crate::get_file_storage;
use crate::models::{File, FileVersion};
use crate::permissions::{FilesDownload, FilesPreview, FilesRead, FilesUpload};
use crate::repository::FileRepository;
use crate::types::{PreviewQuery, TextPageQuery};
use ziee_auth::{Group, User};
use ziee_core::{ApiResult, AppError};
use ziee_framework::permissions::{with_permission, IdentityResolver, RequirePermissions};

/// Pagination default + max page size (mirrors ziee's
/// `common::{DEFAULT_PAGE_SIZE, PAGINATION_MAX_PER_PAGE}` = 100 each; inlined so
/// the store surface has no `crate::common` reach, exactly as the store
/// repository inlined the same constant in chunk `ziee-file`).
const VERSIONS_DEFAULT_LIMIT: i64 = 100;
const VERSIONS_MAX_LIMIT: i64 = 100;

/// Body for `POST /files/{id}/restore`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestoreVersionRequest {
    /// The version number to restore (a new head is appended with its bytes).
    pub version: i32,
}

/// Optional pagination for the versions list. Defaults bound an
/// un-paginated caller to the most recent `DEFAULT_PAGE_SIZE` versions
/// (newest first) instead of returning an unbounded set.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListVersionsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// List versions of a file (newest first), paginated.
pub async fn list_versions<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesRead,)>,
    Path(file_id): Path<Uuid>,
    Query(q): Query<ListVersionsQuery>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Json<Vec<FileVersion>>> {
    let user_id = auth.user.id;
    // 404 if the file isn't the user's (don't leak existence).
    ctx.files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;
    let limit = q
        .limit
        .unwrap_or(VERSIONS_DEFAULT_LIMIT)
        .clamp(1, VERSIONS_MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    let versions = ctx.files
        .list_versions(file_id, user_id, limit, offset)
        .await?;
    Ok((StatusCode::OK, Json(versions)))
}

/// Get the current head version's metadata.
pub async fn get_head_version<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesRead,)>,
    Path(file_id): Path<Uuid>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Json<FileVersion>> {
    let user_id = auth.user.id;
    let head = ctx.files
        .get_head(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;
    Ok((StatusCode::OK, Json(head)))
}

/// Get a specific version's metadata.
pub async fn get_version<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesRead,)>,
    Path((file_id, version)): Path<(Uuid, i32)>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Json<FileVersion>> {
    let user_id = auth.user.id;
    let v = ctx.files
        .get_version(file_id, version, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File version"))?;
    Ok((StatusCode::OK, Json(v)))
}

/// Restore a prior version: append a new head whose bytes equal the target's.
/// No-op (returns the current head) if the target is already the head.
///
/// Gated on `FilesUpload`, not `FilesRead`/`FilesPreview`: restore is a WRITE —
/// it appends a new version and moves the file's head — so it requires the same
/// permission as uploading/editing, not merely reading. (The default `Users`
/// group has `files::upload`, so this is available to normal users.)
pub async fn restore_version<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesUpload,)>,
    Path(file_id): Path<Uuid>,
    origin: ziee_framework::sync::SyncOrigin,
    Extension(ctx): Extension<FileContext>,
    Json(req): Json<RestoreVersionRequest>,
) -> ApiResult<Json<File>> {
    let user_id = auth.user.id;
    let head = ctx.files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;

    // No-op: already at the requested version.
    if req.version == head.version {
        return Ok((StatusCode::OK, Json(head)));
    }
    // Reject a non-existent target with 400 (rather than silently appending).
    if ctx.files
        .get_version(file_id, req.version, user_id)
        .await?
        .is_none()
    {
        return Err(AppError::bad_request(
            "INVALID_VERSION",
            format!("version {} does not exist", req.version),
        )
        .into());
    }

    ctx.files
        .restore_version(file_id, req.version, "user".to_string(), None)
        .await?;

    // Sync: a restore changes the head. Skip the originating device's echo.
    ctx.events.on_file_changed(user_id, file_id, origin.0);

    // Document RAG: a restore makes a different version the head → re-index.
    ctx.events.on_committed(user_id, file_id, false);

    let updated = ctx.files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;
    Ok((StatusCode::OK, Json(updated)))
}

/// Helper: resolve (file head/filename, target version) and load that version's
/// original bytes for the pinned download/preview/text endpoints.
async fn version_and_file(
    files: &FileRepository,
    file_id: Uuid,
    version: i32,
    user_id: Uuid,
) -> Result<(File, FileVersion), AppError> {
    let file = files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;
    let v = files
        .get_version(file_id, version, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File version"))?;
    Ok((file, v))
}

/// Download a specific version's original bytes.
pub async fn download_version<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesDownload,)>,
    Path((file_id, version)): Path<(Uuid, i32)>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Response> {
    let user_id = auth.user.id;
    let (file, v) = version_and_file(&ctx.files, file_id, version, user_id).await?;
    let extension = file
        .filename
        .rsplit('.')
        .next()
        .unwrap_or("bin")
        .to_lowercase();
    let storage = get_file_storage();
    let bytes = storage
        .load_original(user_id, v.blob_version_id, &extension)
        .await
        .map_err(|_| AppError::not_found("File version"))?;
    let headers = [
        (
            header::CONTENT_TYPE,
            v.mime_type
                .as_deref()
                .unwrap_or("application/octet-stream")
                .to_string(),
        ),
        (header::CONTENT_DISPOSITION, content_disposition(&file.filename)),
        (header::CONTENT_LENGTH, bytes.len().to_string()),
        (header::CACHE_CONTROL, FILE_CONTENT_CACHE_CONTROL.to_string()),
    ];
    Ok((StatusCode::OK, (headers, bytes).into_response()))
}

/// Get a specific version's preview image.
pub async fn preview_version<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesPreview,)>,
    Path((file_id, version)): Path<(Uuid, i32)>,
    Query(query): Query<PreviewQuery>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Response> {
    let user_id = auth.user.id;
    let (_file, v) = version_and_file(&ctx.files, file_id, version, user_id).await?;
    let storage = get_file_storage();
    let image = storage
        .load_preview(user_id, v.blob_version_id, query.page)
        .await
        .map_err(|e| AppError::internal_error(format!("preview load failed: {e}")))?;
    let headers = [
        (header::CONTENT_TYPE, "image/jpeg".to_string()),
        (header::CONTENT_LENGTH, image.len().to_string()),
        (header::CACHE_CONTROL, FILE_CONTENT_CACHE_CONTROL.to_string()),
    ];
    Ok((StatusCode::OK, (headers, image).into_response()))
}

/// Get a specific version's extracted text.
pub async fn text_version<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesRead,)>,
    Path((file_id, version)): Path<(Uuid, i32)>,
    Query(query): Query<TextPageQuery>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Response> {
    let user_id = auth.user.id;
    let (_file, v) = version_and_file(&ctx.files, file_id, version, user_id).await?;
    let storage = get_file_storage();
    let text = match query.page {
        Some(page_num) => {
            if page_num < 1 || page_num > v.text_page_count as u32 {
                return Err(AppError::bad_request("INVALID_PAGE", "page out of range").into());
            }
            storage
                .load_text_page(user_id, v.blob_version_id, page_num)
                .await
                .map_err(|e| AppError::internal_error(format!("text load failed: {e}")))?
        }
        None => {
            let mut out = String::new();
            for page_num in 1..=v.text_page_count {
                let page = storage
                    .load_text_page(user_id, v.blob_version_id, page_num as u32)
                    .await
                    .map_err(|e| AppError::internal_error(format!("text load failed: {e}")))?;
                if page_num > 1 {
                    out.push_str("\n\n--- Page ");
                    out.push_str(&page_num.to_string());
                    out.push_str(" ---\n\n");
                }
                out.push_str(&page);
            }
            out
        }
    };
    let headers = [
        (header::CONTENT_TYPE, "text/plain; charset=utf-8".to_string()),
        (header::CONTENT_LENGTH, text.len().to_string()),
        (header::CACHE_CONTROL, FILE_CONTENT_CACHE_CONTROL.to_string()),
    ];
    Ok((StatusCode::OK, (headers, text).into_response()))
}

// ---- OpenAPI docs ----

pub fn list_versions_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(FilesRead,)>(op)
        .id("File.listVersions")
        .tag("Files")
        .summary("List file versions")
        .response::<200, Json<Vec<FileVersion>>>()
        .response_with::<401, (), _>(|res| res.description("Unauthorized"))
        .response_with::<404, (), _>(|res| res.description("File not found"))
}

pub fn get_head_version_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(FilesRead,)>(op)
        .id("File.getHeadVersion")
        .tag("Files")
        .summary("Get the head version")
        .response::<200, Json<FileVersion>>()
        .response_with::<404, (), _>(|res| res.description("File not found"))
}

pub fn get_version_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(FilesRead,)>(op)
        .id("File.getVersion")
        .tag("Files")
        .summary("Get a specific file version")
        .response::<200, Json<FileVersion>>()
        .response_with::<404, (), _>(|res| res.description("File or version not found"))
}

pub fn restore_version_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(FilesUpload,)>(op)
        .id("File.restore")
        .tag("Files")
        .summary("Restore a prior version (append-only)")
        .response::<200, Json<File>>()
        .response_with::<400, (), _>(|res| res.description("Invalid version"))
        .response_with::<404, (), _>(|res| res.description("File not found"))
}

pub fn download_version_docs(op: TransformOperation) -> TransformOperation {
    use crate::types::BlobType;
    with_permission::<(FilesDownload,)>(op)
        .id("File.downloadVersion")
        .tag("Files")
        .summary("Download a specific version's bytes")
        .response::<200, Json<BlobType>>()
        .response_with::<404, (), _>(|res| res.description("File or version not found"))
}

pub fn preview_version_docs(op: TransformOperation) -> TransformOperation {
    use crate::types::BlobType;
    with_permission::<(FilesPreview,)>(op)
        .id("File.previewVersion")
        .tag("Files")
        .summary("Preview a specific version")
        .response::<200, Json<BlobType>>()
        .response_with::<404, (), _>(|res| res.description("File or version not found"))
}

pub fn text_version_docs(op: TransformOperation) -> TransformOperation {
    use crate::types::BlobType;
    with_permission::<(FilesRead,)>(op)
        .id("File.textVersion")
        .tag("Files")
        .summary("Get a specific version's extracted text")
        .response::<200, Json<BlobType>>()
        .response_with::<404, (), _>(|res| res.description("File or version not found"))
}
