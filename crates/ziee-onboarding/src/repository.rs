// Onboarding repository — per-user guide/step completion on the
// dedicated `user_onboarding` table (moved off the `users` table so the
// auth/user module carries no onboarding concerns).

use sqlx::PgPool;
use uuid::Uuid;

use super::models::OnboardingProgress;
use ziee_core::AppError;

#[derive(Clone, Debug)]
pub struct OnboardingRepository {
    pool: PgPool,
}

impl OnboardingRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Current onboarding progress for a user. Users who have never
    /// completed anything have no row yet → return empty arrays, not an
    /// error.
    pub async fn get_progress(&self, user_id: Uuid) -> Result<OnboardingProgress, AppError> {
        let row = sqlx::query_as!(
            OnboardingProgress,
            r#"
            SELECT completed_guide_ids, completed_step_ids
            FROM user_onboarding
            WHERE user_id = $1
            "#,
            user_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(row.unwrap_or_else(|| OnboardingProgress {
            completed_guide_ids: vec![],
            completed_step_ids: vec![],
        }))
    }

    /// Mark a guide completed (idempotent). Lazily creates the row on
    /// first completion; the `WHERE` on the upsert makes a repeat append
    /// a no-op.
    pub async fn complete_guide(
        &self,
        user_id: Uuid,
        guide_id: &str,
        max_completions: i32,
    ) -> Result<OnboardingProgress, AppError> {
        // The `cardinality(...) < $3` guard enforces the completion cap
        // ATOMICALLY inside the upsert, closing the check-then-insert TOCTOU in
        // the handler: two concurrent requests can both pass the handler's
        // pre-check, but only appends that keep the array under the cap commit.
        sqlx::query!(
            r#"
            INSERT INTO user_onboarding (user_id, completed_guide_ids)
            VALUES ($1, ARRAY[$2::TEXT])
            ON CONFLICT (user_id) DO UPDATE
            SET completed_guide_ids = array_append(user_onboarding.completed_guide_ids, $2::TEXT),
                updated_at = NOW()
            WHERE NOT ($2 = ANY(user_onboarding.completed_guide_ids))
              AND cardinality(user_onboarding.completed_guide_ids) < $3
            "#,
            user_id,
            guide_id,
            max_completions
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        self.get_progress(user_id).await
    }

    /// Mark a guide step completed (idempotent). `step_key` is the
    /// "{guide_id}/{step_id}" composite key.
    pub async fn complete_guide_step(
        &self,
        user_id: Uuid,
        step_key: &str,
        max_completions: i32,
    ) -> Result<OnboardingProgress, AppError> {
        // Atomic cap guard (see complete_guide) — closes the handler's
        // check-then-insert TOCTOU on the step-completion cardinality cap.
        sqlx::query!(
            r#"
            INSERT INTO user_onboarding (user_id, completed_step_ids)
            VALUES ($1, ARRAY[$2::TEXT])
            ON CONFLICT (user_id) DO UPDATE
            SET completed_step_ids = array_append(user_onboarding.completed_step_ids, $2::TEXT),
                updated_at = NOW()
            WHERE NOT ($2 = ANY(user_onboarding.completed_step_ids))
              AND cardinality(user_onboarding.completed_step_ids) < $3
            "#,
            user_id,
            step_key,
            max_completions
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        self.get_progress(user_id).await
    }
}
