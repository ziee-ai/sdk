//! `ziee-notification` — a first-class SDK notification feature with a per-module
//! contribution registry (PROPOSED SDK contribution — see `SDK_GAPS.md`).
//!
//! - **Always available (DB-free types):** `models` (`Notification` row with a
//!   generic `payload` JSONB, the `NewNotification` builder, `NotificationPage`,
//!   `UnreadCount`) + the `NotificationsRead` permission key + the crate's own
//!   generic migration (`migrations/`, globbed into an app's merged set).
//! - **Behind the `routes` feature (the RESOLVER-GENERIC engine):** the
//!   `repository` (compile-time `query!`), the `create_and_emit` producer seam +
//!   the pluggable `set_sync_emitter` sync seam (the crate never names the app's
//!   concrete `SyncEntity::Notification`), the aide/axum `handlers` +
//!   `notification_router::<R>()` (generic over the app's `IdentityResolver`,
//!   mirroring `ziee_file::http::file_routes<R>`), and the `NOTIFICATION_KINDS`
//!   per-module contribution `registry`. An app with its own resolver enables
//!   ONLY this and mounts `notification_router::<ItsResolver>()` itself.
//! - **Behind the `module` feature (implies `routes`, turnkey):** the retention
//!   prune loop + the self-registering `AppModule`/`MODULE_ENTRIES` entry that
//!   mounts `notification_router::<DefaultIdentityResolver>()` — batteries for an
//!   app that has no resolver of its own. ziee does NOT enable this.
//!
//! The RENDER half of the per-module seam (register a `kind` + its renderer) is
//! the frontend registry in `@ziee/framework/notification`; the FE inbox
//! dispatches on `kind`. A module CONTRIBUTES its kinds here (a
//! `#[distributed_slice(NOTIFICATION_KINDS)]` static) + its renderers there.

pub mod models;
pub mod permissions;

#[cfg(feature = "routes")]
pub mod events;
#[cfg(feature = "routes")]
mod handlers;
#[cfg(feature = "module")]
mod module;
#[cfg(feature = "routes")]
pub mod registry;
#[cfg(feature = "routes")]
pub mod repository;

#[cfg(feature = "routes")]
pub use events::{
    create_and_emit, emit_bulk_changed, emit_row_changed, set_sync_emitter, NotifSyncAction,
    NotifSyncEmitter,
};
#[cfg(feature = "routes")]
pub use handlers::notification_router;
#[cfg(feature = "module")]
pub use module::NotificationModule;
#[cfg(feature = "routes")]
pub use registry::{
    is_registered_kind, registered_kinds, NotificationKindDescriptor, NOTIFICATION_KINDS,
};

// Re-export the core types at the crate root for ergonomic consumption.
pub use models::{NewNotification, Notification, NotificationPage, UnreadCount};
pub use permissions::NotificationsRead;
