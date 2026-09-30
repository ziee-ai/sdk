//! The session-keyed SSE stream re-check (owner card `sse-stream-recheck`,
//! memo identity-and-access-356 §1).
//!
//! `sync_routes` captures the access token's session id at subscribe
//! ([`IdentityResolver::access_token_session_id`]) and hands it to every
//! periodic [`SyncSurface::recheck`] tick, so an app can tear down the open
//! streams of ONE revoked session while the same user's other sessions keep
//! theirs. The resolver method defaults to `None`, and a `None` session id
//! leaves the app on its prior session-less gate (back-compat).
//!
//! This file is its own test binary because it lowers the debug-only
//! `SYNC_RECHECK_TICK_MS` seam process-wide (the 60 s production tick would
//! make a teardown test take minutes); `sync_routes.rs`'s tests do not see it.
//!
//! Covered:
//! - the default `access_token_session_id` is `None` (an app that does not
//!   override it compiles and is handed `session_id: None`, with its `ver`
//!   still passed through unchanged) — the back-compat path;
//! - a revoked session's already-open stream closes on the next tick, while a
//!   second session of the SAME user stays open;
//! - a session-less (`None`) stream is untouched by a per-session revocation
//!   and still ends on the prior per-user epoch gate.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::Duration;

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

/// Re-check tick used by every test in this binary.
const TICK_MS: u64 = 25;

fn fast_ticks() {
    static ONCE: Once = Once::new();
    // Set once, before any subscribe in this process reads it.
    ONCE.call_once(|| std::env::set_var("SYNC_RECHECK_TICK_MS", TICK_MS.to_string()));
}

// ---- Fake identity --------------------------------------------------------

#[derive(Clone)]
struct TestGroup;

#[derive(Clone)]
struct TestUser {
    id: Uuid,
}

impl Principal for TestUser {
    fn is_admin(&self) -> bool {
        false
    }
    fn direct_permissions(&self) -> &[String] {
        static P: OnceLock<Vec<String>> = OnceLock::new();
        P.get_or_init(|| vec!["profile::read".to_string()])
    }
}

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
    fn from((user, _): (TestUser, Vec<TestGroup>)) -> Self {
        TestPrincipal {
            user_id: user.id,
            direct: vec!["profile::read".to_string()],
        }
    }
}

fn header<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts.headers.get(name).and_then(|h| h.to_str().ok())
}

/// Authenticates `Bearer <user-uuid>`; reads `ver` from `x-ver` and the
/// session id from `x-sid` — the stand-ins for a real token's claims.
struct SessionResolver;

#[async_trait::async_trait]
impl IdentityResolver for SessionResolver {
    type User = TestUser;
    type Group = TestGroup;

    async fn authenticate(&self, parts: &mut Parts) -> Result<TestUser, (StatusCode, AppError)> {
        header(parts, "authorization")
            .and_then(|h| h.strip_prefix("Bearer "))
            .and_then(|t| Uuid::parse_str(t).ok())
            .map(|id| TestUser { id })
            .ok_or((
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized("BAD_TOKEN", "unrecognized token"),
            ))
    }
    async fn load_groups(&self, _: &TestUser) -> Result<Vec<TestGroup>, (StatusCode, AppError)> {
        Ok(vec![])
    }
    fn active_group_permissions(_: &TestGroup) -> Option<&[String]> {
        None
    }
    fn access_token_ver(&self, parts: &Parts) -> Option<i32> {
        header(parts, "x-ver").and_then(|s| s.parse().ok())
    }
    fn access_token_session_id(&self, parts: &Parts) -> Option<Uuid> {
        header(parts, "x-sid").and_then(|s| Uuid::parse_str(s).ok())
    }
}

/// The same resolver WITHOUT the session seam: it overrides `access_token_ver`
/// but not `access_token_session_id`, i.e. an app written before this change.
struct LegacyResolver;

#[async_trait::async_trait]
impl IdentityResolver for LegacyResolver {
    type User = TestUser;
    type Group = TestGroup;

    async fn authenticate(&self, parts: &mut Parts) -> Result<TestUser, (StatusCode, AppError)> {
        SessionResolver.authenticate(parts).await
    }
    async fn load_groups(&self, _: &TestUser) -> Result<Vec<TestGroup>, (StatusCode, AppError)> {
        Ok(vec![])
    }
    fn active_group_permissions(_: &TestGroup) -> Option<&[String]> {
        None
    }
    fn access_token_ver(&self, parts: &Parts) -> Option<i32> {
        SessionResolver.access_token_ver(parts)
    }
}

struct ProfileRead;
impl PermissionCheck for ProfileRead {
    const NAME: &'static str = "ProfileRead";
    const PERMISSION: &'static str = "profile::read";
    const DESCRIPTION: &'static str = "Read own profile";
    const MODULE: &'static str = "profile";
}

#[derive(serde::Serialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct TestWire {
    connection_id: String,
}

// ---- The app's side: a session table and a per-user epoch ------------------

/// Sessions the "app" has ended (the session-row gate).
static REVOKED_SESSIONS: OnceLock<Mutex<HashSet<Uuid>>> = OnceLock::new();
/// The live per-user epoch (the session-less gate); every user starts at 1.
static USER_EPOCH: OnceLock<Mutex<std::collections::HashMap<Uuid, i32>>> = OnceLock::new();
/// Every `(user_id, session_id, token_ver)` the framework handed `recheck`.
static SEEN: OnceLock<Mutex<Vec<(Uuid, Option<Uuid>, Option<i32>)>>> = OnceLock::new();
static REG: OnceLock<SyncRegistry<TestPrincipal>> = OnceLock::new();

fn revoke_session(sid: Uuid) {
    REVOKED_SESSIONS.get_or_init(Default::default).lock().unwrap().insert(sid);
}
fn bump_user_epoch(user: Uuid) {
    *USER_EPOCH
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(user)
        .or_insert(1) += 1;
}
fn seen_for(user: Uuid) -> Vec<(Option<Uuid>, Option<i32>)> {
    SEEN.get_or_init(Default::default)
        .lock()
        .unwrap()
        .iter()
        .filter(|(u, _, _)| *u == user)
        .map(|(_, s, v)| (*s, *v))
        .collect()
}

/// Mirrors the shape memo 356 prescribes for an app: `Some(sid)` → the
/// session row decides; `None` → the prior per-user epoch gate.
struct SessionSurface;

#[async_trait::async_trait]
impl SyncSurface for SessionSurface {
    type Principal = TestPrincipal;
    type Wire = TestWire;
    type BaselinePerms = (ProfileRead,);

    fn registry() -> &'static SyncRegistry<TestPrincipal> {
        REG.get_or_init(SyncRegistry::new)
    }
    fn principal_user_id(p: &TestPrincipal) -> Uuid {
        p.user_id
    }
    fn connected_signal(conn_id: Uuid) -> Event {
        Event::default().event("connected").data(conn_id.to_string())
    }
    async fn recheck(
        user_id: Uuid,
        session_id: Option<Uuid>,
        token_ver: Option<i32>,
    ) -> RecheckOutcome<TestPrincipal> {
        SEEN.get_or_init(Default::default)
            .lock()
            .unwrap()
            .push((user_id, session_id, token_ver));
        let revoked = match session_id {
            Some(sid) => REVOKED_SESSIONS
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .contains(&sid),
            None => {
                let live = *USER_EPOCH
                    .get_or_init(Default::default)
                    .lock()
                    .unwrap()
                    .get(&user_id)
                    .unwrap_or(&1);
                token_ver.unwrap_or(0) != live
            }
        };
        if revoked {
            RecheckOutcome::TearDown
        } else {
            RecheckOutcome::Refresh(TestPrincipal {
                user_id,
                direct: vec!["profile::read".to_string()],
            })
        }
    }
}

fn app<R: IdentityResolver<User = TestUser, Group = TestGroup>>(resolver: R) -> Router {
    fast_ticks();
    let mut api = OpenApi::default();
    sync_routes::<R, SessionSurface>()
        .finish_api(&mut api)
        .layer(Extension(Arc::new(resolver)))
}

fn request(user: Uuid, ver: i32, sid: Option<Uuid>) -> axum::http::Request<Body> {
    let exp = chrono::Utc::now().timestamp() + 3600;
    let mut req = axum::http::Request::builder()
        .uri("/sync/subscribe")
        .header("authorization", format!("Bearer {user}"))
        .header("x-exp", exp.to_string())
        .header("x-ver", ver.to_string());
    if let Some(s) = sid {
        req = req.header("x-sid", s.to_string());
    }
    req.body(Body::empty()).unwrap()
}

/// An open stream, drained by a background task (the re-check ticks run inside
/// the stream, so they only fire while someone polls the body). Dropping it
/// aborts the drain, which drops the stream (a client disconnect).
struct Live(tokio::task::JoinHandle<()>);

impl Drop for Live {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Live {
    /// `true` iff the stream has ENDED within `within`.
    async fn ends_within(&self, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            if self.0.is_finished() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.0.is_finished()
    }
}

/// Open a stream, read its `connected` handshake, then keep it drained.
async fn open<R: IdentityResolver<User = TestUser, Group = TestGroup>>(
    resolver: R,
    user: Uuid,
    ver: i32,
    sid: Option<Uuid>,
) -> Live {
    let res = app(resolver).oneshot(request(user, ver, sid)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "subscribe opens");
    let mut body = res.into_body();
    let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .expect("handshake arrives")
        .expect("a frame")
        .expect("frame ok");
    assert!(String::from_utf8_lossy(&frame.into_data().unwrap_or_default()).contains("connected"));
    Live(tokio::spawn(async move {
        while let Some(Ok(_)) = body.frame().await {}
    }))
}

/// Wait until the framework has run at least `n` re-checks for `user`.
async fn await_rechecks(user: Uuid, n: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while seen_for(user).len() < n {
        assert!(tokio::time::Instant::now() < deadline, "re-check ticks never ran");
        tokio::time::sleep(Duration::from_millis(TICK_MS)).await;
    }
}

// ---- back-compat: the default is None --------------------------------------

/// Unit: an `IdentityResolver` that does not override the new method yields
/// `None` from the default, whatever the request carries.
#[test]
fn default_access_token_session_id_is_none() {
    let (parts, _) = axum::http::Request::builder()
        .header("x-sid", Uuid::new_v4().to_string())
        .header("x-ver", "3")
        .body(())
        .unwrap()
        .into_parts();
    assert_eq!(LegacyResolver.access_token_session_id(&parts), None);
    assert_eq!(LegacyResolver.access_token_ver(&parts), Some(3), "ver unaffected");
    // The overriding resolver does read it — so the test above is not vacuous.
    assert!(SessionResolver.access_token_session_id(&parts).is_some());
}

/// Route: a legacy resolver's stream is re-checked with `session_id: None` and
/// its `ver` unchanged, and it lives on the prior per-user gate: a per-session
/// revocation cannot touch it, a per-user epoch bump ends it.
#[tokio::test]
async fn legacy_resolver_rechecks_with_none_and_keeps_the_user_epoch_gate() {
    let user = Uuid::new_v4();
    let sid = Uuid::new_v4();
    // The request even carries an x-sid: the legacy resolver must not read it.
    let body = open(LegacyResolver, user, 1, Some(sid)).await;

    await_rechecks(user, 2).await;
    assert!(
        seen_for(user).iter().all(|&(s, v)| s.is_none() && v == Some(1)),
        "a resolver without the seam hands recheck (None, its ver): {:?}",
        seen_for(user)
    );

    revoke_session(sid);
    assert!(
        !body.ends_within(Duration::from_millis(TICK_MS * 8)).await,
        "a session-less stream is not reached by a per-session revocation"
    );

    bump_user_epoch(user);
    assert!(
        body.ends_within(Duration::from_secs(5)).await,
        "the prior per-user epoch gate still ends a session-less stream"
    );
}

// ---- the revoked session's stream closes -----------------------------------

/// Two sessions of ONE user each hold an open stream. Ending session A closes
/// A's stream on the next tick; session B's stream stays open.
#[tokio::test]
async fn revoked_sessions_open_stream_closes_and_sibling_session_survives() {
    let user = Uuid::new_v4();
    let (sid_a, sid_b) = (Uuid::new_v4(), Uuid::new_v4());
    let a = open(SessionResolver, user, 1, Some(sid_a)).await;
    let b = open(SessionResolver, user, 1, Some(sid_b)).await;

    await_rechecks(user, 2).await;
    let seen = seen_for(user);
    assert!(seen.iter().any(|&(s, _)| s == Some(sid_a)), "A's sid reaches recheck: {seen:?}");
    assert!(seen.iter().any(|&(s, _)| s == Some(sid_b)), "B's sid reaches recheck: {seen:?}");
    assert!(
        !a.ends_within(Duration::from_millis(TICK_MS * 4)).await,
        "a live session's stream stays open"
    );

    revoke_session(sid_a);
    assert!(
        a.ends_within(Duration::from_secs(5)).await,
        "the revoked session's already-open stream must close within a tick"
    );
    assert!(
        !b.ends_within(Duration::from_millis(TICK_MS * 8)).await,
        "the same user's OTHER session keeps its stream"
    );
    drop(b);
}
