//! Framework `EventHandler` trait — the domain-free half of the event system.
//!
//! The full event system (the `AppEvent` enum + the `EventBus` dispatcher) is
//! **domain-coupled** (every `AppEvent` variant wraps a ziee module event), so
//! it stays app-side and moves to the framework only in a later chunk (plan B5,
//! `SyncEntityKind` / EventBus genericization). But the module system's
//! `AppModule::register_event_handlers` returns `Vec<Arc<dyn EventHandler>>`, so
//! the `EventHandler` *trait* must live in the framework alongside `AppModule`.
//!
//! To keep the framework app-agnostic, the trait's `handle` takes the event as
//! an erased `&(dyn Any + Send + Sync)` instead of a concrete `&AppEvent`. The
//! app's `EventBus` passes `&app_event` (which coerces to the erased type) and
//! each concrete handler recovers it with `event.downcast_ref::<AppEvent>()`.
//! Observable behavior is unchanged — the event is never on the OpenAPI wire.

use async_trait::async_trait;
use sqlx::PgPool;
use std::any::Any;

use ziee_core::AppError;

/// Trait for handling application events.
///
/// Modules implement this to react to events they care about. The event is
/// passed type-erased (`&dyn Any`) so the framework stays domain-free; the
/// implementer downcasts to the app's concrete event enum.
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// Handle an event.
    ///
    /// Return error to log it, but it won't stop other handlers from running.
    /// The `event` is type-erased — downcast it to the app's event enum, e.g.
    /// `event.downcast_ref::<AppEvent>()`.
    async fn handle(
        &self,
        event: &(dyn Any + Send + Sync),
        pool: &PgPool,
    ) -> Result<(), AppError>;

    /// Name for logging and debugging.
    fn handler_name(&self) -> &'static str;
}
