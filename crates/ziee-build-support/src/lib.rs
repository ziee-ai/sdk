//! `ziee-build-support` — the SDK's `build.rs` support crate (decision #11 / G4 / G5).
//!
//! The module-owned migration composition + the per-worktree build-DB keying &
//! provisioning that are an SDK CONTRACT (decision N7: "module-owned migrations,
//! build.rs composes them"), not just DX — every app copying them from ziee is
//! drift-prone. This crate is the single home; `ziee-framework` re-exports it as
//! `ziee_framework::build_support`, and each app's `build.rs` calls it instead of
//! carrying an inline copy.
//!
//! Deliberately LEAN + build-DB-free (no compile-time `query!`) so it is cheap to
//! use as a `[build-dependencies]` entry and needs no build DB to compile itself.

pub mod worktree_db;

mod build_db;
mod migrations;

pub use build_db::{ensure_build_db, provision_build_db, provision_build_db_named};
pub use migrations::compose_merged_migrations;
