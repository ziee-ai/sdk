//! Generic RBAC evaluation — string-set permission matching.
//!
//! Moved verbatim from ziee's `modules::permissions::checker`
//! (`check_permissions_array`). This is the framework-generic core: it operates
//! purely on permission STRINGS and a granted-permission set, with no knowledge
//! of any concrete identity type (`User`/`Group` stay in ziee and drive this via
//! `check_permission_union`, which is unchanged). Wildcard/hierarchical semantics
//! are byte-preserved — the equivalence gate depends on it.

/// Check if a permission array contains the required permission
/// Supports exact match, wildcards, and hierarchical wildcards
pub fn check_permissions_array(permissions: &[String], required_permission: &str) -> bool {
    // Check exact match
    if permissions.contains(&required_permission.to_string()) {
        return true;
    }

    // Check full wildcard
    if permissions.contains(&"*".to_string()) {
        return true;
    }

    // Check hierarchical wildcards
    // "users::*" matches "users::read", "users::edit"
    // "config::auth::*" matches "config::auth::read", "config::auth::edit"
    let parts: Vec<&str> = required_permission.split("::").collect();
    for i in 1..parts.len() {
        let prefix = parts[0..i].join("::");
        let wildcard = format!("{}::*", prefix);
        if permissions.contains(&wildcard) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::check_permissions_array;

    // These directly exercise the moved generic evaluator against permission
    // string-sets. ziee's own suite continues to exercise the same logic through
    // the concrete `check_permission_union(user, groups, ..)` wrapper (which now
    // calls this function), so wildcard/hierarchical behavior is covered on both
    // sides of the boundary.

    fn perms(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn exact_match() {
        assert!(check_permissions_array(&perms(&["users::read"]), "users::read"));
        assert!(!check_permissions_array(&perms(&["users::read"]), "users::edit"));
    }

    #[test]
    fn full_wildcard_matches_anything() {
        assert!(check_permissions_array(&perms(&["*"]), "users::read"));
        assert!(check_permissions_array(&perms(&["*"]), "anything::at::all"));
    }

    #[test]
    fn resource_wildcard() {
        let p = perms(&["users::*"]);
        assert!(check_permissions_array(&p, "users::read"));
        assert!(check_permissions_array(&p, "users::edit"));
        assert!(!check_permissions_array(&p, "groups::read"));
    }

    #[test]
    fn hierarchical_wildcard_arbitrary_depth() {
        let p = perms(&["config::auth::*"]);
        assert!(check_permissions_array(&p, "config::auth::read"));
        assert!(check_permissions_array(&p, "config::auth::edit"));
        // Sibling namespace is not covered.
        assert!(!check_permissions_array(&p, "config::proxy::read"));

        // A 4-level wildcard covers deeper leaves but not shallower ones.
        let deep = perms(&["a::b::c::*"]);
        assert!(check_permissions_array(&deep, "a::b::c::read"));
        assert!(check_permissions_array(&deep, "a::b::c::d::execute"));
        assert!(!check_permissions_array(&deep, "a::b::x::read"));
        // The bare prefix is not a granted leaf.
        assert!(!check_permissions_array(&deep, "a::b::c"));
    }

    #[test]
    fn empty_set_denies() {
        assert!(!check_permissions_array(&perms(&[]), "users::read"));
    }
}
