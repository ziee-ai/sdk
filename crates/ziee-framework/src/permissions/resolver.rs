//! `IdentityResolver` — the injected identity-resolution seam.
//!
//! Chunk B3 moved permission ENFORCEMENT (`RequirePermissions` / `RequireAdmin`
//! / `with_permission`) into the framework, generic over this trait. The
//! framework never names ziee's global `Repos`, `JwtService`, or concrete
//! `User`/`Group`; instead an app installs its own `IdentityResolver` into the
//! axum request extensions at startup, and the extractor pulls it back out. This
//! is the last framework-facing global de-globalized (subsumes chunk BG's
//! Repos-for-permission-resolution refactor — JWT was abstracted in B1b via
//! [`ziee_identity::TokenVerifier`], config in B2 via `ServerConfig`).
//!
//! ziee's concrete resolver (`ZieeIdentityResolver`) is a unit struct backed by
//! ziee's `Repos` + the `Arc<JwtService>` already layered into extensions; the
//! two methods below are the exact former `extract_authenticated_user` +
//! `Repos.user.get_user_groups` touch points, kept byte-identical app-side.

use axum::http::{StatusCode, request::Parts};
use ziee_core::AppError;
use ziee_identity::Principal;

/// Resolves an authenticated identity (+ its groups) from a request, injected by
/// the app so the framework's permission enforcement stays generic (pluggable
/// identity, decision #1).
///
/// Split into two steps to preserve ziee's exact behavior: an admin
/// short-circuits BEFORE its groups are loaded (`RequirePermissions` returns an
/// empty `groups` vec for admins and never issues the group query), so
/// [`authenticate`](IdentityResolver::authenticate) (JWT-validate + load the
/// active acting user) is distinct from [`load_groups`](IdentityResolver::load_groups).
#[async_trait::async_trait]
pub trait IdentityResolver: Send + Sync + 'static {
    /// The authenticated identity the app yields (ziee: `User`). Must be a
    /// [`Principal`] so the framework runs the generic union check, and `Clone`
    /// so the extractor can hand it to the handler.
    type User: Principal + Clone + Send + Sync + 'static;

    /// The app's group type (ziee: `Group`), exposed opaquely to the framework;
    /// its permissions are read via [`active_group_permissions`](IdentityResolver::active_group_permissions).
    type Group: Clone + Send + Sync + 'static;

    /// Validate the request's credentials and load the active acting identity
    /// (no groups). Returns an axum rejection `(status, error)` on any auth
    /// failure (missing/invalid token, unknown/inactive user).
    async fn authenticate(&self, parts: &mut Parts) -> Result<Self::User, (StatusCode, AppError)>;

    /// Load the identity's groups for the non-admin permission union.
    async fn load_groups(
        &self,
        user: &Self::User,
    ) -> Result<Vec<Self::Group>, (StatusCode, AppError)>;

    /// The permission strings of `group` IF it is active, else `None` — mirrors
    /// the `group.is_active` guard in ziee's `check_permission_union`, keeping
    /// the generic union check byte-identical to the concrete one.
    fn active_group_permissions(group: &Self::Group) -> Option<&[String]>;

    /// The access token's unix `exp` (seconds), read from the request parts,
    /// used by [`crate::sync::sync_routes`] to bound an SSE stream's lifetime:
    /// when the token lapses the client reconnects with a fresh token and
    /// re-runs [`authenticate`](IdentityResolver::authenticate). `None` (the
    /// default) → the stream falls back to a far-future deadline. The resolver
    /// owns token verification, so exposing the expiry here keeps the mountable
    /// sync handler generic over the app's JWT scheme (chunk sdk-surfaces).
    fn access_token_exp(&self, _parts: &Parts) -> Option<i64> {
        None
    }
}
