// Auth module events
// Events related to authentication and authorization
// These are part of the event system infrastructure for future use

use uuid::Uuid;

/// Events emitted by the auth module
// Future event-infrastructure: no emitter/subscriber wired yet (kept as the
// intended auth-event vocabulary). Narrow allow instead of the old
// module-level blanket so new dead code in this file is still caught.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum AuthEvent {
    /// User successfully authenticated
    UserAuthenticated { user_id: Uuid, provider: String },

    /// User authentication failed
    AuthenticationFailed { username: String, reason: String },

    /// User session refreshed
    SessionRefreshed { user_id: Uuid },

    /// User session expired
    SessionExpired { user_id: Uuid },
}

// The former `impl AuthEvent` AppEvent-wrapping constructors were removed in
// Chunk BG: they were dead code (no emitter/subscriber wired) and naming
// the app-aggregate `AppEvent` here coupled the auth module to that enum.
// If this vocabulary is ever wired, emit it through the injected
// `AuthEventSink` (see `context.rs`) — the app owns the `AppEvent` wrapping.
