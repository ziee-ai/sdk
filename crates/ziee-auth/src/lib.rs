//! `ziee-auth` — the DEFAULT (optional, replaceable) schema-bound auth module.
//!
//! Owns the concrete user/group/session wire types + (progressively) the auth
//! module's queries, login/register/LDAP/OAuth2, admin CRUD, the Session &
//! Token-Refresh subsystem, and the dual-mode auth strategies. Owns its
//! migrations. Consumed by ziee via equivalence-preserving re-export shims
//! (decision N2), so the ~hundreds of call sites + the wire schemas are
//! unchanged (schemars keys types by short ident, so moving the crate that
//! defines `User`/`Group` does not move the OpenAPI schema name).

mod models;

pub mod auth;
pub mod user;

pub use models::{Group, User};

/// The auth module's own migrations (structural auth-table DDL), embedded at
/// build time. The app composes these ∪ its remaining migrations into one
/// version-sorted merged set for BOTH its runtime `migrate` and its build-DB
/// provisioner; ziee-auth's own `build.rs` provisions an auth-only build DB
/// from exactly this set for its `query!` verification. The moved files keep
/// their original version numbers + byte content (checksums preserved), so the
/// merged set reproduces ziee's exact `_sqlx_migrations` history — deployed DBs
/// are unaffected.
pub static AUTH_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
