//! Self-service profile permissions (a user's OWN profile).
//!
//! Moved from ziee's `modules/user/permissions.rs` (decision N10) because the
//! auth HTTP surface's `/auth/profile` + `/auth/password` handlers gate on
//! `ProfileEdit`, and that surface now lives in this crate. The user-ADMIN
//! permissions (`UsersRead`/`GroupsRead`/…) stay app-side with the user-admin
//! handlers. ziee's `modules/user/permissions.rs` re-exports these two as an
//! equivalence-preserving shim, so every `crate::modules::user::permissions::
//! Profile*` call site (onboarding, llm_provider, the permission catalog) is
//! unchanged. The `NAME`/`PERMISSION`/`DESCRIPTION`/`MODULE` const strings are
//! byte-identical to the originals (the OpenAPI `with_permission` contract keys
//! off those strings, not the type's location).

use ziee_identity::PermissionCheck;

/// Permission for users to view their own profile
pub struct ProfileRead;
impl PermissionCheck for ProfileRead {
    const NAME: &'static str = "ProfileRead";
    const PERMISSION: &'static str = "profile::read";
    const DESCRIPTION: &'static str = "View own profile information";
    const MODULE: &'static str = "user";
}

/// Permission for users to edit their own profile
pub struct ProfileEdit;
impl PermissionCheck for ProfileEdit {
    const NAME: &'static str = "ProfileEdit";
    const PERMISSION: &'static str = "profile::edit";
    const DESCRIPTION: &'static str = "Edit own profile information";
    const MODULE: &'static str = "user";
}
