//! The session record (`auth_sessions`) — one row per signed-in session, and
//! the home of that session's access-token revocation epoch.
//!
//! An access token names its session in the `sid` claim. Route-gating readers
//! compare the token's `ver` against [`get_session_version`] — the session
//! row — rather than against `users.token_version`, so the per-request check
//! never reads the identity table, and a single session can be killed
//! ([`end_session`]) or have its tokens rotated out while it survives
//! ([`bump_session_version`]) without touching the user's other sessions.
//!
//! `users.token_version` stays the write-side master: a session's `ver` is
//! initialised from it — read under a `users` row share lock — when the
//! session is created (`refresh_tokens::mint_session_tokens_for` and
//! [`create_session_at_current_epoch`], the only places the user scalar is
//! read for a session), and the user-level kill (`refresh_tokens::
//! end_session_atomically`, i.e. logout) bumps it AND ends every live session
//! of the user in one transaction. See migration
//! `202607144650_auth_sessions.sql`.

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;
use ziee_core::AppError;

/// Insert a new live session row for `user_id` with epoch `ver`, on the given
/// connection (so the caller can put it in the same transaction as the
/// session's first refresh-token row). Returns the new session id.
pub async fn create_session(
    conn: &mut PgConnection,
    user_id: Uuid,
    ver: i32,
) -> Result<Uuid, AppError> {
    let id = Uuid::new_v4();
    insert_session(conn, id, user_id, ver).await?;
    Ok(id)
}

/// [`create_session`] with a caller-chosen id (the mint picks the session id
/// and the first refresh jti up front and inserts both in one transaction).
pub async fn insert_session(
    conn: &mut PgConnection,
    id: Uuid,
    user_id: Uuid,
    ver: i32,
) -> Result<(), AppError> {
    sqlx::query!(
        r#"INSERT INTO auth_sessions (id, user_id, ver) VALUES ($1, $2, $3)"#,
        id,
        user_id,
        ver,
    )
    .execute(conn)
    .await
    .map_err(AppError::database_error)?;
    Ok(())
}

/// Create a live session at the user's CURRENT master epoch, reading
/// `users.token_version` under a `users` row share lock in the same
/// transaction as the insert — so a racing logout (which updates that row) is
/// strictly ordered with it and can never leave the session born at the
/// pre-logout epoch. Returns `(session_id, ver, values)` — `values` being the
/// claim values the app's source supplied (and recorded) for the session — or
/// `None` if the user row is absent.
///
/// The app's `TokenClaimsSource` is consulted in the same transaction
/// (`record_session` on this connection, method `Unspecified`), so every
/// session the SDK creates has had the app's claim record written with it.
pub async fn create_session_at_current_epoch(
    pool: &PgPool,
    user_id: Uuid,
    source: &dyn crate::auth::jwt::TokenClaimsSource,
) -> Result<Option<(Uuid, i32, crate::auth::jwt::AccessTokenClaimValues)>, AppError> {
    let mut tx = pool.begin().await.map_err(AppError::database_error)?;
    let Some(ver) = sqlx::query_scalar!(
        r#"SELECT token_version AS "adopted_epoch!" FROM users WHERE id = $1 FOR SHARE"#,
        user_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(AppError::database_error)?
    else {
        tx.rollback().await.map_err(AppError::database_error)?;
        return Ok(None);
    };
    let id = Uuid::new_v4();
    insert_session(&mut tx, id, user_id, ver).await?;
    let ctx = crate::auth::jwt::MintContext {
        user_id,
        session_id: id,
        ver,
        method: crate::auth::jwt::AuthMethod::Unspecified,
    };
    let values = source.claims_for(&ctx).await;
    let values = source.record_session(&ctx, values, &mut tx).await?;
    tx.commit().await.map_err(AppError::database_error)?;
    Ok(Some((id, ver, values)))
}

/// The session's current epoch, or `None` when the session is absent or has
/// ended — both of which mean "this session's tokens are dead".
///
/// `Err` only for a genuine DB failure, which callers must surface as 500,
/// never as 401 (a 401 is terminal to the client and would sign the user out
/// over a pool blip).
pub async fn get_session_version(pool: &PgPool, session_id: Uuid) -> Result<Option<i32>, AppError> {
    sqlx::query_scalar!(
        r#"SELECT ver FROM auth_sessions WHERE id = $1 AND ended_at IS NULL"#,
        session_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(AppError::database_error)
}

/// End ONE session: mark it ended AND revoke every active refresh token of its
/// family, in one transaction (so neither its access tokens nor its refresh
/// tokens outlive it). The user's other sessions are untouched. Returns `true`
/// iff a live session was ended by this call.
///
/// Lock order matches logout (`end_session_atomically`) and rotation
/// (`claim_rotation_and_register`): the owning `users` row FIRST (`FOR NO KEY
/// UPDATE`, which conflicts with rotation's `FOR SHARE` — so a racing rotation
/// either commits its successor before this revoke scans, or finds its
/// presented token already revoked), then `refresh_tokens`, then
/// `auth_sessions`. Taking them in another order could deadlock with a logout.
pub async fn end_session(pool: &PgPool, session_id: Uuid) -> Result<bool, AppError> {
    let mut tx = pool.begin().await.map_err(AppError::database_error)?;
    let owner = sqlx::query_scalar!(
        r#"
        SELECT u.id FROM users u
        JOIN auth_sessions s ON s.user_id = u.id
        WHERE s.id = $1
        FOR NO KEY UPDATE OF u
        "#,
        session_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(AppError::database_error)?;
    if owner.is_none() {
        tx.rollback().await.map_err(AppError::database_error)?;
        return Ok(false);
    }
    sqlx::query!(
        r#"
        UPDATE refresh_tokens
        SET revoked_at = NOW()
        WHERE session_id = $1 AND revoked_at IS NULL
        "#,
        session_id,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::database_error)?;
    let ended = sqlx::query!(
        r#"UPDATE auth_sessions SET ended_at = NOW() WHERE id = $1 AND ended_at IS NULL"#,
        session_id,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::database_error)?
    .rows_affected()
        == 1;
    tx.commit().await.map_err(AppError::database_error)?;
    Ok(ended)
}

/// Bump ONE live session's epoch: every access token minted for it so far
/// stops validating, while the session (and its refresh family) survives — the
/// next refresh mints tokens at the new epoch. Returns the new `ver`, or `None`
/// if the session is absent or ended.
pub async fn bump_session_version(
    pool: &PgPool,
    session_id: Uuid,
) -> Result<Option<i32>, AppError> {
    sqlx::query_scalar!(
        r#"
        UPDATE auth_sessions
        SET ver = ver + 1
        WHERE id = $1 AND ended_at IS NULL
        RETURNING ver
        "#,
        session_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(AppError::database_error)
}

/// The session a registered refresh token belongs to (`None` for a legacy row
/// with no session, or an unknown jti). Used by the refresh handler's grace
/// path, which re-issues against the SUCCESSOR row's session.
pub async fn session_of_refresh_token(pool: &PgPool, jti: Uuid) -> Result<Option<Uuid>, AppError> {
    let row = sqlx::query_scalar!(
        r#"SELECT session_id FROM refresh_tokens WHERE jti = $1"#,
        jti,
    )
    .fetch_optional(pool)
    .await
    .map_err(AppError::database_error)?;
    Ok(row.flatten())
}
