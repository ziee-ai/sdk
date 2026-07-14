// User module API request/response types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Group, User};

// =====================================================
// API Request/Response Models
// =====================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
    pub permissions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateUserRequest {
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub is_active: Option<bool>,
    // NOTE: `permissions` is intentionally NOT on this DTO. The previous
    // version exposed it, which let any users::edit holder rewrite the
    // permissions array of any user (including themselves) and escalate
    // to wildcard '*' from a single sub-admin grant — see 03-user F-01
    // (Critical). Permission management for users is handled through
    // group assignment (POST /api/groups/{id}/users) and a dedicated
    // set_permissions endpoint planned in A4. Serde drops unknown fields
    // silently, so old callers sending {"permissions":[...]} now get a
    // no-op for the permissions field.

    // NOTE: `email` is also intentionally NOT on this DTO. The previous
    // version let any users::edit holder silently change a user's email
    // without confirmation token / re-verification / session invalidation
    // — the next OAuth callback for the new (attacker-controlled) email
    // would log the attacker into the victim's account. See 03-user F-03
    // (High). A future re-verification-based email-change flow is the
    // proper path for both admin-driven and self-service email changes.
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateGroupRequest {
    pub name: String,
    pub description: Option<String>,
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateGroupRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub permissions: Option<Vec<String>>,
    pub is_active: Option<bool>,
}

// =====================================================
// Admin API Models
// =====================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResetPasswordRequest {
    pub user_id: Uuid,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UserActiveStatusResponse {
    pub user_id: Uuid,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UserListResponse {
    pub users: Vec<User>,
    pub total: i64,
    pub page: i32,
    pub per_page: i32,
    pub total_pages: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GroupListResponse {
    pub groups: Vec<Group>,
    pub total: i64,
    pub page: i32,
    pub per_page: i32,
    pub total_pages: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssignUserToGroupRequest {
    pub user_id: Uuid,
    pub group_id: Uuid,
}
