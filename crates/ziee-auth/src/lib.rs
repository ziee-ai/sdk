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

pub use models::{Group, User};
