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

    async fn get_user(&self, username: &str) -> Result<Option<User>, AuthError> {
        let users = UserRepository::new(self.pool.clone());
        // Try username first
        if let Some(user) = users
            .get_by_username(username)
            .await
            .map_err(|e| AuthError::InternalError(format!("Database error: {}", e)))?
        {
            return Ok(Some(user));
        }

        // Try email
        users
            .get_by_email(username)
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
