//! The auth core moved into `ziee-auth` (Chunk BA-full): the schema-bound
//! repositories (`query!` macros), login/register/LDAP/OAuth2, the JWT +
//! Session & Token-Refresh subsystem, the at-rest secret provider repository,
//! the cookie helpers, and the injected [`context`] seams.
//!
//! The app keeps the HTTP/aide boundary (`handlers` / `routes` / `permissions`
//! / `jwt_extractor` + the session-settings REST handlers + the `AuthModule`
//! registration) and consumes this crate via equivalence-preserving re-export
//! shims, so every `crate::modules::auth::…` call site is unchanged.

use std::sync::OnceLock;

pub mod context;
pub mod cookie;
pub mod events;
pub mod jwt;
pub mod password;
pub mod permissions;
pub mod providers;
pub mod refresh_tokens;
pub mod repository;
pub mod session_settings;
pub mod types;

/// The mountable HTTP/aide routes bundle (decision N10). Gated behind the
/// default-on `routes` feature so the auth ENGINE can be consumed without the
/// aide/axum surface.
#[cfg(feature = "routes")]
pub mod http;

#[cfg(feature = "routes")]
pub use http::{auth_admin_routes, auth_routes};

// Re-exports (mirror the app auth module's public surface).
pub use context::{AuthContext, AuthEventSink, AuthSyncAction, AuthSyncEntity, AuthSyncSink};
pub use jwt::JwtService;
#[allow(unused_imports)]
pub use password::hash_password;
pub use repository::AuthRepository;
pub use session_settings::SessionSettingsRepository;
pub use types::AuthResponse;

/// Set once at module init from `config.server.trust_forwarded_headers`.
/// When false, the OAuth-authorize handler derives redirect_uri from the HOST
/// header only, and the cookie helper never emits `Secure` off a spoofable
/// `X-Forwarded-Proto` — defending self-hosted-direct deployments against
/// attacker-supplied forwarded headers.
static TRUST_FORWARDED_HEADERS: OnceLock<bool> = OnceLock::new();

/// Install the reverse-proxy trust flag (called once at boot by the app's
/// `AuthModule::init`). Idempotent — a second call is a no-op (module re-init
/// isn't expected but isn't an error).
pub fn set_trust_forwarded_headers(trust: bool) {
    let _ = TRUST_FORWARDED_HEADERS.set(trust);
}

/// Returns true if the deployment configured a trusted reverse proxy in front
/// of the server. Defaults to `false` (the safer self-hosted-direct posture)
/// when `set_trust_forwarded_headers` hasn't run (e.g. in unit tests that
/// bypass the module loader).
pub fn trust_forwarded_headers() -> bool {
    TRUST_FORWARDED_HEADERS.get().copied().unwrap_or(false)
}
