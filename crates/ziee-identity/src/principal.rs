//! `Principal` — the minimal authenticated-identity interface the framework
//! enforces against (pluggable identity, decision #1).
//!
//! ziee's concrete `User` implements this; the framework's authorization layer
//! (the `RequirePermissions` extractor, which moves in B3) depends only on this
//! trait, never on a concrete table type. The default `has_permission` reuses
//! the generic [`crate::rbac::check_permissions_array`] evaluator so wildcard /
//! hierarchical semantics stay identical across every identity implementation.

use crate::rbac::check_permissions_array;
use uuid::Uuid;

/// An authenticated identity, abstracted to exactly what framework enforcement
/// needs: an admin flag, the identity's own directly-granted permissions, the
/// permission sets of its ACTIVE groups, and (sdk#24 / memo-335 §1, queue
/// 417sync) the tenant/account it belongs to.
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

    /// The tenant/account this identity belongs to. `None` = unknown: a `None`
    /// principal never matches a tenant-scoped audience (`deliver`'s `Perm`/
    /// `Tenant` arms compare `account_id() == Some(audience_account_id)`, so
    /// `None` fails closed) and it never blocks a non-tenant (`Everyone`)
    /// frame, which matches no account at all. Pre-tenancy installs whose
    /// snapshot carries no account data leave this at the default — the honest
    /// answer, never a fabricated `Uuid::nil()`.
    fn account_id(&self) -> Option<Uuid> {
        None
    }

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
    use uuid::Uuid;

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

    // TEST-11 (queue fr3-417sync): pre-existing UNION fn — stays byte-equal
    // (ITEM-2 / DEC-1: `has_permission` semantics untouched; runs in phase 8).
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

    // TEST-11 (queue fr3-417sync): pre-existing fn — wildcard-via-group stays
    // byte-equal (ITEM-2; runs in phase 8).
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

    // TEST-11 (queue fr3-417sync): pre-existing default-method fn — stays
    // byte-equal (ITEM-2 / DEC-1's default-method style; runs in phase 8).
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

    // TEST-11 (queue fr3-417sync): pre-existing admin fn — stays byte-equal
    // (ITEM-2: `is_admin` is still caller-applied; runs in phase 8).
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

    /// TEST-11 (queue fr3-417sync) [acceptance] [invariant: INV-4] [covers:
    /// ITEM-2, ITEM-11] — `Principal::account_id()` is a DEFAULTED accessor
    /// (DEC-1): an impl that does not override it honestly reports `None`, and
    /// an explicit override wins. That is why the pre-existing implementors
    /// need NO edit (the default is the honest "unknown" answer, never a
    /// fabricated `Uuid::nil()`), and the accessor reports the configured
    /// account when one is supplied.
    #[test]
    fn principal_account_id_accessor_reports_the_configured_account() {
        // Default half: principals that do not override `account_id()` report
        // None — an unknown-account principal never matches a tenant-scoped
        // audience (fail-closed) and never blocks a non-tenant frame.
        let p = TestPrincipal {
            admin: false,
            direct: v(&["users::read"]),
            groups: vec![],
        };
        assert_eq!(p.account_id(), None, "default accessor reports None (account unknown)");
        let d = DirectOnly { direct: v(&["users::read"]) };
        assert_eq!(d.account_id(), None, "DirectOnly (default-accessor impl) reports None");

        // Override half: an explicit impl returns the configured account, and a
        // second instance carrying a different account reports its own.
        let a = Uuid::from_u128(0xA1);
        let b = Uuid::from_u128(0xB2);
        let pa = Accounted { account: Some(a), direct: v(&[]) };
        let pb = Accounted { account: Some(b), direct: v(&[]) };
        assert_eq!(pa.account_id(), Some(a), "the accessor reports the configured account");
        assert_eq!(pb.account_id(), Some(b), "a different instance reports its own account");
        assert_ne!(pa.account_id(), pb.account_id(), "two accounts stay distinct");
        // The union semantics are untouched by the accessor.
        assert!(!pa.has_permission("users::read"));
        assert!(!pa.is_admin());
    }

    /// A `Principal` that overrides `account_id()` — pins DEC-1's "explicit
    /// overrides actually override" half of the accessor pin.
    struct Accounted {
        account: Option<Uuid>,
        direct: Vec<String>,
    }
    impl Principal for Accounted {
        fn is_admin(&self) -> bool {
            false
        }
        fn direct_permissions(&self) -> &[String] {
            &self.direct
        }
        fn account_id(&self) -> Option<Uuid> {
            self.account
        }
    }
}
