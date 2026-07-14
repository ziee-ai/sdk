//! The single-user auto-login strategy + owner-`*` model (Chunk D design-gate 2).
//!
//! Desktop is a **single-user** deployment: one auto-provisioned owner, a
//! permanent session (the UI never bounces to a login screen), and that owner
//! holds the `"*"` permission wildcard so every permission-gated code path —
//! written once for the multi-user server — "just works" for the desktop owner.
//!
//! This is the harness formalization of what the app does ad hoc today across
//! `auth/commands.rs::{mint_admin_login, auto_login}`, `auth/bootstrap.rs::
//! ensure_desktop_admin`, and the per-boot JWT-secret policy. It selects the
//! concrete pieces from the SDK auth/identity crates:
//!
//! - **mint** — [`ziee_auth::auth::refresh_tokens::mint_session_tokens`], the
//!   SAME jti-whitelisted path every server login uses, so desktop sessions are
//!   revocable (logout-everywhere) and pruned like any other.
//! - **owner lookup** — [`ziee_auth::user::UserRepository`].
//! - **owner-`*`** — the owner is an admin whose Administrators group carries
//!   `"*"`; the RBAC evaluator ([`ziee_identity`]'s `is_admin` short-circuit +
//!   `"*"` wildcard) then satisfies every `RequirePermissions` check.
//!
//! ## What stays app-side (BA decision, preserved here)
//!
//! Owner *creation* — `AppRepository::create_admin_user` + the Administrators
//! `"*"` group grant — is the app's domain admin CRUD, which Chunk BA kept
//! app-side. So this strategy owns the readable/mint half ([`owner_missing`],
//! [`mint_owner_login`]) concretely, and routes *creation* through the
//! [`crate::boot::ServerBoot`] seam the app implements. The strategy names the
//! owner identity ([`SingleUserStrategy`]) so both halves agree on username.
//!
//! [`owner_missing`]: SingleUserStrategy::owner_missing
//! [`mint_owner_login`]: SingleUserStrategy::mint_owner_login

use serde::Serialize;
use sqlx::PgPool;
use ziee_auth::auth::JwtService;
use ziee_auth::auth::refresh_tokens::mint_session_tokens;
use ziee_auth::user::UserRepository;
use ziee_auth::User;
use ziee_core::AppError;

/// The permission the single-user owner holds — the full wildcard. Evaluated by
/// `ziee-identity`'s RBAC wildcard match, so a desktop owner passes every
/// `RequirePermissions<...>` gate the multi-user server defines. Kept as a
/// named constant so the app's Administrators-group grant and the harness agree.
pub const OWNER_WILDCARD_PERMISSION: &str = "*";

/// The identity of the single desktop owner. Selects WHICH user the harness
/// auto-logs-in and (via the app's create seam) provisions on first run.
///
/// `desktop_default()` reproduces today's `"admin"` / `admin@localhost` /
/// `desktop-auto-login` owner verbatim, but the fields are parameterized so a
/// second app can pick its own owner identity (pluggable identity, decision #1).
#[derive(Clone, Debug)]
pub struct SingleUserStrategy {
    /// The owner's username (looked up + minted). Today: `"admin"`.
    pub owner_username: String,
    /// The owner's email, used at creation time. Today: `"admin@localhost"`.
    pub owner_email: String,
    /// The owner's bootstrap password (hashed at creation). Today:
    /// `"desktop-auto-login"` — irrelevant to security since the desktop mints
    /// tokens directly, but present so password-login still resolves the owner.
    pub owner_password: String,
}

impl Default for SingleUserStrategy {
    fn default() -> Self {
        Self::desktop_default()
    }
}

impl SingleUserStrategy {
    /// The current ziee-desktop owner: `"admin"` / `admin@localhost` /
    /// `"desktop-auto-login"`. Equivalence-preserving reproduction of
    /// `ensure_desktop_admin` + `mint_admin_login`.
    pub fn desktop_default() -> Self {
        Self {
            owner_username: "admin".to_string(),
            owner_email: "admin@localhost".to_string(),
            owner_password: "desktop-auto-login".to_string(),
        }
    }

    /// The permission set the owner must hold for owner-`*` to work. Returned so
    /// the app's create-owner seam grants exactly this to the owner's
    /// Administrators group.
    pub fn owner_permissions(&self) -> &'static [&'static str] {
        &[OWNER_WILDCARD_PERMISSION]
    }

    /// Whether the owner is absent (first run) — the readable half of
    /// `ensure_desktop_admin`. `true` ⇒ the app's create seam must provision the
    /// owner (see the module docs on why creation stays app-side).
    pub async fn owner_missing(&self, pool: &PgPool) -> Result<bool, AppError> {
        let has_admin = UserRepository::new(pool.clone()).has_admin().await?;
        Ok(!has_admin)
    }

    /// Mint a permanent-session login for the owner — the harness half of
    /// `mint_admin_login` / `auto_login`. Uses the shared jti-whitelisted mint
    /// path so the desktop session is revocable + prunable like any server one.
    ///
    /// Fails with `Admin not found` when the owner has not been provisioned yet
    /// (server still starting / create seam not yet run) — same message as the
    /// app's `mint_admin_login`.
    pub async fn mint_owner_login(
        &self,
        pool: &PgPool,
        jwt: &JwtService,
    ) -> Result<OwnerLogin, AppError> {
        let owner = UserRepository::new(pool.clone())
            .get_by_username(&self.owner_username)
            .await?
            .ok_or_else(|| {
                AppError::internal_error("Admin not found - server may still be starting")
            })?;

        let minted = mint_session_tokens(
            pool,
            jwt,
            owner.id,
            &owner.username,
            &owner.email,
            owner.is_admin,
        )
        .await?;
        let pair = minted.pair;

        Ok(OwnerLogin {
            access_token: pair.access_token,
            refresh_token: pair.refresh_token,
            expires_in: pair.expires_in,
            user: owner,
        })
    }
}

/// The minted permanent-session login for the desktop owner. Shape-compatible
/// with the app's `AutoLoginResponse` (the Tauri `auto_login` command's return).
#[derive(Serialize, Debug)]
pub struct OwnerLogin {
    /// The owner user record.
    pub user: User,
    /// Signed access token.
    pub access_token: String,
    /// jti-whitelisted refresh token.
    pub refresh_token: String,
    /// Seconds until the access token expires.
    pub expires_in: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_default_owner_matches_current_app() {
        let s = SingleUserStrategy::desktop_default();
        assert_eq!(s.owner_username, "admin");
        assert_eq!(s.owner_email, "admin@localhost");
        assert_eq!(s.owner_password, "desktop-auto-login");
    }

    #[test]
    fn owner_holds_full_wildcard() {
        let s = SingleUserStrategy::desktop_default();
        assert_eq!(s.owner_permissions(), &["*"]);
        assert_eq!(OWNER_WILDCARD_PERMISSION, "*");
    }
}
