//! `ziee-identity` — identity ABSTRACTIONS (build-DB-free).
//!
//! The framework enforces authorization against these traits; the concrete
//! identity (ziee's `User`/`Group`/`JwtService` + the auth tables) is INJECTED,
//! so an app can supply its own (pluggable identity, decision #1). Contents:
//!
//! - [`PermissionCheck`] / [`PermissionList`] — compile-time permission
//!   declarations + the OpenAPI-facing [`PermissionInfo`] projection.
//! - [`rbac::check_permissions_array`] — the generic RBAC evaluator (exact /
//!   full-wildcard / hierarchical `foo::*` matching over permission strings).
//! - [`Principal`] — the minimal authenticated-identity interface (effective
//!   permissions / active groups / is_admin) framework enforcement needs.
//! - [`TokenVerifier`] — the JWT-verify interface (token → claims).
//!
//! ziee consumes these via equivalence-preserving re-export shims (decision N2):
//! `modules::permissions::types` re-exports the traits, `checker` calls the moved
//! evaluator, `User` implements `Principal`, and `JwtService` implements
//! `TokenVerifier` — no call sites change.

pub mod permission;
pub mod principal;
pub mod rbac;
pub mod token;

pub use permission::{PermissionCheck, PermissionInfo, PermissionList};
pub use principal::Principal;
pub use rbac::check_permissions_array;
pub use token::TokenVerifier;
