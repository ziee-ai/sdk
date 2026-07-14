//! `ziee-notification` — the DB-free wire/insert types + permission key for the
//! durable, owner-scoped notification inbox, plus the module's own migrations
//! (Chunk `notification`, build-DB-free).
//!
//! Moved from ziee's `modules/notification`: the `models` (`Notification` row +
//! its `is_unread`, the `NewNotification` builder, `NotificationPage`,
//! `UnreadCount` — schemars keys preserved; `Notification` derives
//! `sqlx::FromRow` but issues no `query!`), the `NotificationsRead` permission
//! key (`ziee_identity::PermissionCheck`), and the two schema migrations
//! (`migrations/` — the app globs `sdk/crates/*/migrations/` into its merged
//! set, so they compose exactly as before with their original filenames + byte
//! content).
//!
//! Retained in ziee (equivalence-preserving): the schema-bound `repository`
//! (compile-time `query_as!`, verified only against the app's full merged build
//! DB because the notification fkeys reference other modules'
//! users/conversations/scheduled_tasks/workflow_runs tables), the `events`
//! `create_and_emit` seam (names the concrete `SyncEntity::Notification`), the
//! aide/axum `handlers`/`routes` (bind ziee's `RequirePermissions` resolver +
//! the global `Repos`), the retention `prune`, and the
//! `AppModule`/`MODULE_ENTRIES` registration. ziee's `notification/mod.rs`
//! re-exports `models`/`permissions` from here as shims (decision N2), so every
//! `super::models::…` / `super::permissions::…` path in the retained
//! repository/events/handlers + the `scheduler/dispatch.rs` uses of
//! `notification::models::NewNotification` resolve unchanged.

pub mod models;
pub mod permissions;
