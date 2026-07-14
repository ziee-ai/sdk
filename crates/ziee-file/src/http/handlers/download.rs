// File download handlers (store-generic subset — chunk `ziee-file-http`).
//
// `download_with_token` STAYED ziee-side: it re-verifies identity BY user-id
// from the signed claim (get_by_id + active + permission-union), which does not
// fit the request-`Parts`-based `IdentityResolver`. The `content_disposition`
// helper + the two cache-control consts moved here (this crate owns them now);
// ziee's retained `download_with_token` + `export_file` re-import them.

use aide::transform::TransformOperation;
use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use jsonwebtoken::{encode, EncodingKey, Header};

use super::super::context::FileContext;
use crate::get_file_storage;
use crate::permissions::{FilesDownload, FilesGenerateToken};
use crate::types::{
    DownloadTokenClaims, DownloadTokenGenQuery, DownloadTokenResponse, DOWNLOAD_TOKEN_AUDIENCE,
};
use ziee_auth::{Group, User};
use ziee_core::{ApiResult, AppError};
use ziee_framework::permissions::{with_permission, IdentityResolver, RequirePermissions};
use uuid::Uuid;

const TOKEN_EXPIRY: i64 = 3600; // 1 hour

/// `Cache-Control` for authenticated file-content responses (originals,
/// previews, thumbnails, extracted text). File bytes never change for a given
/// id, so a `fetch()` of a still-fresh entry serves from the browser's private
/// cache with no network round-trip — that's the reload-perf win for inline
/// previews.
///
/// `private` keeps shared/proxy caches from storing user-scoped content. We
/// deliberately do NOT use `immutable` + a multi-year max-age: this content is
/// access-controlled and can be revoked (account disabled, `files::download`
/// removed) or the file deleted. `immutable` would let a user's browser serve
/// it for the full lifetime with zero server round-trips, sidestepping the
/// server-side revocation re-check. A bounded 1-hour max-age keeps the
/// fast-reload win while capping how long a stale (post-revocation) copy can
/// live in one user's own cache.
pub const FILE_CONTENT_CACHE_CONTROL: &str = "private, max-age=3600";

/// Cache policy for endpoints that serve a file's **head** content keyed by
/// `file_id` (text/content/preview/thumbnail). Unlike a version-pinned URL,
/// the head's bytes CHANGE when a new version is appended (user canvas Save,
/// MCP `edit_file`/`rewrite_file`), so a `max-age=3600` cache would serve the
/// PRE-EDIT content for up to an hour — a co-edited deliverable silently shows
/// stale text after Save + reload. `no-cache` forces revalidation (the browser
/// may store but must re-check before use), so an edit is visible immediately
/// while the response is still `private` (never shared-cached). Closes the
/// canvas stale-after-save bug.
pub const FILE_HEAD_CACHE_CONTROL: &str = "private, no-cache";

/// Build a `Content-Disposition: attachment; filename=...; filename*=UTF-8''...`
/// header value that is safe regardless of what the stored filename
/// contains. Closes 05-file F-08 (Medium): the previous implementation
/// did `format!("attachment; filename=\"{}\"", file.filename)` which
/// inserted user-controlled bytes directly into the header — a
/// filename like `evil";\r\nSet-Cookie: foo=bar` would inject
/// arbitrary response headers (CRLF injection) or break out of the
/// quoted form to confuse downstream parsers.
///
/// Rationale:
///   - filename= form: ASCII-only, all unsafe bytes replaced with '_'.
///     Browsers fall back to this when filename* is missing or unparseable.
///   - filename*=UTF-8''<percent-encoded>: RFC 5987 form. Carries the
///     real (multibyte) filename. Percent-encoding makes CRLF / quote
///     injection impossible.
pub fn content_disposition(filename: &str) -> String {
    let ascii: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let percent: String = filename
        .bytes()
        .flat_map(|b| {
            // RFC 5987 attr-char set + percent-encode everything else.
            if b.is_ascii_alphanumeric() || matches!(b, b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~') {
                vec![b]
            } else {
                format!("%{:02X}", b).into_bytes()
            }
        })
        .map(|b| b as char)
        .collect();
    format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        ascii, percent
    )
}

/// Download file directly (requires authentication)
pub async fn download_file<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesDownload,)>,
    Path(file_id): Path<Uuid>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Response> {
    let user_id = auth.user.id;

    // Get file and verify ownership
    let file = ctx.files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;

    // Extract extension
    let extension = file
        .filename
        .rsplit('.')
        .next()
        .unwrap_or("bin")
        .to_lowercase();

    // Load head version's bytes (keyed by the head's blob_version_id).
    let storage = get_file_storage();
    let file_data = storage
        .load_original(user_id, file.blob_version_id, &extension)
        .await
        .map_err(|_| AppError::not_found("File"))?;

    // Build response headers using array of tuples (like reference implementation)
    let headers = [
        (
            header::CONTENT_TYPE,
            file.mime_type
                .as_deref()
                .unwrap_or("application/octet-stream")
                .to_string(),
        ),
        (
            header::CONTENT_DISPOSITION,
            content_disposition(&file.filename),
        ),
        (header::CONTENT_LENGTH, file_data.len().to_string()),
        // Private, bounded cache so reloads reuse bytes without a round-trip
        // while still re-checking access after the max-age window. See
        // FILE_CONTENT_CACHE_CONTROL.
        (header::CACHE_CONTROL, FILE_CONTENT_CACHE_CONTROL.to_string()),
    ];

    Ok((StatusCode::OK, (headers, file_data).into_response()))
}

/// Generate download token
pub async fn generate_download_token<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesGenerateToken,)>,
    Path(file_id): Path<Uuid>,
    Query(gen_q): Query<DownloadTokenGenQuery>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Json<DownloadTokenResponse>> {
    let user_id = auth.user.id;

    // Verify file exists and user owns it
    ctx.files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;

    // If a version was requested, verify it exists (and is owned) before
    // baking it into the signed claims.
    if let Some(v) = gen_q.version
        && ctx.files.get_version(file_id, v, user_id).await?.is_none()
    {
        return Err(AppError::bad_request("INVALID_VERSION", format!("version {v} does not exist")).into());
    }

    // Generate JWT token. Sets iss + aud (audience=ziee-download)
    // so the validator (ziee's `download_with_token`) can refuse cross-audience
    // replay against the access-token validator. Closes 02-permissions F-03.
    let now = chrono::Utc::now().timestamp() as usize;
    let claims = DownloadTokenClaims {
        file_id: file_id.to_string(),
        user_id: user_id.to_string(),
        version: gen_q.version,
        exp: now + TOKEN_EXPIRY as usize,
        iat: now,
        iss: ctx.download_token.issuer.clone(),
        aud: DOWNLOAD_TOKEN_AUDIENCE.to_string(),
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(ctx.download_token.secret.as_bytes()),
    )
    .map_err(|e| AppError::internal_error(format!("Failed to generate token: {}", e)))?;

    Ok((
        StatusCode::OK,
        Json(DownloadTokenResponse {
            token,
            expires_in: TOKEN_EXPIRY,
        }),
    ))
}

/// Download file OpenAPI documentation
pub fn download_file_docs(op: TransformOperation) -> TransformOperation {
    use crate::types::BlobType;

    with_permission::<(FilesDownload,)>(op)
        .id("File.download")
        .tag("Files")
        .summary("Download file")
        .description("Download the original file (requires authentication)")
        .response::<200, Json<BlobType>>()
        .response_with::<401, (), _>(|res| res.description("Unauthorized"))
        .response_with::<404, (), _>(|res| res.description("File not found"))
}

/// Generate download token OpenAPI documentation
pub fn generate_download_token_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(FilesGenerateToken,)>(op)
        .id("File.generateDownloadToken")
        .tag("Files")
        .summary("Generate download token")
        .description("Generate a time-limited token for downloading a file without authentication")
        .response::<200, Json<DownloadTokenResponse>>()
        .response_with::<401, (), _>(|res| res.description("Unauthorized"))
        .response_with::<404, (), _>(|res| res.description("File not found"))
}
