//! `ziee-sandbox` — the build-DB-free sandbox ENGINE carved out of the ziee
//! server's `code_sandbox` module.
//!
//! This crate owns the OS-independent hardening core (`sandbox::build_bwrap_argv`
//! + cgroup/prlimit/seccomp emission), the per-OS backend seam (`backend/`), the
//! in-memory mount registry (`registry`), the resource-limits value types +
//! process-wide cache, and the value/vocabulary types the ziee server's DB/HTTP
//! halves re-import.
//!
//! It names NO `crate::modules::…` (ziee server) symbol: everything that would
//! require a DB (`sqlx::query!`), an app module (`lit_search`, `handlers`,
//! `runtime_mount`, `runtime_fetch`, `version_manager` DB half), or the app-data
//! dir is injected through the three provider seams in `provider` (`RootfsProvider`,
//! `ResourceLimitsProvider`, `GuestAgentProvider`) held on `CodeSandboxState`, or
//! reached through the engine-owned global state (`config::get_state`).

pub mod backend;
pub mod cgroup;
pub mod config;
pub mod mcp_spawn;
pub mod models;
pub mod mount_provider;
pub mod probes;
pub mod provider;
pub mod registry;
pub mod resource_limits;
pub mod resource_limits_cache;
pub mod sandbox;
pub mod sandbox_config;
pub mod tools;
pub mod types;
pub mod workflow_staging;
