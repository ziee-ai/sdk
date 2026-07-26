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

/// Grace period added to a connection's own `exp` deadline before a sweep
/// treats it as stale. The stream's `select!` breaks AT the deadline and its
/// guard then unregisters, so any connection still present well past its own
/// deadline is definitionally broken — this slack only avoids racing a normal
/// teardown. It is derived from the connection, not a tunable, so there is no
/// arbitrary TTL to configure and no risk of reaping a healthy idle stream.
const DEADLINE_SLACK: std::time::Duration = std::time::Duration::from_secs(60);

/// One live SSE connection's server-side state. `principal` is the permission
/// snapshot captured at connect (refreshed by the handler's periodic re-check)
/// and consulted when routing `Perm`-audience events.
pub struct ClientConn<P: Principal> {
    pub user_id: Uuid,
    pub principal: P,
    pub sender: Sender<Result<Event, axum::Error>>,
    /// When this connection's stream is guaranteed to have ended: the access
    /// token's `exp` deadline, which the subscribe handler already `select!`s
    /// on. `None` = no bound known (the handler's far-future fallback), which
    /// simply opts the connection out of deadline-based reclamation.
    ///
    /// This is the liveness backstop for a peer that vanished WITHOUT the
    /// socket erroring — hyper only learns the client is gone when a write
    /// fails, so an idle stream to a black-holed peer could otherwise hold its
    /// slot far longer than its own token was ever valid for.
    pub expires_at: Option<std::time::Instant>,
}

impl<P: Principal> ClientConn<P> {
    /// Is this connection past its own `exp` deadline (plus slack)? Such a
    /// connection's stream should already have torn itself down.
    fn is_past_deadline(&self, now: std::time::Instant) -> bool {
        self.expires_at
            .is_some_and(|deadline| now.saturating_duration_since(deadline) > DEADLINE_SLACK)
    }

    /// Is this connection reclaimable — its stream gone (receiver dropped), or
    /// still present long past the deadline at which it should have ended?
    fn is_reclaimable(&self, now: std::time::Instant) -> bool {
        self.sender.is_closed() || self.is_past_deadline(now)
    }
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
    ///
    /// A cap is only ever charged for connections that are still ALIVE: before
    /// each cap check the corresponding set is swept of connections whose
    /// stream is gone (`prune_closed_locked`). Without that sweep a user whose
    /// slots leaked would be refused forever — the registry's only other
    /// reaper is `deliver`'s send-failure prune, which never runs on a
    /// quiescent deployment because there is no event to deliver.
    ///
    /// The sweep is deliberately on-demand rather than a background reaper:
    /// a stale slot's ONLY user-visible effect is a cap refusal, and the caps
    /// are evaluated exclusively here (DEC-4).
    pub fn register(&self, conn_id: ConnId, conn: ClientConn<P>) -> Result<(), AppError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        // Per-user first: bounded by PER_USER_MAX_CONNECTIONS, so the common
        // path stays O(cap) rather than O(global).
        let mut user_count = inner.by_user.get(&conn.user_id).map_or(0, |s| s.len());
        if user_count >= PER_USER_MAX_CONNECTIONS {
            prune_closed_for_user_locked(&mut inner, conn.user_id);
            user_count = inner.by_user.get(&conn.user_id).map_or(0, |s| s.len());
        }
        if user_count >= PER_USER_MAX_CONNECTIONS {
            return Err(AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "SYNC_USER_LIMIT",
                "Too many open sync connections for this account",
            ));
        }

        // Global sweep only when the global cap would otherwise trip.
        if inner.clients.len() >= GLOBAL_MAX_CONNECTIONS {
            prune_closed_locked(&mut inner);
        }
        if inner.clients.len() >= GLOBAL_MAX_CONNECTIONS {
            return Err(AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "SYNC_GLOBAL_LIMIT",
                "Realtime sync is at capacity; retry shortly",
            ));
        }

        inner.by_user.entry(conn.user_id).or_default().insert(conn_id);
        inner.clients.insert(conn_id, conn);
        Ok(())
    }

    /// Sweep every reclaimable connection, returning how many were freed. Two
    /// independent liveness signals, neither of which can false-positive a
    /// healthy stream:
    ///
    /// 1. **Receiver dropped** (`sender.is_closed()`) — each connection's
    ///    `Receiver` is owned solely by its own SSE stream, so a closed sender
    ///    is exactly "that stream no longer exists". No TTL to tune, and an
    ///    idle-but-live stream (the normal case on a quiet deployment) is never
    ///    touched.
    /// 2. **Past its own `exp` deadline** — the stream `select!`s on the access
    ///    token's expiry and breaks there, so a connection still registered
    ///    well past that instant is definitionally broken. This covers the peer
    ///    that vanished WITHOUT the socket erroring: hyper only discovers the
    ///    client is gone when a write fails, so an idle stream to a black-holed
    ///    peer would otherwise keep its slot indefinitely.
    ///
    /// This is the BACKSTOP. The primary, deterministic release is the
    /// subscribe handler's connection guard, which is constructed at
    /// registration and moved into the stream so it is dropped even if the
    /// stream is never polled.
    pub fn prune_closed(&self) -> usize {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        prune_closed_locked(&mut inner)
    }

    /// Sweep only `user_id`'s reclaimable connections (same two signals as
    /// [`Self::prune_closed`]), returning how many were freed. Other users'
    /// connections are never inspected or touched.
    pub fn prune_closed_for_user(&self, user_id: Uuid) -> usize {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        prune_closed_for_user_locked(&mut inner, user_id)
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

/// Sweep every reclaimable connection. Caller holds the lock.
/// Collect-then-remove (mirroring `deliver`'s dead-connection handling) so the
/// two-index invariant is maintained by the single `remove_conn` helper.
fn prune_closed_locked<P: Principal>(inner: &mut RegistryInner<P>) -> usize {
    let now = std::time::Instant::now();
    let dead: Vec<ConnId> = inner
        .clients
        .iter()
        .filter(|(_, c)| c.is_reclaimable(now))
        .map(|(id, _)| *id)
        .collect();
    let n = dead.len();
    for cid in dead {
        remove_conn(inner, cid);
    }
    if n > 0 {
        tracing::debug!("sync registry: reclaimed {n} dead/expired connection(s)");
    }
    n
}

/// Sweep only `user_id`'s reclaimable connections. Caller holds the lock.
fn prune_closed_for_user_locked<P: Principal>(
    inner: &mut RegistryInner<P>,
    user_id: Uuid,
) -> usize {
    let Some(set) = inner.by_user.get(&user_id) else {
        return 0;
    };
    let now = std::time::Instant::now();
    let dead: Vec<ConnId> = set
        .iter()
        .filter(|cid| {
            inner
                .clients
                .get(*cid)
                .is_some_and(|c| c.is_reclaimable(now))
        })
        .copied()
        .collect();
    let n = dead.len();
    for cid in dead {
        remove_conn(inner, cid);
    }
    if n > 0 {
        tracing::debug!("sync registry: reclaimed {n} dead/expired connection(s) for one user");
    }
    n
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
            expires_at: None,
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
            expires_at: None,
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
        // The receivers must be HELD: a cap is charged for live connections
        // only, so a per-iteration `_rx` binding (dropped at the end of each
        // iteration) would register connections whose streams are already gone
        // and `register`'s sweep would rightly reclaim them.
        let mut alive = Vec::new();
        for _ in 0..PER_USER_MAX_CONNECTIONS {
            let (c, rx) = conn(uid, principal(false, vec![]));
            reg.register(Uuid::new_v4(), c).unwrap();
            alive.push(rx);
        }
        let (overflow, _rx) = conn(uid, principal(false, vec![]));
        assert!(
            reg.register(Uuid::new_v4(), overflow).is_err(),
            "the (cap+1)th connection for one user must be refused (429)"
        );
        drop(alive);
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
        // Receivers HELD — see `per_user_cap_rejects_excess_connections`: the
        // cap counts live connections only.
        let mut alive = Vec::new();
        for _ in 0..GLOBAL_MAX_CONNECTIONS {
            let (c, rx) = conn(Uuid::new_v4(), principal(false, vec![]));
            reg.register(Uuid::new_v4(), c).unwrap();
            alive.push(rx);
        }
        assert_eq!(reg.connection_count(), GLOBAL_MAX_CONNECTIONS);

        // The (global cap + 1)th connection — a brand-new user well under the
        // per-user cap — must be refused with a 429.
        let (overflow, _rx) = conn(Uuid::new_v4(), principal(false, vec![]));
        let err = reg
            .register(Uuid::new_v4(), overflow)
            .expect_err("global cap must reject the 513th connection");
        assert_eq!(err.status_code(), 429, "global cap must surface 429");
        drop(alive);
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
            expires_at: None,
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

    // ---- slot reclamation (sse-slot-leak) --------------------------------
    //
    // The registry's ONLY pre-existing reaper was `deliver`'s send-failure
    // prune, which requires an event to deliver — so on a quiescent deployment
    // a connection whose stream is gone held its slot forever, and each
    // reconnect burned one until the account was permanently 429'd.

    /// TEST-1 — a connection whose receiver has been dropped is reclaimed from
    /// BOTH indexes by `prune_closed`, and the emptied `by_user` entry is
    /// cleaned up too (so a later Owner delivery is a harmless no-op).
    #[test]
    fn prune_closed_reclaims_a_connection_whose_stream_is_gone() {
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        let (c, rx) = conn(uid, principal(false, vec![]));
        reg.register(Uuid::new_v4(), c).unwrap();
        assert_eq!(reg.connection_count(), 1);

        // The stream went away: its receiver is dropped.
        drop(rx);

        assert_eq!(reg.prune_closed(), 1, "the dead connection must be reclaimed");
        assert_eq!(reg.connection_count(), 0);
        // by_user was cleaned up as well — an Owner delivery finds nothing and
        // must not panic.
        reg.deliver(Audience::Owner(uid), dummy_event(), None);
        assert_eq!(reg.prune_closed(), 0, "a second sweep reclaims nothing");
    }

    /// TEST-2 [acceptance INV-3] — the cap is charged for LIVE connections only.
    /// Two halves in ONE test so a regression in either direction fails:
    /// raising/removing the cap fails the first half; removing the sweep fails
    /// the second.
    #[test]
    fn per_user_cap_counts_live_connections_only() {
        // (a) cap value unchanged: with every receiver ALIVE the (cap+1)th
        //     registration is still refused with 429.
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        let mut alive = Vec::new();
        for _ in 0..PER_USER_MAX_CONNECTIONS {
            let (c, rx) = conn(uid, principal(false, vec![]));
            reg.register(Uuid::new_v4(), c).unwrap();
            alive.push(rx); // hold the stream open
        }
        let (overflow, _rx) = conn(uid, principal(false, vec![]));
        let err = reg
            .register(Uuid::new_v4(), overflow)
            .expect_err("the cap must still refuse a (cap+1)th LIVE connection");
        assert_eq!(err.status_code(), 429, "the per-user cap still surfaces 429");
        assert_eq!(reg.connection_count(), PER_USER_MAX_CONNECTIONS);

        // (b) the same user's streams all go away → the next registration
        //     succeeds, and the registry holds exactly the one new connection.
        drop(alive);
        let (fresh, _rx) = conn(uid, principal(false, vec![]));
        reg.register(Uuid::new_v4(), fresh)
            .expect("a user whose streams are gone must be able to reconnect");
        assert_eq!(
            reg.connection_count(),
            1,
            "all {PER_USER_MAX_CONNECTIONS} dead slots must be reclaimed, leaving only the new one",
        );
    }

    /// TEST-2b [acceptance INV-3] — the GLOBAL cap likewise counts live
    /// connections only: a full registry of dead connections does not lock the
    /// whole deployment out, while a full registry of LIVE ones still 429s.
    #[test]
    fn global_cap_counts_live_connections_only() {
        let reg = empty_registry();
        let mut alive = Vec::new();
        for _ in 0..GLOBAL_MAX_CONNECTIONS {
            let (c, rx) = conn(Uuid::new_v4(), principal(false, vec![]));
            reg.register(Uuid::new_v4(), c).unwrap();
            alive.push(rx);
        }
        let (overflow, _rx) = conn(Uuid::new_v4(), principal(false, vec![]));
        assert_eq!(
            reg.register(Uuid::new_v4(), overflow)
                .expect_err("a FULL registry of live connections still 429s")
                .status_code(),
            429,
        );

        drop(alive);
        let (fresh, _rx) = conn(Uuid::new_v4(), principal(false, vec![]));
        reg.register(Uuid::new_v4(), fresh)
            .expect("a registry full of DEAD connections must not lock everyone out");
        assert_eq!(reg.connection_count(), 1);
    }

    /// TEST-3 — a user-scoped sweep touches ONLY that user: another user's dead
    /// connection survives it (it is reclaimed by that user's own next
    /// registration, or a global sweep), and the target user's LIVE connection
    /// survives it too.
    #[test]
    fn prune_closed_for_user_is_scoped_to_that_user() {
        let reg = empty_registry();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        let (a_dead, a_rx_dead) = conn(a, principal(false, vec![]));
        let (a_live, _a_rx_live) = conn(a, principal(false, vec![]));
        let (b_dead, b_rx_dead) = conn(b, principal(false, vec![]));
        reg.register(Uuid::new_v4(), a_dead).unwrap();
        reg.register(Uuid::new_v4(), a_live).unwrap();
        reg.register(Uuid::new_v4(), b_dead).unwrap();
        drop(a_rx_dead);
        drop(b_rx_dead);
        assert_eq!(reg.connection_count(), 3);

        assert_eq!(
            reg.prune_closed_for_user(a),
            1,
            "only user A's ONE dead connection is reclaimed"
        );
        assert_eq!(
            reg.connection_count(),
            2,
            "A's live connection and B's dead connection both survive an A-scoped sweep"
        );

        // A global sweep then reclaims B's.
        assert_eq!(reg.prune_closed(), 1);
        assert_eq!(reg.connection_count(), 1);
    }

    /// TEST-14 — the deadline backstop: a connection whose stream is STILL
    /// ALIVE (its receiver held open — the peer vanished without the socket
    /// ever erroring, so hyper never noticed) is nevertheless reclaimed once it
    /// is past the `exp` deadline its own stream was bound to. A live
    /// connection whose deadline is in the FUTURE, and one with no deadline at
    /// all, are both left alone.
    #[test]
    fn prune_reclaims_a_connection_past_its_own_exp_deadline_even_if_alive() {
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        let now = std::time::Instant::now();

        let (expired, _expired_rx) = conn_expiring(
            uid,
            Some(now - DEADLINE_SLACK - std::time::Duration::from_secs(30)),
        );
        let (future, _future_rx) =
            conn_expiring(uid, Some(now + std::time::Duration::from_secs(3600)));
        let (unbounded, _unbounded_rx) = conn(uid, principal(false, vec![]));
        reg.register(Uuid::new_v4(), expired).unwrap();
        reg.register(Uuid::new_v4(), future).unwrap();
        reg.register(Uuid::new_v4(), unbounded).unwrap();
        assert_eq!(reg.connection_count(), 3);

        // Every receiver is still held, so `sender.is_closed()` is false for
        // all three: ONLY the deadline signal can distinguish them.
        assert_eq!(
            reg.prune_closed(),
            1,
            "only the past-deadline connection is reclaimed"
        );
        assert_eq!(
            reg.connection_count(),
            2,
            "the future-deadline and no-deadline connections both survive"
        );
    }

    /// A connection barely past its deadline (inside the slack window) is NOT
    /// reclaimed — the slack exists so a normal teardown is never raced.
    #[test]
    fn prune_respects_the_deadline_slack_window() {
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        let just_lapsed = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let (c, _rx) = conn_expiring(uid, Some(just_lapsed));
        reg.register(Uuid::new_v4(), c).unwrap();

        assert_eq!(
            reg.prune_closed(),
            0,
            "a connection whose deadline just lapsed is still inside the slack window"
        );
        assert_eq!(reg.connection_count(), 1);
    }

    /// A connection whose stream was bound to end at `expires_at` — exercises
    /// the deadline staleness backstop.
    fn conn_expiring(
        user_id: Uuid,
        expires_at: Option<std::time::Instant>,
    ) -> (ClientConn<TestPrincipal>, Rx) {
        let (tx, rx) = tokio::sync::mpsc::channel(SYNC_CHANNEL_CAPACITY);
        let c = ClientConn {
            user_id,
            principal: principal(false, vec![]),
            sender: tx,
            expires_at,
        };
        (c, rx)
    }

    /// TEST-9 — a sweep NEVER reclaims a live connection. Asserted on both the
    /// count AND on the connection still being functional afterwards (a
    /// connection left in the map but broken would pass a count-only check).
    #[test]
    fn prune_closed_never_removes_a_live_connection() {
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        let (c, mut rx) = conn(uid, principal(false, vec![]));
        reg.register(Uuid::new_v4(), c).unwrap();

        assert_eq!(reg.prune_closed(), 0, "a live connection is never reclaimed");
        assert_eq!(reg.prune_closed_for_user(uid), 0);
        assert_eq!(reg.connection_count(), 1);

        // Still functional, not merely still counted.
        reg.deliver(Audience::Owner(uid), dummy_event(), None);
        assert!(got(&mut rx), "the surviving connection still receives events");
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
