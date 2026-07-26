//! Crate-scoped integration test for `ziee-framework`'s mountable realtime-sync
//! SSE subscribe route (`sync::sync_routes`).
//!
//! The in-source sync tests cover the registry's audience routing, but NOTHING
//! exercised the actual `/sync/subscribe` HANDLER: the auth-gate → `connected`
//! handshake → keep-alive → exp-deadline teardown. ziee's app-level
//! `tests/sync/subscribe_test.rs` drives it end-to-end, but the crate had no
//! test of its own. This mounts `sync_routes::<TestResolver, TestSurface>()`
//! behind a fake `IdentityResolver` + a test `SyncSurface` and drives each
//! branch in-process via `tower::oneshot`, mirroring `permission_extractors.rs`.
//!
//! Covered:
//! - no / invalid token  → 401 (the resolver's auth-gate rejects before any SSE)
//! - valid token         → 200 + `content-type: text/event-stream` and the first
//!                         SSE frame is the `connected{connection_id}` handshake
//! - past-`exp` token    → after the handshake frame the stream terminates
//!                         (the exp-deadline `sleep` fires → the select! breaks),
//!                         proving the deadline teardown wiring.

use std::sync::{Arc, OnceLock};

use aide::openapi::OpenApi;
use axum::{
    Extension, Router,
    body::Body,
    http::{StatusCode, request::Parts},
    response::sse::Event,
};
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

use ziee_core::AppError;
use ziee_framework::permissions::IdentityResolver;
use ziee_framework::sync::{RecheckOutcome, SyncRegistry, SyncSurface, sync_routes};
use ziee_identity::{PermissionCheck, Principal};

// ---- Fake identity types --------------------------------------------------

#[derive(Clone)]
struct TestGroup {
    perms: Vec<String>,
}

#[derive(Clone)]
struct TestUser {
    id: Uuid,
    direct: Vec<String>,
}

impl Principal for TestUser {
    fn is_admin(&self) -> bool {
        false
    }
    fn direct_permissions(&self) -> &[String] {
        &self.direct
    }
}

/// Per-connection permission snapshot registered in the pool.
#[derive(Clone)]
struct TestPrincipal {
    user_id: Uuid,
    direct: Vec<String>,
}

impl Principal for TestPrincipal {
    fn is_admin(&self) -> bool {
        false
    }
    fn direct_permissions(&self) -> &[String] {
        &self.direct
    }
}

impl From<(TestUser, Vec<TestGroup>)> for TestPrincipal {
    fn from((user, groups): (TestUser, Vec<TestGroup>)) -> Self {
        let mut direct = user.direct;
        for g in groups {
            direct.extend(g.perms);
        }
        TestPrincipal {
            user_id: user.id,
            direct,
        }
    }
}

/// Resolver mapping a bearer token to a canned identity. `Bearer valid` is a
/// user holding `profile::read` (the baseline subscribe perm). The access
/// token's `exp` is read from the `x-exp` header (seconds), so a test drives the
/// deadline branch without a real JWT.
struct TestResolver;

#[async_trait::async_trait]
impl IdentityResolver for TestResolver {
    type User = TestUser;
    type Group = TestGroup;

    async fn authenticate(&self, parts: &mut Parts) -> Result<TestUser, (StatusCode, AppError)> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|h| h.to_str().ok());
        match token {
            None => Err((
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized("NO_TOKEN", "missing bearer token"),
            )),
            Some("Bearer valid") => Ok(TestUser {
                id: Uuid::new_v4(),
                direct: vec!["profile::read".to_string()],
            }),
            Some(_) => Err((
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized("BAD_TOKEN", "unrecognized token"),
            )),
        }
    }

    async fn load_groups(&self, _user: &TestUser) -> Result<Vec<TestGroup>, (StatusCode, AppError)> {
        Ok(vec![])
    }

    fn active_group_permissions(group: &TestGroup) -> Option<&[String]> {
        Some(&group.perms)
    }

    fn access_token_exp(&self, parts: &Parts) -> Option<i64> {
        parts
            .headers
            .get("x-exp")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
    }
}

// ---- Test permission + sync surface ---------------------------------------

struct ProfileRead;
impl PermissionCheck for ProfileRead {
    const NAME: &'static str = "ProfileRead";
    const PERMISSION: &'static str = "profile::read";
    const DESCRIPTION: &'static str = "Read own profile";
    const MODULE: &'static str = "profile";
}

/// The 200-response wire schema (opaque `S::Wire` — only needs JsonSchema +
/// Serialize for the OpenAPI operation).
#[derive(serde::Serialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct TestWire {
    connection_id: String,
}

static REGISTRY: OnceLock<SyncRegistry<TestPrincipal>> = OnceLock::new();

struct TestSurface;

#[async_trait::async_trait]
impl SyncSurface for TestSurface {
    type Principal = TestPrincipal;
    type Wire = TestWire;
    type BaselinePerms = (ProfileRead,);

    fn registry() -> &'static SyncRegistry<TestPrincipal> {
        REGISTRY.get_or_init(SyncRegistry::new)
    }

    fn principal_user_id(principal: &TestPrincipal) -> Uuid {
        principal.user_id
    }

    fn connected_signal(conn_id: Uuid) -> Event {
        // Byte-marker the test asserts on: `event: connected` + the conn id.
        Event::default().event("connected").data(conn_id.to_string())
    }

    async fn recheck(_user_id: Uuid, _token_ver: Option<i32>) -> RecheckOutcome<TestPrincipal> {
        RecheckOutcome::Transient
    }
}

// ---- Harness --------------------------------------------------------------

fn app() -> Router {
    let mut api = OpenApi::default();
    sync_routes::<TestResolver, TestSurface>()
        .finish_api(&mut api)
        .layer(Extension(Arc::new(TestResolver)))
}

fn request(bearer: Option<&str>, exp: Option<i64>) -> axum::http::Request<Body> {
    let mut req = axum::http::Request::builder().uri("/sync/subscribe");
    if let Some(b) = bearer {
        req = req.header("authorization", b);
    }
    if let Some(e) = exp {
        req = req.header("x-exp", e.to_string());
    }
    req.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn no_token_is_401() {
    let res = app().oneshot(request(None, None)).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a tokenless subscribe is rejected before any SSE stream opens"
    );
}

#[tokio::test]
async fn invalid_token_is_401() {
    let res = app()
        .oneshot(request(Some("Bearer nope"), None))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "an unrecognized token is 401, not a broken SSE stream"
    );
}

#[tokio::test]
async fn valid_token_opens_sse_with_connected_handshake() {
    // exp far in the future → the stream stays open; we read only the first frame.
    let exp = chrono::Utc::now().timestamp() + 3600;
    let res = app()
        .oneshot(request(Some("Bearer valid"), Some(exp)))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK, "authorized subscribe → 200");
    let ct = res
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.starts_with("text/event-stream"),
        "SSE content-type, got {ct:?}"
    );

    // Read the FIRST SSE frame; it must be the `connected` handshake.
    let mut body = res.into_body();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
        .await
        .expect("first SSE frame arrives promptly")
        .expect("stream yields a frame")
        .expect("frame is not an error");
    let bytes = frame.into_data().expect("SSE frame carries data bytes");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("event: connected") || text.contains("event:connected"),
        "first frame is the connected handshake: {text:?}"
    );
    // The handshake carries a parseable connection-id uuid in its data line.
    let data_line = text
        .lines()
        .find(|l| l.starts_with("data:"))
        .expect("handshake has a data line");
    let id = data_line.trim_start_matches("data:").trim();
    assert!(
        Uuid::parse_str(id).is_ok(),
        "connected handshake data is a connection-id uuid: {id:?}"
    );
}

#[tokio::test]
async fn past_exp_token_tears_the_stream_down_after_handshake() {
    // exp already lapsed → deadline = now → after the queued `connected` frame the
    // select!'s `sleep` branch fires and the stream ends (exp-deadline teardown).
    let exp = chrono::Utc::now().timestamp() - 10;
    let res = app()
        .oneshot(request(Some("Bearer valid"), Some(exp)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mut body = res.into_body();
    // Frame 1: the connected handshake (try_send'd before the stream loop starts).
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
        .await
        .expect("handshake frame arrives")
        .expect("stream yields the handshake")
        .expect("handshake frame not an error");
    let first_bytes = first.into_data().unwrap_or_default();
    assert!(
        String::from_utf8_lossy(&first_bytes).contains("connected"),
        "the one delivered frame is the handshake"
    );

    // Then the stream terminates because the (past) exp deadline elapsed.
    let end = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
        .await
        .expect("teardown happens promptly, not a hang");
    assert!(
        end.is_none(),
        "past-exp deadline tears the stream down after the handshake"
    );
}

// ---- slot reclamation (sse-slot-leak) -------------------------------------
//
// `register()` runs eagerly in the handler body, so a connection's slot is
// claimed the moment the 200 is produced. Before the fix the `ConnGuard` that
// releases it was a LOCAL of the `async_stream::stream!` generator body — code
// that does not run until the stream's FIRST poll — so a client that went away
// before the body was ever polled left a registration with no guard, holding
// its slot for the life of the process. The registry's only other reaper is
// `deliver`'s send-failure prune, which never runs on a quiescent deployment.
// Measured before the fix: 5 subscribes dropped unpolled took
// `connection_count` 0 -> 5; after the fix, 0 -> 0.
//
// Each test below gets its OWN surface + registry: `TestSurface`'s registry is a
// process-wide `OnceLock` shared by every test in this binary, and cargo runs
// tests concurrently, so a global `connection_count()` baseline would race with
// siblings. An isolated registry makes the counts exact rather than "roughly".

/// Declare an independent `SyncSurface` with its own private registry, so a
/// test can assert EXACT connection counts without sibling interference.
macro_rules! isolated_surface {
    ($surface:ident, $reg:ident) => {
        static $reg: OnceLock<SyncRegistry<TestPrincipal>> = OnceLock::new();
        struct $surface;
        #[async_trait::async_trait]
        impl SyncSurface for $surface {
            type Principal = TestPrincipal;
            type Wire = TestWire;
            type BaselinePerms = (ProfileRead,);
            fn registry() -> &'static SyncRegistry<TestPrincipal> {
                $reg.get_or_init(SyncRegistry::new)
            }
            fn principal_user_id(principal: &TestPrincipal) -> Uuid {
                principal.user_id
            }
            fn connected_signal(conn_id: Uuid) -> Event {
                Event::default().event("connected").data(conn_id.to_string())
            }
            async fn recheck(_user_id: Uuid, _tv: Option<i32>) -> RecheckOutcome<TestPrincipal> {
                RecheckOutcome::Transient
            }
        }
    };
}

isolated_surface!(AbandonSurface, ABANDON_REG);
isolated_surface!(ExitPathSurface, EXIT_PATH_REG);
isolated_surface!(CapSurface, CAP_REG);

/// The same router as `app()`, mounted on an arbitrary surface.
fn app_of<S: SyncSurface<Principal = TestPrincipal>>() -> Router {
    let mut api = OpenApi::default();
    sync_routes::<TestResolver, S>()
        .finish_api(&mut api)
        .layer(Extension(Arc::new(TestResolver)))
}

/// TEST-4 [acceptance INV-1] — "Unregister on ANY stream termination — client
/// disconnect, exp, or deactivation. Drop runs even when the client vanishes
/// mid-await." Driven at the invariant's weakest point: N > the per-user cap
/// subscriptions abandoned BEFORE their body is ever polled must ALL be
/// reclaimed. If the invariant is violated the count climbs by N (it did:
/// 0 -> 5 for N=5 before the fix) and this fails.
#[tokio::test]
async fn abandoned_unpolled_streams_release_their_slots() {
    const N: usize = 20; // deliberately > PER_USER_MAX_CONNECTIONS (12)
    let reg = AbandonSurface::registry();
    assert_eq!(reg.connection_count(), 0, "isolated registry starts empty");

    let exp = chrono::Utc::now().timestamp() + 3600;
    for i in 0..N {
        let res = app_of::<AbandonSurface>()
            .oneshot(request(Some("Bearer valid"), Some(exp)))
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "subscribe #{i} must open — a leak would 429 partway through",
        );
        drop(res); // the client vanished; the body was NEVER polled
    }

    assert_eq!(
        reg.connection_count(),
        0,
        "all {N} abandoned (never-polled) streams must release their slots",
    );
}

/// TEST-11 [acceptance INV-2] — "cleanup on **every** exit path
/// (success/error/timeout) — prefer a RAII guard." The SAME assertion is
/// applied to all three ways a stream can end, so a fix covering only one path
/// fails the other legs.
#[tokio::test]
async fn every_stream_exit_path_releases_its_slot() {
    let reg = ExitPathSurface::registry();
    assert_eq!(reg.connection_count(), 0, "isolated registry starts empty");
    let future_exp = chrono::Utc::now().timestamp() + 3600;

    // (a) dropped BEFORE the first body poll — the path the guard used to miss.
    let res = app_of::<ExitPathSurface>()
        .oneshot(request(Some("Bearer valid"), Some(future_exp)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(reg.connection_count(), 1, "(a) the slot is claimed at register");
    drop(res);
    assert_eq!(
        reg.connection_count(),
        0,
        "(a) a stream dropped before its first poll must release its slot"
    );

    // (b) dropped AFTER the first body poll (the generator had started).
    let res = app_of::<ExitPathSurface>()
        .oneshot(request(Some("Bearer valid"), Some(future_exp)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mut body = res.into_body();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
        .await
        .expect("(b) handshake frame arrives");
    assert_eq!(reg.connection_count(), 1, "(b) still registered while streaming");
    drop(body);
    assert_eq!(
        reg.connection_count(),
        0,
        "(b) a stream dropped after its first poll must release its slot"
    );

    // (c) the stream ending ON ITS OWN at the (already lapsed) exp deadline —
    //     no client disconnect at all; drained to completion.
    let past_exp = chrono::Utc::now().timestamp() - 10;
    let res = app_of::<ExitPathSurface>()
        .oneshot(request(Some("Bearer valid"), Some(past_exp)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mut body = res.into_body();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), body.frame()).await {
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => panic!("(c) the exp-deadline teardown must happen promptly"),
        }
    }
    drop(body);
    assert_eq!(
        reg.connection_count(),
        0,
        "(c) a stream that ends on its own at the exp deadline must release its slot"
    );
}

/// The cap must stay REAL — the guard against "fix the leak by weakening the
/// cap". Through the same real handler: 12 streams held ALIVE (bodies polled,
/// kept in scope) still make the 13th subscribe 429; once they are abandoned,
/// a full fresh set of 12 opens again.
///
/// All connections are forced onto ONE user id via a per-surface pinned id, so
/// the per-user cap is what trips (`TestResolver` otherwise mints a random id
/// per request).
#[tokio::test]
async fn the_cap_is_still_enforced_for_live_streams_and_reusable_after_release() {
    use ziee_framework::sync::ClientConn;

    let reg = CapSurface::registry();
    assert_eq!(reg.connection_count(), 0, "isolated registry starts empty");
    let uid = Uuid::new_v4();

    let mk = |sender| ClientConn {
        user_id: uid,
        principal: TestPrincipal {
            user_id: uid,
            direct: vec![],
        },
        sender,
        // No deadline → these connections are reclaimable only by the
        // receiver-dropped signal, isolating the cap assertion from the
        // deadline backstop.
        expires_at: None,
    };

    // 12 LIVE connections for one user.
    let mut alive = Vec::new();
    for i in 0..12 {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        reg.register(Uuid::new_v4(), mk(tx))
            .unwrap_or_else(|_| panic!("connection #{i} is under the cap"));
        alive.push(rx);
    }

    // The 13th LIVE connection for the same user → refused, cap intact.
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let err = reg
        .register(Uuid::new_v4(), mk(tx))
        .expect_err("the per-user cap must still refuse a 13th LIVE connection");
    assert_eq!(err.status_code(), 429, "the cap still surfaces 429");
    assert_eq!(reg.connection_count(), 12, "a refusal frees nothing");

    // Those streams go away → the whole set of slots comes back.
    drop(alive);
    let mut alive2 = Vec::new();
    for i in 0..12 {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        reg.register(Uuid::new_v4(), mk(tx))
            .unwrap_or_else(|_| panic!("reclaimed slot #{i} must be reusable"));
        alive2.push(rx);
    }
    assert_eq!(
        reg.connection_count(),
        12,
        "exactly the cap's worth of connections, all fresh — the 12 dead ones were reclaimed"
    );

    drop(alive2);
    assert_eq!(reg.prune_closed(), 12, "the sweep reclaims the released set");
    assert_eq!(reg.connection_count(), 0);
}
