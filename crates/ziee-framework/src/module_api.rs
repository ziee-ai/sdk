//! Module system — `AppModule` / `ModuleContext` / `ModuleEntry` /
//! `MODULE_ENTRIES` (moved from ziee's `module_api/` in Chunk B2).
//!
//! Modules register themselves at link time via the linkme `distributed_slice`
//! `MODULE_ENTRIES`. Because the slice is defined here, elements registered from
//! the app crate reference it either directly (`ziee_framework::MODULE_ENTRIES`)
//! or through the app's re-export shim (`crate::module_api::MODULE_ENTRIES`);
//! both resolve to this one slice, so `create_modules()` (see `app_builder`)
//! sees every app module.

use aide::axum::ApiRouter;
use linkme::distributed_slice;
use sqlx::PgPool;
use std::any::Any;
use std::error::Error;
use std::sync::Arc;

use ziee_core::ServerConfig;

use crate::events::EventHandler;

/// Core trait that all app modules must implement.
pub trait AppModule: Send + Sync {
    /// Unique module name.
    fn name(&self) -> &'static str;

    /// Module version.
    fn version(&self) -> &'static str {
        "1.0.0" // Default version
    }

    /// Module description.
    fn description(&self) -> &'static str {
        "" // Default: no description
    }

    /// Initialize module with context.
    fn init(&mut self, ctx: &ModuleContext) -> Result<(), Box<dyn Error>>;

    /// Register API routes.
    fn register_routes(&self, router: ApiRouter) -> ApiRouter;

    /// Register event handlers.
    ///
    /// Returns list of handlers that will receive application events. The
    /// handlers see events type-erased (see [`EventHandler`]); the app's
    /// `EventBus` dispatches to them.
    fn register_event_handlers(&self) -> Vec<Arc<dyn EventHandler>> {
        vec![] // Default: no handlers
    }
}

/// Context provided to modules during initialization.
///
/// `config` carries only the app-agnostic [`ServerConfig`] (postgresql / server
/// / logging / jwt). A module that needs the app's full config reads it from the
/// opaque `app_config` slot — the app injects its monolithic `Config` there and
/// the module downcasts it (`app_config.downcast_ref::<Config>()`). This keeps
/// the framework `ModuleContext` free of any app-specific config type.
pub struct ModuleContext {
    pub db_pool: Arc<PgPool>,
    pub config: Arc<ServerConfig>,
    /// The app's full, monolithic config, type-erased. ziee injects
    /// `Arc<ziee::Config>`; modules recover it via `downcast_ref`/`downcast`.
    pub app_config: Arc<dyn Any + Send + Sync>,
}

impl ModuleContext {
    pub fn new(
        db_pool: Arc<PgPool>,
        config: Arc<ServerConfig>,
        app_config: Arc<dyn Any + Send + Sync>,
    ) -> Self {
        Self {
            db_pool,
            config,
            app_config,
        }
    }
}

/// Module registration entry combining metadata with constructor.
///
/// Use the `#[distributed_slice(MODULE_ENTRIES)]` attribute to register modules:
/// ```ignore
/// #[distributed_slice(ziee_framework::MODULE_ENTRIES)]
/// static USER_MODULE: ModuleEntry = ModuleEntry {
///     name: "user",
///     order: 10,
///     description: "User and group management",
///     constructor: || Box::new(UserModule::new()),
/// };
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ModuleEntry {
    pub name: &'static str,
    pub order: i32,
    pub description: &'static str,
    pub constructor: fn() -> Box<dyn AppModule>,
}

/// Distributed slice for module registrations.
///
/// Modules register themselves by adding to this slice using
/// `#[distributed_slice(MODULE_ENTRIES)]`.
#[distributed_slice]
pub static MODULE_ENTRIES: [ModuleEntry] = [..];
