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
// Chunk sdk-batteries (decision #11): the `build.rs` support crate — module-owned
// migration composition + per-worktree build-DB keying/provisioning — re-exported
// under the canonical framework path so runtime code + any app's `build.rs` reach
// it as `ziee_framework::build_support::{compose_merged_migrations,
// provision_build_db, worktree_db, ...}`. The lean `ziee-build-support` crate is
// the single home (build-DB-free, cheap as a `[build-dependencies]` entry).
pub use ziee_build_support as build_support;
pub mod embedded_pg;
// Gap G8: the generic entity-extension registry primitive — the domain-agnostic
// `#[distributed_slice]` of `{name, order, factory}` + `Vec<Arc<dyn Hooks>>`
// registry (folds routes + fans in-tx hooks over ONE shared `&mut Transaction`)
// + `auto_register` (sort-by-order) + `once_cell` singleton. ziee's `chat` +
// `project` extension registries + CytoAnalyst's `study` are all this same
// shape; they now build on this. Includes the OPTIONAL `on_<entity>_deleted`
// in-tx hook via `fire_in_tx` (offered, not imposed on cascade-only parents).
pub mod entity_extension;
pub mod events;
// Chunk C1: shared built-in-MCP-server scaffolding — the JSON-RPC 2.0 envelope
// types (`JsonRpcRequest`/`JsonRpcResponse`/`JsonRpcError`) + the `loopback_host`
// self-dial pin, dependency-free (serde/serde_json/ziee_core::AppError). Moved
// from ziee's `code_sandbox` so `ziee-control-mcp` + any fresh app's built-in MCP
// servers reuse them; ziee re-exports both via shims (code_sandbox::types /
// code_sandbox::loopback_host) so all call sites are unchanged.
pub mod mcp;
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
// Chunk BG-2: shared infra prerequisites for the auth (BA-full) extraction.
// `url_validator` is the domain-free SSRF outbound-URL guard (BG deferred this
// move; `ziee-auth`'s `core::outbound` now retargets it here). `secret` is the
// build-DB-free at-rest secret crypto (RATIFIED home = ziee-framework); it reads
// its storage key from the `secrets` process-global (moved alongside so
// `resolve_optional_secret` keeps its exact signature). ziee re-exports all three
// via `crate::utils::url_validator` / `crate::common::secret` / `crate::core::secrets`
// shims, so every consumer stays byte-unchanged.
pub mod secret;
pub mod secrets;
pub mod url_validator;

pub use app_builder::serve;
pub use entity_extension::{
    auto_register as auto_register_extensions, ExtensionEntry, ExtensionRegistry,
    ExtensionRegistrySingleton,
};
pub use events::EventHandler;
pub use mcp::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, loopback_host};
pub use module_api::{AppModule, ModuleContext, ModuleEntry, MODULE_ENTRIES};
pub use permissions::{IdentityResolver, RequireAdmin, RequirePermissions, with_permission};
pub use sync::{
    Audience, ClientConn, PermRule, RecheckOutcome, SyncEntityKind, SyncOrigin, SyncRegistry,
    SyncSurface, sync_routes, SYNC_CHANNEL_CAPACITY, SYNC_CONNECTION_HEADER,
};
