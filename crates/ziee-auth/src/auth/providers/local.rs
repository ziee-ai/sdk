// Auth provider infrastructure - part of future auth system

use async_trait::async_trait;
use sqlx::PgPool;

use super::{AuthError, AuthProvider, AuthProviderTrait, AuthResult, UserAttributes};
use crate::auth::password;
use crate::user::{User, UserRepository};

/// Local authentication provider using database-stored passwords
pub struct LocalAuthProvider {
    name: String,
    config: serde_json::Value,
    // Chunk BG: queries now build a `UserRepository` from this pool instead of
    // reaching the global `Repos` aggregator.
    pool: PgPool,
}

impl LocalAuthProvider {
    pub fn new(provider: &AuthProvider, pool: PgPool) -> Result<Self, AuthError> {
        Ok(Self {
            name: provider.name.clone(),
            config: provider.config.clone(),
            pool,
        })
    }

    /// Resolve a login identifier to a user — BYTE-EXACT on both halves.
    ///
    /// # This is the SECOND local-password resolver, and #251 nearly changed it by accident
    ///
    /// `UserRepository::get_by_username_or_email` is the one most people mean; this is the
    /// other, reached through `login_with_provider` when an operator has added an
    /// `auth_providers` row of type `local` under some name other than `"local"`. DEC-15
    /// reverted that one to byte-exact after two attempts to make its email half
    /// case-insensitive were each reproduced as a worse attack — and a blind audit then found
    /// that THIS resolver had silently inherited the case-insensitivity anyway, via
    /// `get_by_email`. The two local login resolvers disagreed about who an identifier names:
    /// with a user stored as `Bob@Corp.com`, `authenticate("bob@corp.com", pw)` succeeded
    /// here while `get_by_username_or_email("bob@corp.com")` returned `None`.
    ///
    /// It also has exactly the shape DEC-15 identifies as the attack: username tried FIRST
    /// and byte-exact, email second — so an attacker registering `username` = a victim's
    /// EMAIL wins deterministically, and the victim's correct password is bcrypt-verified
    /// against the attacker's hash.
    ///
    /// So the email lookup here is byte-exact too. Reachability is admin-gated (no `local`
    /// provider row is seeded and `create_provider` refuses the name `"local"`), which makes
    /// this latent rather than live — but a latent authentication inconsistency in a resolver
    /// nobody remembered is exactly what an audit is for, and consistency between the two
    /// resolvers costs nothing.
    ///
    /// #251's fix is unaffected: the invitation binding and registration's collision
    /// pre-check both go through `get_by_email` directly, not through any login resolver.
    ///
    /// One property this delegation gives up, stated because it is a real trade and not an
    /// oversight: the old two-step deterministically preferred the USERNAME row when one row
    /// matched by username and a different row by email. The shared resolver is a single
    /// `OR` with `fetch_optional` and no `ORDER BY`, so that choice is planner-dependent —
    /// which is precisely the pre-#251 behaviour DEC-15 restored, and precisely the
    /// ambiguity tracked as its own issue. Deterministically preferring the username row is
    /// NOT the safe direction (it is the direction that lets an attacker registering
    /// `username` = a victim's email win every time), so restoring it here would reintroduce
    /// the attack DEC-15 removed. Having ONE resolver, with one known ambiguity, beats two
    /// resolvers that disagree.
    async fn get_user(&self, username: &str) -> Result<Option<User>, AuthError> {
        let users = UserRepository::new(self.pool.clone());
        users
            .get_by_username_or_email(username)
            .await
            .map_err(|e| AuthError::InternalError(format!("Database error: {}", e)))
    }
}

#[async_trait]
impl AuthProviderTrait for LocalAuthProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn provider_type(&self) -> &str {
        "local"
    }

    async fn authenticate(&self, username: &str, password: &str) -> Result<AuthResult, AuthError> {
        // Get user by username or email
        let user = self
            .get_user(username)
            .await?
            .ok_or_else(|| AuthError::InvalidCredentials("User not found".to_string()))?;

        // Check if user has password hash
        let password_hash = user.password_hash.as_ref().ok_or_else(|| {
            AuthError::InvalidCredentials("No password configured for this user".to_string())
        })?;

        // Verify password (password::verify_password uses bcrypt internally)
        let valid = password::verify_password(password, password_hash)
            .map_err(|e| AuthError::InternalError(format!("Password verification error: {}", e)))?;

        if !valid {
            return Err(AuthError::InvalidCredentials(
                "Invalid password".to_string(),
            ));
        }

        // Return auth result
        Ok(AuthResult {
            external_id: user.id.to_string(),
            external_username: Some(user.username.clone()),
            external_email: Some(user.email.clone()),
            metadata: serde_json::json!({
                "provider": "local",
                "auth_method": "password"
            }),
            attributes: UserAttributes {
                username: user.username.clone(),
                email: user.email.clone(),
                display_name: user.display_name.clone(),
                first_name: None,   // Not tracked separately in new schema
                last_name: None,    // Not tracked separately in new schema
                // Empty by design: `UserAttributes.groups` carries EXTERNAL
                // (LDAP/OAuth) group names for the provider layer. A local user's
                // group membership is DB-sourced (`user_groups`, resolved via
                // `get_user_groups` at request time), never from this field — the
                // login handler does not consume `attributes.groups` — so leaving
                // it empty does NOT degrade group-based authz for local users.
                groups: Vec::new(),
            },
        })
    }

    async fn test_connection(&self) -> Result<String, AuthError> {
        // For local provider, just verify database connectivity
        UserRepository::new(self.pool.clone())
            .get_by_username("__test_connection__")
            .await
            .map_err(|e| {
                AuthError::ConnectionFailed(format!("Database connection failed: {}", e))
            })?;

        Ok("Database reachable".to_string())
    }

    fn get_config(&self) -> &serde_json::Value {
        &self.config
    }
}
