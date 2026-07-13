// Permission system infrastructure

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =====================================================
// Permission Trait
// =====================================================

/// Trait for compile-time permission definitions
/// Each module implements this trait for their permissions
pub trait PermissionCheck: Send + Sync + 'static {
    /// Short name for the permission (e.g., "UsersRead")
    const NAME: &'static str;

    /// The permission string (e.g., "users::read")
    const PERMISSION: &'static str;

    /// Human-readable description for documentation
    const DESCRIPTION: &'static str;

    /// The module this permission belongs to
    const MODULE: &'static str;

    /// Extract resource name from permission (e.g., "users" from "users::read")
    fn resource() -> &'static str {
        Self::PERMISSION.split("::").next().unwrap_or("")
    }

    /// Extract action name from permission (e.g., "read" from "users::read")
    fn action() -> &'static str {
        Self::PERMISSION.split("::").last().unwrap_or("")
    }

    /// Convert to PermissionInfo for API response
    fn to_info() -> PermissionInfo {
        PermissionInfo {
            permission: Self::PERMISSION.to_string(),
            description: Self::DESCRIPTION.to_string(),
            module: Self::MODULE.to_string(),
            resource: Self::resource().to_string(),
            action: Self::action().to_string(),
        }
    }
}

// =====================================================
// Permission List Trait (for multiple permissions)
// =====================================================

/// Trait for permission lists (single or multiple permissions)
/// This allows RequirePermissions to accept both single permissions
/// and tuples of multiple permissions.
pub trait PermissionList: Send + Sync + 'static {
    /// Get all permission names
    fn names() -> Vec<&'static str>;

    /// Get all permission strings
    fn permissions() -> Vec<&'static str>;

    /// Get all permission descriptions
    fn descriptions() -> Vec<&'static str>;

    /// Get formatted description for OpenAPI docs
    fn format_description() -> String {
        let perms = Self::permissions();
        let descs = Self::descriptions();

        if perms.len() == 1 {
            format!(
                "\n\n**Required Permission:** `{}`\n\n{}",
                perms[0], descs[0]
            )
        } else {
            let mut result = String::from("\n\n**Required Permissions (ALL):**\n");
            for (perm, desc) in perms.iter().zip(descs.iter()) {
                result.push_str(&format!("- `{}` - {}\n", perm, desc));
            }
            result
        }
    }
}

// Implement PermissionList for single permission
impl<P: PermissionCheck> PermissionList for (P,) {
    fn names() -> Vec<&'static str> {
        vec![P::NAME]
    }

    fn permissions() -> Vec<&'static str> {
        vec![P::PERMISSION]
    }

    fn descriptions() -> Vec<&'static str> {
        vec![P::DESCRIPTION]
    }
}

// Implement PermissionList for 2 permissions
impl<P1, P2> PermissionList for (P1, P2)
where
    P1: PermissionCheck,
    P2: PermissionCheck,
{
    fn names() -> Vec<&'static str> {
        vec![P1::NAME, P2::NAME]
    }

    fn permissions() -> Vec<&'static str> {
        vec![P1::PERMISSION, P2::PERMISSION]
    }

    fn descriptions() -> Vec<&'static str> {
        vec![P1::DESCRIPTION, P2::DESCRIPTION]
    }
}

// Implement PermissionList for 3 permissions
impl<P1, P2, P3> PermissionList for (P1, P2, P3)
where
    P1: PermissionCheck,
    P2: PermissionCheck,
    P3: PermissionCheck,
{
    fn names() -> Vec<&'static str> {
        vec![P1::NAME, P2::NAME, P3::NAME]
    }

    fn permissions() -> Vec<&'static str> {
        vec![P1::PERMISSION, P2::PERMISSION, P3::PERMISSION]
    }

    fn descriptions() -> Vec<&'static str> {
        vec![P1::DESCRIPTION, P2::DESCRIPTION, P3::DESCRIPTION]
    }
}

// Implement PermissionList for 4 permissions
impl<P1, P2, P3, P4> PermissionList for (P1, P2, P3, P4)
where
    P1: PermissionCheck,
    P2: PermissionCheck,
    P3: PermissionCheck,
    P4: PermissionCheck,
{
    fn names() -> Vec<&'static str> {
        vec![P1::NAME, P2::NAME, P3::NAME, P4::NAME]
    }

    fn permissions() -> Vec<&'static str> {
        vec![
            P1::PERMISSION,
            P2::PERMISSION,
            P3::PERMISSION,
            P4::PERMISSION,
        ]
    }

    fn descriptions() -> Vec<&'static str> {
        vec![
            P1::DESCRIPTION,
            P2::DESCRIPTION,
            P3::DESCRIPTION,
            P4::DESCRIPTION,
        ]
    }
}

// =====================================================
// Permission Info (for API responses)
// =====================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PermissionInfo {
    /// The permission string (e.g., "users::read")
    pub permission: String,
    /// Human-readable description
    pub description: String,
    /// The module this permission belongs to
    pub module: String,
    /// The resource being accessed
    pub resource: String,
    /// The action being performed
    pub action: String,
}
#[cfg(test)]
mod format_description_tests {
    use super::{PermissionCheck, PermissionList};


    struct A;
    impl PermissionCheck for A {
        const NAME: &'static str = "A";
        const MODULE: &'static str = "alpha";
        const PERMISSION: &'static str = "alpha::read";
        const DESCRIPTION: &'static str = "Read alpha";
    }

    struct B;
    impl PermissionCheck for B {
        const NAME: &'static str = "B";
        const MODULE: &'static str = "beta";
        const PERMISSION: &'static str = "beta::write";
        const DESCRIPTION: &'static str = "Write beta";
    }


    // audit id all-f0874266c44a — format_description's MULTI-permission branch
    // (types.rs:66-83) was untested; only the single-permission shape is hit by
    // production single-tuple gates. A 2-permission list must render the
    // "Required Permissions (ALL)" bullet list with each perm + its description.
    #[test]
    fn multi_permission_format_lists_all_with_descriptions() {
        let out = <(A, B)>::format_description();
        assert!(out.contains("**Required Permissions (ALL):**"), "multi header: {out}");
        assert!(out.contains("`alpha::read` - Read alpha"), "first perm bullet: {out}");
        assert!(out.contains("`beta::write` - Write beta"), "second perm bullet: {out}");
        // It must NOT use the single-permission phrasing.
        assert!(!out.contains("**Required Permission:**"), "must not use single header: {out}");
    }


    #[test]
    fn single_permission_format_uses_singular_header() {
        let out = <(A,)>::format_description();
        assert!(out.contains("**Required Permission:**"), "single header: {out}");
        assert!(out.contains("alpha::read"), "names the perm: {out}");
        assert!(!out.contains("Required Permissions (ALL)"), "not the multi header: {out}");
    }
}
#[cfg(test)]
mod permission_check_tests {
    use super::{PermissionCheck, PermissionInfo};


    // --- RequirePermissions tuple (PermissionList) AND-combination ---
    //
    // RequirePermissions<L> requires ALL of `L::permissions()`; the extractor
    // grants access only when the caller holds every entry. The tuple impls for
    // 3 and 4 permissions (types.rs:119-168) were previously untested — these
    // assert that a 3- and 4-tuple surface the COMPLETE, ordered permission set
    // (so the extractor AND-checks all of them) and that the OpenAPI
    // `format_description` advertises every one under the "ALL" header.
    use super::PermissionList;


    struct TwoSeg;
    impl PermissionCheck for TwoSeg {
        const NAME: &'static str = "UsersRead";
        const PERMISSION: &'static str = "users::read";
        const DESCRIPTION: &'static str = "Read users";
        const MODULE: &'static str = "users";
    }


    struct ThreeSeg;
    impl PermissionCheck for ThreeSeg {
        const NAME: &'static str = "CodeSandboxResourceLimitsManage";
        const PERMISSION: &'static str = "code_sandbox::resource_limits::manage";
        const DESCRIPTION: &'static str = "Manage limits";
        const MODULE: &'static str = "code_sandbox";
    }


    #[test]
    fn resource_is_first_segment_action_is_last() {
        assert_eq!(TwoSeg::resource(), "users");
        assert_eq!(TwoSeg::action(), "read");
        // For a 3-segment permission, resource = first, action = last.
        assert_eq!(ThreeSeg::resource(), "code_sandbox");
        assert_eq!(ThreeSeg::action(), "manage");
    }


    #[test]
    fn to_info_projects_all_fields() {
        let info: PermissionInfo = TwoSeg::to_info();
        assert_eq!(info.permission, "users::read");
        assert_eq!(info.description, "Read users");
        assert_eq!(info.module, "users");
        assert_eq!(info.resource, "users");
        assert_eq!(info.action, "read");
    }


    struct PRead;
    impl PermissionCheck for PRead {
        const NAME: &'static str = "ProjectsRead";
        const PERMISSION: &'static str = "projects::read";
        const DESCRIPTION: &'static str = "Read projects";
        const MODULE: &'static str = "projects";
    }

    struct PWrite;
    impl PermissionCheck for PWrite {
        const NAME: &'static str = "ProjectsEdit";
        const PERMISSION: &'static str = "projects::edit";
        const DESCRIPTION: &'static str = "Edit projects";
        const MODULE: &'static str = "projects";
    }

    struct PDelete;
    impl PermissionCheck for PDelete {
        const NAME: &'static str = "ProjectsDelete";
        const PERMISSION: &'static str = "projects::delete";
        const DESCRIPTION: &'static str = "Delete projects";
        const MODULE: &'static str = "projects";
    }

    struct PShare;
    impl PermissionCheck for PShare {
        const NAME: &'static str = "ProjectsShare";
        const PERMISSION: &'static str = "projects::share";
        const DESCRIPTION: &'static str = "Share projects";
        const MODULE: &'static str = "projects";
    }


    #[test]
    fn three_tuple_yields_all_three_permissions_in_order() {
        type Required = (PRead, PWrite, PDelete);
        assert_eq!(
            Required::permissions(),
            vec!["projects::read", "projects::edit", "projects::delete"],
        );
        assert_eq!(
            Required::names(),
            vec!["ProjectsRead", "ProjectsEdit", "ProjectsDelete"],
        );
        assert_eq!(
            Required::descriptions(),
            vec!["Read projects", "Edit projects", "Delete projects"],
        );

        // The required set is the full AND-set: holding any 2 of the 3 is not
        // a superset, so the extractor would reject — assert all 3 are present
        // and none is dropped.
        let required = Required::permissions();
        assert_eq!(required.len(), 3, "a 3-tuple must require exactly 3 perms");
        for p in ["projects::read", "projects::edit", "projects::delete"] {
            assert!(required.contains(&p), "missing required perm {p}");
        }
    }


    #[test]
    fn four_tuple_yields_all_four_permissions_in_order() {
        type Required = (PRead, PWrite, PDelete, PShare);
        assert_eq!(
            Required::permissions(),
            vec![
                "projects::read",
                "projects::edit",
                "projects::delete",
                "projects::share",
            ],
        );
        assert_eq!(Required::permissions().len(), 4);
        assert_eq!(Required::descriptions().len(), 4);
    }


    #[test]
    fn multi_permission_format_description_lists_all_under_all_header() {
        // 3+ tuples render the multi-permission "ALL" form (not the single
        // "Required Permission" form), advertising every required permission.
        let doc = <(PRead, PWrite, PDelete) as PermissionList>::format_description();
        assert!(
            doc.contains("**Required Permissions (ALL):**"),
            "3-tuple must use the ALL header, got: {doc}",
        );
        for line in [
            "- `projects::read` - Read projects",
            "- `projects::edit` - Edit projects",
            "- `projects::delete` - Delete projects",
        ] {
            assert!(doc.contains(line), "format_description missing line: {line}");
        }
        // The single-permission phrasing must NOT appear for a multi-tuple.
        assert!(!doc.contains("**Required Permission:**"));
    }
}
#[cfg(test)]
mod tests {
    use super::*;


    struct PermA;
    impl PermissionCheck for PermA {
        const NAME: &'static str = "PermA";
        const MODULE: &'static str = "users";
        const PERMISSION: &'static str = "users::read";
        const DESCRIPTION: &'static str = "Read users";
    }

    struct PermB;
    impl PermissionCheck for PermB {
        const NAME: &'static str = "PermB";
        const MODULE: &'static str = "users";
        const PERMISSION: &'static str = "users::edit";
        const DESCRIPTION: &'static str = "Edit users";
    }


    /// Single-permission format uses the "**Required Permission:**" heading.
    #[test]
    fn format_description_single_permission() {
        let s = <(PermA,)>::format_description();
        assert!(s.contains("**Required Permission:**"), "got: {s}");
        assert!(s.contains("`users::read`"), "got: {s}");
        assert!(s.contains("Read users"), "got: {s}");
        assert!(!s.contains("ALL"), "single-perm must not use the ALL heading: {s}");
    }


    /// Multi-permission format uses the "(ALL)" heading + a bullet per permission,
    /// each pairing its name with its description. Untested before.
    #[test]
    fn format_description_multiple_permissions() {
        let s = <(PermA, PermB)>::format_description();
        assert!(s.contains("**Required Permissions (ALL):**"), "got: {s}");
        assert!(s.contains("- `users::read` - Read users"), "got: {s}");
        assert!(s.contains("- `users::edit` - Edit users"), "got: {s}");
    }


    struct UsersRead;
    impl PermissionCheck for UsersRead {
        const NAME: &'static str = "UsersRead";
        const PERMISSION: &'static str = "users::read";
        const DESCRIPTION: &'static str = "Read users";
        const MODULE: &'static str = "users";
    }


    // A permission with a namespaced action to exercise the split logic.
    struct CoreMemoryWrite;
    impl PermissionCheck for CoreMemoryWrite {
        const NAME: &'static str = "CoreMemoryWrite";
        const PERMISSION: &'static str = "memory::core::write";
        const DESCRIPTION: &'static str = "Write core memory";
        const MODULE: &'static str = "memory";
    }


    struct UsersDelete;
    impl PermissionCheck for UsersDelete {
        const NAME: &'static str = "UsersDelete";
        const PERMISSION: &'static str = "users::delete";
        const DESCRIPTION: &'static str = "Delete users";
        const MODULE: &'static str = "users";
    }


    struct GroupsRead;
    impl PermissionCheck for GroupsRead {
        const NAME: &'static str = "GroupsRead";
        const PERMISSION: &'static str = "groups::read";
        const DESCRIPTION: &'static str = "Read groups";
        const MODULE: &'static str = "groups";
    }


    /// `PermissionList` for a 3-tuple collects all three permissions' name /
    /// permission / description in order — the RequirePermissions<(A,B,C)> path.
    #[test]
    fn permission_list_three_tuple_collects_all() {
        type Three = (UsersRead, CoreMemoryWrite, UsersDelete);
        assert_eq!(
            <Three as PermissionList>::permissions(),
            vec!["users::read", "memory::core::write", "users::delete"]
        );
        assert_eq!(
            <Three as PermissionList>::names(),
            vec!["UsersRead", "CoreMemoryWrite", "UsersDelete"]
        );
        assert_eq!(
            <Three as PermissionList>::descriptions(),
            vec!["Read users", "Write core memory", "Delete users"]
        );
    }


    /// `PermissionList` for a 4-tuple — RequirePermissions<(A,B,C,D)>.
    #[test]
    fn permission_list_four_tuple_collects_all() {
        type Four = (UsersRead, CoreMemoryWrite, UsersDelete, GroupsRead);
        assert_eq!(
            <Four as PermissionList>::permissions(),
            vec!["users::read", "memory::core::write", "users::delete", "groups::read"]
        );
        assert_eq!(<Four as PermissionList>::names().len(), 4);
        assert_eq!(<Four as PermissionList>::descriptions().len(), 4);
    }


    #[test]
    fn resource_is_first_segment_action_is_last() {
        assert_eq!(UsersRead::resource(), "users");
        assert_eq!(UsersRead::action(), "read");
        // For a 3-segment permission, resource() takes the FIRST segment and
        // action() the LAST.
        assert_eq!(CoreMemoryWrite::resource(), "memory");
        assert_eq!(CoreMemoryWrite::action(), "write");
    }


    #[test]
    fn to_info_projects_all_fields() {
        let info = UsersRead::to_info();
        assert_eq!(info.permission, "users::read");
        assert_eq!(info.description, "Read users");
        assert_eq!(info.module, "users");
        assert_eq!(info.resource, "users");
        assert_eq!(info.action, "read");
    }


    #[test]
    fn to_info_serializes_to_expected_json_shape() {
        let info = CoreMemoryWrite::to_info();
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["permission"], "memory::core::write");
        assert_eq!(v["module"], "memory");
        assert_eq!(v["resource"], "memory");
        assert_eq!(v["action"], "write");
    }
}
