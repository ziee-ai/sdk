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
pub mod permissions;

pub use events::EventHandler;
pub use module_api::{AppModule, ModuleContext, ModuleEntry, MODULE_ENTRIES};
pub use permissions::{IdentityResolver, RequireAdmin, RequirePermissions, with_permission};
