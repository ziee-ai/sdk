// File management handlers (store-generic — chunk `ziee-file-http`).

use aide::transform::TransformOperation;
use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

use super::super::context::FileContext;
use super::download::FILE_HEAD_CACHE_CONTROL;
use crate::get_file_storage;
use crate::models::File;
use crate::permissions::{FilesDelete, FilesDownload, FilesPreview, FilesRead};
use crate::types::{FileListResponse, PaginationQuery, PreviewQuery, TextPageQuery};
use ziee_auth::{Group, User};
use ziee_core::{ApiResult, AppError};
use ziee_framework::permissions::{with_permission, IdentityResolver, RequirePermissions};
use ziee_framework::sync::SyncOrigin;
use uuid::Uuid;

/// List user's files
pub async fn list_files<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesRead,)>,
    Query(params): Query<PaginationQuery>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Json<FileListResponse>> {
    let user_id = auth.user.id;

    let (files, total) = ctx.files
        .list_by_user(user_id, params.page, params.per_page)
        .await?;

    Ok((
        StatusCode::OK,
        Json(FileListResponse {
            files,
            total,
            page: params.page,
            per_page: params.per_page,
        }),
    ))
}

/// Get file metadata
pub async fn get_file<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesRead,)>,
    Path(file_id): Path<Uuid>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Json<File>> {
    let user_id = auth.user.id;

    let file = ctx.files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;

    Ok((StatusCode::OK, Json(file)))
}

/// Get file preview image (high quality)
pub async fn get_preview<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesPreview,)>,
    Path(file_id): Path<Uuid>,
    Query(query): Query<PreviewQuery>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Response> {
    let user_id = auth.user.id;

    // Verify file ownership + resolve head blob.
    let file = ctx.files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;

    // Load high-quality preview image
    let storage = get_file_storage();
    let image_data = storage
        .load_preview(user_id, file.blob_version_id, query.page)
        .await
        .map_err(|e| AppError::internal_with_id(&e).to_api_error())?;

    // Private, bounded cache (see FILE_CONTENT_CACHE_CONTROL) so reloads reuse
    // preview bytes without re-downloading every inline image — the main fix
    // for laggy reloads with many inline previews.
    let headers = [
        (header::CONTENT_TYPE, "image/jpeg".to_string()),
        (header::CONTENT_LENGTH, image_data.len().to_string()),
        (header::CACHE_CONTROL, FILE_HEAD_CACHE_CONTROL.to_string()),
    ];

    Ok((StatusCode::OK, (headers, image_data).into_response()))
}

/// Get a file's original bytes inline (for client-side rendering, e.g. PDF.js).
///
/// Gated by `FilesDownload` — same permission as `download_file`, because this
/// serves the EXACT original bytes (byte-identical to a download); it differs
/// only in `Content-Disposition: inline` so the browser hands the bytes to an
/// in-page renderer instead of triggering a save. Gating on `FilesDownload`
/// (not `FilesPreview`) means an admin who withholds download from a group also
/// withholds raw-byte access here — `FilesPreview` still governs the rendered
/// preview *images* (`get_preview`), which reveal visual content without handing
/// over the source file. The PDF viewer loads real PDFs from here (client-side
/// render), which is what removes the 50-page preview cap.
pub async fn get_raw<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesDownload,)>,
    Path(file_id): Path<Uuid>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Response> {
    let user_id = auth.user.id;

    // Verify ownership + resolve the head blob (cross-user → 404).
    let file = ctx.files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;

    // Extract extension (storage keys originals by extension), mirroring
    // download_file.
    let extension = file
        .filename
        .rsplit('.')
        .next()
        .unwrap_or("bin")
        .to_lowercase();

    let storage = get_file_storage();
    let file_data = storage
        .load_original(user_id, file.blob_version_id, &extension)
        .await
        .map_err(|_| AppError::not_found("File").to_api_error())?;

    // Inline disposition + the same private, bounded cache as preview/download
    // so reloads reuse bytes without a round-trip.
    let headers = [
        (
            header::CONTENT_TYPE,
            file.mime_type
                .as_deref()
                .unwrap_or("application/octet-stream")
                .to_string(),
        ),
        (header::CONTENT_DISPOSITION, "inline".to_string()),
        (header::CONTENT_LENGTH, file_data.len().to_string()),
        (header::CACHE_CONTROL, FILE_HEAD_CACHE_CONTROL.to_string()),
    ];

    Ok((StatusCode::OK, (headers, file_data).into_response()))
}

/// Query for the citation text-rects endpoint: a byte range into the page's
/// cleaned text (a chunk's char_start/char_end).
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct TextRectsQuery {
    pub page: u32,
    pub start: usize,
    pub end: usize,
}

#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct HighlightRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct TextRectsResponse {
    /// Rects are fraction-normalized to the page (0..1), origin top-left, so the
    /// UI overlays them on the page image without knowing the render scale.
    pub page_w: f32,
    pub page_h: f32,
    pub rects: Vec<HighlightRect>,
}

#[derive(serde::Deserialize)]
struct PageGeometry {
    text: String,
    boxes: Vec<[f32; 4]>,
}

/// Fraction-of-page vertical-center tolerance for grouping per-char boxes into
/// one line rect. A pure PDF-geometry rendering heuristic (not a deployment
/// policy), so it stays a named const rather than an admin setting.
const LINE_MERGE_Y_TOLERANCE: f32 = 0.012;

/// Relocate a chunk's cleaned span within the raw page text (whitespace-
/// insensitive) and return the merged line-level highlight rects. Empty when the
/// span can't be located (the UI falls back to a page-level open).
fn align_span_to_boxes(cleaned_substr: &str, geom: &PageGeometry) -> Vec<HighlightRect> {
    let raw_chars: Vec<char> = geom.text.chars().collect();
    // Indices (into raw_chars / boxes) of the non-whitespace chars.
    let raw_nows: Vec<usize> = raw_chars
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| i)
        .collect();
    let raw_nows_chars: Vec<char> = raw_nows.iter().map(|&i| raw_chars[i]).collect();
    let needle: Vec<char> = cleaned_substr.chars().filter(|c| !c.is_whitespace()).collect();
    if needle.is_empty() || needle.len() > raw_nows_chars.len() {
        return Vec::new();
    }
    let mut matched: Option<usize> = None;
    for start in 0..=(raw_nows_chars.len() - needle.len()) {
        if raw_nows_chars[start..start + needle.len()] == needle[..] {
            matched = Some(start);
            break;
        }
    }
    let Some(mp) = matched else { return Vec::new() };

    // Boxes for the matched raw chars (skip degenerate zero boxes).
    let mut sel: Vec<[f32; 4]> = raw_nows[mp..mp + needle.len()]
        .iter()
        .filter_map(|&i| geom.boxes.get(i).copied())
        .filter(|b| b[2] > 0.0 && b[3] > 0.0)
        .collect();
    if sel.is_empty() {
        return Vec::new();
    }
    // Merge into line-level rects: group boxes whose vertical center is close.
    sel.sort_by(|a, b| a[1].partial_cmp(&b[1]).unwrap_or(std::cmp::Ordering::Equal));
    let mut lines: Vec<[f32; 4]> = Vec::new(); // [minx, miny, maxx, maxy]
    for b in sel {
        let (bx, by, br, bb) = (b[0], b[1], b[0] + b[2], b[1] + b[3]);
        if let Some(last) = lines.last_mut() {
            let last_cy = (last[1] + last[3]) / 2.0;
            let b_cy = (by + bb) / 2.0;
            if (last_cy - b_cy).abs() < LINE_MERGE_Y_TOLERANCE {
                last[0] = last[0].min(bx);
                last[1] = last[1].min(by);
                last[2] = last[2].max(br);
                last[3] = last[3].max(bb);
                continue;
            }
        }
        lines.push([bx, by, br, bb]);
    }
    lines
        .into_iter()
        .map(|l| HighlightRect {
            x: l[0],
            y: l[1],
            w: (l[2] - l[0]).max(0.0),
            h: (l[3] - l[1]).max(0.0),
        })
        .collect()
}

#[cfg(test)]
mod align_tests {
    use super::{align_span_to_boxes, PageGeometry};

    // Build per-char boxes on a given line-y; each char is `w` wide stepping in x.
    fn line(chars: &str, y: f32, x0: f32, w: f32) -> Vec<[f32; 4]> {
        chars
            .chars()
            .enumerate()
            .map(|(i, _)| [x0 + i as f32 * w, y, w, 0.02])
            .collect()
    }

    // TEST-31 (ITEM-22): a cleaned span relocates onto the RAW page geometry
    // whitespace-insensitively (raw text may have collapsed multi-spaces), and
    // the matched char boxes merge into line-level rects bounding the CORRECT
    // passage — not merely a non-empty result.
    #[test]
    fn divergent_whitespace_span_bounds_correct_chars() {
        // Raw page text has a DOUBLE space that cleaning collapsed to one.
        let text = "Hello  world".to_string(); // 12 chars incl. 2 spaces
        let boxes = line("Hello  world", 0.10, 0.10, 0.05);
        let geom = PageGeometry { text, boxes };

        // The cleaned span uses a single space — must still match.
        let rects = align_span_to_boxes("Hello world", &geom);
        assert_eq!(rects.len(), 1, "one line → one merged rect");
        let r = &rects[0];
        // Bounds the 'H' (index 0, x=0.10) through 'd' (index 11, x=0.10+11*0.05).
        assert!((r.x - 0.10).abs() < 1e-4, "left edge at 'H': {}", r.x);
        let right = r.x + r.w;
        assert!((right - (0.10 + 12.0 * 0.05)).abs() < 1e-4, "right edge at 'd': {right}");
        assert!((r.y - 0.10).abs() < 1e-4);
    }

    #[test]
    fn multiline_span_yields_one_rect_per_line() {
        // "foo" on line 1 (y=0.10), "bar" on line 2 (y=0.50).
        let mut boxes = line("foo", 0.10, 0.10, 0.05);
        boxes.extend(line("bar", 0.50, 0.10, 0.05));
        let geom = PageGeometry { text: "foobar".to_string(), boxes };
        let rects = align_span_to_boxes("foobar", &geom);
        assert_eq!(rects.len(), 2, "two visually-separated lines → two rects");
        assert!(rects[0].y < rects[1].y);
    }

    #[test]
    fn unlocatable_span_returns_empty() {
        let geom = PageGeometry { text: "abc".to_string(), boxes: line("abc", 0.1, 0.1, 0.05) };
        assert!(align_span_to_boxes("xyz", &geom).is_empty());
        assert!(align_span_to_boxes("", &geom).is_empty());
    }
}

/// Citation highlight geometry: the fraction-normalized rectangles bounding a
/// chunk's cleaned `[start, end)` span on a page, for the exact-passage overlay.
/// Owner-scoped (foreign file → 404); non-PDF / no-geometry → `200 {rects:[]}`.
pub async fn get_text_rects<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesRead,)>,
    Path(file_id): Path<Uuid>,
    Query(q): Query<TextRectsQuery>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Json<TextRectsResponse>> {
    let user_id = auth.user.id;
    let file = ctx.files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;

    let empty = TextRectsResponse {
        page_w: 1.0,
        page_h: 1.0,
        rects: Vec::new(),
    };

    let storage = get_file_storage();
    let geom_json = match storage
        .load_geometry_page(user_id, file.blob_version_id, q.page)
        .await
    {
        Ok(s) => s,
        Err(_) => return Ok((StatusCode::OK, Json(empty))), // non-PDF / not-yet-backfilled
    };
    let geom: PageGeometry = match serde_json::from_str(&geom_json) {
        Ok(g) => g,
        Err(_) => return Ok((StatusCode::OK, Json(empty))),
    };

    let page_text = storage
        .load_text_page(user_id, file.blob_version_id, q.page)
        .await
        .unwrap_or_default();
    let start = q.start.min(page_text.len());
    let end = q.end.min(page_text.len()).max(start);
    // Slice on a char boundary defensively.
    let cleaned_substr = page_text
        .get(start..end)
        .unwrap_or("")
        .to_string();

    let rects = align_span_to_boxes(&cleaned_substr, &geom);
    Ok((
        StatusCode::OK,
        Json(TextRectsResponse {
            page_w: 1.0,
            page_h: 1.0,
            rects,
        }),
    ))
}

pub fn get_text_rects_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(FilesRead,)>(op)
        .id("File.getTextRects")
        .summary("Citation highlight rectangles for a chunk's span on a page.")
        .response::<200, Json<TextRectsResponse>>()
}

/// Get file thumbnail (300px, single thumbnail from first page)
pub async fn get_thumbnail<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesPreview,)>,
    Path(file_id): Path<Uuid>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Response> {
    let user_id = auth.user.id;

    // Verify file ownership + resolve head blob.
    let file = ctx.files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;

    // Load thumbnail
    let storage = get_file_storage();
    let thumbnail_data = storage
        .load_thumbnail(user_id, file.blob_version_id)
        .await
        .map_err(|e| AppError::internal_with_id(&e).to_api_error())?;

    // Private, bounded cache (see FILE_CONTENT_CACHE_CONTROL) — reuse across
    // reloads without a round-trip.
    let headers = [
        (header::CONTENT_TYPE, "image/jpeg".to_string()),
        (header::CONTENT_LENGTH, thumbnail_data.len().to_string()),
        (header::CACHE_CONTROL, FILE_HEAD_CACHE_CONTROL.to_string()),
    ];

    Ok((StatusCode::OK, (headers, thumbnail_data).into_response()))
}

/// Get extracted text content
pub async fn get_text_content<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesRead,)>,
    Path(file_id): Path<Uuid>,
    Query(query): Query<TextPageQuery>,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<Response> {
    let user_id = auth.user.id;

    // Verify file ownership and get file info
    let file = ctx.files
        .get_by_id_and_user(file_id, user_id)
        .await?
        .ok_or_else(|| AppError::not_found("File"))?;

    let storage = get_file_storage();
    let text_content = match query.page {
        Some(page_num) => {
            // Return specific page
            if page_num < 1 || page_num > file.text_page_count as u32 {
                return Err(AppError::bad_request(
                    "INVALID_PAGE",
                    format!("Page {} is out of range. Valid range: 1-{}", page_num, file.text_page_count),
                ).to_api_error());
            }
            storage
                .load_text_page(user_id, file.blob_version_id, page_num)
                .await
                .map_err(|e| AppError::internal_with_id(&e).to_api_error())?
        }
        None => {
            // Return all pages concatenated
            let mut text_content = String::new();
            for page_num in 1..=file.text_page_count {
                let page_text = storage
                    .load_text_page(user_id, file.blob_version_id, page_num as u32)
                    .await
                    .map_err(|e| AppError::internal_with_id(&e).to_api_error())?;
                if page_num > 1 {
                    text_content.push_str("\n\n--- Page ");
                    text_content.push_str(&page_num.to_string());
                    text_content.push_str(" ---\n\n");
                }
                text_content.push_str(&page_text);
            }
            text_content
        }
    };

    // Private, bounded cache (see FILE_CONTENT_CACHE_CONTROL) — reuse across
    // reloads without a round-trip.
    let headers = [
        (header::CONTENT_TYPE, "text/plain; charset=utf-8".to_string()),
        (header::CONTENT_LENGTH, text_content.len().to_string()),
        (header::CACHE_CONTROL, FILE_HEAD_CACHE_CONTROL.to_string()),
    ];

    Ok((StatusCode::OK, (headers, text_content).into_response()))
}

/// Delete file
pub async fn delete_file<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (FilesDelete,)>,
    Path(file_id): Path<Uuid>,
    origin: SyncOrigin,
    Extension(ctx): Extension<FileContext>,
) -> ApiResult<StatusCode> {
    let user_id = auth.user.id;

    // Delete from database (returns the DISTINCT blob_version_ids to purge).
    let blob_ids = ctx.files.delete(file_id, user_id).await?;

    // Delete each distinct version blob from storage. Restored versions share a
    // blob, so the repo already deduped — no double-delete / missing-blob.
    let storage = get_file_storage();
    for blob_id in blob_ids {
        storage.delete_all(user_id, blob_id).await?;
    }

    // Notify the owner's other devices so their file/version lists drop it.
    ctx.events.on_file_deleted(user_id, file_id, origin.0);

    Ok((StatusCode::NO_CONTENT, StatusCode::NO_CONTENT))
}

/// List files OpenAPI documentation
pub fn list_files_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(FilesRead,)>(op)
        .id("File.list")
        .tag("Files")
        .summary("List user's files")
        .description("Get paginated list of files uploaded by the current user")
        .response::<200, Json<FileListResponse>>()
        .response_with::<401, (), _>(|res| res.description("Unauthorized"))
}

/// Get file OpenAPI documentation
pub fn get_file_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(FilesRead,)>(op)
        .id("File.get")
        .tag("Files")
        .summary("Get file metadata")
        .description("Retrieve metadata for a specific file")
        .response::<200, Json<File>>()
        .response_with::<401, (), _>(|res| res.description("Unauthorized"))
        .response_with::<404, (), _>(|res| res.description("File not found"))
}

/// Get preview OpenAPI documentation
pub fn get_preview_docs(op: TransformOperation) -> TransformOperation {
    use crate::types::BlobType;

    with_permission::<(FilesPreview,)>(op)
        .id("File.getPreview")
        .tag("Files")
        .summary("Get preview image")
        .description("Get high-quality preview image for a specific page (2000px height)")
        .response::<200, Json<BlobType>>()
        .response_with::<401, (), _>(|res| res.description("Unauthorized"))
        .response_with::<404, (), _>(|res| res.description("File or preview not found"))
}

pub fn get_raw_docs(op: TransformOperation) -> TransformOperation {
    use crate::types::BlobType;

    with_permission::<(FilesDownload,)>(op)
        .id("File.getRaw")
        .tag("Files")
        .summary("Get raw file bytes (inline)")
        .description(
            "Get a file's original bytes inline for client-side rendering \
             (e.g. PDF.js). Gated by files::download (serves the exact original \
             bytes); Content-Disposition: inline.",
        )
        .response::<200, Json<BlobType>>()
        .response_with::<401, (), _>(|res| res.description("Unauthorized"))
        .response_with::<403, (), _>(|res| res.description("Missing files::download"))
        .response_with::<404, (), _>(|res| res.description("File not found"))
}

/// Get thumbnail OpenAPI documentation
pub fn get_thumbnail_docs(op: TransformOperation) -> TransformOperation {
    use crate::types::BlobType;

    with_permission::<(FilesPreview,)>(op)
        .id("File.getThumbnail")
        .tag("Files")
        .summary("Get thumbnail")
        .description("Get thumbnail image from first page (300px height)")
        .response::<200, Json<BlobType>>()
        .response_with::<401, (), _>(|res| res.description("Unauthorized"))
        .response_with::<404, (), _>(|res| res.description("File or thumbnail not found"))
}

/// Get text content OpenAPI documentation
pub fn get_text_content_docs(op: TransformOperation) -> TransformOperation {
    use crate::types::BlobType;

    with_permission::<(FilesRead,)>(op)
        .id("File.getTextContent")
        .tag("Files")
        .summary("Get extracted text")
        .description("Get extracted text content for a specific page or all pages")
        .response::<200, Json<BlobType>>()
        .response_with::<400, (), _>(|res| res.description("Invalid page number"))
        .response_with::<401, (), _>(|res| res.description("Unauthorized"))
        .response_with::<404, (), _>(|res| res.description("File not found"))
}

/// Delete file OpenAPI documentation
pub fn delete_file_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(FilesDelete,)>(op)
        .id("File.delete")
        .tag("Files")
        .summary("Delete file")
        .description("Delete a file and all associated data (thumbnails, extracted text, etc.)")
        .response_with::<204, (), _>(|res| res.description("File deleted successfully"))
        .response_with::<401, (), _>(|res| res.description("Unauthorized"))
        .response_with::<404, (), _>(|res| res.description("File not found"))
}
