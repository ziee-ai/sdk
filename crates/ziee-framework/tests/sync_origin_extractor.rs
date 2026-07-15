//! Crate-scoped integration test for `ziee-framework`'s `SyncOrigin` extractor.
//!
//! The sync registry's self-echo suppression is unit-tested, but the header
//! extractor that FEEDS it the originating connection id has no coverage. This
//! drives the real `FromRequestParts` impl through an axum handler via
//! `tower::oneshot`: a valid `X-Sync-Connection-Id` parses to `Some(uuid)`, and
//! an absent or malformed header degrades to `None` (never a rejection — the
//! extractor is `Infallible`, so a bad header just disables self-echo
//! suppression rather than failing the mutation).

use axum::{Router, body::Body, http::Request, routing::post};
use tower::ServiceExt; // oneshot
use uuid::Uuid;
use ziee_framework::{SyncOrigin, SYNC_CONNECTION_HEADER};

/// Echo the extracted origin back as text: the uuid, or `none`.
fn app() -> Router {
    Router::new().route(
        "/mutate",
        post(|origin: SyncOrigin| async move {
            match origin.0 {
                Some(id) => id.to_string(),
                None => "none".to_string(),
            }
        }),
    )
}

async fn post_with_header(value: Option<&str>) -> String {
    let mut req = Request::builder().method("POST").uri("/mutate");
    if let Some(v) = value {
        req = req.header(SYNC_CONNECTION_HEADER, v);
    }
    let res = app().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    assert!(res.status().is_success(), "SyncOrigin never rejects a request");
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn valid_header_parses_to_some_uuid() {
    let id = Uuid::new_v4();
    let body = post_with_header(Some(&id.to_string())).await;
    assert_eq!(body, id.to_string(), "a valid connection id is threaded through");
}

#[tokio::test]
async fn absent_header_is_none() {
    let body = post_with_header(None).await;
    assert_eq!(body, "none", "no header → None (self-echo suppression simply off)");
}

#[tokio::test]
async fn malformed_header_is_none_not_a_rejection() {
    let body = post_with_header(Some("not-a-uuid")).await;
    assert_eq!(body, "none", "an unparseable header degrades to None, never a 4xx");
}
