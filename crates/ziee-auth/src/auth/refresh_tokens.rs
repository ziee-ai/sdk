//! Whitelist + revocation tracking for refresh tokens.
//!
//! Backs the closure of 01-auth F-02 (logout was a no-op) and F-03
//! (refresh didn't rotate the presented token). See migration 44 for
//! the schema rationale.

use ziee_core::AppError;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::jwt::{JwtService, TokenPairWithJti};
use super::session_settings::SessionSettingsRepository;

/// The admin-configured token lifetimes `(access_hours, refresh_days)`
/// from the `session_settings` singleton, falling back to the YAML
/// `jwt.*` values when the DB read fails (so login never breaks
/// mid-migration or on a transient DB error).
pub async fn session_expiries(pool: &PgPool, jwt_service: &JwtService) -> (i64, i64) {
    match SessionSettingsRepository::new(pool.clone()).get().await {
        Ok(s) => (
            s.access_token_expiry_hours as i64,
            s.refresh_token_expiry_days as i64,
        ),
        Err(e) => {
            tracing::warn!(
                error = ?e,
                "session_settings read failed at token mint; using config fallback"
            );
            jwt_service.config_expiries()
        }
    }
}

/// Mint a full session token pair for a user and whitelist the refresh
/// token — the ONE mint path every login-shaped flow goes through
/// (register / password / LDAP / OAuth / link-account / first-run setup /
/// desktop auto-login / desktop magic-link; the refresh handler uses the
/// same pieces with a different revoke/register ordering).
///
/// Lifetimes come from `session_expiries` (admin-configurable, YAML
/// fallback). The refresh token is registered in the `refresh_tokens`
/// whitelist before the pair is returned (fail-closed: a DB write
/// failure means no usable refresh token was handed out).
pub async fn mint_session_tokens(
    pool: &PgPool,
    jwt_service: &JwtService,
    user_id: Uuid,
    username: &str,
    email: &str,
    is_admin: bool,
) -> Result<TokenPairWithJti, AppError> {
    let (access_hours, refresh_days) = session_expiries(pool, jwt_service).await;

    let minted = jwt_service.generate_tokens_with_jti_expiry(
        user_id,
        username,
        email,
        is_admin,
        access_hours,
        refresh_days,
    )?;
    register(pool, minted.refresh_jti, user_id, minted.refresh_expires_at).await?;
    Ok(minted)
}

/// Insert an active row for a freshly-issued refresh token. Call this
/// immediately AFTER JwtService::generate_tokens_with_jti so the token
/// is whitelisted before being returned to the user.
pub async fn register(
    pool: &PgPool,
    jti: Uuid,
    user_id: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<(), AppError> {
    // sqlx uses time::OffsetDateTime for TIMESTAMPTZ; convert via Unix
    // seconds (chrono and time share the same instant model).
    let expires_at_ts = time::OffsetDateTime::from_unix_timestamp(expires_at.timestamp())
        .map_err(|e| AppError::internal_error(format!("invalid expires_at: {}", e)))?;
    sqlx::query!(
        r#"
        INSERT INTO refresh_tokens (jti, user_id, expires_at)
        VALUES ($1, $2, $3)
        "#,
        jti,
        user_id,
        expires_at_ts,
    )
    .execute(pool)
    .await
    .map_err(AppError::database_error)?;
    Ok(())
}

/// Return true iff the row for this jti exists with revoked_at IS NULL
/// and expires_at > NOW(). The refresh handler now claims rotation
/// atomically (`claim_rotation`) rather than checking this first, so it
/// has no in-crate caller — but it's the whitelist-gate primitive the
/// integration + desktop tests assert against, so keep it (the allow
/// silences the BIN target's dead-code pass, same as `revoke`).
#[allow(dead_code)]
pub async fn is_active(pool: &PgPool, jti: Uuid) -> Result<bool, AppError> {
    let row = sqlx::query!(
        r#"
        SELECT 1 as "exists!"
        FROM refresh_tokens
        WHERE jti = $1 AND revoked_at IS NULL AND expires_at > NOW()
        "#,
        jti,
    )
    .fetch_optional(pool)
    .await
    .map_err(AppError::database_error)?;
    Ok(row.is_some())
}

/// Mark a single jti as revoked. Used for revoked-for-real cases (NOT
/// rotation — that's `revoke_rotated`, which records the successor and
/// thereby opts the row into the 30s rotation grace).
///
/// No in-crate caller today (logout revokes per-user, rotation records
/// the successor), but it's the single-jti revocation primitive used
/// cross-crate by the integration tests (`tests/auth/mod.rs` jti
/// lifecycle) — keep it. The allow silences the BIN target's dead-code
/// pass (same pattern as `JwtService::new`).
#[allow(dead_code)]
pub async fn revoke(pool: &PgPool, jti: Uuid) -> Result<(), AppError> {
    sqlx::query!(
        r#"UPDATE refresh_tokens SET revoked_at = NOW() WHERE jti = $1"#,
        jti,
    )
    .execute(pool)
    .await
    .map_err(AppError::database_error)?;
    Ok(())
}

/// ATOMICALLY claim the presented refresh token for rotation AND register
/// its successor, in ONE transaction. Returns `true` iff THIS call flipped
/// the presented row from active → revoked (i.e. it observed
/// `revoked_at IS NULL` and won).
///
/// This is the single-use guarantee AND the race-free convergence point:
///   * Two concurrent refreshes of the same token both run the UPDATE, but
///     Postgres row-locks the presented row, so only ONE observes
///     `revoked_at IS NULL` and wins (the loser gets 0 rows → `false`).
///   * The successor is inserted BEFORE the transaction commits, while the
///     presented row's lock is still held — so a losing concurrent request
///     (whose UPDATE blocks on that lock) cannot proceed to
///     `rotation_grace_successor` until AFTER the successor row is
///     committed and therefore visible. Splitting claim + register into
///     two autocommit statements left a ~ms window where the loser saw the
///     revoked presented token but not yet its successor, and 401'd a
///     legitimate racing client.
///
/// A token already revoked (rotation OR logout) matches 0 rows → `false`
/// and no successor is written.
pub async fn claim_rotation_and_register(
    pool: &PgPool,
    presented_jti: Uuid,
    successor_jti: Uuid,
    user_id: Uuid,
    successor_expires_at: DateTime<Utc>,
) -> Result<bool, AppError> {
    let mut tx = pool.begin().await.map_err(AppError::database_error)?;

    let r = sqlx::query!(
        r#"
        UPDATE refresh_tokens
        SET revoked_at = NOW(), rotated_to = $2
        WHERE jti = $1 AND revoked_at IS NULL
        "#,
        presented_jti,
        successor_jti,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::database_error)?;

    if r.rows_affected() != 1 {
        // Lost the race / already revoked — release the (unmodified) lock.
        tx.rollback().await.map_err(AppError::database_error)?;
        return Ok(false);
    }

    let expires_at_ts =
        time::OffsetDateTime::from_unix_timestamp(successor_expires_at.timestamp())
            .map_err(|e| AppError::internal_error(format!("invalid expires_at: {}", e)))?;
    sqlx::query!(
        r#"INSERT INTO refresh_tokens (jti, user_id, expires_at) VALUES ($1, $2, $3)"#,
        successor_jti,
        user_id,
        expires_at_ts,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::database_error)?;

    tx.commit().await.map_err(AppError::database_error)?;
    Ok(true)
}

/// How long a rotation-revoked refresh token remains acceptable to a
/// racing legitimate client. Long enough for a slow concurrent request
/// to land, short enough that a stolen-then-rotated token is useless
/// almost immediately.
pub const ROTATION_GRACE_SECONDS: i64 = 30;

/// The rotation-grace lookup. Returns `Some((successor_jti,
/// successor_expires_at))` iff `jti` was rotated within the grace window
/// AND the successor family is STILL ACTIVE — so the refresh handler can
/// re-issue tokens bound to THAT existing successor (never an independent
/// new chain; see `handlers::refresh` + `JwtService::reissue_tokens_for_jti`).
///
/// Returns `None` — i.e. hard-fail — when the presented jti was:
///   * never rotated (logout / password-change set `revoked_at` but leave
///     `rotated_to` NULL),
///   * rotated more than `ROTATION_GRACE_SECONDS` ago,
///   * itself expired, OR
///   * rotated, but its successor was SUBSEQUENTLY revoked (e.g. an
///     explicit logout that followed the rotation) — the `s.revoked_at IS
///     NULL` clause is what makes sign-out hard-fail even a just-rotated
///     token, and what prevents a replayed-within-grace token from
///     outliving the family it belongs to.
pub async fn rotation_grace_successor(
    pool: &PgPool,
    jti: Uuid,
) -> Result<Option<(Uuid, DateTime<Utc>)>, AppError> {
    let row = sqlx::query!(
        r#"
        SELECT s.jti AS "succ_jti!",
               EXTRACT(EPOCH FROM s.expires_at)::bigint AS "succ_exp_unix!"
        FROM refresh_tokens t
        JOIN refresh_tokens s ON s.jti = t.rotated_to
        WHERE t.jti = $1
          AND t.rotated_to IS NOT NULL
          AND t.revoked_at > NOW() - make_interval(secs => $2)
          AND t.expires_at > NOW()
          AND s.revoked_at IS NULL
          AND s.expires_at > NOW()
        "#,
        jti,
        ROTATION_GRACE_SECONDS as f64,
    )
    .fetch_optional(pool)
    .await
    .map_err(AppError::database_error)?;
    Ok(row.map(|r| {
        (
            r.succ_jti,
            DateTime::from_timestamp(r.succ_exp_unix, 0).unwrap_or_else(Utc::now),
        )
    }))
}

/// Revoke every active refresh token belonging to `user_id`. Used by
/// logout (the audit's F-02: logout was a no-op; now it actually signs
/// the user out by killing every refresh token they hold).
pub async fn revoke_all_for_user(pool: &PgPool, user_id: Uuid) -> Result<(), AppError> {
    sqlx::query!(
        r#"
        UPDATE refresh_tokens
        SET revoked_at = NOW()
        WHERE user_id = $1 AND revoked_at IS NULL
        "#,
        user_id,
    )
    .execute(pool)
    .await
    .map_err(AppError::database_error)?;
    Ok(())
}
