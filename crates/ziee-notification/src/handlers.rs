//! Notification inbox HTTP handlers. Owner-scoped, gated `notifications::read`
//! (the inbox is strictly per-user, so one permission covers reads + per-user
//! mutations).
//!
//! RESOLVER-GENERIC: the router + every handler are generic over the app's
//! injected `R: IdentityResolver` (fixed to this crate's `ziee_auth::{User,
//! Group}` wire types, exactly like `ziee_file::http::file_routes<R>`). ziee
//! mounts `notification_router::<ZieeIdentityResolver>()`; the turnkey
//! `module`-feature `NotificationModule` mounts it with `DefaultIdentityResolver`.
//! Both resolve the SAME `Arc<R>` the app installs into the request extensions,
//! so a handler never hard-codes a resolver an app didn't install.

use aide::axum::routing::{delete_with, get_with, post_with};
use aide::axum::ApiRouter;
use aide::transform::TransformOperation;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::{Extension, Json};
use schemars::JsonSchema;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use ziee_auth::{Group, User};
use ziee_core::{ApiResult, AppError};
use ziee_framework::permissions::{with_permission, IdentityResolver, RequirePermissions};
use ziee_framework::sync::SyncOrigin;

use crate::events::{emit_bulk_changed, emit_row_changed, NotifSyncAction};
use crate::models::{Notification, NotificationPage, UnreadCount};
use crate::permissions::NotificationsRead;
use crate::registry::{registered_kinds, NotificationKindDescriptor};
use crate::repository;

fn e(err: AppError) -> (StatusCode, AppError) {
    err.to_api_error()
}

/// Query params for the paged inbox list.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct NotificationListQuery {
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub per_page: Option<i64>,
    /// When true, only unread rows.
    #[serde(default)]
    pub unread_only: Option<bool>,
}

/// Bounds enforced on the raw list query params.
const DEFAULT_PER_PAGE: i64 = 20;
const MAX_PER_PAGE: i64 = 200;

impl NotificationListQuery {
    /// Clamp the raw params to safe bounds — `(page, per_page, unread_only)`:
    /// page floors at 1; per_page defaults to 20 and clamps to `[1, 200]`;
    /// unread_only defaults to false. Pure, so the pagination invariants are
    /// unit-tested independently of the DB handler.
    pub fn normalized(&self) -> (i64, i64, bool) {
        let page = self.page.unwrap_or(1).max(1);
        let per_page = self.per_page.unwrap_or(DEFAULT_PER_PAGE).clamp(1, MAX_PER_PAGE);
        (page, per_page, self.unread_only.unwrap_or(false))
    }
}

/// The generic inbox router, bound over any resolver whose wire types are this
/// crate's `ziee_auth::{User, Group}` — matching what the app installs. A handler
/// only ever reads `auth.user.id`, so this concrete-`User` bound is all it needs
/// (mirrors `file_routes<R: IdentityResolver<User = User, Group = Group>>`).
pub fn notification_router<R: IdentityResolver<User = User, Group = Group>>() -> ApiRouter {
    ApiRouter::new()
        .api_route("/notifications", get_with(list_notifications::<R>, list_docs))
        .api_route(
            "/notifications/unread-count",
            get_with(unread_count::<R>, unread_count_docs),
        )
        .api_route("/notifications/kinds", get_with(list_kinds::<R>, kinds_docs))
        .api_route(
            "/notifications/read-all",
            post_with(read_all::<R>, read_all_docs),
        )
        .api_route(
            "/notifications/{id}/read",
            post_with(mark_read::<R>, mark_read_docs),
        )
        // Single-item GET (consumed by ziee's NotificationToastListener). The
        // more-specific `/notifications/{id}/read` above is matched first, so
        // this `{id}` route is unambiguous.
        .api_route(
            "/notifications/{id}",
            get_with(get_notification::<R>, get_notification_docs),
        )
        .api_route(
            "/notifications/{id}",
            delete_with(delete_notification::<R>, delete_docs),
        )
}

async fn list_notifications<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (NotificationsRead,)>,
    Extension(pool): Extension<PgPool>,
    Query(q): Query<NotificationListQuery>,
) -> ApiResult<Json<NotificationPage>> {
    let (page, per_page, unread_only) = q.normalized();
    let (items, total, unread) =
        repository::list_for_user(&pool, auth.user.id, unread_only, page, per_page)
            .await
            .map_err(e)?;
    Ok((
        StatusCode::OK,
        Json(NotificationPage {
            items,
            total,
            unread,
            page,
            per_page,
        }),
    ))
}

fn list_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(NotificationsRead,)>(op)
        .id("Notification.list")
        .tag("notification")
        .response::<200, Json<NotificationPage>>()
}

async fn get_notification<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (NotificationsRead,)>,
    Extension(pool): Extension<PgPool>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Notification>> {
    let row = repository::get_for_user(&pool, auth.user.id, id)
        .await
        .map_err(e)?
        .ok_or_else(|| e(AppError::not_found("Notification")))?;
    Ok((StatusCode::OK, Json(row)))
}

fn get_notification_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(NotificationsRead,)>(op)
        .id("Notification.get")
        .tag("notification")
        .response::<200, Json<Notification>>()
}

async fn unread_count<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (NotificationsRead,)>,
    Extension(pool): Extension<PgPool>,
) -> ApiResult<Json<UnreadCount>> {
    let unread = repository::unread_count(&pool, auth.user.id)
        .await
        .map_err(e)?;
    Ok((StatusCode::OK, Json(UnreadCount { unread })))
}

fn unread_count_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(NotificationsRead,)>(op)
        .id("Notification.unreadCount")
        .tag("notification")
        .response::<200, Json<UnreadCount>>()
}

async fn list_kinds<R: IdentityResolver<User = User, Group = Group>>(
    _auth: RequirePermissions<R, (NotificationsRead,)>,
) -> ApiResult<Json<Vec<NotificationKindDescriptor>>> {
    Ok((StatusCode::OK, Json(registered_kinds())))
}

fn kinds_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(NotificationsRead,)>(op)
        .id("Notification.kinds")
        .tag("notification")
        .response::<200, Json<Vec<NotificationKindDescriptor>>>()
}

async fn mark_read<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (NotificationsRead,)>,
    Extension(pool): Extension<PgPool>,
    origin: SyncOrigin,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<UnreadCount>> {
    // Contract-preserving (extraction from ziee): returns the caller's UnreadCount
    // after marking, and emits an inbox-changed frame (nil id) only when a row was
    // actually newly-marked. The single-row GET is `get_notification`.
    let user_id = auth.user.id;
    let changed = repository::mark_read(&pool, user_id, id).await.map_err(e)?;
    if changed {
        emit_bulk_changed(user_id, origin.0);
    }
    let unread = repository::unread_count(&pool, user_id)
        .await
        .map_err(e)?;
    Ok((StatusCode::OK, Json(UnreadCount { unread })))
}

fn mark_read_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(NotificationsRead,)>(op)
        .id("Notification.markRead")
        .tag("notification")
        .response::<200, Json<UnreadCount>>()
}

async fn read_all<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (NotificationsRead,)>,
    Extension(pool): Extension<PgPool>,
    origin: SyncOrigin,
) -> ApiResult<Json<UnreadCount>> {
    let user_id = auth.user.id;
    repository::mark_all_read(&pool, user_id).await.map_err(e)?;
    emit_bulk_changed(user_id, origin.0);
    Ok((StatusCode::OK, Json(UnreadCount { unread: 0 })))
}

fn read_all_docs(op: TransformOperation) -> TransformOperation {
    // Contract-preserving operationId (extraction from ziee): the client method is
    // `ApiClient.Notification.markAllRead`, so the id stays `Notification.markAllRead`.
    with_permission::<(NotificationsRead,)>(op)
        .id("Notification.markAllRead")
        .tag("notification")
        .response::<200, Json<UnreadCount>>()
}

async fn delete_notification<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (NotificationsRead,)>,
    Extension(pool): Extension<PgPool>,
    origin: SyncOrigin,
    Path(id): Path<Uuid>,
) -> ApiResult<()> {
    let user_id = auth.user.id;
    let removed = repository::delete(&pool, user_id, id).await.map_err(e)?;
    if !removed {
        return Err(e(AppError::not_found("Notification")));
    }
    emit_row_changed(user_id, NotifSyncAction::Delete, id, origin.0);
    Ok((StatusCode::NO_CONTENT, ()))
}

fn delete_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(NotificationsRead,)>(op)
        .id("Notification.delete")
        .tag("notification")
        .response::<204, ()>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(page: Option<i64>, per_page: Option<i64>, unread_only: Option<bool>) -> NotificationListQuery {
        NotificationListQuery { page, per_page, unread_only }
    }

    #[test]
    fn normalized_applies_defaults() {
        assert_eq!(q(None, None, None).normalized(), (1, DEFAULT_PER_PAGE, false));
    }

    #[test]
    fn normalized_page_floors_at_one() {
        assert_eq!(q(Some(0), None, None).normalized().0, 1);
        assert_eq!(q(Some(-5), None, None).normalized().0, 1);
        assert_eq!(q(Some(7), None, None).normalized().0, 7, "valid page is passed through");
    }

    #[test]
    fn normalized_per_page_clamps_both_ends() {
        assert_eq!(q(None, Some(0), None).normalized().1, 1, "below-min clamps up to 1");
        assert_eq!(q(None, Some(-3), None).normalized().1, 1);
        assert_eq!(q(None, Some(MAX_PER_PAGE + 1000), None).normalized().1, MAX_PER_PAGE, "over-max clamps down");
        assert_eq!(q(None, Some(50), None).normalized().1, 50, "in-range is passed through");
    }

    #[test]
    fn normalized_unread_only_defaults_false_and_passes_through() {
        assert!(!q(None, None, None).normalized().2);
        assert!(q(None, None, Some(true)).normalized().2);
        assert!(!q(None, None, Some(false)).normalized().2);
    }
}
