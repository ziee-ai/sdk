//! SQL for account recovery (recovery codes, security questions, attempt
//! counters). Stateless wrapper over a `PgPool`, like the other auth
//! repositories; every statement is a `query!` macro verified against the
//! build DB.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use ziee_core::AppError;

/// An unused code's row id and bcrypt hash. (No `Debug`: it carries a hash.)
#[derive(Clone)]
pub struct CodeRow {
    pub id: Uuid,
    pub code_hash: String,
}

/// A configured question as stored. (No `Debug`: it carries a hash.)
#[derive(Clone)]
pub struct QuestionRow {
    pub position: i16,
    pub question_key: String,
    pub answer_hash: String,
}

#[derive(Clone, Debug)]
pub struct RecoveryRepository {
    pool: PgPool,
}

impl RecoveryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ───────────── codes ─────────────

    /// How many unused codes the user holds, and when the live set was made.
    pub async fn codes_status(
        &self,
        user_id: Uuid,
    ) -> Result<(i64, Option<DateTime<Utc>>), AppError> {
        let row = sqlx::query!(
            r#"
            SELECT count(*) FILTER (WHERE used_at IS NULL) AS "remaining!",
                   max(created_at) AS "generated_at: DateTime<Utc>"
            FROM auth_recovery_codes
            WHERE user_id = $1
            "#,
            user_id
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok((row.remaining, row.generated_at))
    }

    /// Replace the user's whole code set: the previous rows (used or not) are
    /// deleted and the new hashes inserted in ONE transaction, so there is
    /// never a moment with two live sets and never one with none after a
    /// failure.
    pub async fn replace_codes(
        &self,
        user_id: Uuid,
        hashes: &[String],
    ) -> Result<DateTime<Utc>, AppError> {
        let mut tx = self.pool.begin().await.map_err(AppError::database_error)?;
        sqlx::query!("DELETE FROM auth_recovery_codes WHERE user_id = $1", user_id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::database_error)?;
        let batch = Uuid::new_v4();
        let created = sqlx::query_scalar!(
            r#"
            INSERT INTO auth_recovery_codes (user_id, batch_id, code_hash)
            SELECT $1, $2, h FROM unnest($3::text[]) AS h
            RETURNING created_at AS "created_at!: DateTime<Utc>"
            "#,
            user_id,
            batch,
            hashes
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(AppError::database_error)?;
        tx.commit().await.map_err(AppError::database_error)?;
        created
            .into_iter()
            .next()
            .ok_or_else(|| AppError::internal_error("no recovery codes were stored".to_string()))
    }

    /// Delete every code (used or not) the user holds.
    pub async fn delete_codes(&self, user_id: Uuid) -> Result<(), AppError> {
        sqlx::query!("DELETE FROM auth_recovery_codes WHERE user_id = $1", user_id)
            .execute(&self.pool)
            .await
            .map_err(AppError::database_error)?;
        Ok(())
    }

    /// The user's unused code hashes (at most one set, so at most ten).
    pub async fn unused_codes(&self, user_id: Uuid) -> Result<Vec<CodeRow>, AppError> {
        let rows = sqlx::query!(
            r#"
            SELECT id, code_hash
            FROM auth_recovery_codes
            WHERE user_id = $1 AND used_at IS NULL
            ORDER BY created_at, id
            LIMIT 50
            "#,
            user_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(rows
            .into_iter()
            .map(|r| CodeRow { id: r.id, code_hash: r.code_hash })
            .collect())
    }

    // ───────────── questions ─────────────

    /// The user's questions in position order (hashes included: callers that
    /// return them to a client must project them away).
    pub async fn questions(&self, user_id: Uuid) -> Result<Vec<QuestionRow>, AppError> {
        let rows = sqlx::query!(
            r#"
            SELECT position, question_key, answer_hash
            FROM auth_security_questions
            WHERE user_id = $1
            ORDER BY position
            LIMIT 10
            "#,
            user_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(rows
            .into_iter()
            .map(|r| QuestionRow {
                position: r.position,
                question_key: r.question_key,
                answer_hash: r.answer_hash,
            })
            .collect())
    }

    /// Replace the user's questions atomically (`(key, answer_hash)` in order).
    pub async fn replace_questions(
        &self,
        user_id: Uuid,
        picks: &[(String, String)],
    ) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await.map_err(AppError::database_error)?;
        sqlx::query!("DELETE FROM auth_security_questions WHERE user_id = $1", user_id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::database_error)?;
        for (i, (key, hash)) in picks.iter().enumerate() {
            sqlx::query!(
                r#"
                INSERT INTO auth_security_questions (user_id, position, question_key, answer_hash)
                VALUES ($1, $2, $3, $4)
                "#,
                user_id,
                (i + 1) as i16,
                key,
                hash
            )
            .execute(&mut *tx)
            .await
            .map_err(AppError::database_error)?;
        }
        tx.commit().await.map_err(AppError::database_error)?;
        Ok(())
    }

    pub async fn delete_questions(&self, user_id: Uuid) -> Result<(), AppError> {
        sqlx::query!("DELETE FROM auth_security_questions WHERE user_id = $1", user_id)
            .execute(&self.pool)
            .await
            .map_err(AppError::database_error)?;
        Ok(())
    }

    // ───────────── the reset itself ─────────────

    /// Set the new password, end every session, and, when a code was the
    /// credential, consume it: ALL in ONE transaction. The consume is a
    /// compare-and-set (`used_at IS NULL`), so two concurrent resets with the
    /// same code cannot both win: the loser gets `false` and nothing is written.
    ///
    /// Ending the sessions here (the access-token epoch bump and the refresh
    /// token revocation, the same two statements as
    /// `refresh_tokens::end_session_atomically`) rather than in a second
    /// transaction afterwards is deliberate: a failure between the two would
    /// leave the new password set and the old sessions alive, which is exactly
    /// the state a reset exists to end.
    pub async fn complete_reset(
        &self,
        user_id: Uuid,
        consume_code: Option<Uuid>,
        new_password_hash: &str,
    ) -> Result<bool, AppError> {
        let mut tx = self.pool.begin().await.map_err(AppError::database_error)?;
        if let Some(code_id) = consume_code {
            let claimed = sqlx::query!(
                r#"
                UPDATE auth_recovery_codes
                SET used_at = NOW()
                WHERE id = $1 AND user_id = $2 AND used_at IS NULL
                "#,
                code_id,
                user_id
            )
            .execute(&mut *tx)
            .await
            .map_err(AppError::database_error)?;
            if claimed.rows_affected() != 1 {
                return Ok(false);
            }
        }
        let updated = sqlx::query!(
            r#"
            UPDATE users
            SET password_hash = $2,
                password_changed_at = NOW(),
                updated_at = NOW(),
                token_version = token_version + 1
            WHERE id = $1 AND is_active
            "#,
            user_id,
            new_password_hash
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;
        if updated.rows_affected() != 1 {
            return Ok(false);
        }
        sqlx::query!(
            r#"
            UPDATE refresh_tokens
            SET revoked_at = NOW()
            WHERE user_id = $1 AND revoked_at IS NULL
            "#,
            user_id
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;
        tx.commit().await.map_err(AppError::database_error)?;
        Ok(true)
    }

    // ───────────── attempt counters ─────────────

    /// Reserve ONE attempt against `(scope, key)` BEFORE the credential is
    /// checked, and say whether it may proceed.
    ///
    /// Counting first and verifying second is the whole point. Counting after
    /// (record a failure once the bcrypt check said no) leaves a window of
    /// about half a second per request in which every request already in
    /// flight has passed the lock check, so a burst of N concurrent requests
    /// gets N guesses instead of `max_attempts`. Here one atomic upsert both
    /// increments and reads the count, so concurrent requests are serialised by
    /// the row lock and at most `max_attempts` of them see `allowed = true`.
    ///
    /// Inside the window the count goes up; outside it (and with no live lock)
    /// it restarts at 1; reaching `max_attempts` stamps `locked_until = now +
    /// window`, which also stops the window from restarting early. A SUCCESS
    /// calls [`Self::clear_attempts`] for the keys that should be forgiven.
    /// (`auth::recovery::next_counter` is the pure mirror of this arithmetic.)
    pub async fn begin_attempt(
        &self,
        scope: &str,
        key: &str,
        window_minutes: i32,
        max_attempts: i32,
    ) -> Result<bool, AppError> {
        let failures = sqlx::query_scalar!(
            r#"
            INSERT INTO auth_recovery_attempts AS a (scope, key, failures, window_started_at, locked_until)
            VALUES ($1, $2, 1, NOW(), CASE WHEN $4 <= 1 THEN NOW() + make_interval(mins => $3) END)
            ON CONFLICT (scope, key) DO UPDATE SET
                failures = CASE
                    WHEN a.window_started_at < NOW() - make_interval(mins => $3)
                         AND (a.locked_until IS NULL OR a.locked_until <= NOW()) THEN 1
                    ELSE a.failures + 1 END,
                window_started_at = CASE
                    WHEN a.window_started_at < NOW() - make_interval(mins => $3)
                         AND (a.locked_until IS NULL OR a.locked_until <= NOW()) THEN NOW()
                    ELSE a.window_started_at END,
                locked_until = CASE
                    WHEN a.window_started_at < NOW() - make_interval(mins => $3)
                         AND (a.locked_until IS NULL OR a.locked_until <= NOW())
                        THEN CASE WHEN $4 <= 1 THEN NOW() + make_interval(mins => $3) END
                    WHEN a.locked_until IS NOT NULL AND a.locked_until > NOW() THEN a.locked_until
                    WHEN a.failures + 1 >= $4 THEN NOW() + make_interval(mins => $3)
                    ELSE NULL END
            RETURNING failures AS "failures!"
            "#,
            scope,
            key,
            window_minutes,
            max_attempts
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        // Keep the table bounded: counters whose window ended a day ago and
        // that are not locked carry no information. Batched so one request
        // never deletes an unbounded set.
        sqlx::query!(
            r#"
            DELETE FROM auth_recovery_attempts
            WHERE ctid IN (
                SELECT ctid FROM auth_recovery_attempts
                WHERE window_started_at < NOW() - INTERVAL '1 day'
                  AND (locked_until IS NULL OR locked_until < NOW())
                LIMIT 100
            )
            "#
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(failures <= max_attempts)
    }

    /// Forget the counter (a successful reset or re-auth).
    pub async fn clear_attempts(&self, scope: &str, key: &str) -> Result<(), AppError> {
        sqlx::query!(
            "DELETE FROM auth_recovery_attempts WHERE scope = $1 AND key = $2",
            scope,
            key
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(())
    }
}
