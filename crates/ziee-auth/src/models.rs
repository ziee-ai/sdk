// User + Group models (moved verbatim from ziee `modules/user/models.rs`).
// Concrete, schema-bound wire types owned by the default auth module.

use axum_login::AuthUser as AuthUserTrait;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

// =====================================================
// User Model
// =====================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, JsonSchema)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub email_verified: bool,
    #[serde(skip_serializing)]
    #[schemars(skip)]
    pub password_hash: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_active: bool,
    pub is_admin: bool,
    pub permissions: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    /// When the user last rotated their password. NULL means the
    /// account is still using the bootstrap-issued password (only
    /// meaningful for the desktop `admin` user). The Remote Access
    /// module refuses to enable password authentication unless this
    /// is non-NULL for the admin.
    pub password_changed_at: Option<DateTime<Utc>>,
}

impl AuthUserTrait for User {
    type Id = Uuid;

    fn id(&self) -> Self::Id {
        self.id
    }

    fn session_auth_hash(&self) -> &[u8] {
        // Use password hash for session validation
        // Session is automatically invalidated when password changes
        self.password_hash
            .as_ref()
            .map(|h| h.as_bytes())
            .unwrap_or_else(|| self.id.as_bytes())
    }
}

// Chunk B1b: ziee's concrete `User` implements the framework's pluggable
// identity interface (decision #1). Framework enforcement (the
// `RequirePermissions` extractor, which moves in B3) depends only on
// `ziee_identity::Principal`, never on this table type. Groups are threaded
// through `check_permission_union(user, groups, ..)` at every call site today,
// so this impl exposes the user's DIRECT permissions + admin flag; the active
// group dimension is wired into `Principal` when the extractor moves in B3.
impl ziee_identity::Principal for User {
    fn is_admin(&self) -> bool {
        self.is_admin
    }

    fn direct_permissions(&self) -> &[String] {
        &self.permissions
    }
}

// =====================================================
// Group Model
// =====================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, JsonSchema)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub permissions: Vec<String>, // PostgreSQL array
    pub is_system: bool,
    pub is_active: bool,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
