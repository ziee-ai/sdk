//! Turnkey auth wiring (chunk sdk-batteries / G1).
//!
//! CytoAnalyst's dogfooding showed that mounting "free" auth cost ~230 lines +
//! two trait impls, all mechanical copy-from-ziee. This module collapses that to
//! a few SDK calls WITHOUT touching ziee's own (unchanged) wiring — it is purely
//! ADDITIVE:
//!   - [`DefaultIdentityResolver`] — a ready [`IdentityResolver`] over ziee-auth's
//!     own [`UserRepository`] + [`JwtService`], so an app needn't hand-write
//!     `authenticate`/`load_groups`/`active_group_permissions`. (ziee keeps its
//!     `ZieeIdentityResolver`, which differs only by reaching a global `Repos`.)
//!   - [`mount_auth`] — one call that installs the JWT/AuthContext/resolver
//!     extension layers, sets the reverse-proxy trust flag, seeds the
//!     session-settings singleton, and mounts BOTH `auth_routes` + admin routes.
//!
//! Requires the `routes` feature (aide/axum), on by default.

use std::sync::Arc;

use axum::Extension;
use axum::http::{StatusCode, request::Parts};

use ziee_core::AppError;
use ziee_framework::permissions::IdentityResolver;

use super::JwtService;
use super::context::AuthContext;
use super::http::{auth_admin_routes, auth_routes};
use crate::user::{Group, User, UserRepository};

/// A ready-to-use [`IdentityResolver`] backed by a `PgPool` + an
/// `Arc<JwtService>` — the SDK-provided default so an app installs working
/// permission enforcement directly instead of copying ziee's ~90-line resolver.
///
/// Behaviour matches ziee's `ZieeIdentityResolver`: validate the JWT, load the
/// acting `User`, reject inactive users (403), and load the user's `Group`s for
/// the non-admin permission union. The only difference is this reaches the DB
/// through an explicit pool-bound [`UserRepository`] rather than a global
/// `Repos` singleton, so it needs no app-global installed.
#[derive(Clone)]
pub struct DefaultIdentityResolver {
    users: UserRepository,
    jwt: Arc<JwtService>,
}

impl DefaultIdentityResolver {
    /// Construct from the app's connection pool + JWT service. Both are cheaply
    /// cloneable (pool is Arc-backed; the JWT service is shared behind `Arc`).
    pub fn new(pool: sqlx::PgPool, jwt: Arc<JwtService>) -> Self {
        Self {
            users: UserRepository::new(pool),
            jwt,
        }
    }
}

#[async_trait::async_trait]
impl IdentityResolver for DefaultIdentityResolver {
    type User = User;
    type Group = Group;

    async fn authenticate(&self, parts: &mut Parts) -> Result<User, (StatusCode, AppError)> {
        // Extract the Authorization header.
        let auth_header = parts
            .headers
            .get("Authorization")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    AppError::unauthorized("MISSING_TOKEN", "Authorization header is missing"),
                )
            })?;

        // Extract + validate the access token.
        let token = JwtService::extract_token_from_header(auth_header)
            .map_err(|e| (StatusCode::UNAUTHORIZED, e))?;
        let claims = self
            .jwt
            .validate_access_token(token)
            .map_err(|e| (StatusCode::UNAUTHORIZED, e))?;

        // Parse the user id from the claims.
        let user_id = uuid::Uuid::parse_str(&claims.sub).map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                AppError::internal_error("Invalid user ID in token"),
            )
        })?;

        // Load the acting user.
        let user = self
            .users
            .get_by_id(user_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    AppError::unauthorized("USER_NOT_FOUND", "User not found"),
                )
            })?;

        // Reject inactive accounts.
        if !user.is_active {
            return Err((
                StatusCode::FORBIDDEN,
                AppError::forbidden("USER_INACTIVE", "User account is inactive"),
            ));
        }

        Ok(user)
    }

    async fn load_groups(&self, user: &User) -> Result<Vec<Group>, (StatusCode, AppError)> {
        self.users
            .get_user_groups(user.id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
    }

    fn active_group_permissions(group: &Group) -> Option<&[String]> {
        if group.is_active {
            Some(&group.permissions)
        } else {
            None
        }
    }
}

/// Options for [`mount_auth`]. `Default` gives the safe posture: don't trust
/// forwarded proxy headers, and don't seed the session-settings singleton.
#[derive(Clone, Default)]
pub struct AuthMountOptions {
    /// Honor `X-Forwarded-Host`/`X-Forwarded-Proto` in OAuth redirect derivation.
    /// Only true behind a proxy that STRIPS + sets these itself (see
    /// `HttpServerConfig::trust_forwarded_headers`).
    pub trust_forwarded_headers: bool,
    /// When `Some((access_token_expiry_hours, refresh_token_expiry_days))`, spawn
    /// the one-time session-settings config seed (writes the singleton only while
    /// it hasn't been seeded — the operator's DB values win thereafter). Mirrors
    /// ziee's `AuthModule::init` seed step. Pass `None` to skip it.
    pub session_expiry_seed: Option<(i64, i64)>,
}

/// Mount the complete auth surface onto `router` in ONE call (G1). Folds in
/// everything an app currently hand-writes:
///   1. installs `Extension<AuthContext>`, `Extension<Arc<JwtService>>`, and
///      `Extension<Arc<R>>` (the resolver the framework's `RequirePermissions`
///      extractor pulls back out) onto the router,
///   2. sets the reverse-proxy trust flag,
///   3. (optionally) spawns the one-time session-settings seed,
///   4. nests `auth_routes::<R>()` at `/auth` and merges `auth_admin_routes::<R>()`.
///
/// Layer semantics: the extensions apply to every route present on `router` at
/// call time, so call `mount_auth` AFTER registering your own permission-gated
/// routes (or serve the returned router). The resolver `R` must resolve to
/// ziee-auth's own `User`/`Group` wire types; pass [`DefaultIdentityResolver`]
/// for the batteries-included path.
pub fn mount_auth<R>(
    router: aide::axum::ApiRouter,
    resolver: Arc<R>,
    jwt: Arc<JwtService>,
    ctx: AuthContext,
    opts: AuthMountOptions,
) -> aide::axum::ApiRouter
where
    R: IdentityResolver<User = User, Group = Group>,
{
    // (2) reverse-proxy trust flag.
    super::set_trust_forwarded_headers(opts.trust_forwarded_headers);

    // (3) one-time session-settings seed (spawned; failure is non-fatal — mint
    // falls back to the config values whenever the DB read fails).
    if let Some((access_hours, refresh_days)) = opts.session_expiry_seed {
        let repo = ctx.session_settings();
        tokio::spawn(async move {
            if let Err(e) = repo.seed_from_config_once(access_hours, refresh_days).await {
                tracing::warn!(error = ?e, "session_settings config seed failed; DB defaults remain");
            }
        });
    }

    // (4) mount the routes bundle (same paths + order as ziee).
    let auth_router = aide::axum::ApiRouter::new()
        .nest("/auth", auth_routes::<R>())
        .merge(auth_admin_routes::<R>());

    // (1) install the extension layers + merge the routes.
    router
        .merge(auth_router)
        .layer(Extension(ctx))
        .layer(Extension(jwt))
        .layer(Extension(resolver))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::context::{NoopAuthEventSink, NoopAuthSyncSink};
    use crate::auth::jwt::{JwtService, JwtSettings};

    fn strong_jwt() -> Arc<JwtService> {
        // A ≥32-char non-placeholder secret so try_new accepts it.
        let settings = JwtSettings {
            secret: "0123456789abcdef0123456789abcdef-strong".to_string(),
            issuer: "test".to_string(),
            audience: "test".to_string(),
            access_token_expiry_hours: 24,
            refresh_token_expiry_days: 30,
            access_token_expiry_seconds: None,
        };
        Arc::new(JwtService::try_new(settings).unwrap())
    }

    /// Compile-time proof `DefaultIdentityResolver` satisfies the trait the auth
    /// routes are generic over — this is what lets an app skip the ~90-line impl.
    fn _assert_is_resolver<R: IdentityResolver<User = User, Group = Group>>() {}

    #[test]
    fn default_resolver_implements_identity_resolver() {
        _assert_is_resolver::<DefaultIdentityResolver>();
    }

    /// Smoke: construct the resolver + a noop-sink `AuthContext` over a LAZY pool
    /// (no live DB needed) and mount the full auth surface. Proves the turnkey
    /// path type-checks + wires end to end — an app really does get working auth
    /// in ~5 lines. `connect_lazy` never dials, so this stays unit-test cheap.
    #[tokio::test]
    async fn mount_auth_wires_the_full_surface() {
        let pool = sqlx::postgres::PgPool::connect_lazy(
            "postgresql://postgres:password@127.0.0.1:54321/postgres",
        )
        .unwrap();
        let jwt = strong_jwt();
        let resolver = Arc::new(DefaultIdentityResolver::new(pool.clone(), jwt.clone()));
        let ctx = AuthContext::new(
            Arc::new(pool),
            None,
            Arc::new(NoopAuthEventSink),
            Arc::new(NoopAuthSyncSink),
        );

        let router = aide::axum::ApiRouter::new();
        let mounted = mount_auth(
            router,
            resolver,
            jwt,
            ctx,
            AuthMountOptions {
                trust_forwarded_headers: false,
                // None → don't spawn the seed (no runtime in a plain #[test]).
                session_expiry_seed: None,
            },
        );

        // Finish into a concrete axum Router — this only type-checks if the
        // routes + generic handlers + extension layers all resolved.
        let mut api = aide::openapi::OpenApi::default();
        let _app: axum::Router = mounted.finish_api(&mut api);
        // The auth surface registered operations on the OpenAPI doc.
        assert!(api.paths.is_some(), "auth routes should populate the spec");
    }
}
