//! Permission keys for the auth module's admin surfaces.
//!
//! These gate the `/api/admin/auth-providers` CRUD + test endpoints.
//! The public OAuth flow (`/api/auth/oauth/*`, `/api/auth/providers`,
//! `/api/auth/link-account`) is intentionally NOT permission-gated —
//! those endpoints are reached by unauthenticated users mid-login.
//!
//! Both permissions are implicitly held by the Administrators group
//! via its `*` wildcard, so no seed migration is needed. Operators
//! who want to delegate auth-provider management to a non-admin
//! group can grant `auth_providers::read` and/or `auth_providers::manage`
//! through the existing group-permissions UI.

use ziee_identity::PermissionCheck;

/// Read access to the auth providers list + their (masked) config.
/// Sufficient to render the admin settings page without being able to
/// change anything.
pub struct AuthProvidersRead;

impl PermissionCheck for AuthProvidersRead {
    const NAME: &'static str = "AuthProvidersRead";
    const PERMISSION: &'static str = "auth_providers::read";
    const DESCRIPTION: &'static str =
        "List configured auth providers and view their (masked) config.";
    const MODULE: &'static str = "auth";
}

/// Full CRUD + connection-test on auth providers. Holders can create
/// new providers, change client secrets, toggle enabled, and trigger
/// the OIDC discovery / Apple-JWT health check.
pub struct AuthProvidersManage;

impl PermissionCheck for AuthProvidersManage {
    const NAME: &'static str = "AuthProvidersManage";
    const PERMISSION: &'static str = "auth_providers::manage";
    const DESCRIPTION: &'static str =
        "Create, update, delete, enable/disable, and test auth providers.";
    const MODULE: &'static str = "auth";
}

/// Read the deployment-wide session settings (access-token TTL + max
/// session length). Implicitly held by Administrators via `*`; no seed
/// migration needed.
pub struct SessionSettingsRead;

impl PermissionCheck for SessionSettingsRead {
    const NAME: &'static str = "SessionSettingsRead";
    const PERMISSION: &'static str = "auth::session_settings::read";
    const DESCRIPTION: &'static str =
        "Read session settings (access-token TTL + max session length).";
    const MODULE: &'static str = "auth";
}

/// Mutate the deployment-wide session settings. Changes apply to tokens
/// minted from that moment on; existing tokens keep their original exp.
pub struct SessionSettingsManage;

impl PermissionCheck for SessionSettingsManage {
    const NAME: &'static str = "SessionSettingsManage";
    const PERMISSION: &'static str = "auth::session_settings::manage";
    const DESCRIPTION: &'static str =
        "Update session settings (access-token TTL + max session length).";
    const MODULE: &'static str = "auth";
}
