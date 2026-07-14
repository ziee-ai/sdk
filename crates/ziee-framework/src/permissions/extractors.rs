//! Permission extractors — moved from ziee's
//! `modules/permissions/extractors.rs` in chunk B3, generic over an injected
//! [`IdentityResolver`] instead of ziee's global `Repos` + `JwtService`.
//!
//! ziee re-exports these behind equivalence-preserving type aliases
//! (`RequirePermissions<Perms> = RequirePermissions<ZieeIdentityResolver, Perms>`),
//! so every `RequirePermissions<(UsersRead,)>` call site is unchanged. The
//! enforcement algorithm (JWT → user → admin short-circuit → group union →
//! ALL-of AND logic → 403 formatting) is byte-identical to the former extractor.

use std::{marker::PhantomData, sync::Arc};

use aide::OperationIo;
use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};

use ziee_core::AppError;
use ziee_identity::{PermissionList, Principal, check_permissions_array};

use super::resolver::IdentityResolver;

/// Pull the app-installed resolver out of the request extensions. A missing
/// resolver is a server misconfiguration → 500 (parallels the former
/// "JWT service not configured" branch).
fn get_resolver<R: IdentityResolver>(parts: &Parts) -> Result<Arc<R>, (StatusCode, AppError)> {
    parts.extensions.get::<Arc<R>>().cloned().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            AppError::internal_error("Identity resolver not configured"),
        )
    })
}

/// Whether `user` (with its `groups`) holds `perm` via the UNION of direct
/// permissions and ACTIVE groups' permissions — byte-identical to ziee's
/// `check_permission_union` (direct first, then each active group), just
/// generic over the resolver's `User`/`Group`.
fn user_holds<R: IdentityResolver>(user: &R::User, groups: &[R::Group], perm: &str) -> bool {
    if check_permissions_array(user.direct_permissions(), perm) {
        return true;
    }
    for group in groups {
        if let Some(perms) = R::active_group_permissions(group) {
            if check_permissions_array(perms, perm) {
                return true;
            }
        }
    }
    false
}

// =====================================================
// RequirePermissions - Generic Permission Extractor
// =====================================================

/// Generic permission extractor for checking user permissions
///
/// Supports single or multiple permissions using tuple syntax:
/// - Single: `RequirePermissions<(UsersRead,)>`
/// - Multiple: `RequirePermissions<(UsersRead, UsersEdit)>`
///
/// When multiple permissions are specified, the user must have ALL of them (AND logic).
#[derive(Clone, OperationIo)]
#[aide(input)]
pub struct RequirePermissions<R: IdentityResolver, Perms: PermissionList> {
    pub user: R::User,
    pub groups: Vec<R::Group>,
    _marker: PhantomData<(R, Perms)>,
}

impl<R: IdentityResolver, Perms: PermissionList> FromRequestParts<()>
    for RequirePermissions<R, Perms>
{
    type Rejection = (StatusCode, AppError);

    fn from_request_parts(
        parts: &mut Parts,
        _state: &(),
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let resolver = get_resolver::<R>(parts)?;
            let user = resolver.authenticate(parts).await?;

            // Root admin bypass - is_admin always has full access
            if user.is_admin() {
                return Ok(Self {
                    user,
                    groups: vec![],
                    _marker: PhantomData,
                });
            }

            // Load user's groups with permissions using the injected resolver
            let groups = resolver.load_groups(&user).await?;

            // Check if user has ALL required permissions via union (AND logic)
            let required_permissions = Perms::permissions();
            let missing_permissions: Vec<&str> = required_permissions
                .iter()
                .filter(|&&perm| !user_holds::<R>(&user, &groups, perm))
                .copied()
                .collect();

            if !missing_permissions.is_empty() {
                let error_message = if missing_permissions.len() == 1 {
                    format!("Missing required permission: {}", missing_permissions[0])
                } else {
                    format!(
                        "Missing required permissions: {}",
                        missing_permissions.join(", ")
                    )
                };

                return Err((
                    StatusCode::FORBIDDEN,
                    AppError::forbidden("INSUFFICIENT_PERMISSIONS", error_message),
                ));
            }

            Ok(Self {
                user,
                groups,
                _marker: PhantomData,
            })
        }
    }
}

// =====================================================
// RequireAdmin - Root Admin Only Extractor
// =====================================================

/// Extractor that requires root admin (is_admin = true)
/// Use this for operations that should ONLY be available to the root admin
// Real axum extractor API (impl below) paralleling RequirePermissions; no route
// uses root-admin-only gating yet. Public framework surface (used via the app's
// `RequireAdmin` type alias) rather than deleting an extractor primitive.
#[derive(Clone, OperationIo)]
#[aide(input)]
pub struct RequireAdmin<R: IdentityResolver> {
    pub user: R::User,
}

impl<R: IdentityResolver> FromRequestParts<()> for RequireAdmin<R> {
    type Rejection = (StatusCode, AppError);

    fn from_request_parts(
        parts: &mut Parts,
        _state: &(),
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let resolver = get_resolver::<R>(parts)?;
            let user = resolver.authenticate(parts).await?;

            // Check if user is root admin
            if !user.is_admin() {
                return Err((
                    StatusCode::FORBIDDEN,
                    AppError::forbidden("ADMIN_REQUIRED", "Root administrator access required"),
                ));
            }

            Ok(Self { user })
        }
    }
}
