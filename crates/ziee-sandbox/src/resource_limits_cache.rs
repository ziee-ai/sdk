//! Process-wide snapshot of the resource-limits singleton (Plan 1 §6).
//!
//! The sandbox hot path (Linux `CgroupScope::create`, mac_vm / wsl2 building
//! `ExecRequest.cgroup`, `build_bwrap_argv` prlimit literals) reads these
//! values on **every** `execute_command`. A round-trip to Postgres per call
//! would add measurable latency and a lot of log noise; instead we cache the
//! row in a `RwLock<Arc<...>>` and invalidate on PUT.
//!
//! Why not on `CodeSandboxState`: the hot-path callers (cgroup module,
//! build_bwrap_argv) don't all carry the state — adding it would ripple
//! through every backend's `run` signature. The cache here is module-global
//! by design, owned and invalidated only through this file's public API.
//!
//! Concurrency model: `std::sync::RwLock<Arc<CodeSandboxResourceLimits>>`.
//! Reads are uncontended; writes happen only on first-load + admin PUT. The
//! `Arc` clone on read means callers never hold the lock during their work.

use std::sync::{Arc, OnceLock, RwLock};

use ziee_core::AppError;
use crate::provider::ResourceLimitsProvider;
use crate::resource_limits::CodeSandboxResourceLimits;

static CACHE: OnceLock<RwLock<Arc<CodeSandboxResourceLimits>>> = OnceLock::new();

/// Get the current limits — loading from DB on first call, returning the
/// cached snapshot thereafter. `Arc` so callers can drop the lock immediately
/// and pass the snapshot wherever they need.
///
/// The DB read is injected via [`ResourceLimitsProvider`] (the ziee
/// `ZieeResourceLimitsProvider` calls `Repos.code_sandbox.get_resource_limits`),
/// so this cache stays build-DB-free. Callers hold `state.limits` and pass
/// `state.limits.as_ref()`.
pub async fn get(
    limits: &dyn ResourceLimitsProvider,
) -> Result<Arc<CodeSandboxResourceLimits>, AppError> {
    // Fast path: already loaded.
    if let Some(rw) = CACHE.get() {
        return Ok(rw.read().expect("resource_limits_cache RwLock").clone());
    }
    // Slow path: first call after process start. Read from DB, install into
    // the OnceLock. Race-tolerant: if another caller wins, we just clone
    // their value (the OnceLock::set call no-ops on the loser, and we then
    // hit the fast path below).
    let row = limits.load_from_db().await?;
    let arc = Arc::new(row);
    let _ = CACHE.set(RwLock::new(arc.clone()));
    Ok(CACHE
        .get()
        .expect("just initialized")
        .read()
        .expect("resource_limits_cache RwLock")
        .clone())
}

/// Synchronous variant for callers in non-async contexts (e.g. cgroup setup
/// that runs after `spawn_blocking`). Returns the cached snapshot if any;
/// returns the embedded defaults otherwise. Code paths that need
/// always-fresh values should call [`get`] from an async context first to
/// prime the cache.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn snapshot_or_defaults() -> Arc<CodeSandboxResourceLimits> {
    if let Some(rw) = CACHE.get() {
        return rw.read().expect("resource_limits_cache RwLock").clone();
    }
    Arc::new(defaults())
}

/// Replace the cached snapshot. Called by the PUT handler after a successful
/// DB write so the next `execute_command` picks up the new values
/// immediately. No-op if the cache hasn't been primed (the first
/// [`get`] / [`snapshot_or_defaults`] will load the new row anyway).
pub fn invalidate(new_row: &CodeSandboxResourceLimits) {
    if let Some(rw) = CACHE.get() {
        let mut w = rw.write().expect("resource_limits_cache RwLock");
        *w = Arc::new(new_row.clone());
        tracing::info!(
            memory_max_bytes = new_row.memory_max_bytes,
            pids_max = new_row.pids_max,
            cpu_max = %new_row.cpu_max,
            timeout_secs = new_row.timeout_secs,
            "code_sandbox: resource-limits cache invalidated"
        );
    }
}

/// Hard-coded fallback used when the DB is unavailable (e.g. very early in
/// boot, before migrations ran in dev). MUST match the SQL DEFAULTs in
/// migration 41 so behavior is identical on a fresh install whether or not
/// the cache primed yet.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn defaults() -> CodeSandboxResourceLimits {
    let now = chrono::Utc::now();
    CodeSandboxResourceLimits {
        memory_max_bytes: 512 * 1024 * 1024,
        memory_swap_max_bytes: 0,
        pids_max: 256,
        cpu_max: "100000 100000".to_string(),
        address_space_bytes: 4 * 1024 * 1024 * 1024,
        fsize_bytes: 256 * 1024 * 1024,
        nproc_max: 256,
        nofile_max: 1024,
        cpu_secs_max: 1240,
        timeout_secs: 620,
        vm_idle_evict_secs: 900,
        mac_vm_vcpus: 2,
        mac_vm_ram_mib: 2048,
        vm_max_concurrent_execs: 3,
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_expected_baseline() {
        // Mirrors the SQL DEFAULTs in migration 41 + the in-protocol
        // `CgroupLimits::default_policy()` (asserted against from the
        // protocol crate's own test on platforms that depend on it).
        let d = defaults();
        assert_eq!(d.memory_max_bytes, 512 * 1024 * 1024);
        assert_eq!(d.memory_swap_max_bytes, 0);
        assert_eq!(d.pids_max, 256);
        assert_eq!(d.cpu_max, "100000 100000");
        assert_eq!(d.address_space_bytes, 4 * 1024 * 1024 * 1024);
        assert_eq!(d.fsize_bytes, 256 * 1024 * 1024);
        assert_eq!(d.nproc_max, 256);
        assert_eq!(d.nofile_max, 1024);
        assert_eq!(d.timeout_secs, 620);
        assert_eq!(d.vm_idle_evict_secs, 900);
        assert_eq!(d.mac_vm_vcpus, 2);
        assert_eq!(d.mac_vm_ram_mib, 2048);
        assert_eq!(d.vm_max_concurrent_execs, 3);
    }

    /// On platforms where `sandbox-vm-protocol` is in scope (macOS / Windows),
    /// the cache defaults must match the protocol's `default_policy()` so
    /// the host argv (prlimit) and the guest cgroup don't disagree.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn defaults_match_protocol_default_policy() {
        let d = defaults();
        let p = sandbox_vm_protocol::CgroupLimits::default_policy();
        assert_eq!(d.memory_max_bytes as u64, p.memory_max_bytes);
        assert_eq!(d.memory_swap_max_bytes as u64, p.memory_swap_max_bytes);
        assert_eq!(d.pids_max as u64, p.pids_max);
        assert_eq!(d.cpu_max, p.cpu_max);
    }
}
