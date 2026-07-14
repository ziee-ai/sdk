// User module events
// Event infrastructure for future use

// Events related to user lifecycle and authentication

use crate::User;
use uuid::Uuid;

/// Events emitted by the user module
// Emit-only lifecycle events: `created` is consumed (assistant module reads
// Created); Updated/Deleted payloads are emitted but have no subscriber yet —
// retained as the module's event vocabulary. (LoggedIn/LoggedOut variants were
// removed: they were never emitted anywhere and had no subscriber — dead
// vocabulary for a login-audit feature that isn't wired. Re-add them alongside
// their emit site + subscriber if/when that feature lands.)
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum UserEvent {
    /// A new user was created
    Created { user: User },

    /// An existing user was updated
    Updated { user: User },

    /// A user was deleted
    Deleted { user_id: Uuid },
}

// The former `impl UserEvent` AppEvent-wrapping constructors were removed in
// Chunk BG. Callers now build the raw `UserEvent` variant and hand it to the
// injected `AuthEventSink::emit_user` (see `modules::auth::context`); the app's
// sink impl performs the `AppEvent::User(..)` wrapping, so the user module no
// longer names the app-aggregate event enum.
