//! `ziee-server-update` — the DB-free wire type + permission key for the server
//! self-update notification module (Chunk `server-update`, build-DB-free).
//!
//! Moved from ziee's `modules/server_update`: the `UpdateStatusResponse` wire
//! type (`types`) and the admin-only `ServerUpdateRead` permission key
//! (`permissions`, implementing `ziee_identity::PermissionCheck`).
//!
//! Retained in ziee (equivalence-preserving): the daily-poll `checker` — it
//! embeds `env!("CARGO_PKG_VERSION")`, which must compile to ziee's version (a
//! move would report this crate's `0.0.0`), and a `#[cfg(test)]` names
//! `crate::core::config::UpdateCheckConfig` — plus the aide/axum `handlers`
//! (bind `RequirePermissions`/`with_permission`) + `routes` + the
//! `AppModule`/`MODULE_ENTRIES` registration. ziee's `server_update/mod.rs`
//! re-exports `types`/`permissions` from here as shims (decision N2), so
//! `super::types::UpdateStatusResponse` + `super::permissions::ServerUpdateRead`
//! in the retained handlers resolve unchanged and the schemars schema key +
//! the OpenAPI 403 example (which feeds the UI `Permissions` enum) are
//! byte-identical.

pub mod permissions;
pub mod types;
