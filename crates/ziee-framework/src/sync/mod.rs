//! Realtime cross-device sync core (chunk B5).
//!
//! Moved from ziee's `modules::sync`: the per-user SSE connection **registry**
//! (`registry`), the **audience** machinery (`audience` — `Audience` / `PermRule`
//! + the typed constructors), and the `SyncOrigin` request **extractor**
//! (`extractor`). These are the genuinely app-agnostic half of realtime sync:
//! the framework owns the connection lifecycle (caps / pruning / self-echo) and
//! the delivery routing, generic over
//! - an app **principal** ([`ziee_identity::Principal`]) — the per-connection
//!   permission snapshot the `Perm` audience routes against, and
//! - an app **entity kind** ([`SyncEntityKind`]) — the seam replacing ziee's
//!   closed `SyncEntity` enum. The framework never names ziee's `SyncEntity`,
//!   `SyncEvent`, or `SyncSseEvent` (all of which derive `JsonSchema` and are in
//!   the OpenAPI/`types.ts` surface): the wire types stay **concrete in ziee**,
//!   so the serialized schema — and the generated `sync:<entity>` TS vocabulary —
//!   is byte-unchanged. The app serializes each change into an axum SSE
//!   [`Event`](axum::response::sse::Event) and hands the finished event to
//!   [`SyncRegistry::deliver`]; the session fan-out obtains its per-user event
//!   from [`SyncEntityKind::session_signal`].
//!
//! ziee keeps the process-wide singleton (`registry()`) + its concrete
//! `SyncConnPrincipal` and consumes this via equivalence-preserving re-export
//! shims, so every `publish(...)` emit site + the `SyncEntity` enum are unchanged.

pub mod audience;
pub mod extractor;
pub mod registry;
// Chunk sdk-surfaces: the mountable `GET /sync/subscribe` SSE handler, generic
// over the app's `IdentityResolver` + `SyncSurface` (the concrete wire/registry
// surface). Moved from ziee's `modules::sync::handlers`.
pub mod routes;

use axum::response::sse::Event;
use uuid::Uuid;

pub use audience::{Audience, PermRule};
pub use extractor::{SyncOrigin, SYNC_CONNECTION_HEADER};
pub use registry::{ClientConn, SyncRegistry, SYNC_CHANNEL_CAPACITY};
pub use routes::{RecheckOutcome, SyncSurface, sync_routes};

/// The app's sync-entity vocabulary, abstracted to exactly what the framework
/// registry needs: how to build the SSE event fanned to a user's devices when
/// their session/permissions change.
///
/// This is the app-extensible seam replacing ziee's closed `SyncEntity` enum.
/// ziee's `SyncEntity` (which keeps its `JsonSchema` derive + every variant, so
/// the OpenAPI/`types.ts` surface is unchanged) implements this trait; the
/// framework's [`SyncRegistry::deliver_session_to_users`] is generic over it.
/// The framework deliberately does NOT abstract the per-`{entity, action, id}`
/// wire event — the app serializes that with its own concrete (schema-bearing)
/// types and passes the finished [`Event`] to [`SyncRegistry::deliver`].
pub trait SyncEntityKind {
    /// Build the SSE [`Event`] delivered to `user_id`'s devices for a
    /// session/permissions-changed signal (the batched member fan-out used by
    /// group-permission edits). ziee builds
    /// `SyncSseEvent::Sync(SyncEvent { entity: Session, action: Update, id: user_id })`.
    fn session_signal(user_id: Uuid) -> Event;
}
