//! `ziee-core` — foundation crate (build-DB-free).
//!
//! Holds the platform foundation moved out of the ziee server crate in Chunk
//! B1: the `AppError`/`ApiResult` error surface, the core `#[macro_export]`
//! macros (incl. `sse_event_enum!`), the base app-state globals (with a
//! configurable app-name), and a placeholder `ServerConfig`.
//!
//! ziee consumes these via equivalence-preserving re-export shims (decision
//! N2), so its ~323 `crate::common::AppError` call sites stay unchanged.

pub mod app_state;
pub mod config;
pub mod error;
pub mod macros;

pub use config::ServerConfig;
pub use error::{ApiError, ApiResult, AppError};
