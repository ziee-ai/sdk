//! Permission enforcement (chunk B3) — generic over an injected
//! [`IdentityResolver`], so the framework enforces authorization without naming
//! any concrete identity/repository/JWT type. ziee installs its resolver at
//! startup and consumes these via equivalence-preserving type-alias shims.

pub mod extractors;
pub mod openapi;
pub mod resolver;

pub use extractors::{RequireAdmin, RequirePermissions};
pub use openapi::{PermissionDetail, PermissionError, PermissionErrorDetails, with_permission};
pub use resolver::IdentityResolver;
