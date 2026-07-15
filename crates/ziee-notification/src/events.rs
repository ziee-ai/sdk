//! The single write seam for the inbox: insert a notification row then emit the
//! realtime sync signal so every one of the owner's devices refetches.
//!
//! The crate does NOT name the app's concrete `SyncEntity::Notification` (that
//! is app-owned). Instead the app registers a **sync emitter** once at boot via
//! [`set_sync_emitter`]; `create_and_emit` (and the mark-read/delete handlers)
//! invoke it. This is the pluggable seam that keeps the SDK notification module
//! generic over the app's sync vocabulary.

use std::sync::{Arc, OnceLock};

use sqlx::PgPool;
use uuid::Uuid;
use ziee_core::AppError;

use crate::models::{NewNotification, Notification};

/// What happened to the inbox, mapped by the app onto its own sync action enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifSyncAction {
    Create,
    Update,
    Delete,
}

/// The app-provided sync emitter: `(recipient_user_id, action, notification_id,
/// origin_conn)`. The app maps this to `sync::publish(SyncEntity::Notification,
/// …, Audience::owner(user_id), origin)`.
pub type NotifSyncEmitter =
    Arc<dyn Fn(Uuid, NotifSyncAction, Uuid, Option<Uuid>) + Send + Sync>;

static EMITTER: OnceLock<NotifSyncEmitter> = OnceLock::new();

/// Register the app's sync emitter (idempotent — first registration wins). Call
/// once at boot.
pub fn set_sync_emitter(emitter: NotifSyncEmitter) {
    let _ = EMITTER.set(emitter);
}

fn emit(user_id: Uuid, action: NotifSyncAction, id: Uuid, origin: Option<Uuid>) {
    if let Some(e) = EMITTER.get() {
        e(user_id, action, id, origin);
    }
}

/// Insert a notification and notify the owner's devices.
///
/// Owner-scoped, `origin = None` (a producer has no originating request
/// connection, so even the triggering device refetches). The durable row is
/// always written; the `interrupt` flag is what the client consults to decide
/// whether to raise a live toast — the sync frame itself is payload-free.
pub async fn create_and_emit(
    pool: &PgPool,
    new: NewNotification,
) -> Result<Notification, AppError> {
    let user_id = new.user_id;
    let row = crate::repository::insert(pool, new).await?;
    emit(user_id, NotifSyncAction::Create, row.id, None);
    Ok(row)
}

/// Emit an inbox-changed signal for a single row (mark-read / delete). Carries
/// the originating connection so the acting tab is not echoed.
pub fn emit_row_changed(
    user_id: Uuid,
    action: NotifSyncAction,
    id: Uuid,
    origin: Option<Uuid>,
) {
    emit(user_id, action, id, origin);
}

/// Emit a bulk "the inbox changed, reload" signal (nil id) — used after
/// mark-all-read where no single row addresses the change. Owner-scoped.
pub fn emit_bulk_changed(user_id: Uuid, origin: Option<Uuid>) {
    emit(user_id, NotifSyncAction::Update, Uuid::nil(), origin);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn sync_action_is_copy_and_eq() {
        let a = NotifSyncAction::Create;
        let b = a; // Copy
        assert_eq!(a, b);
        assert_ne!(NotifSyncAction::Create, NotifSyncAction::Update);
        assert_ne!(NotifSyncAction::Update, NotifSyncAction::Delete);
    }

    // One captured emit: the `(recipient, action, notification_id, origin_conn)`
    // tuple the app maps onto `sync::publish` — same shape as `NotifSyncEmitter`.
    type Emitted = (Uuid, NotifSyncAction, Uuid, Option<Uuid>);

    // Captured emitter calls. `EMITTER` is a process-global `OnceLock` (first
    // registration wins), so exactly ONE test may register it — this is that
    // test. It drives both the single-row and bulk helpers through the seam and
    // asserts the exact tuple the app maps onto `sync::publish`.
    static CAPTURED: Mutex<Vec<Emitted>> = Mutex::new(Vec::new());

    #[test]
    fn emitter_seam_forwards_row_and_bulk() {
        set_sync_emitter(Arc::new(|uid, action, id, origin| {
            CAPTURED.lock().unwrap().push((uid, action, id, origin));
        }));

        let uid = Uuid::new_v4();
        let nid = Uuid::new_v4();
        let conn = Uuid::new_v4();
        emit_row_changed(uid, NotifSyncAction::Delete, nid, Some(conn));
        emit_bulk_changed(uid, None);

        let calls = CAPTURED.lock().unwrap();
        assert!(
            calls.contains(&(uid, NotifSyncAction::Delete, nid, Some(conn))),
            "row-changed forwards the (user, action, id, origin) tuple verbatim"
        );
        assert!(
            calls
                .iter()
                .any(|c| c.0 == uid
                    && c.1 == NotifSyncAction::Update
                    && c.2 == Uuid::nil()
                    && c.3.is_none()),
            "bulk emits Update with the nil id + no origin"
        );
    }
}
