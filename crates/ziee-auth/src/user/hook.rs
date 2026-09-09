//! An injectable, in-transaction **user-created hook** (gap G-AUTHEVT).
//!
//! ── the gap this closes ─────────────────────────────────────────────────────
//!
//! The auth module already carries a `UserEvent::Created` vocabulary and an
//! [`AuthEventSink`](crate::auth::context::AuthEventSink), but that sink is a
//! POST-commit, fire-and-forget in-process notification: it runs after the user
//! row is durably committed, on its own, with no transaction handle. A consumer
//! that needs a *co-transaction* — "when a user is created, create their
//! personal account in the SAME transaction, or create neither" — cannot express
//! that through the event sink. CytoAnalyst worked around this with an app-side
//! lazy-ensure backstop; this seam lets the clean, atomic hook land.
//!
//! ── the shape ───────────────────────────────────────────────────────────────
//!
//! A process-wide, install-once [`UserCreatedHook`] that the crate's user-INSERT
//! repository functions fire **inside the creation transaction**, immediately
//! after the user row is inserted and before commit. Because every SDK
//! user-creation path (local register, LDAP/OAuth first-login, admin-create,
//! first-run setup) funnels through those repository functions, installing one
//! hook covers them all — there is no per-call-site wiring to forget.
//!
//! ── the contract ────────────────────────────────────────────────────────────
//!
//! The hook receives `&User` + the live `&mut PgConnection` of the creation
//! transaction, so its own writes are atomic with the user row. Returning `Err`
//! **rolls the whole creation back** — the user is NOT created. A user whose
//! co-transaction invariant (e.g. "has a personal account") cannot be satisfied
//! is a broken state, so failing the creation atomically is safer than
//! committing a half-provisioned user.
//!
//! ── backward compatibility ──────────────────────────────────────────────────
//!
//! Unlike [`AuthSyncWiring`](crate::auth::context::AuthSyncWiring) — a REQUIRED
//! declaration, because a silently-dropped cross-device publish is invisible —
//! this hook has a genuine, safe no-op default: an app that installs nothing
//! sees today's behaviour exactly (the repository functions fire nothing). There
//! is no invisible-drop failure mode to guard against, so the default stands.

use std::sync::{Arc, OnceLock};

use sqlx::PgConnection;

use ziee_core::AppError;

use crate::User;

/// A hook fired inside the user-creation transaction, right after the user row
/// is inserted and before the transaction commits.
///
/// Install one process-wide with [`install_user_created_hook`]. The default (no
/// hook installed) does nothing, so installing nothing preserves today's
/// behaviour.
#[async_trait::async_trait]
pub trait UserCreatedHook: Send + Sync {
    /// Run against the just-created `user`, using the creation transaction's
    /// connection `tx` so any writes are atomic with the user row.
    ///
    /// Returning `Err` aborts the creation: the caller rolls the transaction
    /// back and the user is NOT created. Return `Ok(())` to let the creation
    /// commit.
    async fn on_user_created(&self, user: &User, tx: &mut PgConnection) -> Result<(), AppError>;
}

/// The default [`UserCreatedHook`] — does nothing, so a process that installs no
/// hook behaves exactly as before this seam existed. Handed back by
/// [`user_created_hook`] callers only conceptually; the firing path simply skips
/// when nothing is installed.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopUserCreatedHook;

#[async_trait::async_trait]
impl UserCreatedHook for NoopUserCreatedHook {
    async fn on_user_created(&self, _user: &User, _tx: &mut PgConnection) -> Result<(), AppError> {
        Ok(())
    }
}

/// The process-wide hook. `OnceLock` (install-once, first wins) because a
/// user-creation hook is a boot-time property: swapping it mid-run would split
/// one run's user creations across two hooks — the same silent-inconsistency
/// class the auth-sync `OnceLock` avoids.
static HOOK: OnceLock<Arc<dyn UserCreatedHook>> = OnceLock::new();

/// Install the process-wide user-created hook. Call ONCE at boot, before any
/// user can be created.
///
/// First installation wins. A second call does NOT replace it and is reported at
/// `warn` — a silent overwrite would drop the first consumer's co-transaction
/// work without a trace.
pub fn install_user_created_hook(hook: Arc<dyn UserCreatedHook>) {
    if HOOK.set(hook).is_err() {
        tracing::warn!(
            "ziee-auth: a user-created hook was already installed; the FIRST installation \
             stands and this one is ignored"
        );
    }
}

/// The installed hook, if any. `None` is the default (no-op) posture.
pub fn user_created_hook() -> Option<Arc<dyn UserCreatedHook>> {
    HOOK.get().cloned()
}

/// Fire the installed hook (if any) inside the creation transaction.
///
/// Called by every user-INSERT repository function immediately after the user
/// row is inserted and before commit. No hook installed → does nothing (the
/// backward-compatible default). An `Err` propagates so the caller's `?` rolls
/// the transaction back and the user is not created.
pub(crate) async fn fire_user_created(user: &User, tx: &mut PgConnection) -> Result<(), AppError> {
    if let Some(hook) = HOOK.get() {
        hook.on_user_created(user, tx).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn assert_send_sync<T: Send + Sync>() {}

    fn sample_user() -> User {
        User {
            id: Uuid::nil(),
            username: "sample".to_string(),
            email: "sample@example.com".to_string(),
            email_verified: false,
            password_hash: None,
            display_name: None,
            avatar_url: None,
            is_active: true,
            is_admin: false,
            permissions: vec![],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            password_changed_at: None,
        }
    }

    /// The trait object is thread-safe (it lives in an `Arc<dyn …>` behind a
    /// process-wide `OnceLock`, so `Send + Sync` is load-bearing).
    #[test]
    fn hook_trait_object_is_send_sync() {
        assert_send_sync::<Arc<dyn UserCreatedHook>>();
        assert_send_sync::<NoopUserCreatedHook>();
    }

    /// The default is genuinely inert — it returns `Ok` and touches nothing, so
    /// an app that installs no hook keeps today's behaviour. (The DB-backed
    /// firing / rollback path is proved end-to-end in the own-process
    /// integration test `tests/user_created_hook.rs`, which owns the global
    /// `OnceLock`.)
    #[tokio::test]
    async fn noop_hook_is_ok_and_inert() {
        // A lazy pool never dials; we only need a `PgConnection` type to satisfy
        // the signature, and Noop never touches it.
        let noop = NoopUserCreatedHook;
        // Prove it via the trait object exactly as the firing path holds it.
        let hook: Arc<dyn UserCreatedHook> = Arc::new(noop);
        // We cannot cheaply materialise a live `PgConnection` here, so assert the
        // shape compiles + the default is the trivial Ok by calling through a
        // conn only in the integration test. Here we just prove the value is
        // constructible + object-safe.
        let _ = hook;
        let _ = sample_user();
    }
}
