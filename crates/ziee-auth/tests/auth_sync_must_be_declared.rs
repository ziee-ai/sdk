//! `AuthModule` must be TOLD how auth's cross-device sync is wired.
//!
//! ── the failure this exists to make impossible ──────────────────────────────
//!
//! `AuthModule::init` used to build its one `AuthContext` with a hard-coded
//! `Arc::new(NoopAuthSyncSink)`, and the module is constructed through a fixed
//! `fn() -> Box<dyn AppModule>` in `MODULE_ENTRIES`, so there was no injection
//! point of any kind. Every `ctx.sync.publish(...)` in the auth handlers — the
//! profile update, the session-settings update, logout's session signal, all
//! four auth-provider mutations — ran, computed its frame, and dropped it.
//!
//! Nothing reported that. The handlers returned 200, their tests passed, the
//! call sites read as fully wired. The only symptom was a second device that
//! never converged, which is a symptom nobody attributes to this file. A
//! consuming app spent a debugging session finding it, allow-listed the two
//! affected channels in its own parity guard, and filed the blocker.
//!
//! ── why this test is its own process ────────────────────────────────────────
//!
//! The declaration is a `OnceLock`, so "nothing has been declared yet" exists
//! exactly once per process. A `#[cfg(test)]` sibling that declares would make
//! the negative half unassertable — and, worse, would make it pass or fail on
//! thread scheduling. Integration test files each get their own binary, so this
//! file owns the whole sequence and runs it in order inside ONE `#[test]`.
//!
//! The pure decision (`resolve_auth_sync`) is unit-tested exhaustively next to
//! its definition in `auth/context.rs`; what only this file can prove is that
//! `AuthModule::init` actually consults it and that the declared sink is the one
//! the mounted handlers end up holding.

#![cfg(feature = "module")]

use std::any::Any;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // `oneshot`

use ziee_auth::auth::{
    AuthContext, AuthModule, AuthSyncAction, AuthSyncEntity, AuthSyncSink, AuthSyncWiring,
    declare_auth_sync_inert, declared_auth_sync, install_auth_sync_sink,
};
use ziee_framework::module_api::{AppModule, ModuleContext};
use ziee_framework::sync::Audience;

/// Records the frames it is handed, so "the sink I declared is the sink the
/// handlers hold" is a behavioural assertion and not a pointer comparison.
#[derive(Default)]
struct RecordingSink {
    published: Mutex<Vec<AuthSyncEntity>>,
}

impl AuthSyncSink for RecordingSink {
    fn publish(
        &self,
        entity: AuthSyncEntity,
        _action: AuthSyncAction,
        _id: uuid::Uuid,
        _audience: Audience,
        _origin: Option<uuid::Uuid>,
    ) {
        self.published.lock().unwrap().push(entity);
    }
    fn publish_session_to_users(&self, _user_ids: &[uuid::Uuid], _origin: Option<uuid::Uuid>) {}
}

/// A stand-in NON-auth module with one route that pulls `Extension<AuthContext>`
/// back out and publishes through it — exactly what every real auth handler
/// does. It is the probe that proves the DECLARED sink survives
/// `init` → `register_routes` → the whole-app `Extension` layer → a handler.
struct SyncProbeModule;

async fn probe_handler(axum::Extension(ctx): axum::Extension<AuthContext>) -> &'static str {
    ctx.sync.publish(
        AuthSyncEntity::Profile,
        AuthSyncAction::Update,
        uuid::Uuid::nil(),
        Audience::owner(uuid::Uuid::nil()),
        None,
    );
    "published"
}

impl AppModule for SyncProbeModule {
    fn name(&self) -> &'static str {
        "sync_probe"
    }
    fn init(&mut self, _ctx: &ModuleContext) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
    fn register_routes(&self, router: aide::axum::ApiRouter) -> aide::axum::ApiRouter {
        router.route("/sync-probe", axum::routing::get(probe_handler))
    }
}

fn module_context() -> ModuleContext {
    // `connect_lazy` never dials, so this stays unit-test cheap; a strong,
    // non-placeholder ≥32-char secret so `JwtService::try_new` accepts it.
    let pool = sqlx::postgres::PgPool::connect_lazy(
        "postgresql://postgres:password@127.0.0.1:54321/postgres",
    )
    .expect("lazy pool");
    let cfg: ziee_core::ServerConfig = serde_json::from_value(serde_json::json!({
        "postgresql": { "use_embedded": false },
        "server": { "host": "127.0.0.1", "port": 3000, "api_prefix": "/api" },
        "jwt": {
            "secret": "0123456789abcdef0123456789abcdef-strong",
            "issuer": "test",
            "audience": "test",
            "access_token_expiry_hours": 24
        }
    }))
    .expect("test ServerConfig deserializes");
    let cfg = Arc::new(cfg);
    let app_config: Arc<dyn Any + Send + Sync> = cfg.clone();
    ModuleContext::new(Arc::new(pool), cfg, app_config)
}

#[tokio::test]
async fn auth_sync_wiring_must_be_declared_and_the_declared_sink_is_the_one_used() {
    let ctx = module_context();

    // ── 1. undeclared ────────────────────────────────────────────────────────
    assert!(
        declared_auth_sync().is_none(),
        "this test owns the process; nothing may have declared before it"
    );

    let err = AuthModule::new().init(&ctx).expect_err(
        "with no declaration, AuthModule MUST refuse to boot rather than quietly \
                     installing a no-op sink",
    );
    let msg = err.to_string();
    for expected in [
        "install_auth_sync_sink",
        "declare_auth_sync_inert",
        "Profile",
        "SessionSettings",
        "AuthProvider",
    ] {
        assert!(
            msg.contains(expected),
            "the boot refusal must name `{expected}` so the reader can act on it; got:\n{msg}"
        );
    }

    // ── 2. declare, and prove a SECOND declaration does not silently replace it ──
    let rec = Arc::new(RecordingSink::default());
    install_auth_sync_sink(rec.clone());
    declare_auth_sync_inert("a later caller trying to turn auth sync back off");
    assert!(
        matches!(declared_auth_sync(), Some(AuthSyncWiring::Live(_))),
        "the FIRST declaration must stand; a silent overwrite is the same class of \
         invisible drop this seam removes"
    );

    // ── 3. init now succeeds, and the declared sink reaches a mounted handler ──
    let mut modules: Vec<Box<dyn AppModule>> =
        vec![Box::new(SyncProbeModule), Box::new(AuthModule::new())];
    for m in modules.iter_mut() {
        m.init(&ctx).expect("with a declaration, AuthModule boots");
    }

    let pool = sqlx::postgres::PgPool::connect_lazy(
        "postgresql://postgres:password@127.0.0.1:54321/postgres",
    )
    .expect("lazy pool");
    let (api_router, _doc) = ziee_framework::app_builder::build_api_router(&modules, "/api", pool);
    let mut openapi = aide::openapi::OpenApi::default();
    let app: axum::Router = api_router.finish_api(&mut openapi);

    assert!(
        rec.published.lock().unwrap().is_empty(),
        "nothing should have published before the probe route is hit"
    );

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/sync-probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    assert_eq!(
        *rec.published.lock().unwrap(),
        vec![AuthSyncEntity::Profile],
        "the frame a mounted handler publishes must arrive at the sink the app DECLARED — \
         if this is empty, AuthModule installed something else and every auth publish is \
         being dropped again"
    );
}
