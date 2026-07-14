// Health handlers

use aide::transform::TransformOperation;
use axum::{Json, debug_handler, http::StatusCode};

use super::types::HealthResponse;

// =====================================================
// Route Handlers
// =====================================================

/// GET /api/health
/// Health check endpoint
#[debug_handler]
pub async fn health_check() -> (StatusCode, Json<HealthResponse>) {
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: "ok".to_string(),
        }),
    )
}

/// Documentation for health_check endpoint
pub fn health_check_docs(op: TransformOperation) -> TransformOperation {
    op.description("Health check endpoint")
        .id("Health.check")
        .tag("health")
        .response::<200, Json<HealthResponse>>()
}
#[cfg(test)]
mod tests {
    use super::*;


    /// The health endpoint is an unauthenticated liveness probe: it must always
    /// return 200 with `{"status":"ok"}` (no DB, no auth). Gap 6da0741c8b92 —
    /// the health module had zero unit tests.
    #[tokio::test]
    async fn health_check_returns_ok_200() {
        let (status, Json(body)) = health_check().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.status, "ok");
    }


    /// The handler is a pure function (no DB / no auth) — drive it directly and
    /// assert it yields exactly `200 OK` + the documented `{ "status": "ok" }`
    /// body. The HTTP-level wiring is covered by
    /// `tests/health/mod.rs::health_endpoint_returns_ok_without_auth`; this is
    /// the in-source unit the module previously lacked entirely.
    #[tokio::test]
    async fn health_check_returns_200_and_ok_status() {
        let (status, Json(body)) = health_check().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.status, "ok");
    }


    /// `HealthResponse` must serialize to the exact wire shape clients/load
    /// balancers depend on (`{"status":"..."}`) and round-trip back, so a
    /// field rename can't silently break the contract.
    #[test]
    fn health_response_serde_round_trips_to_status_object() {
        let resp = HealthResponse {
            status: "ok".to_string(),
        };
        let json = serde_json::to_value(&resp).expect("serialize HealthResponse");
        assert_eq!(json, serde_json::json!({ "status": "ok" }));

        let back: HealthResponse =
            serde_json::from_value(json).expect("deserialize HealthResponse");
        assert_eq!(back.status, "ok");
    }


    /// The health endpoint handler returns 200 with a static `{"status":"ok"}`
    /// body. The health module had zero unit tests.
    #[tokio::test]
    async fn health_check_returns_ok_status() {
        let (status, Json(body)) = health_check().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.status, "ok");
    }


    /// HealthResponse serializes to the documented JSON shape.
    #[test]
    fn health_response_serializes_to_status_field() {
        let body = HealthResponse { status: "ok".to_string() };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }
}
