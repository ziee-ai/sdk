//! Injected seams for the auth (+ co-located user) module.
//!
//! The auth module used to reach a set of app-global singletons directly (the
//! global repository aggregator, the in-process event bus, the sync-publish
//! functions, the at-rest secret key, and the SSRF URL-validator helpers).
//! Chunk BG inverted that: the auth module depends on the **abstractions
//! declared here** (`AuthEventSink`, `AuthSyncSink`) and takes a per-request
//! [`AuthContext`] handle carrying a `PgPool` + those sinks. The APP installs
//! concrete implementations once at boot and layers a single
//! `Extension<AuthContext>` onto the router.
//!
//! Chunk BA-full moved this module into `ziee-auth`. Because `ziee-auth` must
//! not name the app's concrete `SyncEntity` / `SyncAction` enums (they derive
//! `JsonSchema` for the OpenAPI codegen contract and stay app-side), the
//! `AuthSyncSink` trait speaks in crate-local [`AuthSyncEntity`] /
//! [`AuthSyncAction`] abstractions; the app-side sink impl maps them to the
//! real `SyncEntity` / `SyncAction` before the real `publish`. Behaviour is
//! byte-identical — same events, same audiences; only the coupling direction
//! and the abstract enum names changed.

use std::sync::{Arc, OnceLock};

use sqlx::PgPool;
use uuid::Uuid;
use ziee_framework::sync::Audience;

use crate::auth::providers::events::AuthProviderEvent;
use crate::auth::{AuthRepository, SessionSettingsRepository};
use crate::user::events::UserEvent;
use crate::user::{GroupRepository, UserRepository};

/// The auth-domain sync entities, abstract over the app's concrete
/// `SyncEntity`. The app-side `AuthSyncSink` impl maps each variant to the
/// real `SyncEntity::{User, Group, Profile, Session, SessionSettings,
/// AuthProvider}` before publishing. Keeping the concrete enum app-side
/// preserves its `JsonSchema` short-name identity (the OpenAPI codegen
/// contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSyncEntity {
    User,
    Group,
    Profile,
    Session,
    SessionSettings,
    AuthProvider,
}

/// The auth-domain sync actions, abstract over the app's concrete
/// `SyncAction`. Mapped app-side to `SyncAction::{Create, Update, Delete}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSyncAction {
    Create,
    Update,
    Delete,
}

/// Emit in-process domain events. Wired app-side to the real event bus
/// (the impl wraps each module event into the app-aggregate event enum).
pub trait AuthEventSink: Send + Sync {
    /// Fire a user-lifecycle event (created / updated / deleted).
    fn emit_user(&self, ev: UserEvent);
    /// Fire an auth-provider-lifecycle event.
    fn emit_auth_provider(&self, ev: AuthProviderEvent);
}

/// Publish cross-device sync notifications. Wired app-side to
/// `crate::modules::sync::{publish, publish_session_to_users}` — the auth
/// module no longer names the global sync-publish functions. It speaks in the
/// crate-local [`AuthSyncEntity`] / [`AuthSyncAction`] abstractions; the app
/// impl maps them to the concrete `SyncEntity` / `SyncAction`.
pub trait AuthSyncSink: Send + Sync {
    /// Notify-and-refetch for a single entity to the chosen audience.
    fn publish(
        &self,
        entity: AuthSyncEntity,
        action: AuthSyncAction,
        id: Uuid,
        audience: Audience,
        origin: Option<Uuid>,
    );
    /// Fan a `Session` permissions-changed signal out to many users at once.
    fn publish_session_to_users(&self, user_ids: &[Uuid], origin: Option<Uuid>);
}

/// Per-request dependency handle the auth + user handlers pull from
/// `Extension<AuthContext>` instead of reaching app globals. Cheaply
/// cloneable (everything behind `Arc`).
#[derive(Clone)]
pub struct AuthContext {
    pool: Arc<PgPool>,
    /// The at-rest secret storage key (app copies it from
    /// the app's at-rest secret key at install time).
    secret_key: Option<String>,
    /// Domain-event sink (app installs an event-bus-backed impl).
    pub events: Arc<dyn AuthEventSink>,
    /// Cross-device sync sink (app installs a `sync::publish`-backed impl).
    pub sync: Arc<dyn AuthSyncSink>,
}

impl AuthContext {
    /// Assemble the handle from a pool + the installed sinks. Called once at
    /// boot by the app wiring.
    pub fn new(
        pool: Arc<PgPool>,
        secret_key: Option<String>,
        events: Arc<dyn AuthEventSink>,
        sync: Arc<dyn AuthSyncSink>,
    ) -> Self {
        Self {
            pool,
            secret_key,
            events,
            sync,
        }
    }

    /// The shared connection pool (replaces `Repos.pool()`).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The at-rest secret storage key (replaces the global storage-key read).
    pub fn secret_key(&self) -> Option<&str> {
        self.secret_key.as_deref()
    }

    /// A fresh auth repository bound to the pool (replaces `Repos.auth`).
    /// Repositories are stateless pool wrappers, so per-call construction is
    /// behaviourally identical to the cached global accessor.
    pub fn auth(&self) -> AuthRepository {
        AuthRepository::new((*self.pool).clone())
    }

    /// A fresh user repository (replaces `Repos.user`).
    pub fn user(&self) -> UserRepository {
        UserRepository::new((*self.pool).clone())
    }

    /// A fresh group repository (replaces `Repos.group`).
    pub fn group(&self) -> GroupRepository {
        GroupRepository::new((*self.pool).clone())
    }

    /// A fresh session-settings repository (replaces `Repos.session_settings`).
    pub fn session_settings(&self) -> SessionSettingsRepository {
        SessionSettingsRepository::new((*self.pool).clone())
    }
}

/// A no-op [`AuthEventSink`] (chunk sdk-batteries / G1) for an app that has no
/// in-process event bus yet. Auth handlers still emit their lifecycle events;
/// this sink simply drops them. An app that later grows an event bus swaps in a
/// real impl without touching the auth surface.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopAuthEventSink;

impl AuthEventSink for NoopAuthEventSink {
    fn emit_user(&self, _ev: UserEvent) {}
    fn emit_auth_provider(&self, _ev: AuthProviderEvent) {}
}

/// A no-op [`AuthSyncSink`] (chunk sdk-batteries / G1) for an app with no
/// cross-device sync stream yet. Auth handlers still call `publish`; this sink
/// drops the notifications. Swap in a real (SSE-backed) impl later without
/// touching the auth surface.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopAuthSyncSink;

impl AuthSyncSink for NoopAuthSyncSink {
    fn publish(
        &self,
        _entity: AuthSyncEntity,
        _action: AuthSyncAction,
        _id: Uuid,
        _audience: Audience,
        _origin: Option<Uuid>,
    ) {
    }
    fn publish_session_to_users(&self, _user_ids: &[Uuid], _origin: Option<Uuid>) {}
}

// ─────────────────── the auth sync wiring DECLARATION ────────────────────
//
// `AuthContext::new` has always made the sink a parameter, so an app that
// builds its own context (the `mount_auth` path) cannot avoid naming one.
// `AuthModule` — the turnkey `MODULE_ENTRIES` path — is constructed by a fixed
// `fn() -> Box<dyn AppModule>` with no arguments, so it had nowhere to take one
// and hard-coded `NoopAuthSyncSink`. Every `ctx.sync.publish(...)` in the auth
// handlers still ran, computed its frame, and dropped it. That is invisible at
// every call site: the code reads as fully wired, the tests that exercise the
// handlers pass, and only a second device notices — by never converging.
//
// So the fix is NOT "make a sink injectable and keep the no-op default". A
// default you fall into by doing nothing is the bug. The declaration below is
// REQUIRED: `AuthModule::init` refuses to boot until the app has said which of
// the two it wants, and `Inert` carries the reason so the choice is recorded
// where a reader will find it.

/// How this process wires auth's cross-device sync notifications — a **required,
/// explicit decision**, not a defaultable option.
///
/// There is deliberately no `Default` impl and no fallback: see
/// [`AuthSyncNotDeclared`] for what `AuthModule::init` does when nothing has
/// been declared.
pub enum AuthSyncWiring {
    /// Route auth's publishes into the app's real sync fan-out. The app's impl
    /// maps [`AuthSyncEntity`] onto its own concrete `SyncEntity`.
    Live(Arc<dyn AuthSyncSink>),
    /// Deliberately drop them, with the reason recorded. Behaviourally
    /// identical to the old hard-coded [`NoopAuthSyncSink`] — the difference is
    /// that somebody chose it and said why, so the next reader finds a sentence
    /// instead of a silence.
    Inert {
        /// Why this deployment has no auth cross-device sync (e.g. "single-user
        /// desktop build: one device, nothing to converge").
        reason: &'static str,
    },
}

impl AuthSyncWiring {
    /// The sink this wiring installs.
    pub fn sink(&self) -> Arc<dyn AuthSyncSink> {
        match self {
            AuthSyncWiring::Live(sink) => Arc::clone(sink),
            AuthSyncWiring::Inert { .. } => Arc::new(NoopAuthSyncSink),
        }
    }

    /// A one-line description for logs / the already-declared warning.
    pub fn describe(&self) -> String {
        match self {
            AuthSyncWiring::Live(_) => "live (app-supplied AuthSyncSink)".to_string(),
            AuthSyncWiring::Inert { reason } => format!("inert ({reason})"),
        }
    }
}

impl std::fmt::Debug for AuthSyncWiring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe())
    }
}

/// The error [`AuthModule::init`](crate::auth::module::AuthModule) returns when
/// no [`AuthSyncWiring`] has been declared.
///
/// Its `Display` is the whole point: it names BOTH ways out, so "I didn't know
/// there was a decision here" stops being a possible state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthSyncNotDeclared;

/// The auth publishes that are dropped when the sink is inert — named in the
/// error so the cost of choosing `Inert` is concrete rather than abstract.
pub(crate) const AUTH_SYNC_PUBLISHES: &str = "Profile/Update (POST /auth/profile), SessionSettings/Update \
     (PUT /auth/admin/session-settings), Session/Update (POST /auth/logout), \
     AuthProvider/{Create,Update,Delete} (the admin auth-provider routes), and the \
     Session fan-out to many users";

impl std::fmt::Display for AuthSyncNotDeclared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ziee-auth: this app has not declared how auth's cross-device sync is wired.\n\
             \n\
             AuthModule mounts handlers that publish {AUTH_SYNC_PUBLISHES}. Installing a \
             no-op sink by default would drop all of them silently — the handlers still \
             run and still compute their frames, so nothing anywhere reports a problem \
             and a second device simply never converges. That is why this is an error \
             and not a default.\n\
             \n\
             Declare the choice ONCE, before `initialize_modules`:\n\
             \x20 • ziee_auth::install_auth_sync_sink(Arc::new(MyAuthSyncSink))  — map \
             AuthSyncEntity onto the app's SyncEntity and publish\n\
             \x20 • ziee_auth::declare_auth_sync_inert(\"<why this deployment drops them>\") \
             — keep the old no-op behaviour, on the record"
        )
    }
}

impl std::error::Error for AuthSyncNotDeclared {}

/// The process-wide declaration. `OnceLock` (not a swappable slot) because the
/// wiring is a boot-time property: a later change would silently split the
/// publishes of one run across two sinks.
static WIRING: OnceLock<AuthSyncWiring> = OnceLock::new();

/// Declare how auth's cross-device sync is wired. Call ONCE at boot, before
/// `initialize_modules`.
///
/// First declaration wins. A second one does NOT replace it and is reported at
/// `warn` — an overwrite that silently discarded the first would be the same
/// class of invisible drop this whole seam exists to remove.
pub fn declare_auth_sync(wiring: AuthSyncWiring) {
    let attempted = wiring.describe();
    if let Err(rejected) = WIRING.set(wiring) {
        let _ = rejected;
        tracing::warn!(
            attempted = %attempted,
            existing = %WIRING.get().map(AuthSyncWiring::describe).unwrap_or_default(),
            "ziee-auth: auth sync wiring was already declared; the FIRST declaration stands \
             and this one is ignored"
        );
    }
}

/// Route auth's sync publishes into the app's fan-out. Shorthand for
/// [`declare_auth_sync`]`(AuthSyncWiring::Live(sink))`.
pub fn install_auth_sync_sink(sink: Arc<dyn AuthSyncSink>) {
    declare_auth_sync(AuthSyncWiring::Live(sink));
}

/// Deliberately drop auth's sync publishes, with the reason on the record.
/// Shorthand for [`declare_auth_sync`]`(AuthSyncWiring::Inert { reason })`.
pub fn declare_auth_sync_inert(reason: &'static str) {
    declare_auth_sync(AuthSyncWiring::Inert { reason });
}

/// The declaration this process made, if any.
pub fn declared_auth_sync() -> Option<&'static AuthSyncWiring> {
    WIRING.get()
}

/// Resolve a declaration into the sink `AuthModule` installs.
///
/// Pure (the `Option` is the caller's business), so the whole decision —
/// including the refusal — is unit-testable without touching the process-wide
/// `OnceLock`.
pub fn resolve_auth_sync(
    declared: Option<&AuthSyncWiring>,
) -> Result<Arc<dyn AuthSyncSink>, AuthSyncNotDeclared> {
    match declared {
        Some(w) => Ok(w.sink()),
        None => Err(AuthSyncNotDeclared),
    }
}

#[cfg(test)]
mod noop_sink_tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn noop_sinks_are_send_sync_and_inert() {
        assert_send_sync::<NoopAuthEventSink>();
        assert_send_sync::<NoopAuthSyncSink>();
        // Behaviourally inert — calling them does nothing and never panics.
        let ev = NoopAuthEventSink;
        ev.emit_user(UserEvent::Deleted { user_id: Uuid::nil() });
        ev.emit_auth_provider(AuthProviderEvent::Created { id: Uuid::nil() });
        let sync = NoopAuthSyncSink;
        sync.publish(
            AuthSyncEntity::User,
            AuthSyncAction::Create,
            Uuid::nil(),
            Audience::owner(Uuid::nil()),
            None,
        );
        sync.publish_session_to_users(&[Uuid::nil()], None);
    }

    /// They satisfy the `Arc<dyn ...>` slots `AuthContext::new` requires — the
    /// whole point of shipping them (G1: apps stop hand-writing 2 trait impls).
    #[test]
    fn noop_sinks_fit_authcontext_slots() {
        let _events: Arc<dyn AuthEventSink> = Arc::new(NoopAuthEventSink);
        let _sync: Arc<dyn AuthSyncSink> = Arc::new(NoopAuthSyncSink);
    }
}

/// The wiring DECISION, tested without the process-wide `OnceLock`.
///
/// Everything here drives [`resolve_auth_sync`] directly, which is why it is a
/// free function taking an `Option` rather than reading the static: a decision
/// that can only be observed through a global is a decision that can only be
/// tested once per process.
#[cfg(test)]
mod sync_wiring_tests {
    use super::*;
    use std::sync::Mutex;

    /// Records what it was asked to publish, so "the sink I declared is the sink
    /// that got used" is a behavioural assertion rather than a pointer compare.
    #[derive(Default)]
    struct RecordingSink {
        published: Mutex<Vec<AuthSyncEntity>>,
        fanouts: Mutex<Vec<usize>>,
    }

    impl AuthSyncSink for RecordingSink {
        fn publish(
            &self,
            entity: AuthSyncEntity,
            _action: AuthSyncAction,
            _id: Uuid,
            _audience: Audience,
            _origin: Option<Uuid>,
        ) {
            self.published.lock().unwrap().push(entity);
        }
        fn publish_session_to_users(&self, user_ids: &[Uuid], _origin: Option<Uuid>) {
            self.fanouts.lock().unwrap().push(user_ids.len());
        }
    }

    /// NOT DECLARING IS AN ERROR. This is the whole fix: the previous behaviour
    /// was to fall back to `NoopAuthSyncSink`, which is indistinguishable from a
    /// working sink at every call site.
    #[test]
    fn an_undeclared_wiring_is_refused_not_defaulted() {
        // `Arc<dyn AuthSyncSink>` is not `Debug`, so match rather than `expect_err`.
        match resolve_auth_sync(None) {
            Err(e) => assert_eq!(e, AuthSyncNotDeclared),
            Ok(_) => panic!(
                "no declaration must be an ERROR — falling back to a no-op sink is the bug \
                 this seam exists to remove"
            ),
        }
    }

    /// The refusal names BOTH ways out. An error that only says "you must supply
    /// a sink" pushes every app that genuinely does not want one into either
    /// hand-rolling a no-op or reverting the guard.
    #[test]
    fn the_refusal_names_both_ways_out() {
        let msg = AuthSyncNotDeclared.to_string();
        assert!(
            msg.contains("install_auth_sync_sink"),
            "the error must name the LIVE path; got:\n{msg}"
        );
        assert!(
            msg.contains("declare_auth_sync_inert"),
            "the error must name the explicit-opt-out path, or an app that wants no sync \
             has no sanctioned way to say so; got:\n{msg}"
        );
        // …and it must say what is being dropped, so choosing `Inert` is an
        // informed choice rather than a shrug.
        for named in ["Profile", "SessionSettings", "Session", "AuthProvider"] {
            assert!(
                msg.contains(named),
                "the error must name the `{named}` publish it drops; got:\n{msg}"
            );
        }
    }

    /// A declared LIVE sink is the sink that gets installed — the frames reach it.
    #[test]
    fn a_declared_live_sink_receives_the_publishes() {
        let rec = Arc::new(RecordingSink::default());
        let wiring = AuthSyncWiring::Live(rec.clone());
        let sink = resolve_auth_sync(Some(&wiring)).expect("a declared wiring resolves");

        sink.publish(
            AuthSyncEntity::Profile,
            AuthSyncAction::Update,
            Uuid::nil(),
            Audience::owner(Uuid::nil()),
            None,
        );
        sink.publish_session_to_users(&[Uuid::nil(), Uuid::nil()], None);

        assert_eq!(
            *rec.published.lock().unwrap(),
            vec![AuthSyncEntity::Profile],
            "a resolved Live wiring must hand back the DECLARED sink, not a fresh no-op"
        );
        assert_eq!(*rec.fanouts.lock().unwrap(), vec![2]);
    }

    /// `Inert` really is inert — the opt-out has to be the old behaviour, or an
    /// app choosing it would be choosing something else.
    #[test]
    fn an_inert_wiring_drops_without_panicking() {
        let wiring = AuthSyncWiring::Inert {
            reason: "single-device desktop build",
        };
        let sink = resolve_auth_sync(Some(&wiring)).expect("inert is a valid declaration");
        sink.publish(
            AuthSyncEntity::Session,
            AuthSyncAction::Update,
            Uuid::nil(),
            Audience::owner(Uuid::nil()),
            None,
        );
        sink.publish_session_to_users(&[Uuid::nil()], None);
    }

    /// The reason survives into the description — an `Inert` choice whose reason
    /// never reaches a log or a message is back to being a silent default.
    #[test]
    fn the_inert_reason_is_carried_into_the_description() {
        let wiring = AuthSyncWiring::Inert {
            reason: "no SSE stream in the CLI build",
        };
        assert!(
            wiring.describe().contains("no SSE stream in the CLI build"),
            "got: {}",
            wiring.describe()
        );
        assert!(
            AuthSyncWiring::Live(Arc::new(NoopAuthSyncSink))
                .describe()
                .contains("live"),
            "a Live wiring must describe itself as live"
        );
    }

    // NOTE: the process-wide declaration (`declare_auth_sync` / `declared_auth_sync`)
    // is deliberately NOT tested here. It is a `OnceLock`, so its interesting
    // property — "the first declaration wins, and until one is made the module
    // refuses to boot" — is order-dependent and cannot be asserted from a test
    // that shares a process with siblings that declare. It is proved end to end,
    // in declaration order, in the OWN-PROCESS integration test
    // `tests/auth_sync_must_be_declared.rs`.
}
