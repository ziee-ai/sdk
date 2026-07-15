//! Crate-scoped integration test for `ziee-health`.
//!
//! The in-source unit tests call the pure `health_check()` handler directly.
//! This exercises the layer they don't: the real `routes()` router wiring —
//! path (`/health`), method (GET), handler mount, and the JSON response over
//! HTTP — in-process via `tower::oneshot`. DB-free crate, so no test-server /
//! harness is needed; mirrors the `finish_api → oneshot` pattern already used
//! in `ziee-auth`'s in-source tests.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // for `oneshot`

/// Build the crate's real router (the same `routes()` ziee mounts).
fn app() -> axum::Router {
    let mut openapi = aide::openapi::OpenApi::default();
    ziee_health::routes().finish_api(&mut openapi)
}

/// `GET /health` routes to the handler and returns 200 + the documented
/// `{"status":"ok"}` body — the unauthenticated liveness contract clients and
/// load balancers depend on.
#[tokio::test]
async fn get_health_returns_200_ok_json() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json, serde_json::json!({ "status": "ok" }));
}

/// An unknown path on the crate's router yields 404 — the router only mounts
/// `/health`, nothing else.
#[tokio::test]
async fn unknown_route_returns_404() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/this-route-does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
