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

/// Shipped default for the site-wide cap on concurrent sync SSE connections
/// across all users (C0, chat epic): 8× the pre-C0 constant (512), 3.4× the
/// chat boards' drawn "1 204 here" scale, with the measured 10 000-connection
/// frontier ~2.5× farther away (C0 evidence: p50 0.99 ms / p99 4.85 ms per
/// `Audience::Everyone` publish at 10k, ~1.3 KiB idle per connection, no cliff
/// in the measured range).
pub const DEFAULT_GLOBAL_MAX_CONNECTIONS: usize = 4096;
/// Shipped default for the per-user cap (multiple tabs/devices). Bounds a
/// single account from exhausting the global pool. Kept at the pre-C0 value
/// 12: C0's measurement subject is the global pool, and no evidence names a
/// problem with the tab/multi-device multiplier bound.
pub const DEFAULT_PER_USER_MAX_CONNECTIONS: usize = 12;
/// Bounded per-connection queue depth. A reader that falls this far behind is
/// treated as stalled: the connection is dropped (so the client reconnects +
/// resyncs) rather than buffering unbounded memory. Sized generously so a
/// normal burst never trips it.
pub const SYNC_CHANNEL_CAPACITY: usize = 1024;

type ConnId = Uuid;

/// The registry's connection caps, per deployment. The whole-site cap is a
/// deploy-config value (comic: `AppConfig.sync.max_connections`, pushed into
/// [`SyncRegistry::new_with_config`]); [`Default`] is the evidence-sized
/// site-wide default. The per-user cap is deliberately part of the config
/// object — the registry is the thing that enforces it — but apps expose only
/// the global knob (C0 DEC-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncRegistryConfig {
    /// Site-wide cap on concurrent sync SSE connections across ALL users.
    pub global_max_connections: usize,
    /// Cap per user (multiple tabs/devices).
    pub per_user_max_connections: usize,
}

impl Default for SyncRegistryConfig {
    fn default() -> Self {
        Self {
            global_max_connections: DEFAULT_GLOBAL_MAX_CONNECTIONS,
            per_user_max_connections: DEFAULT_PER_USER_MAX_CONNECTIONS,
        }
    }
}

impl SyncRegistryConfig {
    /// Refuse the cap values that silently disable the surface instead of
    /// sizing it: a `0` on either slot makes `register` refuse EVERY
    /// connection (`len >= 0`), i.e. realtime sync dead site-wide with no
    /// error anywhere — the exact footgun the app's config validation exists
    /// to prevent (C0, chat epic). `per_user > global` is NOT refused:
    /// the cap-check ORDER contract (global first) means the global arm
    /// answers once the pool saturates and the per-user arm is only dead
    /// weight. Debug-only assertion: a constructor precondition, not a
    /// runtime condition — `new_with_config` is the one construction site and
    /// its caller is an app developer, so a panic-in-tests (debug) surfaces
    /// the misconfiguration where it can be seen, while release keeps the
    /// plain constructor (the app's own config load is the fail-loud boundary
    /// there).
    pub fn validate(&self) {
        debug_assert!(
            self.global_max_connections > 0 && self.per_user_max_connections > 0,
            "SyncRegistryConfig: a zero cap would refuse every connection \
             (sync dead site-wide, silently) — set a positive cap or omit the \
             config (Default)"
        );
    }
}

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
    config: SyncRegistryConfig,
}

impl<P: Principal + Send + 'static> Default for SyncRegistry<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Principal + Send + 'static> SyncRegistry<P> {
    /// Construct an empty registry with the shipped-default caps. The app wraps
    /// ONE of these in a process-wide singleton (ziee: a `lazy_static`) keyed
    /// to its concrete principal type. Keeps its no-arg signature — a consumer
    /// that never configures the registry compiles unchanged against a pin
    /// bump; the only semantic difference is the default's VALUE (an increase
    /// in the site-wide cap, never a narrowing).
    pub fn new() -> Self {
        Self::new_with_config(SyncRegistryConfig::default())
    }

    /// Construct an empty registry with the given caps. The intended source is
    /// the app's deploy config (comic: `AppConfig.sync.max_connections`); an
    /// app that never calls this behaves exactly as before, per the config's
    /// `Default`. A zero cap on either slot is a programming error (it would
    /// refuse every connection, silently) — refused by
    /// [`SyncRegistryConfig::validate`] in debug, and the caller's own config
    /// load is the release boundary.
    pub fn new_with_config(config: SyncRegistryConfig) -> Self {
        config.validate();
        SyncRegistry {
            inner: Mutex::new(RegistryInner {
                clients: HashMap::new(),
                by_user: HashMap::new(),
            }),
            config,
        }
    }

    /// The caps this registry enforces (test/diagnostic helper).
    #[cfg(test)]
    pub fn config(&self) -> SyncRegistryConfig {
        self.config
    }

    /// Register a new connection. Returns a 429 `AppError` when a global
    /// or per-user connection cap is hit.
    ///
    /// A cap is only ever charged for connections that are still ALIVE: before
    /// each cap check the corresponding set is swept of connections whose
    /// stream is gone (`prune_closed_locked`). Without that sweep a user whose
    /// slots leaked would be refused forever. The primary release path is the
    /// subscribe handler's connection guard (`unregister` on drop), which since
    /// the guard was hoisted out of the stream generator covers EVERY
    /// termination including a never-polled stream; this sweep is the backstop
    /// for any future path that loses the guard. The remaining reapers
    /// (`deliver`'s and `deliver_session_to_users`' send-failure prunes) need an
    /// event to deliver, so they reclaim nothing on a quiescent deployment.
    ///
    /// The sweep is deliberately on-demand rather than a background reaper: a
    /// stale slot's user-visible effect is a cap refusal, and the caps are
    /// evaluated exclusively here (DEC-4).
    ///
    /// **Cap-check ORDER is part of the contract**: global first, then
    /// per-user, so a saturated deployment reports `SYNC_GLOBAL_LIMIT` (a
    /// capacity incident) rather than masking it as `SYNC_USER_LIMIT` (a
    /// per-account problem) for a user who happens to also be at their own cap.
    /// Each cap sweeps its own scope only when it would otherwise trip, so the
    /// common path does no extra work at all.
    ///
    /// The caps are the registry's stored [`SyncRegistryConfig`] (the
    /// deploy-config values), never package constants — the whole point of the
    /// config is that `register`'s only behavior change is WHERE the cap count
    /// comes from.
    pub fn register(&self, conn_id: ConnId, conn: ClientConn<P>) -> Result<(), AppError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        if inner.clients.len() >= self.config.global_max_connections {
            prune_closed_locked(&mut inner);
        }
        if inner.clients.len() >= self.config.global_max_connections {
            return Err(AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "SYNC_GLOBAL_LIMIT",
                "Realtime sync is at capacity; retry shortly",
            ));
        }

        let mut user_count = inner.by_user.get(&conn.user_id).map_or(0, |s| s.len());
        if user_count >= self.config.per_user_max_connections {
            prune_closed_for_user_locked(&mut inner, conn.user_id);
            user_count = inner.by_user.get(&conn.user_id).map_or(0, |s| s.len());
        }
        if user_count >= self.config.per_user_max_connections {
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

    /// Sweep every connection whose stream is gone, returning how many were
    /// freed.
    ///
    /// The liveness signal is `sender.is_closed()`, and it is the ONLY one used
    /// deliberately. Each connection's `Receiver` is owned solely by its own SSE
    /// stream, so a closed sender means exactly "that stream no longer exists" —
    /// there is nothing left to bound. An idle-but-live stream (the normal state
    /// of a sync connection on a quiet deployment) is never touched.
    ///
    /// This is the BACKSTOP. The primary, deterministic release is the
    /// subscribe handler's connection guard, which is constructed at
    /// registration and moved into the stream so it is dropped even if the
    /// stream is never polled.
    ///
    /// **Load-bearing invariant**: the registry holds the SOLE surviving
    /// `Sender` clone (the handler's own `tx` drops when the handler returns,
    /// and `rx` lives inside the stream). That is what makes `remove_conn`
    /// terminate the stream rather than orphan it — dropping the sender closes
    /// the channel, `rx.recv()` yields `None` and the loop breaks. Do not
    /// introduce a second long-lived `Sender` clone.
    #[cfg(test)]
    pub fn prune_closed(&self) -> usize {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        prune_closed_locked(&mut inner)
    }

    /// Sweep only `user_id`'s dead connections (same signal as
    /// [`Self::prune_closed`]), returning how many were freed. Other users'
    /// connections are never inspected or touched.
    ///
    /// `#[cfg(test)]` like its sibling: on the production path the sweep runs
    /// inside `register`, which already holds the lock and therefore calls
    /// `prune_closed_for_user_locked` directly. Shipping a lock-taking wrapper
    /// with no production caller would be dead API (§15).
    #[cfg(test)]
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

/// Sweep every connection whose stream is gone. Caller holds the lock.
///
/// **Liveness signal — `sender.is_closed()`, and deliberately only that.** Each
/// connection's `Receiver` is owned solely by its own SSE stream, so a closed
/// sender means exactly "that stream no longer exists": there is nothing left to
/// bound. An idle-but-live stream (the normal state of a connection on a quiet
/// deployment) is never touched.
///
/// **Deliberately NOT reclaimed: a connection that is merely OLD.** An earlier
/// revision also reaped connections past their token's `exp`, to cover a peer
/// that vanished without the socket ever erroring. That is wrong: in exactly
/// that case the stream future is not being polled (which is *why* it outlived
/// its own deadline), so dropping the registry entry frees the accounting slot
/// while the stream future, its channel, its tokio task and its socket all
/// survive. The cap would then bound bookkeeping rather than real resources,
/// letting a client accumulate server-side connections *past* the cap — trading
/// a fail-closed bound for unbounded growth. A backpressured-but-healthy stream
/// (suspended at `yield`, or inside the periodic re-check's DB await) is also
/// past its deadline and would be reaped. That case stays bounded the honest
/// way: axum's keep-alive writes eventually fail on a dead peer, hyper drops the
/// body, and the connection guard fires.
///
/// **Load-bearing invariant**: the registry holds the SOLE surviving `Sender`
/// clone (the handler's own `tx` drops when the handler returns, and `rx` lives
/// inside the stream). That is what makes `remove_conn` TERMINATE the stream
/// rather than orphan it — dropping the sender closes the channel, `rx.recv()`
/// yields `None`, and the loop breaks. Do not introduce a second long-lived
/// `Sender` clone.
///
/// Collect-then-remove so the two-index invariant is maintained by the single
/// `remove_conn` helper.
fn prune_closed_locked<P: Principal>(inner: &mut RegistryInner<P>) -> usize {
    let dead: Vec<ConnId> = inner
        .clients
        .iter()
        .filter(|(_, c)| c.sender.is_closed())
        .map(|(id, _)| *id)
        .collect();
    let n = dead.len();
    for cid in dead {
        remove_conn(inner, cid);
    }
    if n > 0 {
        tracing::debug!("sync registry: reclaimed {n} dead connection(s)");
    }
    n
}

/// Sweep only `user_id`'s dead connections. Caller holds the lock.
///
/// An id present in `by_user` but MISSING from `clients` is treated as dead too.
/// `register` and `remove_conn` keep the two indexes in lockstep under one lock,
/// so that desync should be unreachable — but `user_count` is derived from
/// `by_user`, so an orphan would count against the per-user cap forever and
/// NEITHER sweep could clear it (this one would filter it out, the global one
/// iterates `clients`). That is the permanently-429'd account this module exists
/// to prevent, so the sweep repairs it rather than skipping it.
fn prune_closed_for_user_locked<P: Principal>(
    inner: &mut RegistryInner<P>,
    user_id: Uuid,
) -> usize {
    let Some(set) = inner.by_user.get(&user_id) else {
        return 0;
    };
    let dead: Vec<ConnId> = set
        .iter()
        .filter(|cid| {
            // Missing from `clients` => an orphaned index entry; see the doc
            // comment. `is_none_or` makes the sweep repair it.
            inner.clients.get(*cid).is_none_or(|c| c.sender.is_closed())
        })
        .copied()
        .collect();
    let n = dead.len();
    for cid in dead {
        // `remove_conn` is keyed off `clients`: it no-ops entirely when the row
        // is missing (an orphan), and when `clients[cid].user_id` disagrees with
        // `user_id` it would clean the OTHER user's index. So drop THIS user's
        // index entry unconditionally as well — that is what guarantees the id
        // stops counting against `user_id`'s cap for every shape that REACHES
        // this loop (normal-dead, orphan, cross-user-dead). One shape cannot
        // reach it: an id in `by_user[user_id]` whose `clients` row is a LIVE
        // connection of a DIFFERENT user is filtered out of `dead` by design
        // (nothing live is ever swept), so it would keep counting. That desync
        // is unreachable — `register` and `remove_conn` are the only writers and
        // both move the two indexes together under one lock.
        remove_conn(inner, cid);
        if let Some(set) = inner.by_user.get_mut(&user_id) {
            set.remove(&cid);
            if set.is_empty() {
                inner.by_user.remove(&user_id);
            }
        }
    }
    if n > 0 {
        tracing::debug!("sync registry: reclaimed {n} dead connection(s) for one user");
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
        // The receivers must be HELD: a cap is charged for live connections
        // only, so a per-iteration `_rx` binding (dropped at the end of each
        // iteration) would register connections whose streams are already gone
        // and `register`'s sweep would rightly reclaim them.
        // The caps are `SyncRegistryConfig::default()` — `new()` builds the
        // default, and the default is what these regression tests pin.
        let per_user = SyncRegistryConfig::default().per_user_max_connections;
        let mut alive = Vec::new();
        for _ in 0..per_user {
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
        // Fill the registry to the DEFAULT global cap with one connection per
        // distinct user, so the GLOBAL cap (not the per-user cap) is what trips.
        // `new()` builds `SyncRegistryConfig::default()`; the default is what
        // this regression pins.
        let global = SyncRegistryConfig::default().global_max_connections;
        let reg = empty_registry();
        // Receivers HELD — see `per_user_cap_rejects_excess_connections`: the
        // cap counts live connections only.
        let mut alive = Vec::new();
        for _ in 0..global {
            let (c, rx) = conn(Uuid::new_v4(), principal(false, vec![]));
            reg.register(Uuid::new_v4(), c).unwrap();
            alive.push(rx);
        }
        assert_eq!(reg.connection_count(), global);

        // The (global cap + 1)th connection — a brand-new user well under the
        // per-user cap — must be refused with a 429.
        let (overflow, _rx) = conn(Uuid::new_v4(), principal(false, vec![]));
        let err = reg
            .register(Uuid::new_v4(), overflow)
            .expect_err("global cap must reject the (cap+1)th connection");
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
    // COVERAGE NOTE: the `prune_*` tests below drive the sweep helpers in
    // isolation — they do NOT prove the sweep is wired into production. What
    // proves the wiring is the cap tests (`*_cap_counts_live_connections_only`),
    // which go through `register` and fail if its sweep calls are removed.
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
        let per_user = SyncRegistryConfig::default().per_user_max_connections;
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        let mut alive = Vec::new();
        for _ in 0..per_user {
            let (c, rx) = conn(uid, principal(false, vec![]));
            reg.register(Uuid::new_v4(), c).unwrap();
            alive.push(rx); // hold the stream open
        }
        let (overflow, _rx) = conn(uid, principal(false, vec![]));
        let err = reg
            .register(Uuid::new_v4(), overflow)
            .expect_err("the cap must still refuse a (cap+1)th LIVE connection");
        assert_eq!(err.status_code(), 429, "the per-user cap still surfaces 429");
        assert_eq!(reg.connection_count(), per_user);

        // (b) the same user's streams all go away → the next registration
        //     succeeds, and the registry holds exactly the one new connection.
        drop(alive);
        let (fresh, _rx) = conn(uid, principal(false, vec![]));
        reg.register(Uuid::new_v4(), fresh)
            .expect("a user whose streams are gone must be able to reconnect");
        assert_eq!(
            reg.connection_count(),
            1,
            "all {per_user} dead slots must be reclaimed, leaving only the new one",
        );
    }

    /// TEST-2b [acceptance INV-3] — the GLOBAL cap likewise counts live
    /// connections only: a full registry of dead connections does not lock the
    /// whole deployment out, while a full registry of LIVE ones still 429s.
    #[test]
    fn global_cap_counts_live_connections_only() {
        let global = SyncRegistryConfig::default().global_max_connections;
        let reg = empty_registry();
        let mut alive = Vec::new();
        for _ in 0..global {
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

    /// TEST-21 — the user-scoped sweep REPAIRS an orphaned index entry (an id
    /// in `by_user` with no row in `clients`) rather than skipping it.
    ///
    /// `user_count` is derived from `by_user`, so an orphan would count against
    /// the per-user cap forever and neither sweep could clear it — this sweep
    /// would filter it out, and the global sweep iterates `clients`, where it
    /// does not exist. That is precisely the permanently-429'd account this
    /// module exists to prevent. The desync is unreachable today (both indexes
    /// move together under one lock), which is exactly why it needs a test: the
    /// repair is defensive code with no natural trigger.
    #[test]
    fn prune_closed_for_user_repairs_an_orphaned_index_entry() {
        let reg = empty_registry();
        let uid = Uuid::new_v4();
        let orphan = Uuid::new_v4();

        // Forge the desync: an id in `by_user` with no `clients` row.
        {
            let mut inner = reg.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.by_user.entry(uid).or_default().insert(orphan);
        }

        assert_eq!(
            reg.prune_closed_for_user(uid),
            1,
            "the orphaned index entry must be counted as reclaimed"
        );
        {
            let inner = reg.inner.lock().unwrap_or_else(|e| e.into_inner());
            assert!(
                inner.by_user.get(&uid).is_none(),
                "the orphan must actually be REMOVED from by_user (an emptied \
                 user entry is dropped), otherwise it counts against the cap \
                 forever"
            );
        }

        // The other branch: an orphan ALONGSIDE a live connection. The user's
        // index entry must SHRINK (not vanish), the live connection must be
        // untouched, and `clients` must be left alone.
        let (live, _live_rx) = conn(uid, principal(false, vec![]));
        let live_id = Uuid::new_v4();
        reg.register(live_id, live).unwrap();
        let orphan2 = Uuid::new_v4();
        {
            let mut inner = reg.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.by_user.entry(uid).or_default().insert(orphan2);
        }

        assert_eq!(reg.prune_closed_for_user(uid), 1, "only the orphan is freed");
        let inner = reg.inner.lock().unwrap_or_else(|e| e.into_inner());
        let set = inner
            .by_user
            .get(&uid)
            .expect("the user entry survives because a LIVE connection remains");
        assert_eq!(set.len(), 1, "the index shrank to just the live connection");
        assert!(set.contains(&live_id));
        assert_eq!(inner.clients.len(), 1, "`clients` is left untouched");
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

    // ---- C0: configurable caps (chat epic; the whole-site SSE cap becomes a
    // ---- deploy-config value) ---------------------------------------------

    /// TEST-1 [acceptance INV-2/INV-4/INV-5] — the shipped defaults, the
    /// config object's storage, and `new()` ≡ `new_with_config(Default)`.
    #[test]
    fn defaults_and_new_with_config_are_equivalent() {
        let d = SyncRegistryConfig::default();
        assert_eq!(
            d.global_max_connections,
            4096,
            "INV-4: the default is the evidence-sized 4096 (measured limit 10 \
             000 — C0/evidence/MEASUREMENTS.md; 8× the pre-C0 512, 3.4× the \
             chat boards' drawn '1 204 here')"
        );
        assert_eq!(
            d.per_user_max_connections,
            12,
            "INV-2: the per-user default stays 12 (C0's evidence names no \
             problem with the tab/multi-device bound)"
        );

        // INV-5: the no-arg ctor keeps its signature (ziee/cytoanalyst compile
        // unchanged against a pin bump) and builds the default; the config is
        // stored and readable.
        let reg_new = empty_registry(); // SyncRegistry::new()
        assert_eq!(reg_new.config(), d, "new() builds the config's Default");
        let reg_cfg = SyncRegistry::<TestPrincipal>::new_with_config(d);
        assert_eq!(reg_cfg.config(), d, "new_with_config stores the config");

        // ...and both enforce identically at the default per-user cap.
        for reg in [&reg_new, &reg_cfg] {
            let uid = Uuid::new_v4();
            let mut alive = Vec::new();
            for _ in 0..d.per_user_max_connections {
                let (c, rx) = conn(uid, principal(false, vec![]));
                reg.register(Uuid::new_v4(), c).unwrap();
                alive.push(rx);
            }
            let (overflow, _rx) = conn(uid, principal(false, vec![]));
            let err = reg.register(Uuid::new_v4(), overflow).unwrap_err();
            assert_eq!(
                err.error_code(),
                "SYNC_USER_LIMIT",
                "new() and new_with_config(Default) enforce identically"
            );
            drop(alive);
        }
    }

    /// A zero cap (either slot) is a programming error — it would refuse
    /// every connection. `SyncRegistryConfig::validate` is a debug-only
    /// constructor assertion (F-A2 round-1 fix), so this test only runs in
    /// debug builds, which is the entire point.
    #[test]
    #[should_panic(expected = "a zero cap")]
    fn zero_cap_is_refused_to_build() {
        let _ = SyncRegistry::<TestPrincipal>::new_with_config(SyncRegistryConfig {
            global_max_connections: 0,
            per_user_max_connections: 12,
        });
    }

    /// TEST-2 [acceptance INV-3] — the CONFIGURED cap is the one enforced: a
    /// registry built with `{global: 7, per_user: 3}` refuses the 8th global
    /// and the 4th per-user connection with the UNCHANGED 429 vocabulary
    /// (INV-3), and the sweep-then-refuse arms still free a slot after
    /// `unregister`.
    #[test]
    fn configured_caps_are_the_enforced_caps() {        let cfg = SyncRegistryConfig {
            global_max_connections: 7,
            per_user_max_connections: 3,
        };

        // Global arm: 7 distinct users fill the pool; the 8th is refused with
        // the unchanged SYNC_GLOBAL_LIMIT code.
        let reg = SyncRegistry::<TestPrincipal>::new_with_config(cfg);
        let mut alive = Vec::new();
        for _ in 0..cfg.global_max_connections {
            let (c, rx) = conn(Uuid::new_v4(), principal(false, vec![]));
            reg.register(Uuid::new_v4(), c).unwrap();
            alive.push(rx);
        }
        let (overflow, _rx) = conn(Uuid::new_v4(), principal(false, vec![]));
        let err = reg.register(Uuid::new_v4(), overflow).unwrap_err();
        assert_eq!(err.status_code(), 429, "the configured cap surfaces 429");
        assert_eq!(
            err.error_code(),
            "SYNC_GLOBAL_LIMIT",
            "the configured cap refuses with the unchanged vocabulary (INV-3)"
        );

        // Per-user arm in a fresh registry (global not saturated): one user's
        // (cap+1)th is refused with the unchanged SYNC_USER_LIMIT code.
        let reg2 = SyncRegistry::<TestPrincipal>::new_with_config(cfg);
        let uid = Uuid::new_v4();
        let mut alive2 = Vec::new();
        let mut ids = Vec::new();
        for _ in 0..cfg.per_user_max_connections {
            let (c, rx) = conn(uid, principal(false, vec![]));
            let id = Uuid::new_v4();
            reg2.register(id, c).unwrap();
            ids.push(id);
            alive2.push(rx);
        }
        let (overflow2, _rx) = conn(uid, principal(false, vec![]));
        let err2 = reg2.register(Uuid::new_v4(), overflow2).unwrap_err();
        assert_eq!(err2.status_code(), 429);
        assert_eq!(
            err2.error_code(),
            "SYNC_USER_LIMIT",
            "the configured per-user cap refuses with the unchanged vocabulary (INV-3)"
        );

        // The sweep-then-refuse arms still run: an unregistered slot is free.
        reg2.unregister(ids[0]);
        let (fresh, fresh_rx) = conn(uid, principal(false, vec![]));
        reg2.register(Uuid::new_v4(), fresh)
            .expect("a slot freed by unregister is reusable under the configured cap");
        drop((alive, alive2, fresh_rx));
    }

    /// TEST-3 [acceptance INV-1] — the cap-check ORDER contract: global first,
    /// so a user at BOTH caps is refused with `SYNC_GLOBAL_LIMIT` (the
    /// saturated-deployment code), never `SYNC_USER_LIMIT`; and the per-user
    /// arm is reachable only while the global pool is not saturated.
    #[test]
    fn cap_check_order_is_global_first_then_per_user() {
        let cfg = SyncRegistryConfig {
            global_max_connections: 3,
            per_user_max_connections: 2,
        };
        let reg = SyncRegistry::<TestPrincipal>::new_with_config(cfg);
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        // A holds 2 (at THEIR cap); B holds 1 → the GLOBAL pool is full (3).
        let mut alive = Vec::new();
        for uid in [a, a, b] {
            let (c, rx) = conn(uid, principal(false, vec![]));
            reg.register(Uuid::new_v4(), c).unwrap();
            alive.push(rx);
        }

        // A's 3rd: BOTH caps would refuse it — the global one must win.
        let (overflow, _rx) = conn(a, principal(false, vec![]));
        let err = reg.register(Uuid::new_v4(), overflow).unwrap_err();
        assert_eq!(
            err.error_code(),
            "SYNC_GLOBAL_LIMIT",
            "a saturated deployment reports SYNC_GLOBAL_LIMIT even for a user \
             at their own cap (INV-1: order contract, registry.rs register)"
        );

        // The per-user arm is reachable when the global pool is NOT saturated.
        let reg2 = SyncRegistry::<TestPrincipal>::new_with_config(cfg);
        let mut alive2 = Vec::new();
        for _ in 0..cfg.per_user_max_connections {
            let (c, rx) = conn(a, principal(false, vec![]));
            reg2.register(Uuid::new_v4(), c).unwrap();
            alive2.push(rx);
        }
        let (overflow2, _rx) = conn(a, principal(false, vec![]));
        let err2 = reg2.register(Uuid::new_v4(), overflow2).unwrap_err();
        assert_eq!(
            err2.error_code(),
            "SYNC_USER_LIMIT",
            "the per-user arm is reachable only when the global pool has room"
        );
        drop((alive, alive2));
    }

    /// TEST-4 [acceptance INV-4] — the measured-frontier bound holds: a
    /// registry built at the measured 10 000-connection limit keeps the stated
    /// fan-out bound (p50 ≤ 25 ms per `Audience::Everyone` publish; the probe
    /// measured 0.99 ms p50 on this host — 25× headroom, so the bound is
    /// non-flaky) and every live connection's queue received the event.
    #[test]
    fn fan_out_to_ten_thousand_connections_meets_the_stated_bound() {
        let cfg = SyncRegistryConfig {
            global_max_connections: 10_000,
            per_user_max_connections: 10_000,
        };
        let reg = SyncRegistry::<TestPrincipal>::new_with_config(cfg);
        let mut rxs = Vec::new();
        for _ in 0..cfg.global_max_connections {
            let (c, rx) = conn(Uuid::new_v4(), principal(false, vec![]));
            reg.register(Uuid::new_v4(), c).unwrap();
            rxs.push(rx);
        }
        assert_eq!(reg.connection_count(), 10_000);

        let mut runs = Vec::new();
        for _ in 0..100 {
            let t0 = std::time::Instant::now();
            reg.deliver(Audience::Everyone, dummy_event(), None);
            runs.push(t0.elapsed());
        }
        runs.sort();
        let p50 = runs[49]; // 100 samples → the 50th
        // Observed margin is recorded at run time (a loaded shared box makes a
        // hardcoded "25× headroom" claim stale; the debug profile is 5-15×
        // slower than the probe's release measurement, so the true debug
        // margin is a few multiples, not 25 — still far above the bound).
        eprintln!("sync fan-out: 100 × Everyone delivers at 10k — p50 {p50:?} (bound 25 ms)");
        assert!(
            p50 <= std::time::Duration::from_millis(25),
            "the stated bound: p50 ≤ 25 ms (measured ~1 ms p50 in release by the \
             C0 probe; debug is slower) — got {p50:?}"
        );

        // Every connection's queue received the event (none lost, none pruned).
        let mut received = 0;
        for rx in &mut rxs {
            if got(rx) {
                received += 1;
            }
        }
        assert_eq!(
            received,
            10_000,
            "every live connection's queue received the fan-out event"
        );
    }
}
