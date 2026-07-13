//! Minimal placeholder `ServerConfig` for the SDK foundation.
//!
//! This is a deliberate stub. The real Config split — where ziee's monolithic
//! `Config` composes `ServerConfig` + domain sub-configs, and `ModuleContext`
//! carries only `ServerConfig` — lands in Chunk B2. Until then this type exists
//! only so downstream SDK crates can name it; ziee keeps its own concrete
//! `core::config::ServerConfig` unchanged and does NOT yet consume this one.

use serde::Deserialize;

/// Placeholder for the framework-level server configuration (host/port/prefix,
/// CORS, JWT boot seed, …). Fields are intentionally empty in B1 and are filled
/// in by the B2 Config split.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServerConfig {}
