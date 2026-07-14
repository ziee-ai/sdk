//! In-process, per-user SSE connection registry for realtime sync (chunk B5,
//! moved from ziee's `modules::sync::registry`).
//!
//! Unlike the global broadcast pool used by download/hardware SSE (every
//! connected client receives every event), this registry is **keyed by
//! user** so a `publish` targets exactly one user's connections, a
//! permission-holding subset, or everyone — a change to user A's data is
//! never delivered to user B. Single-process / single-Postgres today; a
//! future multi-instance deployment would fan out via LISTEN/NOTIFY.
//!
//! The registry is generic over the app's per-connection permission snapshot
//! `P: `[`Principal`] — ziee installs a `SyncConnPrincipal { user, groups }` —
//! and the framework never names ziee's concrete `User`/`Group`. It also never
//! names ziee's wire types: `deliver` takes an already-serialized axum SSE
//! [`Event`], and the session fan-out obtains each per-user event from an app
//! [`SyncEntityKind`](super::SyncEntityKind).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use axum::http::StatusCode;
use axum::response::sse::Event;
use tokio::sync::mpsc::Sender;
use uuid::Uuid;

use ziee_core::AppError;
use ziee_identity::Principal;

use super::audience::{Audience, PermRule};
use super::SyncEntityKind;

/// Global cap on concurrent sync SSE connections across all users.
const GLOBAL_MAX_CONNECTIONS: usize = 512;
/// Per-user cap (multiple tabs/devices). Bounds a single account from
/// exhausting the global pool.
const PER_USER_MAX_CONNECTIONS: usize = 12;
/// Bounded per-connection queue depth. A reader that falls this far behind is
/// treated as stalled: the connection is dropped (so the client reconnects +
/// resyncs) rather than buffering unbounded memory. Sized generously so a
/// normal burst never trips it.
pub const SYNC_CHANNEL_CAPACITY: usize = 1024;

type ConnId = Uuid;

/// One live SSE connection's server-side state. `principal` is the permission
/// snapshot captured at connect (refreshed by the handler's periodic re-check)
/// and consulted when routing `Perm`-audience events.
pub struct ClientConn<P: Principal> {
    pub user_id: Uuid,
    pub principal: P,
    pub sender: Sender<Result<Event, axum::Error>>,
}

struct RegistryInner<P: Principal> {
    clients: HashMap<ConnId, ClientConn<P>>,
    by_user: HashMap<Uuid, HashSet<ConnId>>,
}

pub struct SyncRegistry<P: Principal> {
    inner: Mutex<RegistryInner<P>>,
}

impl<P: Principal + Send + 'static> Default for SyncRegistry<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Principal + Send + 'static> SyncRegistry<P> {
    /// Construct an empty registry. The app wraps ONE of these in a
    /// process-wide singleton (ziee: a `lazy_static`) keyed to its concrete
    /// principal type.
    pub fn new() -> Self {
        SyncRegistry {
            inner: Mutex::new(RegistryInner {
                clients: HashMap::new(),
                by_user: HashMap::new(),
            }),
        }
    }

    /// Register a new connection. Returns a 429 `AppError` when a global
    /// or per-user connection cap is hit.
    pub fn register(&self, conn_id: ConnId, conn: ClientConn<P>) -> Result<(), AppError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        if inner.clients.len() >= GLOBAL_MAX_CONNECTIONS {
            return Err(AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "SYNC_GLOBAL_LIMIT",
                "Realtime sync is at capacity; retry shortly",
            ));
        }
        let user_count = inner.by_user.get(&conn.user_id).map_or(0, |s| s.len());
        if user_count >= PER_USER_MAX_CONNECTIONS {
            return Err(AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "SYNC_USER_LIMIT",
                "Too many open sync connections for this account",
            ));
        }

        inner.by_user.entry(conn.user_id).or_default().insert(conn_id);
        inner.clients.insert(conn_id, conn);
        Ok(())
    }

    /// Remove a connection (called on stream termination).
    pub fn unregister(&self, conn_id: ConnId) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = inner.clients.remove(&conn_id) {
            if let Some(set) = inner.by_user.get_mut(&conn.user_id) {
                set.remove(&conn_id);
                if set.is_empty() {
                    inner.by_user.remove(&conn.user_id);
                }
            }
        }
    }

    /// Refresh a connection's permission snapshot (the periodic re-check).
    pub fn refresh(&self, conn_id: ConnId, principal: P) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = inner.clients.get_mut(&conn_id) {
            conn.principal = principal;
        }
    }

    /// Route one already-serialized event to the connections its audience
    /// permits, skipping the originating connection (self-echo suppression). A
    /// connection whose bounded queue is full (stalled reader) or closed is
    /// pruned.
    pub fn deliver(&self, audience: Audience, event: Event, origin_conn: Option<ConnId>) {
        let sse: Event = event;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        let mut dead: Vec<ConnId> = Vec::new();
        {
            let RegistryInner { clients, by_user } = &*inner;
            let mut try_send = |conn_id: &ConnId, conn: &ClientConn<P>| {
                if Some(*conn_id) == origin_conn {
                    return;
                }
                // Full = stalled reader, Closed = receiver dropped. Either
                // way the connection is no longer useful; prune it (the
                // client reconnects + resyncs).
                if conn.sender.try_send(Ok(sse.clone())).is_err() {
                    dead.push(*conn_id);
                }
            };

            match audience {
                Audience::Owner(uid) => {
                    if let Some(set) = by_user.get(&uid) {
                        for cid in set {
                            if let Some(conn) = clients.get(cid) {
                                try_send(cid, conn);
                            }
                        }
                    }
                }
                Audience::Perm(rule) => {
                    for (cid, conn) in clients.iter() {
                        let granted = conn.principal.is_admin()
                            || match &rule {
                                PermRule::All(perms) => {
                                    perms.iter().all(|p| conn.principal.has_permission(p))
                                }
                                PermRule::Any(perms) => {
                                    perms.iter().any(|p| conn.principal.has_permission(p))
                                }
                            };
                        if granted {
                            try_send(cid, conn);
                        }
                    }
                }
                Audience::Everyone => {
                    for (cid, conn) in clients.iter() {
                        try_send(cid, conn);
                    }
                }
            }
        }

        for cid in dead {
            remove_conn(&mut inner, cid);
        }
    }

    /// Deliver a session/permissions-changed signal to many users at once,
    /// taking the registry lock a SINGLE time. Used by group-permission edits
    /// that fan out to every member (avoids N lock acquisitions). Each
    /// recipient's event is built by the app's [`SyncEntityKind::session_signal`].
    /// Skips the originating connection and prunes stalled connections.
    pub fn deliver_session_to_users<E: SyncEntityKind>(
        &self,
        user_ids: &[Uuid],
        origin_conn: Option<ConnId>,
    ) {
        if user_ids.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut dead: Vec<ConnId> = Vec::new();
        {
            let RegistryInner { clients, by_user } = &*inner;
            for &uid in user_ids {
                let Some(set) = by_user.get(&uid) else {
                    continue;
                };
                let sse: Event = E::session_signal(uid);
                for cid in set {
                    if Some(*cid) == origin_conn {
                        continue;
                    }
                    if let Some(conn) = clients.get(cid) {
                        if conn.sender.try_send(Ok(sse.clone())).is_err() {
                            dead.push(*cid);
                        }
                    }
                }
            }
        }
        for cid in dead {
            remove_conn(&mut inner, cid);
        }
    }

    /// Number of live connections (test/diagnostic helper).
    #[allow(dead_code)]
    pub fn connection_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clients
            .len()
    }
}

/// Remove a connection from both indexes (shared by unregister + deliver's
/// stalled-connection pruning).
fn remove_conn<P: Principal>(inner: &mut RegistryInner<P>, conn_id: ConnId) {
    if let Some(conn) = inner.clients.remove(&conn_id) {
        if let Some(set) = inner.by_user.get_mut(&conn.user_id) {
            set.remove(&conn_id);
            if set.is_empty() {
                inner.by_user.remove(&conn.user_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::Receiver;
    use ziee_identity::check_permissions_array;

    /// A framework-side stand-in for ziee's `SyncConnPrincipal`: the routing
    /// logic depends only on `Principal::{is_admin, has_permission}`, so this
    /// collapses direct + group permissions into a single string set (the union
    /// semantics themselves are covered by ziee's `check_permission_union`
    /// tests). Group-derived routing is still exercised via the
    /// `active_group_permissions` override below.
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

    fn principal(admin: bool, perms: Vec<&str>) -> TestPrincipal {
        TestPrincipal {
            admin,
            direct: perms.into_iter().map(String::from).collect(),
            groups: Vec::new(),
        }
    }

    /// A framework-side stand-in for ziee's `SyncEntity`: builds a distinct
    /// dummy session event (the routing/pruning logic doesn't inspect the
    /// payload — ziee's wire-format tests cover the serialized shape).
    struct TestEntity;
    impl SyncEntityKind for TestEntity {
        fn session_signal(user_id: Uuid) -> Event {
            Event::default().event("sync").data(user_id.to_string())
        }
    }

    fn dummy_event() -> Event {
        Event::default().event("sync").data("x")
    }

    type Rx = Receiver<Result<Event, axum::Error>>;

    fn empty_registry() -> SyncRegistry<TestPrincipal> {
        SyncRegistry::new()
    }

    /// Build a ClientConn + its receiver at the default channel capacity.
    fn conn(user_id: Uuid, p: TestPrincipal) -> (ClientConn<TestPrincipal>, Rx) {
        let (tx, rx) = tokio::sync::mpsc::channel(SYNC_CHANNEL_CAPACITY);
        let c = ClientConn {
            user_id,
            principal: p,
            sender: tx,
        };
        (c, rx)
    }

    /// A delivered message is `Ok(_)` on try_recv; a non-delivery is Empty.
    fn got(rx: &mut Rx) -> bool {
        rx.try_recv().is_ok()
    }

    #[test]
    fn owner_audience_isolates_users() {
        let reg = empty_registry();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let (ca, mut rxa) = conn(a, principal(false, vec![]));
        let (cb, mut rxb) = conn(b, principal(false, vec![]));
        let (ida, idb) = (Uuid::new_v4(), Uuid::new_v4());
        reg.register(ida, ca).unwrap();
        reg.register(idb, cb).unwrap();

        reg.deliver(Audience::Owner(a), dummy_event(), None);

        assert!(got(&mut rxa), "owner A must receive their own event");
        assert!(!got(&mut rxb), "user B must NOT receive user A's event");
    }

    #[test]
    fn origin_connection_is_skipped_but_other_tabs_are_not() {
        let reg = empty_registry();
        let a = Uuid::new_v4();
        let (c1, mut rx1) = conn(a, principal(false, vec![]));
        let (c2, mut rx2) = conn(a, principal(false, vec![]));
        let (id1, id2) = (Uuid::new_v4(), Uuid::new_v4());
        reg.register(id1, c1).unwrap();
        reg.register(id2, c2).unwrap();

        // Mutation originated on conn1.
        reg.deliver(Audience::Owner(a), dummy_event(), Some(id1));

        assert!(!got(&mut rx1), "originating tab must be skipped (self-echo)");
        assert!(got(&mut rx2), "the user's OTHER tab must still update");
    }

    #[test]
    fn permission_audience_excludes_non_holders_includes_holders_and_admins() {
        let reg = empty_registry();
        let (c_admin, mut rx_admin) = conn(Uuid::new_v4(), principal(true, vec![]));
        let (c_holder, mut rx_holder) = conn(Uuid::new_v4(), principal(false, vec!["x::read"]));
        let (c_other, mut rx_other) = conn(Uuid::new_v4(), principal(false, vec![]));
        reg.register(Uuid::new_v4(), c_admin).unwrap();
        reg.register(Uuid::new_v4(), c_holder).unwrap();
        reg.register(Uuid::new_v4(), c_other).unwrap();

        reg.deliver(
            Audience::Perm(PermRule::All(vec!["x::read"])),
            dummy_event(),
            None,
        );

        assert!(got(&mut rx_admin), "admin (wildcard) must receive");
        assert!(got(&mut rx_holder), "perm holder must receive");
        assert!(!got(&mut rx_other), "non-holder must NOT receive");
    }

    /// Build a ClientConn whose permission comes ONLY from group membership
    /// (the connection's direct permissions are empty), exercising the
    /// `Principal::active_group_permissions` routing path.
    fn conn_with_group(user_id: Uuid, group_perms: Vec<&str>) -> (ClientConn<TestPrincipal>, Rx) {
        let (tx, rx) = tokio::sync::mpsc::channel(SYNC_CHANNEL_CAPACITY);
        let p = TestPrincipal {
            admin: false,
            direct: Vec::new(),
            groups: vec![group_perms.into_iter().map(String::from).collect()],
        };
        let c = ClientConn {
            user_id,
            principal: p,
            sender: tx,
        };
        (c, rx)
    }

    #[test]
    fn group_scoped_audience_routes_by_group_membership() {
        // The group-scoped user-view entities (UserMcpServer / UserLlmProvider /
        // Group) deliver to perm holders — and the perm is typically granted via
        // GROUP MEMBERSHIP, not the direct permissions. Assert the group-derived
        // path is honored for BOTH PermRule::All and PermRule::Any, while a
        // member-less connection is excluded.
        let reg = empty_registry();
        let (c_group, mut rx_group) = conn_with_group(Uuid::new_v4(), vec!["users::read"]);
        let (c_none, mut rx_none) = conn(Uuid::new_v4(), principal(false, vec![]));
        reg.register(Uuid::new_v4(), c_group).unwrap();
        reg.register(Uuid::new_v4(), c_none).unwrap();

        // All-rule: the group grants the only required perm.
        reg.deliver(
            Audience::Perm(PermRule::All(vec!["users::read"])),
            dummy_event(),
            None,
        );
        assert!(got(&mut rx_group), "group-derived perm must receive (All)");
        assert!(!got(&mut rx_none), "connection with no group/perm must NOT receive");

        // Any-rule: one of the alternatives is granted via the group.
        reg.deliver(
            Audience::Perm(PermRule::Any(vec!["users::read", "mcp::admin"])),
            dummy_event(),
            None,
        );
        assert!(got(&mut rx_group), "group-derived perm must receive (Any)");
        assert!(!got(&mut rx_none), "connection with no group/perm must NOT receive (Any)");
    }

    #[test]
    fn everyone_audience_reaches_all_connections() {
        let reg = empty_registry();
        let (c1, mut rx1) = conn(Uuid::new_v4(), principal(false, vec![]));
        let (c2, mut rx2) = conn(Uuid::new_v4(), principal(false, vec![]));
        reg.register(Uuid::new_v4(), c1).unwrap();
        reg.register(Uuid::new_v4(), c2).unwrap();

        reg.deliver(Audience::Everyone, dummy_event(), None);

        assert!(got(&mut rx1));
        assert!(got(&mut rx2));
    }

    #[test]
    fn per_user_cap_rejects_excess_connections() {
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        for _ in 0..PER_USER_MAX_CONNECTIONS {
            let (c, _rx) = conn(uid, principal(false, vec![]));
            reg.register(Uuid::new_v4(), c).unwrap();
        }
        let (overflow, _rx) = conn(uid, principal(false, vec![]));
        assert!(
            reg.register(Uuid::new_v4(), overflow).is_err(),
            "the (cap+1)th connection for one user must be refused (429)"
        );
    }

    #[test]
    fn rapid_fire_deliveries_are_all_enqueued_in_order() {
        // A burst of mutations for one owner (e.g. several quick edits) must all
        // reach the connection — none silently dropped — and, because the
        // per-connection channel is FIFO, in submission order. We assert the
        // no-loss property: N rapid deliveries yield exactly N queued events.
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        let (c, mut rx) = conn(uid, principal(false, vec![]));
        reg.register(Uuid::new_v4(), c).unwrap();

        const BURST: usize = 25;
        for _ in 0..BURST {
            reg.deliver(Audience::Owner(uid), dummy_event(), None);
        }

        let mut received = 0;
        while rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(
            received, BURST,
            "every rapid-fire delivery must be enqueued (no drops under cap)"
        );
    }

    #[test]
    fn global_cap_rejects_excess_connections_across_users() {
        // Fill the registry to GLOBAL_MAX_CONNECTIONS with one connection per
        // distinct user, so the GLOBAL cap (not the per-user cap) is what trips.
        let reg = empty_registry();
        for _ in 0..GLOBAL_MAX_CONNECTIONS {
            let (c, _rx) = conn(Uuid::new_v4(), principal(false, vec![]));
            reg.register(Uuid::new_v4(), c).unwrap();
        }
        assert_eq!(reg.connection_count(), GLOBAL_MAX_CONNECTIONS);

        // The (global cap + 1)th connection — a brand-new user well under the
        // per-user cap — must be refused with a 429.
        let (overflow, _rx) = conn(Uuid::new_v4(), principal(false, vec![]));
        let err = reg
            .register(Uuid::new_v4(), overflow)
            .expect_err("global cap must reject the 513th connection");
        assert_eq!(err.status_code(), 429, "global cap must surface 429");
    }

    #[test]
    fn unregister_cleans_up_indexes() {
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        let (c, _rx) = conn(uid, principal(false, vec![]));
        let id = Uuid::new_v4();
        reg.register(id, c).unwrap();
        assert_eq!(reg.connection_count(), 1);

        reg.unregister(id);
        assert_eq!(reg.connection_count(), 0);
        // The per-user index entry is removed when its last conn leaves, so a
        // later Owner delivery is a no-op (and doesn't panic).
        reg.deliver(Audience::Owner(uid), dummy_event(), None);
    }

    #[test]
    fn refresh_updates_permission_snapshot() {
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        let (c, mut rx) = conn(uid, principal(false, vec![]));
        let id = Uuid::new_v4();
        reg.register(id, c).unwrap();

        // Before refresh: no perm → excluded from a Permission audience.
        reg.deliver(
            Audience::Perm(PermRule::All(vec!["x::read"])),
            dummy_event(),
            None,
        );
        assert!(!got(&mut rx));

        // After a re-check grants the perm, the same connection is included.
        reg.refresh(id, principal(false, vec!["x::read"]));
        reg.deliver(
            Audience::Perm(PermRule::All(vec!["x::read"])),
            dummy_event(),
            None,
        );
        assert!(got(&mut rx));
    }

    /// Build a ClientConn with an explicit channel capacity (to exercise the
    /// stalled-reader pruning without queueing a full `SYNC_CHANNEL_CAPACITY`).
    fn conn_with_cap(user_id: Uuid, cap: usize) -> (ClientConn<TestPrincipal>, Rx) {
        let (tx, rx) = tokio::sync::mpsc::channel(cap);
        let c = ClientConn {
            user_id,
            principal: principal(false, vec![]),
            sender: tx,
        };
        (c, rx)
    }

    #[test]
    fn deliver_session_to_users_targets_only_listed_users_and_skips_origin() {
        let reg = empty_registry();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let (a1, mut rx_a1) = conn(a, principal(false, vec![]));
        let (a2, mut rx_a2) = conn(a, principal(false, vec![]));
        let (cb, mut rx_b) = conn(b, principal(false, vec![]));
        let (id_a1, id_a2, id_b) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        reg.register(id_a1, a1).unwrap();
        reg.register(id_a2, a2).unwrap();
        reg.register(id_b, cb).unwrap();

        // Fan a session signal to user A only, originating on a1.
        reg.deliver_session_to_users::<TestEntity>(&[a], Some(id_a1));

        assert!(!got(&mut rx_a1), "originating connection is skipped");
        assert!(got(&mut rx_a2), "A's other connection receives the signal");
        assert!(!got(&mut rx_b), "an unlisted user receives nothing");
    }

    /// audit id 97e64997158 — the session fan-out path (deliver_session_to_users,
    /// used by group-permission edits to re-bootstrap every member) has its OWN
    /// stalled-connection prune distinct from `deliver`'s. A member whose
    /// bounded queue is full must be pruned here too, so the client is forced to
    /// reconnect + resync (the SSE auth/permission-loss lifecycle).
    #[test]
    fn deliver_session_to_users_prunes_a_lagging_connection() {
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        // Capacity 1, never drained → the second session signal can't enqueue.
        let (c, _rx) = conn_with_cap(uid, 1);
        let id = Uuid::new_v4();
        reg.register(id, c).unwrap();

        // First fan-out fills the 1-slot queue (origin=None so it's delivered).
        reg.deliver_session_to_users::<TestEntity>(&[uid], None);
        assert_eq!(reg.connection_count(), 1);

        // Second fan-out → try_send returns Full → the connection is pruned.
        reg.deliver_session_to_users::<TestEntity>(&[uid], None);
        assert_eq!(
            reg.connection_count(),
            0,
            "a session-fanout target whose bounded queue is full must be pruned",
        );
    }

    #[test]
    fn lagging_connection_is_pruned() {
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        // Capacity 1, and we never read `_rx` — so the second delivery can't
        // enqueue (Full). Keep `_rx` alive so the channel isn't Closed (which
        // would also prune, but we're testing the Full → prune path).
        let (c, _rx) = conn_with_cap(uid, 1);
        let id = Uuid::new_v4();
        reg.register(id, c).unwrap();

        reg.deliver(Audience::Owner(uid), dummy_event(), None); // fills the 1-slot queue
        assert_eq!(reg.connection_count(), 1);

        reg.deliver(Audience::Owner(uid), dummy_event(), None); // Full → prune
        assert_eq!(
            reg.connection_count(),
            0,
            "a connection whose bounded queue is full must be pruned"
        );
    }

    /// Sanity: `check_permissions_array` (the evaluator behind `has_permission`)
    /// is re-exported by ziee-identity and drives the `Perm` routing above.
    #[test]
    fn has_permission_uses_the_shared_evaluator() {
        assert!(check_permissions_array(
            &["users::*".to_string()],
            "users::read"
        ));
    }
}
