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

use std::sync::Arc;

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
