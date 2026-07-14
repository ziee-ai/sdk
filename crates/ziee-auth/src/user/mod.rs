//! The user core moved into `ziee-auth` (Chunk BA-full): the schema-bound
//! user/group repositories (`query!` macros), the wire DTOs, the effective-
//! permissions service, and the user-lifecycle events.
//!
//! The app keeps the HTTP/aide boundary (`handlers` / `routes` / `permissions`
//! — including the domain-coupled `delete_user` admin cascade over
//! skill/file/hub cleanup) and consumes this crate via re-export shims.

pub mod events;
pub mod repository;
pub mod service;
pub mod types;

// Re-exports (mirror the app user module's public surface). `User` / `Group`
// live at the crate root (`models.rs`); re-export them here so the app shim
// `pub use ziee_auth::user::*` keeps `crate::modules::user::{User, Group}`.
pub use crate::{Group, User};
pub use repository::{GroupRepository, UserRepository};
pub use service::UserService;
