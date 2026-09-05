// User repository infrastructure

use sqlx::PgPool;
use uuid::Uuid;

use crate::{Group, User};
use ziee_core::AppError;

// =====================================================
// User Repository
// =====================================================

/// `users` row + its access-token revocation epoch, for the one query that
/// needs both (`get_by_id_with_token_version`).
///
/// PRIVATE and NOT serializable, deliberately: `token_version` must never
/// reach a response body. `User` itself is `Serialize + JsonSchema` and is
/// embedded in `MeResponse` and the Tauri `AutoLoginResponse`, so the epoch
/// lives here — split back out into `(User, i32)` at the call boundary —
/// rather than as a field on `User` guarded only by a `#[serde(skip)]`.
struct UserWithTokenVersion {
    id: Uuid,
    username: String,
    email: String,
    email_verified: bool,
    password_hash: Option<String>,
    display_name: Option<String>,
    avatar_url: Option<String>,
    is_active: bool,
    is_admin: bool,
    permissions: Vec<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    last_login_at: Option<chrono::DateTime<chrono::Utc>>,
    password_changed_at: Option<chrono::DateTime<chrono::Utc>>,
    token_version: i32,
}

#[derive(Clone, Debug)]
pub struct UserRepository {
    pool: PgPool,
}

impl UserRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Get a user together with their access-token revocation epoch
    /// (`users.token_version`), in ONE query.
    ///
    /// This exists so the auth hot path (the framework's permission extractor,
    /// which already loads the user on every `RequirePermissions` request) can
    /// check the epoch without paying a second round-trip.
    ///
    /// `token_version` is deliberately NOT a field on `User`: `User` is
    /// `Serialize + JsonSchema` and is embedded in `MeResponse` and the Tauri
    /// `AutoLoginResponse`, so a session-revocation value living there would be
    /// one missing `#[serde(skip_serializing)]` away from a public response.
    /// Returning it alongside keeps it off every serialized surface.
    pub async fn get_by_id_with_token_version(
        &self,
        id: Uuid,
    ) -> Result<Option<(User, i32)>, AppError> {
        let row = sqlx::query_as!(
            UserWithTokenVersion,
            r#"
            SELECT id, username, email, email_verified, password_hash, display_name,
                   avatar_url, is_active, is_admin, permissions,
                   created_at as "created_at: _", updated_at as "updated_at: _", last_login_at as "last_login_at: _", password_changed_at as "password_changed_at: _",
                   token_version
            FROM users
            WHERE id = $1
            "#,
            id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(row.map(|r| {
            (
                User {
                    id: r.id,
                    username: r.username,
                    email: r.email,
                    email_verified: r.email_verified,
                    password_hash: r.password_hash,
                    display_name: r.display_name,
                    avatar_url: r.avatar_url,
                    is_active: r.is_active,
                    is_admin: r.is_admin,
                    permissions: r.permissions,
                    created_at: r.created_at,
                    updated_at: r.updated_at,
                    last_login_at: r.last_login_at,
                    password_changed_at: r.password_changed_at,
                },
                r.token_version,
            )
        }))
    }

    /// Read just the access-token revocation epoch. Used by the bare `JwtAuth`
    /// extractor, which (unlike the permission extractor) never loads the
    /// user, so there is nothing to fold the read into.
    pub async fn get_token_version(&self, id: Uuid) -> Result<Option<i32>, AppError> {
        sqlx::query_scalar!(r#"SELECT token_version FROM users WHERE id = $1"#, id)
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::database_error)
    }

    /// Get user by ID
    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<User>, AppError> {
        sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, email_verified, password_hash, display_name,
                   avatar_url, is_active, is_admin, permissions,
                   created_at as "created_at: _", updated_at as "updated_at: _", last_login_at as "last_login_at: _", password_changed_at as "password_changed_at: _"
            FROM users
            WHERE id = $1
            "#,
            id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Get user by username
    pub async fn get_by_username(&self, username: &str) -> Result<Option<User>, AppError> {
        sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, email_verified, password_hash, display_name,
                   avatar_url, is_active, is_admin, permissions,
                   created_at as "created_at: _", updated_at as "updated_at: _", last_login_at as "last_login_at: _", password_changed_at as "password_changed_at: _"
            FROM users
            WHERE username = $1
            "#,
            username
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Get user by email. CASE-INSENSITIVE (issue #251).
    ///
    /// `lower(email) = lower($1)` matches the `users_email_lower_key` unique index
    /// (`202609050010`) exactly, which is what makes this both correct and index-using: the
    /// functional index is only consulted when the query repeats its expression verbatim.
    ///
    /// Before #251 this was `WHERE email = $1` against a case-SENSITIVE unique constraint, so
    /// `bob@corp.com` and `BOB@corp.com` were two principals and an invitation issued to one
    /// was satisfied by the other. Every caller here — the registration pre-check, login by
    /// identifier, the admin create-user pre-check, invite-by-email member lookup — is asking
    /// "is there a principal at this mailbox?", and the case-insensitive answer is the one
    /// they all want.
    ///
    /// The input is trimmed in RUST and case-folded in POSTGRES, and neither side does the
    /// other's job — see [`crate::auth::email`] for why that split is load-bearing rather
    /// than stylistic.
    pub async fn get_by_email(&self, email: &str) -> Result<Option<User>, AppError> {
        let email = crate::auth::email::trim_email_for_lookup(email);
        sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, email_verified, password_hash, display_name,
                   avatar_url, is_active, is_admin, permissions,
                   created_at as "created_at: _", updated_at as "updated_at: _", last_login_at as "last_login_at: _", password_changed_at as "password_changed_at: _"
            FROM users
            WHERE lower(email) = lower($1)
            "#,
            email
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Get user by username or email.
    ///
    /// The EMAIL half is case-insensitive (issue #251, as [`Self::get_by_email`]); the
    /// USERNAME half stays byte-exact, because `users_username_key` is still a
    /// case-sensitive UNIQUE constraint and matching usernames loosely than the constraint
    /// enforces would let one identifier resolve to a row it does not name.
    pub async fn get_by_username_or_email(
        &self,
        identifier: &str,
    ) -> Result<Option<User>, AppError> {
        let identifier = crate::auth::email::trim_email_for_lookup(identifier);
        sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, email_verified, password_hash, display_name,
                   avatar_url, is_active, is_admin, permissions,
                   created_at as "created_at: _", updated_at as "updated_at: _", last_login_at as "last_login_at: _", password_changed_at as "password_changed_at: _"
            FROM users
            WHERE username = $1 OR lower(email) = lower($1)
            "#,
            identifier
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// List users with pagination
    pub async fn list(&self, page: i32, per_page: i32) -> Result<(Vec<User>, i64), AppError> {
        let offset = ((page - 1) * per_page) as i64;

        // Get total count
        let total: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM users"#)
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::database_error)?;

        // Get paginated users
        let users = sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, email_verified, password_hash, display_name,
                   avatar_url, is_active, is_admin, permissions,
                   created_at as "created_at: _", updated_at as "updated_at: _", last_login_at as "last_login_at: _", password_changed_at as "password_changed_at: _"
            FROM users
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
            "#,
            per_page as i64,
            offset
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok((users, total))
    }

    /// Create a new user
    pub async fn create(
        &self,
        username: &str,
        email: &str,
        password_hash: Option<String>,
        display_name: Option<String>,
        permissions: Option<Vec<String>>,
    ) -> Result<User, AppError> {
        // Trim at the WRITE (issue #251). The `users_email_lower_key` unique index does not
        // trim, so an untrimmed row would sit beside its trimmed twin as a second principal
        // and the bypass would survive via a leading/trailing Unicode-whitespace variant.
        // `str::trim` here and `btrim(<the same 25 code points>)` in the migration's CHECK
        // are asserted equal by test; see `crate::auth::email`.
        let email = &crate::auth::email::normalize_email_for_storage(email)?;

        // A transaction (was a single pool write) so the in-transaction
        // user-created hook (gap G-AUTHEVT) is atomic with the insert. This is
        // the admin-create + first-run/setup path (the app's user handlers call
        // it), so firing here — not only in the auth handlers — is what gives the
        // hook full creation-path coverage. Hook error → rollback → no user.
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
            permissions.as_deref().unwrap_or(&[])
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

        crate::user::hook::fire_user_created(&user, &mut tx).await?;

        tx.commit().await.map_err(AppError::database_error)?;
        Ok(user)
    }

    pub async fn has_admin(&self) -> Result<bool, AppError> {
        let result = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM users WHERE is_admin = true) as "exists!""#
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(result)
    }

    /// Update user
    pub async fn update(
        &self,
        id: Uuid,
        username: Option<String>,
        email: Option<String>,
        display_name: Option<String>,
        permissions: Option<Vec<String>>,
    ) -> Result<User, AppError> {
        sqlx::query_as!(
            User,
            r#"
            UPDATE users
            SET username = COALESCE($2, username),
                email = COALESCE($3, email),
                display_name = COALESCE($4, display_name),
                permissions = COALESCE($5, permissions),
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, username, email, email_verified, password_hash, display_name,
                      avatar_url, is_active, is_admin, permissions,
                      created_at as "created_at: _", updated_at as "updated_at: _", last_login_at as "last_login_at: _", password_changed_at as "password_changed_at: _"
            "#,
            id,
            username,
            email,
            display_name,
            permissions.as_deref()
        )
        .fetch_one(&self.pool)
        .await
        // A username/email collision that slips past the caller's pre-check
        // (the pre-check is a TOCTOU window) is caught by the DB UNIQUE
        // constraint and surfaced as a 409 — not a raw 500. Mirrors the
        // mapping `update_profile` below has always had.
        .map_err(|e| match e {
            sqlx::Error::Database(db) if db.is_unique_violation() => AppError::conflict("Username"),
            other => AppError::database_error(other),
        })
    }

    /// Self-service profile update (username + display_name only).
    ///
    /// Unlike [`update`], `display_name` is a true tri-state so a user can
    /// CLEAR it back to NULL: `set_display_name = false` leaves it
    /// untouched; `set_display_name = true` writes `display_name` verbatim
    /// (including `None`, which clears it). A concurrent username collision
    /// that slips past the caller's pre-check is caught by the DB UNIQUE
    /// constraint here and surfaced as a 409 conflict (not a raw 500).
    pub async fn update_profile(
        &self,
        id: Uuid,
        username: Option<String>,
        set_display_name: bool,
        display_name: Option<String>,
    ) -> Result<User, AppError> {
        sqlx::query_as!(
            User,
            r#"
            UPDATE users
            SET username = COALESCE($2, username),
                display_name = CASE WHEN $3 THEN $4 ELSE display_name END,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, username, email, email_verified, password_hash, display_name,
                      avatar_url, is_active, is_admin, permissions,
                      created_at as "created_at: _", updated_at as "updated_at: _", last_login_at as "last_login_at: _", password_changed_at as "password_changed_at: _"
            "#,
            id,
            username,
            set_display_name,
            display_name,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db) if db.is_unique_violation() => AppError::conflict("Username"),
            other => AppError::database_error(other),
        })
    }

    /// Update password hash. Also bumps `password_changed_at` so the
    /// Remote Access module can tell whether the admin has rotated
    /// the bootstrap default password.
    pub async fn update_password(&self, id: Uuid, password_hash: &str) -> Result<(), AppError> {
        sqlx::query!(
            r#"
            UPDATE users
            SET password_hash = $2,
                password_changed_at = NOW(),
                updated_at = NOW()
            WHERE id = $1
            "#,
            id,
            password_hash
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(())
    }

    /// Update last login timestamp
    pub async fn update_last_login(&self, id: Uuid) -> Result<(), AppError> {
        sqlx::query!(
            r#"
            UPDATE users
            SET last_login_at = NOW()
            WHERE id = $1
            "#,
            id
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(())
    }

    /// Set user active status
    pub async fn set_active(&self, id: Uuid, is_active: bool) -> Result<(), AppError> {
        sqlx::query!(
            r#"
            UPDATE users
            SET is_active = $2, updated_at = NOW()
            WHERE id = $1
            "#,
            id,
            is_active
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(())
    }

    /// Atomically delete a user only if they are NOT an admin. Returns `true`
    /// if a row was deleted, `false` if no matching non-admin row existed
    /// (either the user is gone, or it became admin since the caller's check).
    /// Re-checking `is_admin` inside the DELETE closes the read-then-write
    /// TOCTOU where a concurrent promotion could let an admin be deleted.
    pub async fn delete_if_not_admin(&self, id: Uuid) -> Result<bool, AppError> {
        let res = sqlx::query!(
            r#"
            DELETE FROM users WHERE id = $1 AND is_admin = FALSE
            "#,
            id
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(res.rows_affected() > 0)
    }

    /// Get user's groups
    pub async fn get_user_groups(&self, user_id: Uuid) -> Result<Vec<Group>, AppError> {
        sqlx::query_as!(
            Group,
            r#"
            SELECT g.id, g.name, g.description, g.permissions, g.is_system, g.is_active, g.is_default,
                   g.created_at as "created_at: _", g.updated_at as "updated_at: _"
            FROM groups g
            INNER JOIN user_groups ug ON ug.group_id = g.id
            WHERE ug.user_id = $1
            "#,
            user_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Assign user to group
    pub async fn assign_to_group(
        &self,
        user_id: Uuid,
        group_id: Uuid,
        assigned_by: Option<Uuid>,
    ) -> Result<(), AppError> {
        sqlx::query!(
            r#"
            INSERT INTO user_groups (user_id, group_id, assigned_by)
            VALUES ($1, $2, $3)
            ON CONFLICT (user_id, group_id) DO NOTHING
            "#,
            user_id,
            group_id,
            assigned_by
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(())
    }

    /// Remove user from group
    pub async fn remove_from_group(&self, user_id: Uuid, group_id: Uuid) -> Result<(), AppError> {
        sqlx::query!(
            r#"
            DELETE FROM user_groups
            WHERE user_id = $1 AND group_id = $2
            "#,
            user_id,
            group_id
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(())
    }
}

// =====================================================
// Group Repository
// =====================================================

#[derive(Clone, Debug)]
pub struct GroupRepository {
    pool: PgPool,
}

impl GroupRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Get group by ID
    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<Group>, AppError> {
        sqlx::query_as!(
            Group,
            r#"
            SELECT id, name, description, permissions, is_system, is_active, is_default,
                   created_at as "created_at: _", updated_at as "updated_at: _"
            FROM groups
            WHERE id = $1
            "#,
            id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Get group by name
    pub async fn get_by_name(&self, name: &str) -> Result<Option<Group>, AppError> {
        sqlx::query_as!(
            Group,
            r#"
            SELECT id, name, description, permissions, is_system, is_active, is_default,
                   created_at as "created_at: _", updated_at as "updated_at: _"
            FROM groups
            WHERE name = $1
            "#,
            name
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Get default group (where is_default = true)
    pub async fn get_default(&self) -> Result<Option<Group>, AppError> {
        sqlx::query_as!(
            Group,
            r#"
            SELECT id, name, description, permissions, is_system, is_active, is_default,
                   created_at as "created_at: _", updated_at as "updated_at: _"
            FROM groups
            WHERE is_default = true AND is_active = true
            LIMIT 1
            "#
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// All groups, unpaginated. Used cross-crate by the desktop's
    /// single-admin event handlers (assign new providers/MCP servers to
    /// every group), so it reads as dead from the server bin — keep it
    /// (same pattern as `JwtService::new`).
    #[allow(dead_code)]
    pub async fn get_all(&self) -> Result<Vec<Group>, AppError> {
        sqlx::query_as!(
            Group,
            r#"
            SELECT id, name, description, permissions, is_system, is_active, is_default,
                   created_at as "created_at: _", updated_at as "updated_at: _"
            FROM groups
            ORDER BY name
            "#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// List groups with pagination
    pub async fn list(&self, page: i32, per_page: i32) -> Result<(Vec<Group>, i64), AppError> {
        let offset = (page - 1) * per_page;

        // Get total count
        let total = sqlx::query_scalar!("SELECT COUNT(*) FROM groups")
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::database_error)?
            .unwrap_or(0);

        // Get paginated results
        let groups = sqlx::query_as!(
            Group,
            r#"
            SELECT id, name, description, permissions, is_system, is_active, is_default,
                   created_at as "created_at: _", updated_at as "updated_at: _"
            FROM groups
            ORDER BY name
            LIMIT $1 OFFSET $2
            "#,
            per_page as i64,
            offset as i64
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok((groups, total))
    }

    /// Create a new group
    pub async fn create(
        &self,
        name: &str,
        description: Option<String>,
        permissions: Vec<String>,
    ) -> Result<Group, AppError> {
        sqlx::query_as!(
            Group,
            r#"
            INSERT INTO groups (name, description, permissions)
            VALUES ($1, $2, $3)
            RETURNING id, name, description, permissions, is_system, is_active, is_default,
                      created_at as "created_at: _", updated_at as "updated_at: _"
            "#,
            name,
            description,
            &permissions
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Update group
    pub async fn update(
        &self,
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
        permissions: Option<Vec<String>>,
        is_active: Option<bool>,
    ) -> Result<Group, AppError> {
        sqlx::query_as!(
            Group,
            r#"
            UPDATE groups
            SET name = COALESCE($2, name),
                description = COALESCE($3, description),
                permissions = COALESCE($4, permissions),
                is_active = COALESCE($5, is_active),
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, name, description, permissions, is_system, is_active, is_default,
                      created_at as "created_at: _", updated_at as "updated_at: _"
            "#,
            id,
            name,
            description,
            permissions.as_deref(),
            is_active
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Delete group (only non-system groups)
    pub async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        sqlx::query!(
            r#"
            DELETE FROM groups WHERE id = $1 AND is_system = FALSE
            "#,
            id
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(())
    }

    /// Get just the user ids of a group's members (unpaginated). Used by
    /// realtime sync to fan a `session/permissions_changed` signal out to
    /// every member when the group's permissions are edited.
    pub async fn get_member_ids(&self, group_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        sqlx::query_scalar!(
            "SELECT user_id FROM user_groups WHERE group_id = $1",
            group_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Get members of a group with pagination
    pub async fn get_members(
        &self,
        group_id: Uuid,
        page: i32,
        per_page: i32,
    ) -> Result<(Vec<User>, i64), AppError> {
        let offset = (page - 1) * per_page;

        // Get total count
        let total = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) FROM user_groups WHERE group_id = $1
            "#,
            group_id
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::database_error)?
        .unwrap_or(0);

        // Get paginated members
        let users = sqlx::query_as!(
            User,
            r#"
            SELECT u.id, u.username, u.email, u.email_verified, u.password_hash,
                   u.display_name, u.avatar_url, u.is_active, u.is_admin,
                   ARRAY[]::TEXT[] as "permissions!",
                   u.created_at as "created_at: _", u.updated_at as "updated_at: _",
                   u.last_login_at as "last_login_at: _",
                   u.password_changed_at as "password_changed_at: _"
            FROM users u
            INNER JOIN user_groups ug ON u.id = ug.user_id
            WHERE ug.group_id = $1
            ORDER BY u.username
            LIMIT $2 OFFSET $3
            "#,
            group_id,
            per_page as i64,
            offset as i64
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok((users, total))
    }
}
