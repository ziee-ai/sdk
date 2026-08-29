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
pub mod registration;
pub mod repository;
pub mod session_settings;
pub mod types;
pub mod username;

/// The mountable HTTP/aide routes bundle (decision N10). Gated behind the
/// default-on `routes` feature so the auth ENGINE can be consumed without the
/// aide/axum surface.
#[cfg(feature = "routes")]
pub mod http;

/// Turnkey auth wiring (chunk sdk-batteries / G1): `DefaultIdentityResolver` +
/// `mount_auth` so an app mounts working auth in one call. Gated on `routes`
/// (needs aide/axum). ADDITIVE — ziee keeps its own `ZieeIdentityResolver`.
#[cfg(feature = "routes")]
pub mod turnkey;

/// Turnkey auth MODULE for the standard `build_api_router` path (gap N-3): an
/// [`AppModule`](ziee_framework::module_api::AppModule) that registers the auth
/// routes through the module system (so they land at `/api/auth/*`) AND installs
/// the resolver/JWT/AuthContext extensions at the whole-app router level. Gated
/// behind the non-default `module` feature (implies `routes`); inert for ziee,
/// which keeps its own resolver-backed module.
#[cfg(feature = "module")]
pub mod module;

#[cfg(feature = "routes")]
pub use http::{auth_admin_routes, auth_routes};

#[cfg(feature = "routes")]
pub use turnkey::{mount_auth, AuthMountOptions, DefaultIdentityResolver};

#[cfg(feature = "module")]
pub use module::AuthModule;

// The registration POLICY seam: how a deployment closes account creation.
// Engine-level (no `routes` feature) so an app that mounts the router by hand
// gets the same gate.
pub use registration::{
    OpenRegistrationPolicy, RegistrationChannel, RegistrationPolicy, RegistrationRefused,
    check_registration_allowed, install_registration_policy, registration_policy,
    resolve_registration,
};

// Re-exports (mirror the app auth module's public surface).
pub use context::{
    AuthContext, AuthEventSink, AuthSyncAction, AuthSyncEntity, AuthSyncNotDeclared, AuthSyncSink,
    AuthSyncWiring, NoopAuthEventSink, NoopAuthSyncSink, declare_auth_sync,
    declare_auth_sync_inert, declared_auth_sync, install_auth_sync_sink, resolve_auth_sync,
};
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

/// Set once at module init from the operator-configured public origin (see
/// `set_configured_public_origin`). Holds the https-validated origin, or
/// `None` to fall back to request-header derivation.
static CONFIGURED_PUBLIC_ORIGIN: OnceLock<Option<String>> = OnceLock::new();

/// Install the operator-configured https public origin (called once at boot by
/// the app's `AuthModule::init`, passing e.g. `code_sandbox.public_base_url`).
/// The raw value passes through the `https_public_origin` gate — only a
/// non-empty `https://` origin is adopted; an http/loopback value (the LOCAL
/// dev default) is ignored so local dev keeps deriving the redirect_uri from
/// request headers. Idempotent — a second call is a no-op.
pub fn set_configured_public_origin(raw: Option<&str>) {
    let _ = CONFIGURED_PUBLIC_ORIGIN.set(https_public_origin(raw));
}

/// The operator-configured https public origin (no trailing slash) that OAuth
/// `redirect_uri`s should be rooted at, or `None` to derive the origin from
/// request headers. Behind an HTTPS edge that terminates TLS and forwards plain
/// HTTP to this container, the header-derived scheme is `http`, producing
/// `http://` redirect_uris that Google rejects; a configured https origin fixes
/// that deterministically. Safe against the header-spoofing class because the
/// value is operator-controlled, not request-derived.
pub fn configured_public_origin() -> Option<String> {
    CONFIGURED_PUBLIC_ORIGIN.get().cloned().flatten()
}

/// Return the trimmed origin (no trailing slash) IFF `raw` is a non-empty
/// `https://` URL; otherwise `None`. An http / loopback value — e.g. the LOCAL
/// default `http://172.21.0.1:8080` that `code_sandbox.public_base_url` carries
/// for host-gateway file fetches — returns `None`, so local dev keeps deriving
/// the redirect_uri from request headers and is unaffected.
pub(crate) fn https_public_origin(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    // Case-insensitive scheme check; must be EXACTLY the https scheme, not
    // merely a string that contains "https".
    if s.is_empty() || !s.to_ascii_lowercase().starts_with("https://") {
        return None;
    }
    Some(s.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod public_origin_tests {
    use super::https_public_origin;

    #[test]
    fn only_https_origins_are_used() {
        // Deploy: an https public origin is adopted (trailing slash trimmed).
        assert_eq!(
            https_public_origin(Some("https://biognosia.tinnguyen-lab.com")).as_deref(),
            Some("https://biognosia.tinnguyen-lab.com")
        );
        assert_eq!(
            https_public_origin(Some("https://x.example/")).as_deref(),
            Some("https://x.example")
        );
        // Local/dev: http, loopback, empty, and None all fall through to header
        // derivation (return None).
        assert_eq!(https_public_origin(Some("http://172.21.0.1:8080")), None);
        assert_eq!(https_public_origin(Some("http://localhost:8080")), None);
        assert_eq!(https_public_origin(Some("")), None);
        assert_eq!(https_public_origin(Some("   ")), None);
        assert_eq!(https_public_origin(None), None);
    }
}
