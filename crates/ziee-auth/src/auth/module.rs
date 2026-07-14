//! Turnkey auth **module** for the standard `build_api_router` path (gap N-3).
//!
//! [`mount_auth`](super::turnkey::mount_auth) is the batteries-included call for
//! an app that builds + nests its router BY HAND. But the framework's standard
//! path runs the module system: [`build_api_router`] collects every
//! `#[distributed_slice(MODULE_ENTRIES)]` module's routes and nests the whole
//! combined router under `server.api_prefix` (`/api`). On that path `mount_auth`
//! does NOT compose — it would nest auth at `/auth` (not `/api/auth`) and its
//! `Extension` layers would only cover the auth sub-router, so the framework's
//! `RequirePermissions` extractor (which pulls `Extension<Arc<R>>` back out)
//! would find no resolver on the OTHER modules' user routes.
//!
//! [`AuthModule`] is the turnkey fit for that path. It is an [`AppModule`] that:
//!   1. **routes** — mounts `auth_routes::<DefaultIdentityResolver>()` +
//!      `auth_admin_routes::<…>()` through `register_routes`, so they flow
//!      through `build_api_router`'s `/api` nesting and land at `/api/auth/*`.
//!   2. **whole-app extensions** — layers `Extension<AuthContext>`,
//!      `Extension<Arc<JwtService>>`, and `Extension<Arc<DefaultIdentityResolver>>`
//!      onto the combined router at registration time. `Router::layer` covers
//!      every route PRESENT when it is called, and this module registers LAST
//!      (`order = i32::MAX`), so the layers cover every other module's routes —
//!      exactly what `RequirePermissions` needs on all routes.
//!   3. **boot side-effects** — sets the reverse-proxy trust flag and spawns the
//!      one-time session-settings config seed in `init`, mirroring ziee's own
//!      `AuthModule` (which stays as-is; this is purely additive and OFF unless
//!      an app opts into the `module` feature).
//!
//! It uses [`DefaultIdentityResolver`] as the concrete resolver, so an app gets
//! working auth by (a) enabling the `module` feature and (b) injecting its
//! `Config` into `ModuleContext` — nothing else. ziee keeps its own
//! `ZieeIdentityResolver`-backed module and does NOT enable this feature, so its
//! wiring + API surface are unchanged.
//!
//! Gated behind the non-default `module` feature (which implies `routes`) so it
//! is inert unless an app asks for it — a crate that only wants the engine or
//! the hand-built `mount_auth` never links the linkme registration.

use std::error::Error;
use std::sync::Arc;

use aide::axum::ApiRouter;
use axum::Extension;
use linkme::distributed_slice;

use ziee_framework::module_api::{AppModule, ModuleContext, ModuleEntry, MODULE_ENTRIES};

use super::context::{AuthContext, NoopAuthEventSink, NoopAuthSyncSink};
use super::http::{auth_admin_routes, auth_routes};
use super::jwt::JwtService;
use super::turnkey::DefaultIdentityResolver;

/// Register the turnkey auth module at link time. Runs LAST (`order = i32::MAX`)
/// so its whole-app `Extension` layers cover every other module's routes.
#[distributed_slice(MODULE_ENTRIES)]
static AUTH_MODULE_REGISTRATION: ModuleEntry = ModuleEntry {
    name: "auth",
    // Must be the LAST module: `register_routes` layers the resolver/JWT/ctx
    // extensions onto the combined router, and an axum `.layer()` only covers
    // routes already present. Registering last makes "already present" = every
    // module.
    order: i32::MAX,
    description: "Turnkey JWT auth (DefaultIdentityResolver) for the build_api_router path",
    constructor: || Box::new(AuthModule::new()),
};

/// Turnkey auth [`AppModule`] — see the module docs. Construct-free; all state
/// is populated in [`AppModule::init`] from the [`ModuleContext`].
pub struct AuthModule {
    resolver: Option<Arc<DefaultIdentityResolver>>,
    jwt: Option<Arc<JwtService>>,
    ctx: Option<AuthContext>,
}

impl AuthModule {
    pub fn new() -> Self {
        Self {
            resolver: None,
            jwt: None,
            ctx: None,
        }
    }
}

impl Default for AuthModule {
    fn default() -> Self {
        Self::new()
    }
}

impl AppModule for AuthModule {
    fn name(&self) -> &'static str {
        "auth"
    }

    fn description(&self) -> &'static str {
        "Turnkey JWT-based authentication and authorization"
    }

    fn init(&mut self, ctx: &ModuleContext) -> Result<(), Box<dyn Error>> {
        // Build the JWT service from the framework config's `jwt` block (weak /
        // placeholder secrets are refused by `try_new`, matching ziee).
        let jwt = Arc::new(JwtService::try_new(ctx.config.jwt.clone())?);

        // The batteries-included resolver over ziee-auth's own repositories +
        // this JWT service — no per-app resolver impl needed.
        let resolver = Arc::new(DefaultIdentityResolver::new(
            (*ctx.db_pool).clone(),
            jwt.clone(),
        ));

        // A no-op-sink AuthContext (an app with no event bus / sync stream yet).
        // The `RequirePermissions`-gated user routes don't need the sinks; the
        // auth handlers still emit lifecycle events, which the no-op sinks drop.
        let auth_ctx = AuthContext::new(
            ctx.db_pool.clone(),
            None,
            Arc::new(NoopAuthEventSink),
            Arc::new(NoopAuthSyncSink),
        );

        // Reverse-proxy trust flag (idempotent OnceLock set), mirroring ziee.
        super::set_trust_forwarded_headers(ctx.config.server.trust_forwarded_headers);

        // One-time session-settings config seed (migration 129): copy the YAML
        // jwt lifetimes into the singleton only while it hasn't been seeded, so
        // an operator's DB values win thereafter. Non-fatal on failure — the
        // mint path falls back to the config values when the DB read fails.
        let pool = ctx.db_pool.clone();
        let access_hours = ctx.config.jwt.access_token_expiry_hours;
        let refresh_days = ctx.config.jwt.refresh_token_expiry_days;
        tokio::spawn(async move {
            let repo = super::SessionSettingsRepository::new((*pool).clone());
            if let Err(e) = repo.seed_from_config_once(access_hours, refresh_days).await {
                tracing::warn!(error = ?e, "session_settings config seed failed; DB defaults remain");
            }
        });

        self.jwt = Some(jwt);
        self.resolver = Some(resolver);
        self.ctx = Some(auth_ctx);
        Ok(())
    }

    fn register_routes(&self, router: ApiRouter) -> ApiRouter {
        let (Some(resolver), Some(jwt), Some(ctx)) =
            (self.resolver.clone(), self.jwt.clone(), self.ctx.clone())
        else {
            tracing::error!("AuthModule: not initialized before register_routes");
            return router;
        };

        // Mount the SDK routes bundle with the batteries-included resolver.
        // Nested at `/auth` here → `/api/auth/*` once `build_api_router` nests
        // the combined router under `api_prefix`.
        let auth_router = ApiRouter::new()
            .nest("/auth", auth_routes::<DefaultIdentityResolver>())
            .merge(auth_admin_routes::<DefaultIdentityResolver>());

        // Merge the auth routes into the combined router, then install the
        // whole-app extension layers. These cover every route present on
        // `router` at this point; because this module registers LAST
        // (`order = i32::MAX`), that is every module's routes — so
        // `RequirePermissions` resolves the injected resolver on all of them.
        router
            .merge(auth_router)
            .layer(Extension(ctx))
            .layer(Extension(jwt))
            .layer(Extension(resolver))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // for `oneshot`

    use ziee_framework::app_builder::build_api_router;
    use ziee_framework::module_api::ModuleContext;
    use ziee_framework::permissions::RequirePermissions;

    use crate::auth::permissions::AuthProvidersRead;

    /// A stand-in NON-auth module with a single permission-gated route. Its route
    /// needs the injected resolver (`Extension<Arc<DefaultIdentityResolver>>`) —
    /// so it is the probe that proves `AuthModule`'s whole-app layer reaches
    /// routes OTHER than auth's own.
    struct GatedProbeModule;

    async fn gated_handler(
        _g: RequirePermissions<DefaultIdentityResolver, (AuthProvidersRead,)>,
    ) -> &'static str {
        "ok"
    }

    impl AppModule for GatedProbeModule {
        fn name(&self) -> &'static str {
            "gated_probe"
        }
        fn init(&mut self, _ctx: &ModuleContext) -> Result<(), Box<dyn Error>> {
            Ok(())
        }
        fn register_routes(&self, router: ApiRouter) -> ApiRouter {
            router.route("/whoami", axum::routing::get(gated_handler))
        }
    }

    fn test_config() -> Arc<ziee_core::ServerConfig> {
        // Minimal ServerConfig via serde_json (serde_json is already a dep). A
        // strong (non-placeholder, ≥32-char) jwt secret so `JwtService::try_new`
        // accepts it; `use_embedded:false` with no external block is fine — the
        // test never calls `database_url()`.
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
        Arc::new(cfg)
    }

    /// The turnkey module is collected into the framework's `MODULE_ENTRIES`
    /// slice (proves the `#[distributed_slice]` registration links) and runs
    /// LAST so its whole-app layers cover every module.
    #[test]
    fn auth_module_registered_in_slice_and_runs_last() {
        let auth = MODULE_ENTRIES
            .iter()
            .find(|e| e.name == "auth")
            .expect("AuthModule must be registered in MODULE_ENTRIES under the `module` feature");
        assert_eq!(
            auth.order,
            i32::MAX,
            "AuthModule must register LAST so its whole-app extension layers cover all routes"
        );
    }

    /// End-to-end wiring proof on the standard `build_api_router` path:
    ///   1. auth routes land under `/api/auth/*` (NOT `/auth`), and
    ///   2. the resolver/JWT/AuthContext extensions reach a NON-auth gated route
    ///      — an unauthenticated hit returns 401 (resolver present, token
    ///      missing), NOT the 500 a missing resolver extension would produce.
    #[tokio::test]
    async fn auth_module_nests_under_api_and_resolver_reaches_non_auth_route() {
        let pool = sqlx::postgres::PgPool::connect_lazy(
            "postgresql://postgres:password@127.0.0.1:54321/postgres",
        )
        .unwrap();
        let config = test_config();
        let app_config: Arc<dyn Any + Send + Sync> = config.clone();
        let ctx = ModuleContext::new(Arc::new(pool.clone()), config, app_config);

        // Order matters: AuthModule LAST (mirrors the `order`-sorted production
        // sequence) so its layer covers the probe module's route.
        let mut modules: Vec<Box<dyn AppModule>> =
            vec![Box::new(GatedProbeModule), Box::new(AuthModule::new())];
        for m in modules.iter_mut() {
            m.init(&ctx).expect("module init");
        }

        let (api_router, _api_doc) = build_api_router(&modules, "/api", pool);

        // Finish the router into a concrete axum Router; this populates `openapi`
        // with every route's path/operation.
        let mut openapi = aide::openapi::OpenApi::default();
        let app: axum::Router = api_router.finish_api(&mut openapi);

        // (1) auth routes are nested under the /api prefix (NOT bare `/auth`).
        let paths = openapi.paths.expect("openapi has paths");
        assert!(
            paths.paths.keys().any(|p| p.starts_with("/api/auth")),
            "auth routes must be nested under /api/auth; got: {:?}",
            paths.paths.keys().collect::<Vec<_>>()
        );

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/whoami")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "gated non-auth route must reach the injected resolver (401 missing-token), \
             NOT 500 (missing resolver extension)"
        );
    }
}
