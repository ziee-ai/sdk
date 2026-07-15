//! `Principal` — the minimal authenticated-identity interface the framework
//! enforces against (pluggable identity, decision #1).
//!
//! ziee's concrete `User` implements this; the framework's authorization layer
//! (the `RequirePermissions` extractor, which moves in B3) depends only on this
//! trait, never on a concrete table type. The default `has_permission` reuses
//! the generic [`crate::rbac::check_permissions_array`] evaluator so wildcard /
//! hierarchical semantics stay identical across every identity implementation.

use crate::rbac::check_permissions_array;

/// An authenticated identity, abstracted to exactly what framework enforcement
/// needs: an admin flag, the identity's own directly-granted permissions, and
/// the permission sets of its ACTIVE groups.
///
/// The default [`Principal::has_permission`] mirrors ziee's
/// `check_permission_union`: a permission is held when it matches the direct
/// permissions OR any active group's permissions (the admin flag is a separate,
/// caller-applied short-circuit — it is intentionally NOT folded in here so this
/// trait stays a pure UNION check, exactly like `check_permission_union`).
pub trait Principal {
    /// Whether this identity is a root admin (bypasses permission checks at the
    /// call site; not folded into `has_permission`).
    fn is_admin(&self) -> bool;

    /// The identity's directly-granted permission strings.
    fn direct_permissions(&self) -> &[String];

    /// The permission strings of each of the identity's ACTIVE groups. The
    /// caller is responsible for filtering out inactive groups (mirrors the
    /// `group.is_active` guard in `check_permission_union`).
    fn active_group_permissions(&self) -> Vec<&[String]> {
        Vec::new()
    }

    /// True iff this identity holds `required` via the UNION of its direct
    /// permissions and its active groups' permissions (wildcard/hierarchical
    /// aware). Does not consider `is_admin` — apply that at the call site.
    fn has_permission(&self, required: &str) -> bool {
        if check_permissions_array(self.direct_permissions(), required) {
            return true;
        }
        for group_perms in self.active_group_permissions() {
            if check_permissions_array(group_perms, required) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::Principal;

    struct TestPrincipal {
        admin: bool,
        direct: Vec<String>,
        groups: Vec<Vec<String>>,
    }

    impl Principal for TestPrincipal {
        fn is_admin(&self) -> bool {
            self.admin
        }
        fn direct_permissions(&self) -> &[String] {
            &self.direct
        }
        fn active_group_permissions(&self) -> Vec<&[String]> {
            self.groups.iter().map(|g| g.as_slice()).collect()
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn union_of_direct_and_group_permissions() {
        let p = TestPrincipal {
            admin: false,
            direct: v(&["users::read"]),
            groups: vec![v(&["groups::edit"])],
        };
        assert!(p.has_permission("users::read"));
        assert!(p.has_permission("groups::edit"));
        assert!(!p.has_permission("users::delete"));
    }

    #[test]
    fn wildcard_via_group() {
        let p = TestPrincipal {
            admin: false,
            direct: v(&[]),
            groups: vec![v(&["config::auth::*"])],
        };
        assert!(p.has_permission("config::auth::read"));
        assert!(!p.has_permission("config::proxy::read"));
    }

    /// A `Principal` that overrides only the two required methods gets the
    /// DEFAULT `active_group_permissions()` (empty), so `has_permission` checks
    /// direct grants only. This pins the default trait-method behavior that the
    /// other tests bypass by always overriding it.
    struct DirectOnly {
        direct: Vec<String>,
    }
    impl Principal for DirectOnly {
        fn is_admin(&self) -> bool {
            false
        }
        fn direct_permissions(&self) -> &[String] {
            &self.direct
        }
        // active_group_permissions() intentionally NOT overridden → default [].
    }

    #[test]
    fn default_active_group_permissions_is_empty() {
        let p = DirectOnly {
            direct: v(&["users::read"]),
        };
        assert!(p.active_group_permissions().is_empty());
        assert!(p.has_permission("users::read"));
        // With no groups, a permission not directly granted is denied.
        assert!(!p.has_permission("groups::edit"));
    }

    #[test]
    fn is_admin_is_not_folded_into_has_permission() {
        // An admin with no explicit grants does NOT auto-pass has_permission —
        // the admin short-circuit is a call-site concern (matches
        // check_permission_union, which likewise ignores is_admin).
        let p = TestPrincipal {
            admin: true,
            direct: v(&[]),
            groups: vec![],
        };
        assert!(p.is_admin());
        assert!(!p.has_permission("users::read"));
    }
}
