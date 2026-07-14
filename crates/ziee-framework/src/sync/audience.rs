//! Sync delivery **audience** — the typed scope a publishing handler chooses for
//! one event. Moved from ziee's `modules::sync::event` (chunk B5), byte-preserved.
//!
//! There is NO central per-entity table: the module that owns the mutation
//! decides who may learn of it, using its OWN typed permissions. Build an
//! `Audience` with the constructors below so a renamed/removed permission is a
//! compile error. The permission strings are consumed by
//! [`SyncRegistry::deliver`](super::registry::SyncRegistry::deliver), which routes
//! against each connection's [`Principal`](ziee_identity::Principal) snapshot.

use ziee_identity::{PermissionCheck, PermissionList};
use uuid::Uuid;

/// Delivery scope for one event, chosen by the publishing handler. There is
/// NO central per-entity table: the module that owns the mutation decides who
/// may learn of it, using its OWN typed permissions. Build it with the typed
/// constructors below so a renamed/removed permission is a compile error.
#[derive(Debug, Clone)]
pub enum Audience {
    /// Only the owning user's connections.
    Owner(Uuid),
    /// Only connections whose permission snapshot satisfies the rule
    /// (admins always qualify).
    Perm(PermRule),
    /// Every authenticated connection. No current prod caller (owner/perm
    /// scoping covers today's entities); retained as intentional API surface.
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

    /// Deliver to every authenticated connection. Part of the audience API for
    /// genuinely-public entities; no current caller (owner/perm scoping covers
    /// today's entities), so retained as intentional API surface.
    #[allow(dead_code)]
    pub fn everyone() -> Self {
        Audience::Everyone
    }

    /// Deliver to holders of a single typed permission, e.g.
    /// `Audience::perm::<LlmModelsRead>()`.
    pub fn perm<P: PermissionCheck>() -> Self {
        Audience::Perm(PermRule::All(vec![P::PERMISSION]))
    }

    /// Deliver to holders of ALL permissions in the tuple, e.g.
    /// `Audience::all_of::<(LlmProvidersRead, LlmModelsRead)>()`. Reuses the
    /// same `PermissionList` tuple machinery as `RequirePermissions<(A, B)>`.
    #[allow(dead_code)]
    pub fn all_of<L: PermissionList>() -> Self {
        Audience::Perm(PermRule::All(L::permissions()))
    }

    /// Deliver to holders of ANY permission in the tuple, e.g.
    /// `Audience::any_of::<(McpServersRead, McpServersAdminRead)>()`.
    #[allow(dead_code)]
    pub fn any_of<L: PermissionList>() -> Self {
        Audience::Perm(PermRule::Any(L::permissions()))
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

    #[test]
    fn perm_constructor_carries_the_typed_permission_string() {
        match Audience::perm::<PermA>() {
            Audience::Perm(PermRule::All(ps)) => assert_eq!(ps, vec!["a::read"]),
            other => panic!("expected Perm(All), got {other:?}"),
        }
    }

    #[test]
    fn all_of_and_any_of_collect_the_permission_tuple() {
        match Audience::all_of::<(PermA, PermB)>() {
            Audience::Perm(PermRule::All(ps)) => assert_eq!(ps, vec!["a::read", "b::read"]),
            other => panic!("expected Perm(All), got {other:?}"),
        }
        match Audience::any_of::<(PermA, PermB)>() {
            Audience::Perm(PermRule::Any(ps)) => assert_eq!(ps, vec!["a::read", "b::read"]),
            other => panic!("expected Perm(Any), got {other:?}"),
        }
    }
}
