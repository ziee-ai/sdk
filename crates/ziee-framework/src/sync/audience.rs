//! Sync delivery **audience** — the typed scope a publishing handler chooses for
//! one event. Moved from ziee's `modules::sync::event` (chunk B5); the tenant
//! operand is an sdk#24 / memo-335 §1 change (queue 417sync).
//!
//! There is NO central per-entity table: the module that owns the mutation
//! decides who may learn of it, using its OWN typed permissions. Build an
//! `Audience` with the constructors below so a renamed/removed permission is a
//! compile error. The permission strings are consumed by
//! [`SyncRegistry::deliver`](super::registry::SyncRegistry::deliver), which routes
//! against each connection's [`Principal`](ziee_identity::Principal) snapshot.
//!
//! **The fanout key is (tenant, permission), never permission alone** (memo-335
//! §1 Recommendation, verbatim). A perm audience carries the account it was
//! published for (`Perm { account_id, .. }`), and `deliver` matches the
//! connection principal's [`account_id`](ziee_identity::Principal::account_id)
//! against it FIRST — `is_admin()` no longer bypasses that equality (cross-tenant
//! is cross-tenant regardless of admin; INV-3). `Tenant(account_id)` is the
//! tenant-scoped broadcast (that account's connections, no permission check).
//! `Everyone` is retained as the Everyone-EQUIVALENT for **genuinely non-tenant
//! frames only** (deployment-global, e.g. install-scope singleton config); the
//! sync entity registry refuses `Everyone` for any entity declared tenant-scoped.

use ziee_identity::{PermissionCheck, PermissionList};
use uuid::Uuid;

/// Delivery scope for one event, chosen by the publishing handler. There is
/// NO central per-entity table: the module that owns the mutation decides who
/// may learn of it, using its OWN typed permissions. Build it with the typed
/// constructors below so a renamed/removed permission is a compile error.
#[derive(Debug, Clone)]
pub enum Audience {
    /// Only the owning user's connections. `Owner(Uuid)` stays: user ids are
    /// globally unique in the single credential store (`users` carries no
    /// account column), so owner-fanout needs no account comparison.
    Owner(Uuid),
    /// Only connections of `account_id` whose permission snapshot satisfies
    /// the rule. Tenant equality is checked FIRST (the connection principal's
    /// `account_id()` must equal `account_id`); within that tenant, admins
    /// always qualify. A `None`-account principal never matches (fail-closed).
    Perm {
        /// The tenant/account this frame was published for.
        account_id: Uuid,
        /// The permission requirement, evaluated within the tenant.
        rule: PermRule,
    },
    /// Every connection whose principal account is `account_id` — the
    /// tenant-scoped broadcast (no permission check, origin skipped via the
    /// existing `try_send` closure in `deliver`).
    Tenant(Uuid),
    /// Every authenticated connection — the Everyone-EQUIVALENT, retained only
    /// for GENUINELY NON-TENANT frames (deployment-global signals such as the
    /// install-scope singleton config kinds). The sync entity registry refuses
    /// `Everyone` for any entity declared tenant-scoped. No current prod caller
    /// (owner/perm scoping covers today's entities); retained as intentional
    /// API surface.
    #[allow(dead_code)]
    Everyone,
}

/// A composable permission requirement
#[derive(Debug, Clone)]
pub enum PermRule {
    /// The connection must hold EVERY listed permission.
    All(Vec<&'static str>),
    /// The connection must hold AT LEAST ONE listed permission.
    Any(Vec<&'static str>),
}

impl Audience {
    /// Deliver only to `user_id`'s own connections.
    pub fn owner(user_id: Uuid) -> Self {
        Audience::Owner(user_id)
    }

    /// Deliver to every connection of `account_id` — the tenant-scoped
    /// broadcast (no permission check, origin skipped by `deliver`). No
    /// current prod caller; retained as intentional API surface.
    #[allow(dead_code)]
    pub fn tenant(account_id: Uuid) -> Self {
        Audience::Tenant(account_id)
    }

    /// Deliver to every authenticated connection. The Everyone-EQUIVALENT for
    /// GENUINELY NON-TENANT frames only (e.g. install-scope singleton config)
    /// — the sync entity registry refuses it for any entity declared
    /// tenant-scoped. No current caller (owner/perm scoping covers today's
    /// entities), so retained as intentional API surface.
    #[allow(dead_code)]
    pub fn everyone() -> Self {
        Audience::Everyone
    }

    /// Deliver to holders of a single typed permission within one account, e.g.
    /// `Audience::perm::<LlmModelsRead>(account_id)`. The account is the first
    /// runtime operand: the fanout key is (tenant, permission), never
    /// permission alone — the same permission string in two accounts is two
    /// different fanouts (memo-335 §1 / sdk#24).
    pub fn perm<P: PermissionCheck>(account_id: Uuid) -> Self {
        Audience::Perm {
            account_id,
            rule: PermRule::All(vec![P::PERMISSION]),
        }
    }

    /// Deliver to holders of ALL permissions in the tuple, within one account,
    /// e.g. `Audience::all_of::<(LlmProvidersRead, LlmModelsRead)>(account_id)`.
    /// Reuses the same `PermissionList` tuple machinery as
    /// `RequirePermissions<(A, B)>`.
    #[allow(dead_code)]
    pub fn all_of<L: PermissionList>(account_id: Uuid) -> Self {
        Audience::Perm {
            account_id,
            rule: PermRule::All(L::permissions()),
        }
    }

    /// Deliver to holders of ANY permission in the tuple, within one account,
    /// e.g. `Audience::any_of::<(McpServersRead, McpServersAdminRead)>(account_id)`.
    #[allow(dead_code)]
    pub fn any_of<L: PermissionList>(account_id: Uuid) -> Self {
        Audience::Perm {
            account_id,
            rule: PermRule::Any(L::permissions()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PermA;
    impl PermissionCheck for PermA {
        const NAME: &'static str = "PermA";
        const PERMISSION: &'static str = "a::read";
        const DESCRIPTION: &'static str = "";
        const MODULE: &'static str = "test";
    }
    struct PermB;
    impl PermissionCheck for PermB {
        const NAME: &'static str = "PermB";
        const PERMISSION: &'static str = "b::read";
        const DESCRIPTION: &'static str = "";
        const MODULE: &'static str = "test";
    }

    /// TEST-1 (queue fr3-417sync) [covers: ITEM-1, ITEM-11] — the typed
    /// constructors carry BOTH the typed permission (compile-time const) AND
    /// the account operand (DEC-4's `perm::<P>(account_id)` rendering of the
    /// memo's `Audience::perm(account_id, audit_log::read)`). A constructor
    /// that drops/zeroes the operand reddens the `account_id == acct` match.
    #[test]
    fn typed_constructors_carry_the_account_and_the_permission_string() {
        let acct = Uuid::from_u128(0x41);

        match Audience::perm::<PermA>(acct) {
            Audience::Perm { account_id, rule: PermRule::All(ps) } => {
                assert_eq!(account_id, acct, "perm::<P> must carry the account operand");
                assert_eq!(ps, vec!["a::read"], "perm::<P> must carry the typed permission");
            }
            other => panic!("expected Perm{{account_id, All}}, got {other:?}"),
        }

        match Audience::all_of::<(PermA, PermB)>(acct) {
            Audience::Perm { account_id, rule: PermRule::All(ps) } => {
                assert_eq!(account_id, acct, "all_of must carry the account operand");
                assert_eq!(ps, vec!["a::read", "b::read"]);
            }
            other => panic!("expected Perm{{account_id, All}}, got {other:?}"),
        }

        match Audience::any_of::<(PermA, PermB)>(acct) {
            Audience::Perm { account_id, rule: PermRule::Any(ps) } => {
                assert_eq!(account_id, acct, "any_of must carry the account operand");
                assert_eq!(ps, vec!["a::read", "b::read"]);
            }
            other => panic!("expected Perm{{account_id, Any}}, got {other:?}"),
        }
    }

    /// TEST-2 (queue fr3-417sync) [acceptance] [invariant: INV-2] [covers:
    /// ITEM-1, ITEM-11] — the enum shape is exactly
    /// `Owner(Uuid) | Perm { account_id: Uuid, rule: PermRule } | Tenant(Uuid)
    /// | Everyone`: the `Tenant(account_id)` variant exists (the memo's
    /// "becomes `Audience::Tenant(account_id)` for tenant-scoped broadcasts"),
    /// `Owner(Uuid)` is unchanged, the Everyone-EQUIVALENT constructor
    /// `everyone()` is retained (DEC-3's spelling) and documented as
    /// genuinely-non-tenant-frames-only, and the BARE `Perm(PermRule)` shape no
    /// longer exists (the struct-variant match would fail to compile). A
    /// removed `Tenant` or a dropped `account_id` field reddens by compile
    /// failure.
    #[test]
    fn audience_variants_are_perm_with_account_tenant_and_everyone_equivalent() {
        let acct = Uuid::from_u128(0x42);
        let (uid_a, uid_b) = (Uuid::from_u128(0x51), Uuid::from_u128(0x52));

        // Owner(Uuid) unchanged — user ids stay globally unique.
        match Audience::owner(uid_a) {
            Audience::Owner(u) => assert_eq!(u, uid_a),
            other => panic!("expected Owner, got {other:?}"),
        }

        // Perm is the struct variant carrying the account; the bare-tuple shape
        // is gone. `Tenant(account_id)` exists and is constructible.
        match Audience::perm::<PermA>(acct) {
            Audience::Perm { account_id, .. } => assert_eq!(account_id, acct),
            other => panic!("expected Perm{{account_id, ..}}, got {other:?}"),
        }
        match Audience::tenant(acct) {
            Audience::Tenant(t) => assert_eq!(t, acct),
            other => panic!("expected Tenant(account_id), got {other:?}"),
        }

        // The Everyone-EQUIVALENT: `everyone()` retained (DEC-3 spelling).
        match Audience::everyone() {
            Audience::Everyone => {}
            other => panic!("expected Everyone (the equivalent), got {other:?}"),
        }

        // Owner isolation is untouched: a second user/account is a distinct
        // variant payload, never coerced into the tenant field.
        match Audience::owner(uid_b) {
            Audience::Owner(u) => assert_eq!(u, uid_b),
            other => panic!("expected Owner, got {other:?}"),
        }
    }
}
