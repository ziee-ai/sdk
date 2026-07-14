//! The auth module's HTTP/aide surface — the mountable routes bundle.
//!
//! Moved from ziee's `modules/auth/{handlers,routes,jwt_extractor,session_settings}`
//! (decision N10) so a second app mounts working auth endpoints instead of
//! re-implementing them. The route builders ([`auth_routes`] / [`auth_admin_routes`])
//! are generic over the app's injected [`ziee_framework::permissions::IdentityResolver`]
//! (fixed to this crate's own `User`/`Group` wire types), so the auth MECHANISM
//! stays pluggable (decision N1) while the wire schema + OpenAPI are byte-identical
//! (decision N2). Gated behind the default-on `routes` cargo feature (which turns on
//! the aide/axum deps); the auth ENGINE compiles without it.

pub mod handlers;
pub mod jwt_extractor;
pub mod routes;
pub mod session_settings;

pub use routes::{auth_admin_routes, auth_routes};
