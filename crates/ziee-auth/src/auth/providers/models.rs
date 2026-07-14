use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Auth provider configuration - matches new schema
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AuthProvider {
    pub id: Uuid,
    pub name: String,
    pub provider_type: String,
    pub enabled: bool,
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// When the admin last clicked Test on this row.
    pub last_test_at: Option<DateTime<Utc>>,
    /// Result of that test — null if never tested.
    pub last_test_ok: Option<bool>,
    /// Human-readable detail from test_connection.
    pub last_test_message: Option<String>,
}

/// OAuth session for OAuth/OIDC flows - matches new schema
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct OAuthSession {
    pub id: Uuid,
    pub state: String,
    pub provider_id: Uuid,
    pub pkce_verifier: Option<String>,
    pub nonce: Option<String>,
    pub redirect_uri: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Same-origin path the SPA wants to land on after login.
    /// Captured by `oauth_authorize` from the `return_to` query
    /// parameter and stored here so it survives the provider
    /// round-trip without being exposed to the provider URL.
    pub return_to: Option<String>,
}

/// Pending account link for the First-Broker-Login flow.
/// When a social-login email collides with an existing local
/// account, the user is bounced to /auth/link-account where they
/// confirm with their local password before we bind the social
/// identity. This row holds the unconfirmed binding for the 10
/// minutes between the OAuth callback and the password confirmation.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PendingAccountLink {
    pub link_token: String,
    pub provider_id: Uuid,
    pub target_user_id: Uuid,
    pub external_id: String,
    pub external_email: Option<String>,
    pub external_data: Option<serde_json::Value>,
    pub attempts: i32,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

