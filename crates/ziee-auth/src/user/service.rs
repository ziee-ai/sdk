// User service infrastructure

use std::collections::HashSet;
use uuid::Uuid;

use super::repository::UserRepository;
use ziee_core::AppError;

// =====================================================
// User Service
// =====================================================

#[derive(Clone)]
pub struct UserService {
    user_repo: UserRepository,
}

impl UserService {
    pub fn new(user_repo: UserRepository) -> Self {
        Self { user_repo }
    }

    /// Get effective permissions (union of user's direct permissions + all group permissions)
    pub async fn get_effective_permissions(&self, user_id: Uuid) -> Result<Vec<String>, AppError> {
        let user = self
            .user_repo
            .get_by_id(user_id)
            .await?
            .ok_or_else(|| AppError::not_found("User"))?;

        let groups = self.user_repo.get_user_groups(user_id).await?;

        let mut permissions = HashSet::new();

        // Add user's direct permissions
        permissions.extend(user.permissions.into_iter());

        // Add permissions from all active groups
        for group in groups {
            if group.is_active {
                permissions.extend(group.permissions.into_iter());
            }
        }

        // Convert to sorted vector for consistent output
        let mut permissions_vec: Vec<String> = permissions.into_iter().collect();
        permissions_vec.sort();
        Ok(permissions_vec)
    }
}
