use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

// =====================================================
// API Result Type
// =====================================================

/// API result type that includes HTTP status code
/// This is the standard return type for all API handlers
///
/// # Examples
///
/// ```ignore
/// use axum::{http::StatusCode, Json};
/// use crate::common::type::ApiResult;
///
/// async fn my_handler() -> ApiResult<Json<MyResponse>> {
///     Ok((StatusCode::OK, Json(MyResponse { /* ... */ })))
/// }
/// ```
pub type ApiResult<T> = Result<(StatusCode, T), (StatusCode, AppError)>;

// =====================================================
// Error Types
// =====================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
    pub error_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppError {
    status_code: u16,
    error_code: String,
    message: String,
    details: Option<serde_json::Value>,
}

impl AppError {
    pub fn new(
        status_code: StatusCode,
        error_code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status_code: status_code.as_u16(),
            error_code: error_code.into(),
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// The HTTP status code this error will be serialized with.
    /// Exposed so callers can branch on common cases (e.g.
    /// `if err.status_code() == 404` to tolerate idempotent deletes
    /// of already-gone resources).
    pub fn status_code(&self) -> u16 {
        self.status_code
    }

    /// The stable machine-readable error code this error serializes with
    /// (e.g. `VALIDATION_ERROR`, `RESOURCE_NOT_FOUND`,
    /// `MCP_TRANSPORT_NOT_ALLOWED`). Exposed so callers + tests can assert on
    /// it without parsing the human-readable `message`, and so the JSON-RPC
    /// handlers can map a specific code onto the right error class (e.g.
    /// `UNKNOWN_TOOL` → method-not-found) instead of collapsing to internal.
    pub fn error_code(&self) -> &str {
        &self.error_code
    }

    // Common convenience constructors
    pub fn not_found(resource: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "RESOURCE_NOT_FOUND",
            format!("{} not found", resource),
        )
    }

    pub fn conflict(resource: &str) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "RESOURCE_CONFLICT",
            format!("{} already exists", resource),
        )
    }

    pub fn bad_request(error_code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, error_code, message)
    }

    pub fn unprocessable_entity(error_code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, error_code, message)
    }

    pub fn unauthorized(error_code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, error_code, message)
    }

    pub fn forbidden(error_code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, error_code, message)
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "SYSTEM_INTERNAL_ERROR",
            message,
        )
    }

    /// Convert a database error into a client-safe AppError.
    ///
    /// The inner error's Display (and Debug) text — which frequently contains
    /// SQL constraint names, column values, or bound parameters from sqlx —
    /// is NEVER returned to the client. The full error is logged server-side
    /// via `tracing::error!` with a UUID trace_id; the same trace_id is
    /// embedded in the response's `details.trace_id` so support can grep the
    /// log to find the original error.
    pub fn database_error(err: impl std::fmt::Display) -> Self {
        let trace_id = Uuid::new_v4();
        tracing::error!(%trace_id, error = %err, "Database error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "SYSTEM_DATABASE_ERROR",
            "An internal database error occurred",
        )
        .with_details(serde_json::json!({ "trace_id": trace_id.to_string() }))
    }

    /// Convert a non-database error into a client-safe AppError.
    ///
    /// Use this for any error chain you DON'T want surfaced to the client
    /// (filesystem errors, third-party library errors, deserialization
    /// internals). The inner error is logged with a UUID trace_id; the
    /// client sees only a generic message + the trace_id for correlation.
    ///
    /// For developer-curated safe messages (\"resource not ready\",
    /// \"feature not enabled\"), use [`AppError::internal_error`] instead —
    /// it does no logging and embeds the supplied string verbatim.
    pub fn internal_with_id<E: std::fmt::Display>(err: E) -> Self {
        let trace_id = Uuid::new_v4();
        tracing::error!(%trace_id, error = %err, "Internal server error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "SYSTEM_INTERNAL_ERROR",
            "An internal error occurred",
        )
        .with_details(serde_json::json!({ "trace_id": trace_id.to_string() }))
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = Json(ApiError {
            error: self.message,
            error_code: self.error_code,
            details: self.details,
        });

        let status =
            StatusCode::from_u16(self.status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, body).into_response()
    }
}

// Conversion from common error types
impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::RowNotFound => AppError::not_found("Resource"),
            _ => AppError::database_error(err),
        }
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for AppError {
    fn from(err: Box<dyn std::error::Error + Send + Sync>) -> Self {
        AppError::internal_with_id(err)
    }
}

// Helper conversions for ApiResult
impl From<AppError> for (StatusCode, AppError) {
    fn from(err: AppError) -> Self {
        let status =
            StatusCode::from_u16(err.status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, err)
    }
}

// Helper to convert sqlx errors to ApiResult error type
impl AppError {
    pub fn to_api_error(self) -> (StatusCode, Self) {
        let status =
            StatusCode::from_u16(self.status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    /// Regression test for the 2026-05 audit cross-cutting finding:
    /// `AppError::database_error` MUST NOT include the inner error's Display
    /// (or Debug) text in the response body — that text often contains SQL
    /// constraint names, table names, columns, or even bound parameter values
    /// from `sqlx` and similar libraries. The real error stays in the server
    /// log via `tracing::error!`; the client gets only a correlation id.
    #[test]
    fn database_error_does_not_leak_inner_error_display() {
        let inner = io::Error::other(
            "secret_constraint_uq_users_email_AT_user_a@example.com",
        );
        let err = AppError::database_error(&inner);
        let body = serde_json::to_string(&err).expect("serialize AppError");
        assert!(
            !body.contains("secret_constraint_uq_users_email"),
            "AppError::database_error leaked inner error to response body: {}",
            body
        );
        assert!(
            !body.contains("user_a@example.com"),
            "AppError::database_error leaked sensitive value to response body: {}",
            body
        );
    }

    /// `internal_with_id` (the redacted boxed-error path) must not leak its
    /// inner error to the response body either.
    #[test]
    fn internal_with_id_does_not_leak_inner_error_display() {
        let inner: Box<dyn std::error::Error + Send + Sync> =
            "leaked_sentinel_internal_error_text".into();
        let err = AppError::internal_with_id(&*inner);
        let body = serde_json::to_string(&err).expect("serialize AppError");
        assert!(
            !body.contains("leaked_sentinel_internal_error_text"),
            "AppError::internal_with_id leaked inner error to response body: {}",
            body
        );
    }

    /// `From<sqlx::Error>` is invoked implicitly via `?` across the codebase.
    /// It MUST route through `database_error` so the inner SQL details never
    /// reach the client. Use `Encode` so we get a deterministic Display that
    /// would obviously be a leak.
    #[test]
    fn from_sqlx_error_does_not_leak_inner_error_display() {
        let inner = sqlx::Error::Configuration(
            "sentinel_pgpassword=hunter2_LEAKED".into(),
        );
        let err: AppError = inner.into();
        let body = serde_json::to_string(&err).expect("serialize AppError");
        assert!(
            !body.contains("hunter2_LEAKED"),
            "From<sqlx::Error> leaked inner to response body: {}",
            body
        );
        assert!(
            !body.contains("sentinel_pgpassword"),
            "From<sqlx::Error> leaked inner to response body: {}",
            body
        );
    }

    /// `From<Box<dyn Error>>` must also not leak — historically it called
    /// `internal_error(err.to_string())` which embedded the chain verbatim.
    #[test]
    fn from_boxed_error_does_not_leak_inner_error_display() {
        let inner: Box<dyn std::error::Error + Send + Sync> =
            "boxed_sentinel_LEAKED_secret_path=/etc/shadow".into();
        let err: AppError = inner.into();
        let body = serde_json::to_string(&err).expect("serialize AppError");
        assert!(
            !body.contains("boxed_sentinel_LEAKED"),
            "From<Box<dyn Error>> leaked inner to response body: {}",
            body
        );
    }

    /// Redacted errors should include a trace_id in `details` so support can
    /// grep the server log for the matching tracing event.
    #[test]
    fn database_error_includes_trace_id_for_correlation() {
        let inner = io::Error::other("x");
        let err = AppError::database_error(&inner);
        let body = serde_json::to_value(&err).expect("serialize AppError");
        let trace_id = body
            .get("details")
            .and_then(|d| d.get("trace_id"))
            .and_then(|t| t.as_str())
            .expect("AppError::database_error must embed trace_id in details");
        assert_eq!(
            trace_id.len(),
            36,
            "trace_id should be a UUID v4 ({} chars), got: {}",
            36,
            trace_id
        );
    }

    /// The static-message convenience constructors (`not_found`, `forbidden`,
    /// etc.) are explicitly safe — keep behavior so callers don't have to
    /// switch to a different API.
    #[test]
    fn not_found_does_not_route_through_redaction() {
        let err = AppError::not_found("Conversation");
        let body = serde_json::to_value(&err).expect("serialize AppError");
        assert_eq!(body["error_code"], "RESOURCE_NOT_FOUND");
        // No trace_id for safe constructors — they aren't logging anything
        // sensitive that a developer would need to correlate.
        assert!(body.get("details").is_none() || body["details"].is_null());
    }

    /// The convenience constructors must map to the correct HTTP status +
    /// stable machine-readable error_code — both feed the wire contract and
    /// callers branch on `status_code()` (e.g. tolerate idempotent 404 deletes).
    #[test]
    fn convenience_constructors_map_status_and_code() {
        let cases: &[(AppError, u16, &str)] = &[
            (AppError::not_found("X"), 404, "RESOURCE_NOT_FOUND"),
            (AppError::conflict("X"), 409, "RESOURCE_CONFLICT"),
            (
                AppError::bad_request("BAD", "m"),
                400,
                "BAD",
            ),
            (
                AppError::unprocessable_entity("UNPROC", "m"),
                422,
                "UNPROC",
            ),
            (
                AppError::unauthorized("UNAUTH", "m"),
                401,
                "UNAUTH",
            ),
            (AppError::forbidden("FORB", "m"), 403, "FORB"),
            (
                AppError::internal_error("m"),
                500,
                "SYSTEM_INTERNAL_ERROR",
            ),
        ];
        for (err, status, code) in cases {
            assert_eq!(err.status_code(), *status, "status for {code}");
            assert_eq!(err.error_code(), *code, "error_code for {code}");
        }
    }

    #[test]
    fn conflict_and_not_found_messages_name_the_resource() {
        assert_eq!(AppError::not_found("User").to_string(), "User not found");
        assert_eq!(
            AppError::conflict("User").to_string(),
            "User already exists"
        );
    }

    #[test]
    fn with_details_attaches_and_serializes_details() {
        let err = AppError::bad_request("BAD", "nope")
            .with_details(serde_json::json!({ "field": "email" }));
        let body = serde_json::to_value(&err).unwrap();
        assert_eq!(body["details"]["field"], "email");
    }

    /// `IntoResponse` must translate the stored u16 status into the real HTTP
    /// response status and emit the `ApiError` body (error/error_code/details).
    #[tokio::test]
    async fn into_response_uses_stored_status_and_body() {
        use axum::body::to_bytes;
        use axum::response::IntoResponse;

        let err = AppError::forbidden("FORBIDDEN", "denied")
            .with_details(serde_json::json!({ "k": "v" }));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "denied");
        assert_eq!(body["error_code"], "FORBIDDEN");
        assert_eq!(body["details"]["k"], "v");
    }

    /// An out-of-range stored status falls back to 500 rather than panicking.
    #[tokio::test]
    async fn into_response_bad_status_falls_back_to_500() {
        use axum::response::IntoResponse;
        // Construct via serde to smuggle in an impossible status code (below the
        // http crate's valid 100..=999 range).
        let err: AppError = serde_json::from_value(serde_json::json!({
            "status_code": 42,
            "error_code": "WAT",
            "message": "m",
            "details": null
        }))
        .unwrap();
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn to_api_error_and_tuple_from_carry_matching_status() {
        let (status, err) = AppError::unauthorized("UNAUTH", "m").to_api_error();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(err.status_code(), 401);

        // `From<AppError> for (StatusCode, AppError)` (used by `?` in handlers).
        let tuple: (StatusCode, AppError) = AppError::conflict("Y").into();
        assert_eq!(tuple.0, StatusCode::CONFLICT);
    }

    /// `From<sqlx::Error>` maps `RowNotFound` to a clean 404 (not the redacted
    /// 500 database_error path) so `?` on a missing row yields NOT_FOUND.
    #[test]
    fn from_sqlx_row_not_found_is_404() {
        let err: AppError = sqlx::Error::RowNotFound.into();
        assert_eq!(err.status_code(), 404);
        assert_eq!(err.error_code(), "RESOURCE_NOT_FOUND");
    }
}
