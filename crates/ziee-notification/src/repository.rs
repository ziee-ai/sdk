//! DB access for `notifications`. Uses sqlx COMPILE-TIME `query!`/`query_as!`
//! macros (verified against this crate's own build DB — see `build.rs`, gated on
//! the `module` feature). `timestamptz` columns need the `col as "col:
//! DateTime<Utc>"` override (sqlx's `time` feature is unified in across the
//! workspace). Every query is owner-scoped (`WHERE user_id = $1`).

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use ziee_core::AppError;

use crate::models::{NewNotification, Notification};

/// Insert one notification, returning the full row.
pub async fn insert(pool: &PgPool, n: NewNotification) -> Result<Notification, AppError> {
    let id = Uuid::new_v4();
    sqlx::query_as!(
        Notification,
        r#"INSERT INTO notifications (id, user_id, kind, title, body, interrupt, payload)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           RETURNING id, user_id, kind, title, body, interrupt,
                     payload as "payload: serde_json::Value",
                     read_at as "read_at: DateTime<Utc>",
                     created_at as "created_at: DateTime<Utc>""#,
        id,
        n.user_id,
        n.kind,
        n.title,
        n.body,
        n.interrupt,
        n.payload,
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::database_error)
}

/// A single notification, owner-scoped (None if not found / not owned).
pub async fn get_for_user(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
) -> Result<Option<Notification>, AppError> {
    sqlx::query_as!(
        Notification,
        r#"SELECT id, user_id, kind, title, body, interrupt,
                  payload as "payload: serde_json::Value",
                  read_at as "read_at: DateTime<Utc>",
                  created_at as "created_at: DateTime<Utc>"
           FROM notifications WHERE id = $1 AND user_id = $2"#,
        id,
        user_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(AppError::database_error)
}

/// List a user's inbox, newest-first. Returns `(rows, total, unread)`.
pub async fn list_for_user(
    pool: &PgPool,
    user_id: Uuid,
    unread_only: bool,
    page: i64,
    per_page: i64,
) -> Result<(Vec<Notification>, i64, i64), AppError> {
    let per_page = per_page.clamp(1, 200);
    let offset = (page - 1).max(0) * per_page;

    let rows = sqlx::query_as!(
        Notification,
        r#"SELECT id, user_id, kind, title, body, interrupt,
                  payload as "payload: serde_json::Value",
                  read_at as "read_at: DateTime<Utc>",
                  created_at as "created_at: DateTime<Utc>"
           FROM notifications
           WHERE user_id = $1 AND ($2::bool = FALSE OR read_at IS NULL)
           ORDER BY created_at DESC LIMIT $3 OFFSET $4"#,
        user_id,
        unread_only,
        per_page,
        offset,
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::database_error)?;

    let counts = sqlx::query!(
        r#"SELECT count(*) as "total!",
                  count(*) FILTER (WHERE read_at IS NULL) as "unread!"
           FROM notifications WHERE user_id = $1"#,
        user_id,
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::database_error)?;

    Ok((rows, counts.total, counts.unread))
}

/// The user's unread count.
pub async fn unread_count(pool: &PgPool, user_id: Uuid) -> Result<i64, AppError> {
    sqlx::query_scalar!(
        r#"SELECT count(*) as "count!" FROM notifications
           WHERE user_id = $1 AND read_at IS NULL"#,
        user_id,
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::database_error)
}

/// Mark one notification read (idempotent, owner-scoped). Returns rows-affected>0.
pub async fn mark_read(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<bool, AppError> {
    let res = sqlx::query!(
        "UPDATE notifications SET read_at = now() \
         WHERE id = $1 AND user_id = $2 AND read_at IS NULL",
        id,
        user_id,
    )
    .execute(pool)
    .await
    .map_err(AppError::database_error)?;
    Ok(res.rows_affected() > 0)
}

/// Mark all of a user's notifications read. Returns the number affected.
pub async fn mark_all_read(pool: &PgPool, user_id: Uuid) -> Result<u64, AppError> {
    let res = sqlx::query!(
        "UPDATE notifications SET read_at = now() WHERE user_id = $1 AND read_at IS NULL",
        user_id,
    )
    .execute(pool)
    .await
    .map_err(AppError::database_error)?;
    Ok(res.rows_affected())
}

/// Delete one notification (owner-scoped). Returns rows-affected>0.
pub async fn delete(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<bool, AppError> {
    let res = sqlx::query!(
        "DELETE FROM notifications WHERE id = $1 AND user_id = $2",
        id,
        user_id,
    )
    .execute(pool)
    .await
    .map_err(AppError::database_error)?;
    Ok(res.rows_affected() > 0)
}

/// Retention prune: delete rows older than `days` (0 = keep forever, no-op).
pub async fn prune_older_than(pool: &PgPool, days: i64) -> Result<u64, AppError> {
    if days <= 0 {
        return Ok(0);
    }
    let res = sqlx::query!(
        "DELETE FROM notifications WHERE created_at < now() - make_interval(days => $1::int)",
        days as i32,
    )
    .execute(pool)
    .await
    .map_err(AppError::database_error)?;
    Ok(res.rows_affected())
}
