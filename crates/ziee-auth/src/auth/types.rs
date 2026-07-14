// Auth request/response type definitions

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::jwt::TokenPair;
use crate::user::User;

// =====================================================
// Request Types
// =====================================================

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// Optional provider name for LDAP/OAuth authentication
    /// If not specified, defaults to local password authentication
    pub provider: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RefreshTokenRequest {
    /// The refresh token. Optional: web clients omit it and the server
    /// reads the httpOnly `ziee_refresh` cookie instead; desktop/tunnel
    /// clients keep sending it in the body. An explicit body token wins
    /// over the cookie, and the response mirrors the source (body-in→
    /// body-out, cookie-in→cookie-out).
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OAuthAuthorizeQuery {
    /// Same-origin SPA path the user wants to land on after a
    /// successful login. Validated server-side as a same-origin path
    /// (no `//`, no absolute URLs) and stored in `oauth_sessions.return_to`
    /// — never forwarded through the provider URL.
    pub return_to: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OAuthCallbackQuery {
    /// Authorization code from provider
    pub code: String,
    /// State parameter for CSRF protection
    pub state: String,
}

/// Apple Sign In `form_post` body. Apple POSTs the callback when
/// scope includes `name` or `email` (mandatory per their HIG).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AppleCallbackForm {
    pub code: String,
    pub state: String,
    /// Apple sends the id_token in the form_post too, but we ignore
    /// it — the server-side flow exchanges `code` for a token via
    /// Apple's /auth/token endpoint and verifies that token against
    /// the JWKS. Field is captured for schema completeness.
    #[serde(default)]
    #[allow(dead_code)]
    pub id_token: Option<String>,
    /// First-auth-only JSON blob: `{"name":{"firstName":..,"lastName":..},"email":..}`.
    /// Apple sends this exactly ONCE, in the POST body, the very
    /// first time a user authorizes our app. We must persist it on
    /// arrival or lose access to the user's name forever.
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LinkAccountRequest {
    /// Single-use token from /auth/link-account?token=...
    pub link_token: String,
    /// User's existing local password — the auth proof for binding
    /// the social identity.
    pub password: String,
}

/// Self-service profile update for the authenticated user. Only the
/// safe fields are accepted here: `email` is intentionally NOT
/// editable (re-verification flow not built; was removed to close an
/// OAuth account-takeover vector) and `is_active`/`is_admin`/
/// `permissions` are admin-only and can never be set through this path.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateProfileRequest {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// Self-service password change for the authenticated user. Requires
/// the current password as proof. Only valid for local-password
/// accounts (`password_hash IS NOT NULL`).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

// =====================================================
// Response Types
// =====================================================

#[derive(Debug, Serialize, JsonSchema)]
pub struct AuthResponse {
    pub user: User,
    #[serde(flatten)]
    pub tokens: TokenPair,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MeResponse {
    pub user: User,
    /// Effective permissions (union of user's direct permissions + all active group permissions)
    pub permissions: Vec<String>,
    /// Whether this account has a local password set (`password_hash IS
    /// NOT NULL`). `password_hash` itself is never serialized, so this
    /// derived flag is how the client decides whether to offer a
    /// self-service "change password" form (false for OAuth/LDAP-only).
    pub has_password: bool,
}

/// Public-safe summary of an enabled auth provider — what the login
/// page needs to render a provider button. NEVER includes client_id,
/// client_secret, or any other config.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PublicProvider {
    pub name: String,
    pub provider_type: String,
    pub display_name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PublicProvidersResponse {
    pub providers: Vec<PublicProvider>,
}

/// Admin-view summary of an auth provider. The `config` JSONB is
/// returned with `client_secret` and any sensitive key contents
/// MASKED — see handlers.rs::mask_provider_config.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AuthProviderResponse {
    pub id: uuid::Uuid,
    pub name: String,
    pub provider_type: String,
    pub enabled: bool,
    pub config: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// When the admin last clicked Test on this row (null = never).
    pub last_test_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Outcome of the last test (null = never tested).
    pub last_test_ok: Option<bool>,
    /// Human-readable detail from the last test.
    pub last_test_message: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateAuthProviderRequest {
    pub name: String,
    /// One of: `oidc`, `oauth2`, `apple`, `ldap`, `local`.
    pub provider_type: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub config: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateAuthProviderRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Patch the provider's JSONB config. Empty `client_secret`
    /// field is treated as "leave the existing value unchanged" so
    /// admins can edit other fields without re-entering secrets.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TestProviderResponse {
    pub ok: bool,
    pub message: String,
}

/// Response body for `POST /admin/auth-providers`. Wraps the persisted
/// row in an envelope so we can attach a `connection_warning` when a
/// create-time probe failed and the row was auto-downgraded to
/// `enabled=false`. Mirrors `llm_repository`'s create-response shape.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CreateAuthProviderResponse {
    pub provider: AuthProviderResponse,
    /// Populated when the provider was created with `enabled=true` but
    /// the connection probe failed; the row is persisted with
    /// `enabled=false` and this carries the failure reason so the UI
    /// can render an inline warning. `null` on the happy path.
    pub connection_warning: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteProviderResponse {
    pub deleted: bool,
    pub affected_user_links: i64,
}

fn default_true() -> bool {
    true
}

// =====================================================
// Change password
// =====================================================
//
// NOTE: `AuthConfigResponse` and `PasswordOnlyLoginRequest` (which
// used to live here) moved with their handlers into the desktop
// tauri crate (`desktop/tauri/src/modules/tunnel_auth/`) — they
// depend on the desktop-only `remote_access_settings` table.

// NOTE: the desktop tauri crate keeps its OWN `ChangePasswordRequest`
// + `change_password` handler at `/users/me/password`
// (`desktop/tauri/src/modules/tunnel_auth/`) for the Remote Access
// password gate. The server-crate `ChangePasswordRequest` above backs
// the web self-service `POST /auth/password` and is intentionally
// separate (no shared route, no shared handler).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_profile_request_all_fields_optional() {
        // Empty object → both None (the no-op patch).
        let r: UpdateProfileRequest = serde_json::from_str("{}").unwrap();
        assert!(r.username.is_none() && r.display_name.is_none());

        // Display-only.
        let r: UpdateProfileRequest =
            serde_json::from_str(r#"{"display_name":"Dee"}"#).unwrap();
        assert_eq!(r.display_name.as_deref(), Some("Dee"));
        assert!(r.username.is_none());

        // Username-only.
        let r: UpdateProfileRequest =
            serde_json::from_str(r#"{"username":"neo"}"#).unwrap();
        assert_eq!(r.username.as_deref(), Some("neo"));
        assert!(r.display_name.is_none());

        // Both.
        let r: UpdateProfileRequest =
            serde_json::from_str(r#"{"username":"neo","display_name":"Dee"}"#).unwrap();
        assert_eq!(r.username.as_deref(), Some("neo"));
        assert_eq!(r.display_name.as_deref(), Some("Dee"));
    }

    #[test]
    fn update_profile_request_distinguishes_empty_string_from_null() {
        // The handler relies on this: `null` (and absent) deserialize to
        // None → no-op; empty string deserializes to Some("") → which the
        // handler normalizes into "clear display_name to NULL".
        let null: UpdateProfileRequest =
            serde_json::from_str(r#"{"display_name":null}"#).unwrap();
        assert!(null.display_name.is_none());

        let empty: UpdateProfileRequest =
            serde_json::from_str(r#"{"display_name":""}"#).unwrap();
        assert_eq!(empty.display_name.as_deref(), Some(""));
    }

    #[test]
    fn update_profile_request_drops_unknown_privileged_fields() {
        // serde ignores unknown fields by default — is_admin / permissions
        // / email / is_active simply don't exist on the struct, so they
        // can never be set through this path.
        let r: UpdateProfileRequest = serde_json::from_str(
            r#"{"display_name":"ok","is_admin":true,"permissions":["*"],"email":"x@y.z","is_active":false}"#,
        )
        .unwrap();
        assert_eq!(r.display_name.as_deref(), Some("ok"));
        assert!(r.username.is_none());
    }

    #[test]
    fn change_password_request_requires_both_fields() {
        // Both present → ok.
        let r: ChangePasswordRequest =
            serde_json::from_str(r#"{"current_password":"a","new_password":"b"}"#).unwrap();
        assert_eq!(r.current_password, "a");
        assert_eq!(r.new_password, "b");

        // Missing new_password → deserialization error.
        assert!(
            serde_json::from_str::<ChangePasswordRequest>(r#"{"current_password":"a"}"#).is_err()
        );
        // Missing current_password → deserialization error.
        assert!(
            serde_json::from_str::<ChangePasswordRequest>(r#"{"new_password":"b"}"#).is_err()
        );
    }
}
