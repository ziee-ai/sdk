use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use ziee_core::AppError;
use crate::auth::providers::models::OAuthSession;
use crate::user::{Group, User};

/// Auth Repository
pub struct AuthRepository {
    pool: PgPool,
}

impl AuthRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Get the default group
    pub async fn get_default_group(&self) -> Result<Option<Group>, AppError> {
        sqlx::query_as!(
            Group,
            r#"
            SELECT id, name, description, permissions, is_system, is_active, is_default,
                   created_at as "created_at: _", updated_at as "updated_at: _"
            FROM groups
            WHERE is_default = true
            LIMIT 1
            "#
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Create a local (password) user AND assign the default group in ONE
    /// transaction. `register()` previously did these as two independent writes:
    /// a failure of the group assignment after the user INSERT committed left an
    /// orphan user with no group membership (hence no permissions), unrecoverable
    /// without manual cleanup. Mirrors the atomicity of
    /// `create_external_user_with_link`.
    pub async fn create_local_user_with_default_group(
        &self,
        username: &str,
        email: &str,
        password_hash: Option<String>,
        display_name: Option<String>,
    ) -> Result<User, AppError> {
        let mut tx = self.pool.begin().await.map_err(AppError::database_error)?;

        let user = sqlx::query_as!(
            User,
            r#"
            INSERT INTO users (username, email, password_hash, display_name, permissions)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, username, email, email_verified, password_hash, display_name,
                      avatar_url, is_active, is_admin, permissions,
                      created_at as "created_at: _", updated_at as "updated_at: _", last_login_at as "last_login_at: _", password_changed_at as "password_changed_at: _"
            "#,
            username,
            email,
            password_hash,
            display_name,
            &[] as &[String],
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            // A duplicate username/email racing past the handler's pre-check
            // must surface as 409 Conflict, not a generic 500.
            if let sqlx::Error::Database(db_err) = &e
                && db_err.is_unique_violation()
            {
                return AppError::conflict("Username or email");
            }
            AppError::database_error(e)
        })?;

        let default_group = self.get_default_group().await?;
        if let Some(group) = default_group {
            sqlx::query!(
                r#"
                INSERT INTO user_groups (user_id, group_id, assigned_at)
                VALUES ($1, $2, NOW())
                "#,
                user.id,
                group.id
            )
            .execute(&mut *tx)
            .await
            .map_err(AppError::database_error)?;
        }

        tx.commit().await.map_err(AppError::database_error)?;
        Ok(user)
    }

    /// Find user auth link by provider and external ID
    pub async fn find_user_by_auth_link(
        &self,
        provider_id: Uuid,
        external_id: &str,
    ) -> Result<Option<Uuid>, AppError> {
        let result = sqlx::query!(
            r#"
            SELECT user_id
            FROM user_auth_links
            WHERE provider_id = $1 AND external_id = $2
            "#,
            provider_id,
            external_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(result.map(|r| r.user_id))
    }

    /// Create a user auth link
    #[allow(dead_code)]
    pub async fn create_auth_link(
        &self,
        user_id: Uuid,
        provider_id: Uuid,
        external_id: &str,
    ) -> Result<(), AppError> {
        sqlx::query!(
            r#"
            INSERT INTO user_auth_links (user_id, provider_id, external_id, created_at, last_login_at)
            VALUES ($1, $2, $3, NOW(), NOW())
            "#,
            user_id,
            provider_id,
            external_id
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(())
    }

    /// Create a user auth link including the provider's email + raw
    /// claims, WITHOUT touching the target user's `email_verified`.
    ///
    /// Currently has no in-tree caller: the First-Broker-Link flow it
    /// used to serve now uses [`Self::link_verified_external_identity`],
    /// which additionally marks the matching email verified, atomically
    /// with the link. Kept because it is `pub` on an SDK crate other
    /// applications consume. Prefer the verifying variant for any
    /// provider-asserted identity; reach for this one only to bind an
    /// identity whose email the provider did NOT vouch for.
    pub async fn create_auth_link_with_data(
        &self,
        user_id: Uuid,
        provider_id: Uuid,
        external_id: &str,
        external_email: Option<&str>,
        external_data: Option<&serde_json::Value>,
    ) -> Result<(), AppError> {
        sqlx::query!(
            r#"
            INSERT INTO user_auth_links (user_id, provider_id, external_id, external_email, external_data, created_at, last_login_at)
            VALUES ($1, $2, $3, $4, $5, NOW(), NOW())
            "#,
            user_id,
            provider_id,
            external_id,
            external_email,
            external_data,
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(())
    }

    /// Bind a PROVIDER-VERIFIED external identity to an existing user,
    /// and mark that user's email verified — both in ONE transaction.
    ///
    /// This is the First-Broker-Login confirmation write. The provider
    /// asserted `external_email` as verified, and the FBL flow only ever
    /// reaches here because that address matched this user's email, so
    /// the identity proof carries over to the local row: an account the
    /// user has now demonstrably received mail at is verified.
    ///
    /// The `lower(email) = lower($2)` guard re-states that invariant AT
    /// THE WRITE rather than trusting the caller — a mismatched (or
    /// absent) `external_email` links the identity but leaves
    /// `email_verified` alone.
    ///
    /// Returns the DELTA, not the resulting state: `true` only when this
    /// call flipped the column. An already-verified user returns `false`
    /// because `AND email_verified = false` makes the UPDATE a no-op
    /// (which also avoids pointless `updated_at` churn from the
    /// `update_users_updated_at` trigger). Callers wanting the resulting
    /// state must OR it with what they already knew.
    pub async fn link_verified_external_identity(
        &self,
        user_id: Uuid,
        provider_id: Uuid,
        external_id: &str,
        external_email: Option<&str>,
        external_data: Option<&serde_json::Value>,
    ) -> Result<bool, AppError> {
        let mut tx = self.pool.begin().await.map_err(AppError::database_error)?;

        sqlx::query!(
            r#"
            INSERT INTO user_auth_links (user_id, provider_id, external_id, external_email, external_data, created_at, last_login_at)
            VALUES ($1, $2, $3, $4, $5, NOW(), NOW())
            "#,
            user_id,
            provider_id,
            external_id,
            external_email,
            external_data,
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        let verified = sqlx::query!(
            r#"
            UPDATE users
            SET email_verified = true, updated_at = NOW()
            WHERE id = $1
              AND email_verified = false
              -- Spelled out rather than leaning on `= NULL` evaluating to
              -- NULL: "no external email ⇒ never verify" is a rule, not a
              -- side effect of three-valued logic. (Both occurrences are
              -- cast for symmetry; the parameter's type comes from the
              -- `lower()` call below either way.)
              AND $2::text IS NOT NULL
              AND lower(email) = lower($2::text)
            RETURNING id
            "#,
            user_id,
            external_email,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(AppError::database_error)?
        .is_some();

        tx.commit().await.map_err(AppError::database_error)?;
        Ok(verified)
    }

    /// Bump `last_login_at` on an existing user_auth_links row.
    /// Called whenever a returning user re-authenticates via the
    /// social provider — distinct from the `users.last_login_at`
    /// bump because a user may have multiple linked providers.
    #[allow(dead_code)]
    pub async fn update_auth_link_last_login(
        &self,
        provider_id: Uuid,
        external_id: &str,
    ) -> Result<(), AppError> {
        sqlx::query!(
            r#"
            UPDATE user_auth_links
            SET last_login_at = NOW(), updated_at = NOW()
            WHERE provider_id = $1 AND external_id = $2
            "#,
            provider_id,
            external_id
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(())
    }

    /// Find a local user by email — used to detect First-Broker-Login
    /// collisions. Case-insensitive (LOWER(email)) so a user who
    /// registered as `Bob@corp.com` is matched when an OAuth provider
    /// hands back the canonical `bob@corp.com`. Without this, FBL is
    /// bypassed silently and the user gets a duplicate account.
    ///
    /// SECURITY: filters on `is_active = true`. A disabled user's
    /// email would otherwise trigger the FBL flow → /auth/link-account
    /// page renders → attacker learns the email is registered. The
    /// auto-provision branch creates a fresh account instead (which
    /// can later be reconciled if/when the original account is
    /// reactivated).
    pub async fn find_user_by_email_for_linking(
        &self,
        email: &str,
    ) -> Result<Option<Uuid>, AppError> {
        let row = sqlx::query!(
            r#"
            SELECT id FROM users
            WHERE LOWER(email) = LOWER($1) AND is_active = true
            LIMIT 1
            "#,
            email
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(row.map(|r| r.id))
    }

    /// Atomically provision a brand-new external-only user (no
    /// password) + bind the social identity + assign the default
    /// group, in a single transaction. If any step fails, the whole
    /// thing rolls back — without this, a partial failure leaves
    /// orphan rows that lock the user out forever (no password →
    /// can't local-login, no auth_link → can't social-login,
    /// email-collision check on retry refuses to provision).
    /// Returns the new user_id.
    ///
    /// `email_verified` is the caller's assertion about THE `email`
    /// PASSED HERE — not a general trust level for the identity. It is
    /// threaded rather than assumed so the row stays honest if a
    /// caller's upstream guards ever change; a signup with no
    /// provider-verified email must pass `false`.
    ///
    /// CAVEAT the caller owns: the OIDC `email_verified` claim is read
    /// from a fixed key, while which claim becomes `email` is
    /// admin-configurable per provider (`attribute_mapping.email`). An
    /// operator who maps `email` to some OTHER claim (e.g. `upn`) makes
    /// the verdict describe a different address than the one stored.
    /// This predates the flag being persisted, and the same pairing
    /// already drives the callback's drop-unverified-email guard — but
    /// it is a real limit on what `true` here proves, so do not treat
    /// this column as an authorization input without tightening that.
    pub async fn provision_external_user_atomic(
        &self,
        username: &str,
        email: Option<&str>,
        email_verified: bool,
        display_name: &str,
        provider_id: Uuid,
        external_id: &str,
        external_data: Option<&serde_json::Value>,
    ) -> Result<Uuid, AppError> {
        let mut tx = self.pool.begin().await.map_err(AppError::database_error)?;
        let new_user_id = Uuid::new_v4();

        sqlx::query!(
            r#"
            INSERT INTO users (id, username, email, email_verified, display_name, is_active, is_admin, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, true, false, NOW(), NOW())
            "#,
            new_user_id, username, email, email_verified, display_name,
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        sqlx::query!(
            r#"
            INSERT INTO user_auth_links (user_id, provider_id, external_id, external_email, external_data, created_at, last_login_at)
            VALUES ($1, $2, $3, $4, $5, NOW(), NOW())
            "#,
            new_user_id, provider_id, external_id, email, external_data,
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        // Assign default group — fetch within the same tx so we see
        // a consistent snapshot.
        let default_group = sqlx::query!(
            r#"SELECT id FROM groups WHERE is_default = true LIMIT 1"#
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(AppError::database_error)?;
        if let Some(group) = default_group {
            sqlx::query!(
                r#"INSERT INTO user_groups (user_id, group_id, assigned_at) VALUES ($1, $2, NOW())"#,
                new_user_id, group.id,
            )
            .execute(&mut *tx)
            .await
            .map_err(AppError::database_error)?;
        }

        tx.commit().await.map_err(AppError::database_error)?;
        Ok(new_user_id)
    }

    /// Atomic SELECT + UPDATE on user_auth_links: bump last_login_at
    /// and return the user_id in a single round-trip. Replaces the
    /// prior SELECT-then-UPDATE pattern in oauth_callback.
    pub async fn touch_auth_link_and_get_user_id(
        &self,
        provider_id: Uuid,
        external_id: &str,
    ) -> Result<Option<Uuid>, AppError> {
        let row = sqlx::query!(
            r#"
            UPDATE user_auth_links
            SET last_login_at = NOW(), updated_at = NOW()
            WHERE provider_id = $1 AND external_id = $2
            RETURNING user_id
            "#,
            provider_id, external_id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(row.map(|r| r.user_id))
    }

    /// Peek a pending link by token WITHOUT consuming it. Used in
    /// `link_account` so a wrong-password attempt doesn't burn the
    /// single-use token — the user gets to retry without re-running
    /// the whole OAuth flow.
    pub async fn peek_pending_link(
        &self,
        link_token: &str,
    ) -> Result<Option<crate::auth::providers::models::PendingAccountLink>, AppError> {
        sqlx::query_as!(
            crate::auth::providers::models::PendingAccountLink,
            r#"
            SELECT link_token, provider_id, target_user_id, external_id,
                   external_email, external_data, attempts,
                   created_at as "created_at: _",
                   expires_at as "expires_at: _"
            FROM pending_account_links
            WHERE link_token = $1 AND expires_at > NOW()
            "#,
            link_token
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Atomically increment `attempts` and return the new value. Used
    /// in `link_account` to enforce a per-token attempt cap: at the
    /// global 5 req/s rate limit a single IP could try ~3000 passwords
    /// in the 10-minute TTL; this gate cuts that to single digits and
    /// makes brute-forcing impractical even from a botnet.
    pub async fn bump_pending_link_attempts(
        &self,
        link_token: &str,
    ) -> Result<Option<i32>, AppError> {
        let row = sqlx::query!(
            r#"
            UPDATE pending_account_links
               SET attempts = attempts + 1
             WHERE link_token = $1 AND expires_at > NOW()
            RETURNING attempts
            "#,
            link_token,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(row.map(|r| r.attempts))
    }

    /// Delete a pending link by token (best-effort — no error if
    /// the row's already gone). Paired with `peek_pending_link`
    /// when single-use semantics need to be enforced after the
    /// password verification step succeeds.
    pub async fn delete_pending_link(
        &self,
        link_token: &str,
    ) -> Result<(), AppError> {
        sqlx::query!(
            r#"DELETE FROM pending_account_links WHERE link_token = $1"#,
            link_token,
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(())
    }

    /// Create a new user from external auth (used by LDAP/OAuth)
    /// Returns the created user's ID
    #[allow(dead_code)]
    pub async fn create_external_user(
        &self,
        username: &str,
        email: Option<String>,
        display_name: &str,
    ) -> Result<Uuid, AppError> {
        let new_user_id = Uuid::new_v4();

        sqlx::query!(
            r#"
            INSERT INTO users (id, username, email, display_name, is_active, is_admin, created_at, updated_at)
            VALUES ($1, $2, $3, $4, true, false, NOW(), NOW())
            "#,
            new_user_id,
            username,
            email,
            display_name
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(new_user_id)
    }

    /// Create external user with auth link and assign to default group
    /// This is a convenience method that combines multiple operations
    pub async fn create_external_user_with_link(
        &self,
        username: &str,
        email: Option<String>,
        display_name: &str,
        provider_id: Uuid,
        external_id: &str,
    ) -> Result<Uuid, AppError> {
        // All three writes (user row, auth link, default-group assignment) must
        // be atomic: a failure after the user INSERT would otherwise leave an
        // orphan user with no auth link (unable to log in, blocking the
        // username/email) or no default group.
        let user_id = Uuid::new_v4();
        let mut tx = self.pool.begin().await.map_err(AppError::database_error)?;

        sqlx::query!(
            r#"
            INSERT INTO users (id, username, email, display_name, is_active, is_admin, created_at, updated_at)
            VALUES ($1, $2, $3, $4, true, false, NOW(), NOW())
            "#,
            user_id,
            username,
            email,
            display_name
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        sqlx::query!(
            r#"
            INSERT INTO user_auth_links (user_id, provider_id, external_id, created_at, last_login_at)
            VALUES ($1, $2, $3, NOW(), NOW())
            "#,
            user_id,
            provider_id,
            external_id
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        let default_group = sqlx::query!(
            r#"SELECT id FROM groups WHERE is_default = true LIMIT 1"#
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(AppError::database_error)?;
        if let Some(group) = default_group {
            sqlx::query!(
                r#"
                INSERT INTO user_groups (user_id, group_id, assigned_at)
                VALUES ($1, $2, NOW())
                "#,
                user_id,
                group.id
            )
            .execute(&mut *tx)
            .await
            .map_err(AppError::database_error)?;
        }

        tx.commit().await.map_err(AppError::database_error)?;
        Ok(user_id)
    }

    /// Create OAuth session for OAuth/OIDC flows
    pub async fn create_oauth_session(&self, session: &OAuthSession) -> Result<(), AppError> {
        let expires_at_timestamp = session.expires_at.timestamp() as f64;
        sqlx::query!(
            r#"
            INSERT INTO oauth_sessions (id, state, provider_id, pkce_verifier, nonce, redirect_uri, return_to, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, to_timestamp($8))
            "#,
            session.id,
            session.state,
            session.provider_id,
            session.pkce_verifier,
            session.nonce,
            session.redirect_uri,
            session.return_to,
            expires_at_timestamp
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(())
    }

    /// Get OAuth session by state
    pub async fn get_oauth_session_by_state(
        &self,
        state: &str,
    ) -> Result<Option<OAuthSession>, AppError> {
        sqlx::query_as!(
            OAuthSession,
            r#"
            SELECT id, state, provider_id, pkce_verifier, nonce, redirect_uri, return_to,
                   created_at as "created_at: _",
                   expires_at as "expires_at: _"
            FROM oauth_sessions
            WHERE state = $1 AND expires_at > NOW()
            "#,
            state
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Delete OAuth session by state
    pub async fn delete_oauth_session(&self, state: &str) -> Result<(), AppError> {
        sqlx::query!(
            r#"
            DELETE FROM oauth_sessions
            WHERE state = $1
            "#,
            state
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(())
    }

    /// Create a pending account link with a 10-minute TTL. The
    /// returned token is what we put in the `/auth/link-account?token=...`
    /// redirect URL.
    pub async fn create_pending_link(
        &self,
        provider_id: Uuid,
        target_user_id: Uuid,
        external_id: &str,
        external_email: Option<&str>,
        external_data: Option<&serde_json::Value>,
    ) -> Result<String, AppError> {
        let link_token = Uuid::new_v4().to_string();
        let expires_at: DateTime<Utc> = Utc::now() + Duration::minutes(10);
        let expires_at_ts = expires_at.timestamp() as f64;
        sqlx::query!(
            r#"
            INSERT INTO pending_account_links (link_token, provider_id, target_user_id, external_id, external_email, external_data, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, to_timestamp($7))
            "#,
            link_token,
            provider_id,
            target_user_id,
            external_id,
            external_email,
            external_data,
            expires_at_ts,
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(link_token)
    }

    /// Best-effort cleanup of expired oauth_sessions, pending_account_links,
    /// and stale refresh_tokens rows. Designed to be called from a periodic
    /// background task or at server boot; safe to invoke at any time.
    /// Returns `(sessions_pruned, pending_links_pruned, refresh_tokens_pruned)`
    /// counts.
    ///
    /// Even with the per-row TTL columns, rows we never re-touch (a
    /// user who abandons the OAuth dance mid-flow) would otherwise
    /// accumulate forever — the tables would grow without bound.
    ///
    /// Refresh tokens keep a 7-day post-expiry/post-revocation buffer:
    /// revoked rows must outlive the 30s rotation-grace window (and stay
    /// visible for a while for audit/debugging), and there is no reason
    /// to keep them longer. Desktop deployments mint a session per app
    /// launch, so without this the table grows one row per launch.
    pub async fn cleanup_expired_auth_rows(&self) -> Result<(u64, u64, u64), AppError> {
        let s = sqlx::query!(
            r#"DELETE FROM oauth_sessions WHERE expires_at < NOW()"#
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        let p = sqlx::query!(
            r#"DELETE FROM pending_account_links WHERE expires_at < NOW()"#
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        let r = sqlx::query!(
            r#"
            DELETE FROM refresh_tokens
            WHERE expires_at < NOW() - INTERVAL '7 days'
               OR (revoked_at IS NOT NULL AND revoked_at < NOW() - INTERVAL '7 days')
            "#
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok((s.rows_affected(), p.rows_affected(), r.rows_affected()))
    }
}
