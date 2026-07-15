//! Cross-platform sandbox backend seam (Plan 1).
//!
//! Everything in `code_sandbox` runs commands, mounts the rootfs, evicts a
//! flavor, and shuts down through [`active()`] — without knowing *where* bwrap
//! actually executes:
//!
//! | Backend            | bwrap runs            | gate                       |
//! |--------------------|-----------------------|----------------------------|
//! | `LinuxBwrapBackend`| host, directly        | `cfg(target_os = "linux")` |
//! | `MacVmBackend`     | inside a libkrun VM   | `cfg(target_os = "macos")` |
//! | `Wsl2Backend`      | inside a WSL2 distro  | `cfg(target_os = "windows")`|
//!
//! **Key invariant:** `sandbox::build_bwrap_argv` and the rootfs *fetch*
//! coordination (`runtime_fetch`) stay shared and OS-independent — every
//! backend produces the identical bwrap argv; they differ only in where it is
//! spawned and how the rootfs is mounted. This module is the seam that lets the
//! whole crate compile on all three OSes (the Linux-only primitives —
//! `pre_exec`, `libc::prctl`/`kill`, the seccomp pipe — live behind the same
//! `cfg(target_os = "linux")` gate as `LinuxBwrapBackend`).

use std::path::Path;

use async_trait::async_trait;
use once_cell::sync::Lazy;

use ziee_core::AppError;
use crate::sandbox_config::CodeSandboxConfig;
use crate::provider::{EnsureOutcome, EvictOutcome};
use crate::sandbox::SandboxRunResult;
use crate::types::{CodeSandboxState, HostCapabilities, SandboxContext};

#[cfg(target_os = "linux")]
mod linux_bwrap;
// Shared host-side client for the in-guest agent (vsock/unix on macOS,
// AF_HYPERV vsock on Windows) — used by both VM backends.
// Also compiled under `test` on every platform so the untrusted-`rel_path`
// path-confinement unit test (`write_artifact`) runs in CI on Linux. This adds
// `test` ONLY to the cfg predicate — the production (non-test) build on Linux
// is byte-identical (module still excluded); macOS/Windows unchanged.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
mod vm_client;
// Long-lived multiplexed session over a single agent connection —
// the host side of the MCP-in-sandbox protocol extension. Pure
// Rust+tokio, intentionally not platform-gated: the type needs to
// exist on Linux too so `SandboxBackend::open_long_lived_session`
// can have a uniform return signature (Linux returns Ok(None)).
pub mod vm_long_lived;
#[cfg(target_os = "macos")]
mod mac_vm;
// AF_HYPERV (Hyper-V vsock) FFI + WSL utility-VM resolution (HIGH-1 fix).
#[cfg(target_os = "windows")]
mod hvsocket;
#[cfg(target_os = "windows")]
mod wsl2;
// LocalSystem helper service: brokers the privileged WSL ops (VmId
// resolution + vsock-port registration) so the unprivileged server never
// needs Hyper-V Admin or runtime UAC. Mirrors Docker Desktop's model.
#[cfg(target_os = "windows")]
pub(crate) mod helper_service;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;

/// The execution + lifecycle operations that differ by host OS. Selected once
/// at process start via `cfg` and reached through [`active()`].
#[async_trait]
pub trait SandboxBackend: Send + Sync {
    /// One-time boot probe. Cheap, host-only, sync (sub-10 ms). `None` means
    /// "this host cannot run the sandbox" — `init()` then logs the reason and
    /// skips MCP-row registration entirely (the rest of the server boots
    /// fine). Per-backend prerequisites:
    ///   - Linux: `bwrap` on PATH, cgroup probe, seccomp-filter compile.
    ///   - macOS: launcher reachable + arch is aarch64 (libkrun is Apple-Silicon).
    ///   - Windows: `wsl.exe` present + reports WSL v2 (v1-only refuses).
    ///   - Unsupported OS: always `None`.
    ///
    /// The returned `HostCapabilities` is consumed by the Linux path
    /// (`runtime_mount` re-uses `bwrap_path` for the lazy PID-ns probe); on the
    /// VM backends it is an opaque placeholder — the real *guest* caps are
    /// rebuilt per-call in `ensure_rootfs_ready` / `run`.
    fn probe_host(&self, cfg: &CodeSandboxConfig) -> Option<HostCapabilities>;

    /// Make the requested flavor's rootfs available (fetch if missing, then
    /// mount in whatever way this backend mounts). The fetch half is shared
    /// (`runtime_fetch`); only the mount differs per backend.
    async fn ensure_rootfs_ready(
        &self,
        state: &CodeSandboxState,
        flavor: &str,
    ) -> Result<EnsureOutcome, AppError>;

    /// Run `command` in an isolated sandbox against `flavor`'s rootfs. The argv
    /// (`sandbox::build_bwrap_argv`) is identical across backends; this method
    /// owns *where* it runs (host / microVM / WSL2) and capture/timeout.
    async fn run(
        &self,
        state: &CodeSandboxState,
        ctx: &SandboxContext,
        command: &str,
        timeout_secs: Option<u64>,
        flavor: &str,
    ) -> Result<SandboxRunResult, AppError>;

    /// Like [`run`], but threads workflow-runner extra mounts through to
    /// `build_bwrap_argv` (B4). Used by `workflow::dispatch::SandboxDispatcher`
    /// to bind the per-run `workflow/` (RO) + per-step `artifacts/<step_id>/`
    /// (RW) dirs into the bwrap mount tree. Default implementation falls
    /// through to `run` (ignoring mounts) — the VM backends override
    /// only when the workflow runtime grows cross-OS support; Phase 1
    /// runs the workflow runtime Linux-only.
    async fn run_with_mounts(
        &self,
        state: &CodeSandboxState,
        ctx: &SandboxContext,
        command: &str,
        timeout_secs: Option<u64>,
        flavor: &str,
        _extra_mounts: &[crate::workflow_staging::StagedMount],
        // Live-progress sink (workflow sandbox step). When `Some`, the backend
        // binds the `/ziee/progress` FIFO into the sandbox and forwards each
        // newline-trimmed line (one raw FIFO write) to this sender. The Linux
        // backend reads a host FIFO; the VM backends route the agent's
        // `Frame::ProcessProgress`. `None` for every chat/MCP exec.
        _progress_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    ) -> Result<SandboxRunResult, AppError> {
        // Default: behave like `run`, ignoring extra mounts + progress (VM
        // backends override). Linux backend overrides to actually wire both.
        self.run(state, ctx, command, timeout_secs, flavor).await
    }

    /// Whether this backend actually binds the `extra_mounts` passed to
    /// `run_with_mounts` (vs. the default which silently ignores them).
    ///
    /// The Linux backend binds them directly. The VM backends (macOS libkrun,
    /// Windows WSL2) do NOT yet — host folders would need, respectively, an
    /// extra virtio-fs share (launcher + guest-agent change) or a `/mnt/<drive>`
    /// 9p bind (a consented MED-1 carve-out). Until that lands, callers use
    /// this to report host mounts as "unsupported on this backend" rather than
    /// silently dropping them. See feature #3 Part B follow-ups.
    fn supports_extra_mounts(&self) -> bool {
        false
    }

    /// Tear down anything this backend owns (FUSE daemons / VMs / distros).
    async fn shutdown(&self);

    /// Evict a specific (version, flavor) mount after the version
    /// manager's drain task observed the inflight counter hit zero
    /// (Plan 5 Phase 3). Default impl is version-aware — it kills
    /// only the matching `(version, flavor)` squashfuse via
    /// `runtime_mount::evict_by_version_flavor`, leaving any sibling
    /// version of the same flavor (e.g. the new pin) alive.
    ///
    /// VM-backed overrides (mac_vm, wsl2) layer per-version cleanup
    /// on top of this default (libkrun VM teardown, `wsl --unregister`).
    async fn evict_artifact(
        &self,
        mount_dir: &Path,
        flavor: &str,
        version: &str,
    ) -> EvictOutcome {
        // `mount_dir` is `<cache_root>/<version>/<asset_stem>`; the
        // parent is the per-version cache subdir that evict_by_version_flavor
        // expects (it'll find + remove the per-flavor `.squashfs` next to it).
        //
        // Engine-carve seam (evict/shutdown reachability): the version-aware
        // squashfuse eviction owns the ziee-side `runtime_mount` mount statics
        // (READY/MOUNTED), so it stays in the server crate behind the injected
        // `RootfsProvider`. Reach it through the engine-owned global state
        // (`config::get_state`) so this default impl's signature is unchanged
        // (the un-compilable-on-Linux mac_vm/wsl2 overrides keep their VM
        // teardown). No `crate::runtime_mount` symbol is named here.
        let version_cache_dir = mount_dir.parent().unwrap_or(mount_dir);
        match crate::config::get_state() {
            Some(state) => {
                state
                    .rootfs
                    .evict_by_version_flavor(version_cache_dir, version, flavor)
                    .await
            }
            None => EvictOutcome {
                bytes_freed: 0,
                was_cached: false,
            },
        }
    }


    /// Open a long-lived multiplexed session for a sandboxed MCP
    /// stdio server using `flavor`. The MCP client holds the returned
    /// `LongLivedSession` for the server's lifetime; dropping it tears
    /// down the underlying connection (and, on VM backends, releases
    /// the per-flavor inflight guard that keeps the VM warm).
    ///
    /// Backends:
    ///   - **Linux** — `Ok(None)` (default). The Linux MCP path spawns
    ///     bwrap directly via `Command::new(bwrap_path).args(argv)`;
    ///     no agent is involved.
    ///   - **macOS** — ensure the flavor's libkrun VM is warm, dial a
    ///     fresh `UnixStream` against its vsock bridge socket, wrap in
    ///     `open_long_lived_with_guard`. Returns `Ok(Some(session))`.
    ///   - **Windows (WSL2)** — equivalent dial against the AF_HYPERV
    ///     vsock; same wrap.
    ///   - **Unsupported** — `Ok(None)` (the caller falls back to a
    ///     friendly "sandbox not available" error).
    ///
    /// Default implementation returns `Ok(None)` so backends that
    /// can't or shouldn't expose a session don't need to override.
    async fn open_long_lived_session(
        &self,
        state: &CodeSandboxState,
        flavor: &str,
    ) -> Result<Option<vm_long_lived::LongLivedSession>, AppError> {
        let _ = (state, flavor);
        Ok(None)
    }

    /// Ensure the per-server MCP workspace exists where the long-lived bwrap
    /// argv binds it (`/workspace/mcp/<server_id>` → `/home/sandboxuser`).
    ///
    /// Default no-op: on macOS the host workspace is virtio-fs-shared at
    /// `/workspace`, and on the Linux host path bwrap binds the host dir
    /// directly — so the bind source already exists. The WSL2 backend has no
    /// virtio-fs, so it overrides this to create + rsync the per-server
    /// workspace into the distro before the spawn (mirroring the one-shot
    /// `run` path's `sync_workspace_in`). Without it, bwrap fails with
    /// "Can't find source path /workspace/mcp/<id>".
    async fn prepare_mcp_vm_workspace(
        &self,
        state: &CodeSandboxState,
        flavor: &str,
        server_id: uuid::Uuid,
    ) -> Result<(), AppError> {
        let _ = (state, flavor, server_id);
        Ok(())
    }

    /// **Test-only seam.** Execute an arbitrary `bwrap` argv against the
    /// given rootfs and return the raw stdout/stderr/exit. Tier-4 tests
    /// use this to verify the hardening primitives.
    #[allow(dead_code)]
    async fn exec_raw_argv(
        &self,
        argv: Vec<String>,
        rootfs_squashfs: &Path,
        timeout: std::time::Duration,
    ) -> Result<RawExecResult, AppError>;
}

/// Captured output of a raw bwrap execution. Used by tier-4 hardening
/// tests via the per-backend `exec_raw_argv` seam.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RawExecResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// True if killed by the wall-clock timeout (`timeout` arg expired).
    pub timed_out: bool,
}

#[cfg(target_os = "linux")]
static ACTIVE: Lazy<linux_bwrap::LinuxBwrapBackend> =
    Lazy::new(linux_bwrap::LinuxBwrapBackend::new);
#[cfg(target_os = "macos")]
static ACTIVE: Lazy<mac_vm::MacVmBackend> = Lazy::new(mac_vm::MacVmBackend::new);
#[cfg(target_os = "windows")]
static ACTIVE: Lazy<wsl2::Wsl2Backend> = Lazy::new(wsl2::Wsl2Backend::new);
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
static ACTIVE: Lazy<unsupported::UnsupportedBackend> =
    Lazy::new(unsupported::UnsupportedBackend::new);

/// The backend for this host OS. Resolved once via `cfg`.
pub fn active() -> &'static dyn SandboxBackend {
    &*ACTIVE
}
