//! Deployment-wide JWT session settings — REST handlers (the HTTP/aide
//! boundary of the auth surface).
//!
//! The schema-bound DTOs (`SessionSettings` / `UpdateSessionSettingsRequest`)
//! and the repository (`query!` macros) live in [`crate::auth::session_settings`];
//! this module holds the permission-gated GET/PUT handlers + the sync notify.
//! The handlers are generic over the app's injected [`IdentityResolver`] (fixed
//! to this crate's own `User`/`Group` wire types), so a second app mounts them
//! with its own auth mechanism while the wire schema stays identical.

use aide::transform::TransformOperation;
use axum::Extension;
use axum::{Json, http::StatusCode};
use uuid::Uuid;

use ziee_core::{ApiResult, AppError};
use ziee_framework::permissions::{IdentityResolver, RequirePermissions, with_permission};
use ziee_framework::sync::{Audience, SyncOrigin};

use crate::auth::context::{AuthContext, AuthSyncAction, AuthSyncEntity};
use crate::auth::permissions::{SessionSettingsManage, SessionSettingsRead};
use crate::auth::session_settings::{SessionSettings, UpdateSessionSettingsRequest};
use crate::user::{Group, User};

// ─────────────────────────── REST handlers ───────────────────────────

pub async fn get_session_settings<R: IdentityResolver<User = User, Group = Group>>(
    _auth: RequirePermissions<R, (SessionSettingsRead,)>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<SessionSettings>> {
    let row = ctx.session_settings().get().await?;
    Ok((StatusCode::OK, Json(row)))
}

pub fn get_session_settings_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(SessionSettingsRead,)>(op)
        .id("Auth.getSessionSettings")
        .tag("auth")
        .summary("Read session settings (access-token TTL + max session length)")
        .response::<200, Json<SessionSettings>>()
}

pub async fn update_session_settings<R: IdentityResolver<User = User, Group = Group>>(
    _auth: RequirePermissions<R, (SessionSettingsManage,)>,
    origin: SyncOrigin,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<UpdateSessionSettingsRequest>,
) -> ApiResult<Json<SessionSettings>> {
    if let Some(n) = body.access_token_expiry_hours
        && !(1..=8760).contains(&n)
    {
        return Err(AppError::bad_request(
            "VALIDATION_ERROR",
            "access_token_expiry_hours out of range (1..=8760)",
        )
        .into());
    }
    if let Some(n) = body.refresh_token_expiry_days
        && !(1..=3650).contains(&n)
    {
        return Err(AppError::bad_request(
            "VALIDATION_ERROR",
            "refresh_token_expiry_days out of range (1..=3650)",
        )
        .into());
    }

    let row = ctx
        .session_settings()
        .update(body.access_token_expiry_hours, body.refresh_token_expiry_days)
        .await?;

    ctx.sync.publish(
        AuthSyncEntity::SessionSettings,
        AuthSyncAction::Update,
        Uuid::nil(),
        Audience::perm::<SessionSettingsRead>(),
        origin.0,
    );
    Ok((StatusCode::OK, Json(row)))
}

pub fn update_session_settings_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(SessionSettingsManage,)>(op)
        .id("Auth.updateSessionSettings")
        .tag("auth")
        .summary("Update session settings (access-token TTL + max session length)")
        .response::<200, Json<SessionSettings>>()
}
