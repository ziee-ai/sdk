//! `ziee-framework` — module machinery + app_builder (build-DB-free).
//!
//! Chunk B2 moved the module system (`AppModule` / `ModuleContext` /
//! `ModuleEntry` / `MODULE_ENTRIES`) and `app_builder` (module discovery,
//! router assembly, CORS + rate-limit layers) here from the ziee server crate.
//! The domain-free `EventHandler` trait lives here too, since `AppModule`
//! returns it; the domain-coupled `AppEvent` enum + `EventBus` dispatcher stay
//! app-side (they move in a later chunk — plan B5).
//!
//! ziee consumes these via equivalence-preserving re-export shims (decision
//! N2): `crate::module_api` and `crate::core::app_builder` re-export from here,
//! so the module registration sites and boot path stay unchanged.

pub mod app_builder;
pub mod events;
pub mod module_api;
// Chunk B6: the OpenAPI TypeScript-client generator (`emit_ts`, a pure function
// of the spec) + the app-agnostic generation driver TAIL (`finish_and_emit`,
// with the output paths parameterized). The app-specific config-load + module
// init stays app-side and feeds a finished router/doc into `finish_and_emit`.
pub mod openapi;
pub mod permissions;
// Chunk B5: the realtime-sync core (per-user SSE connection registry + caps +
// audience routing + self-echo + the `SyncOrigin` extractor), generic over the
// app's `Principal` snapshot + `SyncEntityKind` entity seam. ziee keeps its
// concrete wire types (`SyncEntity`/`SyncEvent`/`SyncSseEvent`, all in the
// OpenAPI surface) + the process-wide singleton, consuming this via re-export
// shims so every `publish(...)` emit site is unchanged.
pub mod sync;
// Chunk B4: the `declare_repositories!` macro lives here (exported at crate root
// via `#[macro_export]`, so ziee invokes `ziee_framework::declare_repositories!`).
// Its expansion generates the `Repos` global + factory machinery in the invoking
// crate; the concrete repo LIST + `Repos.xxx` call sites stay in ziee.
pub mod repository;

pub use events::EventHandler;
pub use module_api::{AppModule, ModuleContext, ModuleEntry, MODULE_ENTRIES};
pub use permissions::{IdentityResolver, RequireAdmin, RequirePermissions, with_permission};
pub use sync::{
    Audience, ClientConn, PermRule, SyncEntityKind, SyncOrigin, SyncRegistry,
    SYNC_CHANNEL_CAPACITY, SYNC_CONNECTION_HEADER,
};
