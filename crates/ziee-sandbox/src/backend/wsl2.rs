//! Windows backend: bwrap runs inside a per-flavor WSL2 distro (Plan 1 §3).
//!
//! Architecture (full parity with the macOS backend — same `sandbox-guest-agent`,
//! same control protocol, same `build_bwrap_argv` hardening; only the transport
//! and the "VM" differ):
//!   - The flavor rootfs is fetched as a **`.tar.zst`** (the squashfs can't be
//!     `wsl --import`ed) and registered as a distro: `wsl --import
//!     ziee-sandbox-<flavor>-v<schema> <dir> <tarball> --version 2`. The imported
//!     distro filesystem **is** the flavor rootfs (R/torch/etc.).
//!   - A one-time provision step (run as root in the distro) installs `bwrap`,
//!     drops in the `ziee-sandbox-agent` binary, writes the synthetic
//!     passwd/group, and installs a **narrow AppArmor profile** that grants
//!     `userns` only to `/usr/bin/bwrap` — leaving Ubuntu 24.04/noble's
//!     `kernel.apparmor_restrict_unprivileged_userns` defense in place for
//!     everything else. The profile + sysctls are re-applied on every VM boot
//!     via `/etc/wsl.conf` `[boot] command =` (WSL has no systemd by default;
//!     [microsoft/WSL#4232]).
//!   - The agent is started inside the distro listening on an **AF_VSOCK**
//!     port. The Windows host dials it via Hyper-V vsock
//!     (`AF_HYPERV`/`HV_PROTOCOL_RAW`) — point-to-point, host ⟷ this-guest,
//!     so no other distro in the shared utility VM can reach the agent. The
//!     Windows-side hvsocket is additionally DACL'd per-user by HCS
//!     (`HcsVirtualMachine.cpp:245-254`: `D:P(A;;FA;;;SY)(A;;FA;;;<user-sid>)`).
//!     Per `execute_command` this backend dials vsock + sends the bwrap argv
//!     (guest paths) — the agent applies the shared seccomp + cgroup
//!     in-guest, identical to macOS.
//!
//! ## Threat model (cross-platform delta with the Linux backend)
//!
//!   - **Cross-distro reachability (HIGH-1, closed):** WSL2 distros share
//!     **one network namespace** in the utility VM ([microsoft/WSL#4304],
//!     `init/main.cpp:2283` — no `CLONE_NEWNET`), so an earlier
//!     `127.0.0.1:<port>` listener was reachable from every other distro the
//!     user had installed. We switched the agent transport to **AF_VSOCK**;
//!     vsock is point-to-point so cross-distro reachability is now
//!     structurally impossible.
//!   - **`networkingMode = mirrored` collapses host↔guest loopback:** in
//!     mirrored mode (Win11 22H2+) the Windows host's `127.0.0.1` services
//!     (Postgres, the Ziee server itself, …) are reachable from inside the
//!     sandbox because of `--share-net`. `probe_host` warn-logs when this
//!     mode is detected in `%USERPROFILE%\.wslconfig`.
//!   - **9p workspace bind (MED-1 — reopened, by decision):** bwrap binds the
//!     workspace (and host-folder mounts) from their `/mnt/<drive>/…` 9p paths
//!     directly, so sandboxed code does file ops over the WSL2 kernel's plan9
//!     client. This is the same "bind the host path, no copy" model as Linux
//!     (native bind) and macOS (virtio-fs), and it's what makes host-folder
//!     mounting of multi-GB files possible (a copy would defeat the point).
//!
//!     History: an earlier design (Plan 1 §3) rsync-copied the workspace into
//!     the distro's ext4 (`/var/lib/ziee/workspace/<conv>/`) so sandboxed code
//!     never touched 9p — defense against the WSL Plan 9 BSOD class ([McAfee
//!     Labs WSL Plan 9 BSOD research] + [CVE-2026-43053]). That bug class is
//!     **now patched**, so the rsync-to-ext4 indirection was removed (2026-06)
//!     in favor of the direct 9p bind. Residual posture: 9p is again reachable
//!     from sandboxed code — acceptable because (a) the acute BSOD CVE is
//!     fixed, and (b) the sandbox threat model already excludes kernel 0-days
//!     (it is not a hostile-multi-tenant boundary without an outer microVM).
//!     ⚠️ Re-validate on Windows: drvfs/9p must let the sandbox uid (1001) read
//!     and write the bound workspace (default drvfs presents mode 0777; confirm
//!     on the target WSL build).
//!
//! ⚠️ **Validation status:** this file cannot be compiled or run on Linux (it is
//! `cfg(target_os = "windows")`). It is grounded in the documented WSL2 + agent
//! behavior + the prior-art deep read in `.sec-audits/`, but must be compiled
//! and validated on Windows 11 + WSL2. Points flagged `WIN-TODO` need
//! first-run attention.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use sandbox_vm_protocol::{CgroupLimits, ExecRequest, PROTOCOL_VERSION};
use tokio::sync::{Mutex, Semaphore};
use windows_sys::core::GUID;

use super::helper_service;
use super::hvsocket;

use super::SandboxBackend;
use ziee_core::AppError;
use crate::sandbox_config::CodeSandboxConfig;
use crate::resource_limits_cache;
use crate::provider::{EnsureOutcome, EvictOutcome, RootfsFormat};
use crate::sandbox::{
    self, SandboxRunResult, SYNTHETIC_GROUP, SYNTHETIC_PASSWD,
};
use crate::types::{
    CgroupMode, CodeSandboxState, HardeningCapabilities, HostCapabilities, PidNsMode,
    SandboxContext, SeccompMode,
};
/// Composite key used by `DISTROS` + `BOOT_LOCKS` so two pinned rootfs
/// versions of the same flavor (the new pin + a draining old pin
/// during a swap) can coexist without colliding in either registry.
/// Plan 5 Phase 3.
fn distro_key(version: &str, flavor: &str) -> String {
    format!("{version}/{flavor}")
}

// ── Guest contract (paths INSIDE the imported distro) ────────────────────────
// The distro filesystem is the flavor rootfs, so bwrap's rootfs_dir is "/"
// (`build_bwrap_argv` binds `{rootfs}/usr` → `/usr`; `//usr` == `/usr`).
const GUEST_ROOTFS_MOUNT: &str = "/";
const GUEST_BWRAP_PATH: &str = "/usr/bin/bwrap";
const GUEST_AGENT_PATH: &str = "/usr/local/bin/ziee-sandbox-agent";
// Fixed fd the agent dup2's the seccomp BPF pipe to in the bwrap child (matches
// the macOS backend + the agent's GUEST_SECCOMP_FD). Out of the stdio range.
const GUEST_SECCOMP_FD: i32 = 10;
const GUEST_PASSWD: &str = "/etc/ziee-sandbox-passwd";
const GUEST_GROUP: &str = "/etc/ziee-sandbox-group";
/// Empty regular file provisioned in the distro for the
/// `DANGEROUS_DOTFILES` masks. MUST be a normal file — see
/// `build_bwrap_argv::mask_path` doc for why `/dev/null` doesn't work
/// (bwrap's `--ro-bind` inherits `nodev`).
const GUEST_EMPTY: &str = "/etc/ziee-sandbox-empty";
// Marker written at the end of a successful provision so re-boots skip it.
// Bumped to v3 when the provisioning surface changed:
//   v1 → original baseline.
//   v2 → narrow AppArmor profile + /etc/wsl.conf [boot] command (replaced
//        global apparmor_restrict_*=0 sysctl).
//   v3 → rsync + /var/lib/ziee/workspace root (MED-1 9p-workspace fix —
//        sandboxed code no longer touches /mnt 9p). NOTE (2026-06): the
//        execute_command path later DROPPED that rsync for a direct 9p bind
//        (the 9p BSOD CVE is patched); the provisioning surface itself is
//        unchanged, so the sentinel is not bumped for it.
// Older distros with a lower-version sentinel get re-provisioned on the next
// boot — earlier state is otherwise indistinguishable, and re-running
// provisioning is idempotent.
const PROVISION_SENTINEL: &str = "/etc/ziee-sandbox-provisioned-v4";

const GUEST_APPARMOR_PROFILE: &str = "/etc/apparmor.d/bwrap";
const GUEST_SYSCTL_CONF: &str = "/etc/sysctl.d/99-ziee-sandbox.conf";
const GUEST_WSL_CONF: &str = "/etc/wsl.conf";

/// VESTIGIAL: was the root of the in-distro ext4 per-conversation workspaces
/// back when each `execute_command` rsync-copied the Windows-host workspace
/// in/out (the old MED-1 design). The workspace is now bound directly from its
/// `/mnt/<drive>` 9p path (see `run_with_mounts`), so this dir is no longer
/// used. The provisioner still pre-creates it — a harmless empty dir; kept to
/// avoid editing the delicate one-time provisioning script.
const GUEST_WORKSPACE_ROOT: &str = "/var/lib/ziee/workspace";

/// Narrow AppArmor profile. Grants `userns` ONLY to `/usr/bin/bwrap`, leaving
/// Ubuntu 24.04/noble's `kernel.apparmor_restrict_unprivileged_userns` in
/// force for every other binary. The earlier wsl2 implementation flipped that
/// sysctl to `0` globally — that disables Ubuntu's documented mitigation
/// against in-userns kernel exploits for the WHOLE distro, including any
/// sandbox-escaped process. This profile keeps the kernel-level restriction
/// and only carves out our explicit bwrap binary.
///
/// `flags=(unconfined)` was REMOVED (audit H-1) — under that flag the profile
/// is no-op, so the kernel's `apparmor_restrict_unprivileged_userns` either
/// blocks bwrap (sandbox broken) or grants userns globally (the very thing
/// this profile was trying to scope). The Canonical-documented recipe is the
/// bare-profile body with the explicit `userns` capability; that's a
/// *confined* profile that grants exactly the one capability bwrap needs.
const APPARMOR_BWRAP_PROFILE: &str = "abi <abi/4.0>,\n\
include <tunables/global>\n\
\n\
profile bwrap /usr/bin/bwrap {\n\
  userns,\n\
  include if exists <local/bwrap>\n\
}\n";

/// `/etc/wsl.conf` we install. WSL has no systemd by default, so the standard
/// systemd-sysctl / apparmor.service paths don't run on boot — sysctls in
/// `/etc/sysctl.d/` are NEVER re-applied ([microsoft/WSL#4232], open since
/// 2019), and any AppArmor profile we wrote is unloaded on `wsl --shutdown`.
/// `[boot] command` IS reliably run as root via `execl("/bin/sh", "sh", "-c",
/// Command, …)` (`microsoft/WSL: src/linux/init/config.cpp:1002`) on every VM
/// boot — that's where the re-apply lives.
const WSL_CONF_CONTENT: &str = "# Managed by ziee-sandbox (Plan 1 §3) — DO NOT EDIT.\n\
# Re-applies the narrow bwrap AppArmor profile + sysctls on every VM boot.\n\
# WSL2 has no systemd by default, so /etc/sysctl.d/* and /etc/apparmor.d/* are\n\
# otherwise NOT applied on boot (microsoft/WSL#4232).\n\
[boot]\n\
command = apparmor_parser -r /etc/apparmor.d/bwrap 2>/dev/null || apparmor_parser /etc/apparmor.d/bwrap 2>/dev/null || true; sysctl --system >/dev/null 2>&1 || true\n";

const SYSCTL_CONF_CONTENT: &str = "# Managed by ziee-sandbox (Plan 1 §3) — DO NOT EDIT.\n\
# The Ubuntu/Debian downstream sysctl that enables unprivileged user-namespace\n\
# creation. May be a no-op on the Microsoft kernel build (which can have it\n\
# always-on); the `2>/dev/null` in the wsl.conf reload makes that case silent.\n\
kernel.unprivileged_userns_clone=1\n";

/// Minimum WSL versions free of CVE-2025-53788 (TOCTOU LPE in the WSL2 kernel
/// code, fixed in 2.5.10 on the 2.5 channel and 2.6.1 on the 2.6 channel).
/// Anything older lets unprivileged code inside the sandbox race the kernel to
/// SYSTEM on the Windows host — we refuse to register.
const WSL_MIN_VERSION_25: (u32, u32, u32) = (2, 5, 10);
const WSL_MIN_VERSION_26: (u32, u32, u32) = (2, 6, 1);

// Per-distro concurrent-exec cap now lives in the runtime-tunable
// `code_sandbox_settings.vm_max_concurrent_execs` row (Plan 1 §6). Same
// knob the macOS backend reads (the contention shape is identical:
// N parallel cgroup-capped execs must not sum past the VM RAM ceiling).
// Defaults match the prior const (3) via the SQL DEFAULT in migration 42.

/// Monotonic request id (matches the macOS B4 fix — avoids cgroup-path
/// collisions a timestamp id risked under concurrency).
static REQ_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Vsock ports we assign to per-flavor agents. Picked from a registered
/// range (10001..10100 — see `scripts/register-sandbox-vsock-ports.ps1`).
/// We seed by PID so two parallel `ziee.exe` processes (e.g. consecutive
/// Tier 6 test invocations, each spawning a fresh server child) don't
/// collide on the same port: stale agents from a prior process can outlive
/// the parent (HIGH-3 / no-PDEATHSIG-across-WSL), and a colliding bind
/// inside the shared WSL utility VM hits EADDRINUSE silently — the new
/// agent exits, but the host's hvsocket connect still succeeds against the
/// STALE listener, executing the new request in the prior conversation's
/// context. PID-seeded offsets reduce the collision odds to (1/100)^N for
/// N parallel processes.
///
/// `Wsl2Backend::new()` initializes the atomic from the PID at startup.
static NEXT_VSOCK_PORT: AtomicU32 = AtomicU32::new(10001);

/// Bottom + count of the registered vsock port range. Must match the
/// `PortStart` and `Count` in `scripts/register-sandbox-vsock-ports.ps1`.
const VSOCK_PORT_BASE: u32 = 10001;
const VSOCK_PORT_COUNT: u32 = 100;

/// A booted, warm per-flavor distro: the imported WSL2 distro plus the agent
/// process listening on an AF_VSOCK port inside the utility VM.
struct DistroHandle {
    /// The `wsl.exe -d <distro> -- agent …` relay child. Killing it terminates
    /// the wsl.exe relay (necessary to free the Windows-side child slot) but
    /// does NOT, on its own, stop the in-distro agent — `Frame::Shutdown` does
    /// (sent from `stop_agent` before the relay-kill backstop). See HIGH-3 in
    /// `.sec-audits/`.
    agent: Mutex<tokio::process::Child>,
    distro: String,
    /// The AF_VSOCK port the agent listens on inside the utility VM.
    vsock_port: u32,
    /// The utility VM's VmId, copied from `Wsl2Backend::wsl_vm_id` so free
    /// functions (`stop_agent`, `evict_dead_distro`, the reaper) can reach it
    /// without holding a backend reference. All distros share one VmId on a
    /// given session.
    vm_id: GUID,
    last_used: Mutex<Instant>,
    /// In-flight exec count — the reaper never evicts a distro mid-command.
    inflight: AtomicUsize,
    /// Bounds concurrent execs in this distro so they can't OOM the WSL2 VM.
    sem: Semaphore,
}

/// Decrements `inflight` on drop so a cancelled `run()` future (aborted chat
/// turn) can't leak the count and wedge the reaper (mirrors macOS B2).
struct InflightGuard(Arc<DistroHandle>);
impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

static REAPER_STARTED: AtomicBool = AtomicBool::new(false);

fn ensure_reaper() {
    if REAPER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            // Idle-evict threshold is runtime-configurable (Plan 1 §6);
            // resolve on every tick so an admin's PUT takes effect within
            // ~1 minute. `0` = never evict.
            let idle_evict_secs = resource_limits_cache::snapshot_or_defaults()
                .vm_idle_evict_secs
                .max(0) as u64;
            if idle_evict_secs == 0 {
                continue;
            }
            let mut distros = DISTROS.lock().await;
            let mut evict = Vec::new();
            // `key` is `<version>/<flavor>` (Plan 5 Phase 3).
            for (key, h) in distros.iter() {
                if h.inflight.load(Ordering::SeqCst) == 0
                    && h.last_used.lock().await.elapsed().as_secs() >= idle_evict_secs
                {
                    evict.push(key.clone());
                }
            }
            for key in evict {
                if let Some(h) = distros.remove(&key) {
                    stop_agent(&h).await;
                    // `wsl --terminate <distro>` stops THIS distro's in-VM init
                    // but does NOT free the shared WSL2 utility VM's RAM — that
                    // requires `wsl --shutdown` (which also kills every other
                    // distro the user has running, so we don't do it here). The
                    // value of `--terminate` is bounding the agent + tearing
                    // down anything the distro side was holding open; cached
                    // page memory in the utility VM only frees if the user
                    // separately runs `wsl --shutdown`. See microsoft/WSL FAQ
                    // + #13291.
                    let _ = run_wsl(&["--terminate", &h.distro]).await;
                    tracing::info!(key = %key, distro = %h.distro, "code_sandbox: WSL2 distro evicted (idle)");
                }
            }
        }
    });
}

/// Per-flavor warm distros.
static DISTROS: Lazy<Mutex<HashMap<String, Arc<DistroHandle>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Per-flavor boot serialization (mirrors macOS B3) — held only during a boot,
/// not during warm reuse, so booting flavor A doesn't block running flavor B.
static BOOT_LOCKS: Lazy<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

async fn boot_lock_for(version: &str, flavor: &str) -> Arc<Mutex<()>> {
    BOOT_LOCKS
        .lock()
        .await
        .entry(distro_key(version, flavor))
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub struct Wsl2Backend {
    /// WSL2 uses a SHARED utility VM for every distro per session, so all
    /// flavors connect to the same VmId. Resolved once via
    /// `hvsocket::wsl_utility_vm_id()` (env override, else `hcsdiag list`),
    /// then cached for the lifetime of the server.
    wsl_vm_id: OnceLock<GUID>,
}

impl Wsl2Backend {
    pub fn new() -> Self {
        // PID-seed the vsock port allocator so two parallel `ziee.exe`
        // processes (e.g. Tier 6 sequential `cargo test` runs that spawn
        // a fresh server per test) start at different ports and don't
        // race for the same vsock binding inside the shared WSL utility
        // VM. See the doc on NEXT_VSOCK_PORT for the failure mode.
        let pid = std::process::id();
        let offset = pid % VSOCK_PORT_COUNT;
        NEXT_VSOCK_PORT.store(VSOCK_PORT_BASE + offset, Ordering::Relaxed);
        Self { wsl_vm_id: OnceLock::new() }
    }

    /// Resolve + cache the WSL2 utility VM's VmId. Resolution order:
    ///   1. cached value (every call after the first),
    ///   2. `ZIEE_WSL_VM_ID=<guid>` env override — the dev/test bypass that
    ///      needs neither the helper service nor Hyper-V Admin,
    ///   3. the LocalSystem **helper service** (`helper_service::client`),
    ///      which does the privileged `hcsdiag`/HCS call as SYSTEM so the
    ///      unprivileged server never needs Hyper-V Admin (no log-out/in).
    ///
    /// When neither the env override nor the helper service is available, this
    /// hard-fails with an install instruction — the sandbox **requires** the
    /// helper on Windows (design decision: no silent fallback to an
    /// in-process `hcsdiag` that would demand Hyper-V Admin on the user token).
    fn vm_id(&self) -> Result<GUID, AppError> {
        if let Some(g) = self.wsl_vm_id.get() {
            return Ok(*g);
        }
        // (2) Dev/test bypass: explicit VmId, no privileged path at all.
        let g = if let Ok(s) = std::env::var("ZIEE_WSL_VM_ID") {
            hvsocket::parse_guid(&s).map_err(|e| {
                AppError::internal_with_id(format!("ZIEE_WSL_VM_ID malformed: {e}"))
            })?
        } else {
            // (3) Normal path: broker through the LocalSystem helper service.
            helper_service::client::resolve_vm_id()?
        };
        // Race-tolerant: another caller may have set it concurrently; we just
        // get our own copy back. set() only fails if already set.
        let _ = self.wsl_vm_id.set(g);
        Ok(*self.wsl_vm_id.get().expect("just set"))
    }

    /// The bundled Linux `ziee-sandbox-agent` binary (Windows-side path),
    /// copied into the distro at provision time. Resolution order:
    ///   1. `ZIEE_SANDBOX_AGENT` env (explicit dev override).
    ///   2. Embedded bundle, extracted to `<app_data>/bin/ziee-sandbox-agent`
    ///      on first use — the production self-contained path, mirrors
    ///      macOS's `code_sandbox::embedded::ensure()`. The build-time
    ///      helper at `build_helper/wsl2_agent.rs` cross-compiles the
    ///      agent into `binaries/x86_64-pc-windows-msvc/sandbox-runtime/`,
    ///      which `wsl2_agent_embedded` `include_bytes!`s.
    ///   3. Sibling of the running exe — legacy dev-path used by Tier 4/6
    ///      tests where `scripts/build-sandbox-agent-linux.sh` drops the
    ///      ELF next to the test binary. Kept as a final fallback so a
    ///      dev box without Docker still works (build.rs's wsl2_agent
    ///      step is fail-soft, leaving an empty placeholder; the embedded
    ///      ensure() then errors and we fall through to here).
    fn agent_host_path() -> PathBuf {
        if let Ok(p) = std::env::var("ZIEE_SANDBOX_AGENT") {
            return PathBuf::from(p);
        }
        // Coupling D seam: `wsl2_agent_embedded.rs` stays in the ziee server
        // crate (its `include_bytes!` reads the SERVER `CARGO_MANIFEST_DIR`);
        // reach its `ensure()` via the injected `GuestAgentProvider`.
        if let Some(agent) = crate::config::get_state()
            .and_then(|s| s.guest_agent.ensure_wsl2().ok().cloned())
        {
            return agent;
        }
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
            .map(|dir| dir.join("ziee-sandbox-agent"))
            .unwrap_or_else(|| PathBuf::from("ziee-sandbox-agent"))
    }

    /// WSL distro name for `(flavor, version)`. Two pinned rootfs
    /// versions of the same flavor get distinct distros so they can
    /// coexist during a drain-on-swap cycle. Plan 5 Phase 3.
    fn distro_name(flavor: &str, version: &str) -> String {
        format!("ziee-sandbox-{flavor}-v{version}")
    }

    /// Per-distro install dir (where `wsl --import` lays down the ext4 vhdx).
    fn import_dir(state: &CodeSandboxState, flavor: &str, version: &str) -> PathBuf {
        state.rootfs.cache_dir().join("wsl").join(Self::distro_name(flavor, version))
    }

    /// Get the warm distro for `(version, flavor)`, importing +
    /// provisioning + starting the agent (single-flight) if needed.
    async fn ensure_distro(
        &self,
        state: &CodeSandboxState,
        flavor: &str,
        tarball: &Path,
        version: &str,
    ) -> Result<Arc<DistroHandle>, AppError> {
        let key = distro_key(version, flavor);
        // Fast path: warm distro (don't hold the lock across a boot).
        if let Some(h) = DISTROS.lock().await.get(&key) {
            return Ok(h.clone());
        }
        let boot_lock = boot_lock_for(version, flavor).await;
        let _boot = boot_lock.lock().await;
        if let Some(h) = DISTROS.lock().await.get(&key) {
            return Ok(h.clone());
        }

        let distro = Self::distro_name(flavor, version);

        // 1. Import the distro if it isn't already registered (idempotent).
        if !distro_registered(&distro).await {
            let import_dir = Self::import_dir(state, flavor, version);
            std::fs::create_dir_all(&import_dir)
                .map_err(|e| AppError::internal_with_id(format!("create WSL import dir: {e}")))?;
            run_wsl(&[
                "--import",
                &distro,
                &import_dir.to_string_lossy(),
                &tarball.to_string_lossy(),
                "--version",
                "2",
            ])
            .await
            .map_err(|e| AppError::internal_with_id(format!("wsl --import {distro}: {e}")))?;
        }

        // 2. Provision once (bwrap, agent, identity, AppArmor profile, wsl.conf).
        self.provision_distro(&distro).await?;

        // 2b. Kill any stale agent inside the distro left over from a
        //     previous server process. Required because:
        //       (a) The agent runs INSIDE the distro as a wsl.exe-spawned
        //           child; there is no PDEATHSIG across the WSL boundary,
        //           so when the previous ziee.exe died (test teardown,
        //           crash, …) the in-distro agent kept running.
        //       (b) NEXT_VSOCK_PORT resets to 10001 in each fresh process,
        //           so a new spawn collides with the stale listener and
        //           the new agent exits with `EADDRINUSE`. The host's
        //           connect then reaches the STALE agent, which still
        //           has the prior process's request_id space and
        //           workspace assumptions — wrong-context execs follow.
        //     This is a stronger guarantee than the audit's HIGH-3 fix
        //     (Frame::Shutdown handshake on Drop) — that path is best-
        //     effort and the agent can survive `kill -9 ziee.exe`.
        let _ = run_in_distro(
            &distro,
            "pkill -9 -f /usr/local/bin/ziee-sandbox-agent 2>/dev/null; \
             pkill -9 ziee-sandbox 2>/dev/null; \
             sleep 0.2; true",
        )
        .await;

        // 3. Start the agent on an AF_VSOCK port inside the utility VM. This is
        //    the HIGH-1 fix: vsock is point-to-point (host ↔ this-guest), so
        //    no other distro in the shared utility VM can reach the agent the
        //    way `127.0.0.1:<port>` previously was reachable.
        let vsock_port = NEXT_VSOCK_PORT.fetch_add(1, Ordering::Relaxed);
        let vm_id = self.vm_id()?;
        let agent = tokio::process::Command::new("wsl.exe")
            .args([
                "-d",
                &distro,
                "-u",
                "root",
                "--cd",
                "/",
                "--",
                GUEST_AGENT_PATH,
                "--listen",
                &format!("vsock:{vsock_port}"),
            ])
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AppError::internal_with_id(format!("spawn WSL2 agent: {e}")))?;

        // Wait for the agent's vsock listener to accept. Probe via the same
        // hvsocket API the hot path uses, so a failure here surfaces the exact
        // problem the hot path would hit.
        //
        // 60s, not 30s — observed empirically that even with the GUID
        // registered and a listening agent, fresh `wsl.exe -d <distro>
        // -- agent --listen vsock:N` invocations on a cold WSL utility
        // VM take 20-30s before the host's AF_HYPERV connect actually
        // routes through. The exact mechanism is vmcompute-internal
        // (the `D:P(A;;FA;;;SY)…` DACL on the VM is per-user, so
        // route-table propagation appears to be lazy). The 30s ceiling
        // we copied from the macOS libkrun path was racing this window.
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut last_err: Option<String> = None;
        loop {
            match hvsocket::connect(vm_id, vsock_port).await {
                Ok(_) => break,
                Err(e) => last_err = Some(e.to_string()),
            }
            if Instant::now() > deadline {
                // Same WSA-10060 remediation hint as the test path: the
                // port-template GUID must be registered under
                // HKLM\…\GuestCommunicationServices AND vmcompute must
                // have picked it up since (needs `wsl --shutdown`).
                return Err(AppError::internal_with_id(format!(
                    "WSL2 agent did not start listening on vsock:{vsock_port} \
                     within 30s. Last connect error: {}\n\
                     \n\
                     If you see WSA error 10060, the AF_HYPERV port \
                     template GUID is not registered. Run from an admin \
                     PowerShell:\n\
                     \n  \
                     scripts/register-sandbox-vsock-ports.ps1\n\
                     \n  \
                     (registers ports 10001..10100 + runs `wsl --shutdown`)",
                    last_err.unwrap_or_else(|| "<no error captured>".to_string())
                )));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }

        // Per-distro concurrent-exec cap from the §6 config; resolved at
        // boot time (subsequent admin tunes apply on next cold boot of this
        // flavor). `max(1)` guards a degenerate config value.
        let max_concurrent = resource_limits_cache::snapshot_or_defaults()
            .vm_max_concurrent_execs
            .max(1) as usize;
        let handle = Arc::new(DistroHandle {
            agent: Mutex::new(agent),
            distro,
            vsock_port,
            vm_id,
            last_used: Mutex::new(Instant::now()),
            inflight: AtomicUsize::new(0),
            sem: Semaphore::new(max_concurrent),
        });
        DISTROS.lock().await.insert(key, handle.clone());
        ensure_reaper();
        Ok(handle)
    }

    /// One-time, idempotent in-distro setup. Skips if the sentinel is present.
    ///
    /// Steps (each idempotent on its own — a partial provision rerunning is
    /// safe):
    ///   1. Install the `ziee-sandbox-agent` Linux binary at `GUEST_AGENT_PATH`.
    ///   2. Write the synthetic passwd / group files (via stdin pipes — no
    ///      `WSLENV`, no env crossing the boundary — MED-3).
    ///   3. Write the narrow `/etc/apparmor.d/bwrap` profile (HIGH-2).
    ///   4. Write `/etc/sysctl.d/99-ziee-sandbox.conf` + `/etc/wsl.conf`
    ///      `[boot] command =` so the AppArmor profile + sysctl re-apply on
    ///      every VM boot (HIGH-4 — fixes the [microsoft/WSL#4232] silent
    ///      breakage where sysctls don't re-apply across `wsl --shutdown`).
    ///   5. `apt-get install bubblewrap` if absent.
    ///   6. Apply the AppArmor profile + sysctls LIVE in this boot too (the
    ///      wsl.conf hook handles every SUBSEQUENT boot).
    ///   7. Write the sentinel.
    async fn provision_distro(&self, distro: &str) -> Result<(), AppError> {
        if wsl_test_f(distro, PROVISION_SENTINEL).await {
            return Ok(());
        }

        // 1. Agent binary. The pure-Rust `win_to_wsl_path` does the
        //    Windows→WSL conversion deterministically without spawning
        //    `wslpath` (which isn't available in every distro — Alpine
        //    test rootfs ships without it, since the binary's a separate
        //    `wslu` package). WSL2 auto-mounts /mnt/<drive>/* regardless
        //    of distro init, so the converted path resolves at runtime.
        let agent_host = Self::agent_host_path();
        if !agent_host.exists() {
            return Err(AppError::internal_error(format!(
                "bundled sandbox agent not found at {} (set ZIEE_SANDBOX_AGENT)",
                agent_host.display()
            )));
        }
        let agent_mnt = win_to_wsl_path(&agent_host);
        run_in_distro(
            distro,
            &format!("install -m 0755 '{agent_mnt}' '{GUEST_AGENT_PATH}'"),
        )
        .await?;

        // 2. Synthetic identity, content piped via stdin. Replaces the earlier
        //    WSLENV approach — no env vars cross from Windows. A future
        //    maintainer adding a `WSLENV=…` would silently leak; this design
        //    closes that footgun.
        write_file_into_distro(distro, GUEST_PASSWD, SYNTHETIC_PASSWD, 0o644).await?;
        write_file_into_distro(distro, GUEST_GROUP, SYNTHETIC_GROUP, 0o644).await?;
        // Empty regular file used as the bind source for the DANGEROUS_DOTFILES
        // masks. See GUEST_EMPTY doc + build_bwrap_argv::mask_path.
        write_file_into_distro(distro, GUEST_EMPTY, "", 0o644).await?;

        // 3 + 4. AppArmor profile + sysctl.d + wsl.conf (re-apply on boot).
        run_in_distro(distro, "mkdir -p /etc/apparmor.d /etc/sysctl.d").await?;
        write_file_into_distro(distro, GUEST_APPARMOR_PROFILE, APPARMOR_BWRAP_PROFILE, 0o644).await?;
        write_file_into_distro(distro, GUEST_SYSCTL_CONF, SYSCTL_CONF_CONTENT, 0o644).await?;
        write_file_into_distro(distro, GUEST_WSL_CONF, WSL_CONF_CONTENT, 0o644).await?;

        // 5. Install bwrap + rsync (only step that needs network + can be
        //    slow). rsync is the MED-1 workspace-sync tool — copies the host
        //    workspace into the in-distro ext4 workspace before bwrap fires
        //    + back after exec, so the sandboxed code never touches 9p.
        //    WIN-TODO: prefer baking `bubblewrap` + `rsync` into the flavor
        //    recipe's APT_PACKAGES so this step disappears in v3 of the
        //    rootfs schema.
        run_in_distro(
            distro,
            "missing=''; \
             command -v bwrap >/dev/null 2>&1 || missing=\"$missing bubblewrap\"; \
             command -v rsync >/dev/null 2>&1 || missing=\"$missing rsync\"; \
             if [ -n \"$missing\" ]; then \
               apt-get update -qq && apt-get install -y -qq $missing; \
             fi",
        )
        .await?;

        // 5b. Workspace root: per-conversation subdirs live under here. The
        //     reaper periodically prunes stale ones (see `run` below); for
        //     now we just ensure the parent exists.
        run_in_distro(
            distro,
            &format!("mkdir -p '{GUEST_WORKSPACE_ROOT}' && chmod 0755 '{GUEST_WORKSPACE_ROOT}'"),
        )
        .await?;

        // 5c. Audit H-5: strip setuid/setgid from every binary in the distro
        //     rootfs. `wsl --import` reuses the flavor tarball AS the distro
        //     filesystem itself, so any setuid-root binary it ships (sudo,
        //     mount, passwd, chsh, pkexec, newgrp, …) becomes reachable to
        //     bwrap-spawned workloads. bwrap's `--unshare-user` decouples
        //     in-userns root from host root, so setuid is mostly defanged on
        //     its own — but combined with a mount-ns escape primitive (e.g.
        //     CVE-2022-0185 class) it's a documented escalation vector.
        //     Defense in depth: chmod -s the lot up front, refuse to register
        //     if any survive a re-scan (corrupted dpkg / weird flavor recipe).
        run_in_distro(
            distro,
            "find / -xdev \\( -perm -4000 -o -perm -2000 \\) -type f \
             -exec chmod u-s,g-s {} + 2>/dev/null; \
             remaining=$(find / -xdev \\( -perm -4000 -o -perm -2000 \\) -type f 2>/dev/null | head -5); \
             if [ -n \"$remaining\" ]; then \
               echo \"setuid binaries remain after strip: $remaining\" >&2; \
               exit 1; \
             fi",
        )
        .await?;

        // 6. Apply LIVE for this boot too. `apparmor_parser -r` reloads if
        //    already loaded; plain `apparmor_parser` adds a new profile.
        //    Either-or-true keeps provision idempotent across both states.
        run_in_distro(
            distro,
            "apparmor_parser -r /etc/apparmor.d/bwrap 2>/dev/null \
             || apparmor_parser /etc/apparmor.d/bwrap 2>/dev/null || true; \
             sysctl --system >/dev/null 2>&1 || true",
        )
        .await?;

        // 7. Sentinel.
        run_in_distro(distro, &format!("touch '{PROVISION_SENTINEL}'")).await?;
        Ok(())
    }
}

// `sync_workspace_in` / `sync_workspace_out` were removed: the WSL2 workspace is
// now bound directly from its `/mnt/<drive>` 9p path (like Linux's host bind /
// macOS's virtio-fs share), so there is no in-distro ext4 copy to sync. The
// per-conversation host workspace dir is created by the shared
// `execute_command` path before the backend runs. See the threat-model header
// for the posture change (the `p9rdr.sys`/9p BSOD class that motivated the old
// ext4 design is patched).

/// Run a single bash command inside `distro` as root, erroring on non-zero exit.
async fn run_in_distro(distro: &str, script: &str) -> Result<(), AppError> {
    let status = tokio::process::Command::new("wsl.exe")
        .args(["-d", distro, "-u", "root", "--", "bash", "-c", script])
        .status()
        .await
        .map_err(|e| AppError::internal_with_id(format!("wsl bash -c: {e}")))?;
    if !status.success() {
        return Err(AppError::internal_error(format!(
            "in-distro command failed (exit {:?}): {}",
            status.code(),
            // Truncate so a multiline AppArmor profile / wsl.conf doesn't
            // dominate the error.
            script.chars().take(80).collect::<String>()
        )));
    }
    Ok(())
}

/// Write `content` to `dest_path` inside `distro` via stdin (no env crossings,
/// no temp files on the Windows host, no quoting issues with newlines / shell
/// metacharacters). Replaces the earlier `WSLENV=ZIEE_PASSWD:ZIEE_GROUP`
/// pattern that risked leaking any future credential added there.
async fn write_file_into_distro(
    distro: &str,
    dest_path: &str,
    content: &str,
    mode: u32,
) -> Result<(), AppError> {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;
    let script = format!(
        "umask 077 && cat > '{dest_path}' && chmod {mode:o} '{dest_path}'"
    );
    let mut child = Command::new("wsl.exe")
        .args(["-d", distro, "-u", "root", "--", "bash", "-c", &script])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| AppError::internal_with_id(format!("spawn wsl write {dest_path}: {e}")))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::internal_error("wsl stdin missing"))?;
        stdin
            .write_all(content.as_bytes())
            .await
            .map_err(|e| AppError::internal_with_id(format!("pipe {dest_path}: {e}")))?;
        // Drop closes stdin → cat sees EOF → exits → bash chmods → exits.
    }
    let status = child
        .wait()
        .await
        .map_err(|e| AppError::internal_with_id(format!("wait wsl write {dest_path}: {e}")))?;
    if !status.success() {
        return Err(AppError::internal_error(format!(
            "wsl write {dest_path} failed (exit {:?})",
            status.code()
        )));
    }
    Ok(())
}

/// Translate a Windows host path to its in-distro `/mnt/<drive>` path
/// (e.g. `C:\Users\me\ws\<conv>` → `/mnt/c/Users/me/ws/<conv>`).
///
/// Used to build the bwrap bind *sources* for the workspace and host-folder
/// mounts (`run_with_mounts`) — these are 9p paths the sandboxed code DOES see
/// now (the direct-bind posture; see the file-top threat-model header). Also
/// still used by the MCP-session workspace prep, which keeps its own rsync.
fn win_to_wsl_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        return format!("/mnt/{}{}", drive, &s[2..]);
    }
    s
}

#[async_trait]
impl SandboxBackend for Wsl2Backend {
    fn probe_host(&self, cfg: &CodeSandboxConfig) -> Option<HostCapabilities> {
        // Cheap host-only probe (sub-10 ms): wsl.exe must be on PATH, the
        // default version must be 2 (bwrap needs the WSL2 Linux kernel; WSL1
        // is syscall emulation with no namespaces and never works), AND the
        // WSL runtime must be patched against CVE-2025-53788 (TOCTOU LPE in
        // the WSL2 kernel code, ≥ 2.5.10 / 2.6.1). We deliberately do NOT
        // import a distro / flip sysctls / load AppArmor here — that's
        // `ensure_rootfs_ready`'s job on first `execute_command`, lazy by design.

        // 1. v2-default check.
        let out = match std::process::Command::new("wsl.exe").args(["--status"]).output() {
            Ok(o) => o,
            Err(e) => {
                tracing::error!(
                    "code_sandbox: wsl.exe not found ({e}); sandbox MCP row \
                     will NOT be registered. Install WSL2: `wsl --install`."
                );
                return None;
            }
        };
        let status_text = decode_wsl_output(&out.stdout);
        if !status_text.contains('2') {
            tracing::error!(
                "code_sandbox: WSL2 not the default version on this host; \
                 sandbox MCP row will NOT be registered. Set with \
                 `wsl --set-default-version 2`. Probe output: {status_text:?}"
            );
            return None;
        }

        // 2. CVE-2025-53788 version gate.
        match probe_wsl_version() {
            Ok(v) => {
                if !wsl_version_is_patched(v) {
                    tracing::error!(
                        wsl_version = ?v,
                        "code_sandbox: WSL runtime is older than the \
                         CVE-2025-53788 fix (need {:?} on the 2.5 channel or \
                         {:?} on the 2.6 channel). Run `wsl --update`. \
                         Sandbox MCP row will NOT be registered.",
                        WSL_MIN_VERSION_25, WSL_MIN_VERSION_26
                    );
                    return None;
                }
            }
            Err(e) => {
                // Old WSL releases don't support `wsl --version` at all. Treat
                // as unpatched and refuse.
                tracing::error!(
                    "code_sandbox: could not determine WSL version ({e}); \
                     `wsl --version` may be missing on pre-2.0 WSL. Run \
                     `wsl --update`. Sandbox MCP row will NOT be registered."
                );
                return None;
            }
        }

        // 3. Audit H-8: REFUSE on mirrored networking mode unless the operator
        //    has explicitly opted in. `--share-net` already bridges egress;
        //    mirrored mode additionally collapses the Windows host's
        //    `127.0.0.1` services (Postgres, the Ziee server itself, browser
        //    DevTools, …) into the sandbox's reach. The Ziee API is reachable
        //    on the loopback the sandbox just gained access to, and the user
        //    is already JWT-signed-in for this session — sandbox-escaped or
        //    prompt-injected code can authenticate to the admin API. Was a
        //    warn-log; now hard-refuse + `allow_wsl2_mirrored_mode: true`
        //    operator opt-in.
        if user_wslconfig_uses_mirrored_mode() {
            if cfg.allow_wsl2_mirrored_mode {
                tracing::warn!(
                    "code_sandbox: WSL2 mirrored networking mode is enabled \
                     AND allow_wsl2_mirrored_mode: true is set — sandboxed \
                     commands can reach the Windows host's 127.0.0.1 services \
                     via `--share-net`. Make sure your host services that \
                     listen on loopback are intentionally trusting this."
                );
            } else {
                tracing::error!(
                    "code_sandbox: WSL2 mirrored networking mode is enabled in \
                     .wslconfig. Sandboxed commands could reach the Windows \
                     host's 127.0.0.1 services (Postgres, the Ziee API, …) via \
                     `--share-net` — including the Ziee admin API the user is \
                     JWT-signed-in for. Either switch back to NAT mode in \
                     .wslconfig (remove the `networkingMode=mirrored` line) \
                     OR set code_sandbox.allow_wsl2_mirrored_mode: true to \
                     accept the risk. Sandbox MCP row will NOT be registered."
                );
                return None;
            }
        }

        Some(HostCapabilities {
            bwrap_path: PathBuf::from(GUEST_BWRAP_PATH),
            cgroup: CgroupMode::None,
            seccomp: SeccompMode::NotLinked,
        })
    }

    async fn ensure_rootfs_ready(
        &self,
        state: &CodeSandboxState,
        flavor: &str,
    ) -> Result<EnsureOutcome, AppError> {
        // Shared fetch coordination (OS-independent), but the WINDOWS packaging:
        // the `.tar.zst` that `wsl --import` consumes (Plan 1 §4).
        let cache = state.rootfs.cache_dir();
        let outcome =
            state.rootfs.ensure_fetched_format(&cache, flavor, RootfsFormat::TarZst, Box::new(|_| {}))
                .await
                .map_err(|e| AppError::internal_with_id(format!("rootfs fetch failed: {e}")))?;

        let guest_caps = HardeningCapabilities {
            bwrap_path: PathBuf::from(GUEST_BWRAP_PATH),
            pid_namespace: PidNsMode::Strict,
            // The guest agent applies cgroup v2 from ExecRequest.cgroup (recent
            // WSL2 ships cgroup v2); prlimit in the argv is the backstop. See run().
            cgroup: CgroupMode::None,
            // The agent builds + applies the shared seccomp filter itself.
            seccomp: SeccompMode::NotLinked,
        };
        let artifact_id = outcome.artifact_id;
        let artifact_version = outcome.version.clone();
        crate::registry::register_mount(
            artifact_id,
            &artifact_version,
            std::env::consts::ARCH,
            flavor,
            PathBuf::from(GUEST_ROOTFS_MOUNT),
        );
        Ok(EnsureOutcome {
            caps: Arc::new(guest_caps),
            // The imported distro filesystem IS the rootfs. The rootfs image
            // ships a `/sandbox-rootfs -> .` symlink, so the bwrap argv's
            // `/sandbox-rootfs/usr` resolves to the distro's real `/usr`
            // WITHOUT any bind mount. We deliberately do NOT bind `/` to
            // `/sandbox-rootfs` in the agent (as the macOS libkrun path mounts
            // a squashfs there): binding the mount-ns root makes
            // `unshare(CLONE_NEWUSER)` fail with EPERM, breaking bwrap's
            // `--unshare-user`.
            mount_dir: PathBuf::from(GUEST_ROOTFS_MOUNT),
            fetch_info: Some(outcome),
            artifact_id: Some(artifact_id),
        })
    }

    async fn run(
        &self,
        state: &CodeSandboxState,
        ctx: &SandboxContext,
        command: &str,
        timeout_secs: Option<u64>,
        flavor: &str,
    ) -> Result<SandboxRunResult, AppError> {
        self.run_with_mounts(state, ctx, command, timeout_secs, flavor, &[], None)
            .await
    }

    fn supports_extra_mounts(&self) -> bool {
        // Host folders (and the workspace itself) are bound from their
        // /mnt/<drive> 9p path — WSL auto-mounts Windows drives in the distro.
        true
    }

    async fn run_with_mounts(
        &self,
        state: &CodeSandboxState,
        ctx: &SandboxContext,
        command: &str,
        timeout_secs: Option<u64>,
        flavor: &str,
        extra_mounts: &[crate::workflow_staging::StagedMount],
        progress_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    ) -> Result<SandboxRunResult, AppError> {
        use crate::workflow_staging::StagedMount;

        // Locate (fetch if needed) the flavor tarball; idempotent on cache hit.
        // Capture `version` so `ensure_distro` can name + cache the
        // per-(version, flavor) WSL distro distinctly from any
        // draining old-pin distro (Plan 5 Phase 3).
        let cache = state.rootfs.cache_dir();
        let fetched =
            state.rootfs.ensure_fetched_format(&cache, flavor, RootfsFormat::TarZst, Box::new(|_| {}))
                .await
                .map_err(|e| AppError::internal_with_id(format!("rootfs fetch failed: {e}")))?;
        let tarball = fetched.installed_path;
        let version = fetched.version;

        // Runtime-configurable resource caps (Plan 1 §6). Snapshot once per
        // exec so the host argv (prlimit) and the guest cgroup (via
        // ExecRequest.cgroup) read the same row.
        let limits = resource_limits_cache::get(state.limits.as_ref()).await?;

        // Extra mounts (feature #3 host folders + any workflow staged dirs):
        // translate each Windows host path to its in-distro /mnt/<drive> 9p
        // path and let bwrap bind it directly — same model as the workspace
        // bind below, and as Linux's host bind / macOS's virtio-fs share. No
        // copy, so host-mounted multi-GB files are read in place.
        let guest_mounts: Vec<StagedMount> = extra_mounts
            .iter()
            .map(|m| StagedMount {
                mode: m.mode,
                host_path: PathBuf::from(win_to_wsl_path(&m.host_path)),
                sandbox_path: m.sandbox_path.clone(),
            })
            .collect();

        // Build the bwrap argv with GUEST paths so the agent execs it verbatim —
        // identical hardening to the Linux/macOS backends.
        let guest_caps = HardeningCapabilities {
            bwrap_path: PathBuf::from(GUEST_BWRAP_PATH),
            pid_namespace: PidNsMode::Strict,
            cgroup: CgroupMode::None,
            seccomp: SeccompMode::NotLinked,
        };
        // Workspace bind: the Windows-host workspace is the source of truth
        // (chat-side `tools/files.rs` writes there). bwrap binds its in-distro
        // 9p path (`/mnt/<drive>/…/<conv>`) directly — no rsync — so the
        // sandbox's writes land on the host live, exactly like Linux's host
        // bind and macOS's virtio-fs share, and `version_back` reads the live
        // host workspace at turn end. (This reopens 9p to sandboxed code; the
        // `p9rdr.sys`/9p BSOD class that motivated the old rsync-to-ext4 design
        // is now patched — see the threat-model header.)
        let conv_dir = win_to_wsl_path(&ctx.workspace);
        let guest_ctx = SandboxContext {
            conversation_id: ctx.conversation_id,
            user_id: ctx.user_id,
            workspace: PathBuf::from(&conv_dir),
            files: ctx.files.clone(),
            // Carry the caller-injected extra RO binds (the /lit view) forward
            // onto the guest argv — byte-identical to the pre-lift inline block.
            extra_ro_binds: ctx.extra_ro_binds.clone(),
        };
        let secs = timeout_secs.unwrap_or(limits.timeout_secs.max(1) as u64);
        // Live-progress: when a sink is requested, the agent provisions the FIFO
        // guest-side (`progress: true`) at the guest-local path and bwrap binds
        // it onto `/ziee/progress`; the agent's reader forwards each line as a
        // `Frame::ProcessProgress`, routed to `progress_tx` by `run_on_stream`.
        let want_progress = progress_tx.is_some();
        let progress_fifo_src = want_progress
            .then(|| sandbox_vm_protocol::PROGRESS_GUEST_FIFO_PATH);
        let req = ExecRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: REQ_COUNTER.fetch_add(1, Ordering::Relaxed),
            bwrap_path: GUEST_BWRAP_PATH.to_string(),
            argv: sandbox::build_bwrap_argv(
                &guest_caps,
                &guest_ctx.workspace,
                &guest_ctx,
                Path::new(GUEST_ROOTFS_MOUNT),
                command,
                Path::new(GUEST_PASSWD),
                Path::new(GUEST_GROUP),
                Path::new(GUEST_EMPTY),
                Some(GUEST_SECCOMP_FD),
                &limits,
                &guest_mounts,
                progress_fifo_src,
            ),
            timeout_ms: secs * 1000,
            seccomp_fd: Some(GUEST_SECCOMP_FD),
            progress: want_progress,
            // In-guest cgroup v2 (the agent applies it; prlimit is the backstop).
            // Graceful degradation is handled agent-side: if cgroup v2
            // delegation is missing, `GuestCgroup::create` logs a warn
            // and returns None, and the bwrap argv's prlimit wrapper
            // continues to enforce the limits (see sandbox-guest-agent
            // line ~391). No host-side check needed.
            cgroup: Some(CgroupLimits {
                memory_max_bytes: limits.memory_max_bytes as u64,
                memory_swap_max_bytes: limits.memory_swap_max_bytes as u64,
                pids_max: limits.pids_max as u64,
                cpu_max: limits.cpu_max.clone(),
            }),
            // The artifact-streaming write-back is a macOS-libkrun-only
            // workaround for virtio-fs CREATE-EPERM. On WSL2 the RW binds use
            // the rsync'd distro filesystem directly (no virtio-fs), so RW
            // writes land; leave this empty (the agent no-ops on empty).
            collect_artifacts: Vec::new(),
        };

        // Up to 2 attempts: a dead/unreachable distro (connect fails — the
        // command never ran, so retry is safe) is evicted + re-booted once
        // (mirrors macOS B1). A failure AFTER connect is NOT retried.
        let mut attempt = 0;
        loop {
            attempt += 1;
            let h = self.ensure_distro(state, flavor, &tarball, &version).await?;
            let _permit = h.sem.acquire().await.expect("distro semaphore never closed");
            h.inflight.fetch_add(1, Ordering::SeqCst);
            let _guard = InflightGuard(h.clone());
            *h.last_used.lock().await = Instant::now();

            // No workspace sync: bwrap binds the host workspace's 9p path
            // directly (see `conv_dir` above), so the sandbox reads/writes the
            // Windows-host workspace live — no copy in, no copy out.
            let vm_id = self.vm_id()?;
            let result = match hvsocket::connect(vm_id, h.vsock_port).await {
                Ok(stream) => {
                    // WSL2 copies artifacts out via rsync (not the ArtifactFile
                    // frame), so pass no artifact dirs — but DO forward live
                    // progress (`progress_tx`) for the workflow sandbox step.
                    let r = super::vm_client::run_on_stream_collecting(
                        stream,
                        req.clone(),
                        secs,
                        progress_tx.clone(),
                        &[],
                    )
                    .await;
                    *h.last_used.lock().await = Instant::now();
                    r
                }
                Err(e) if attempt < 2 => {
                    tracing::warn!(
                        flavor,
                        "code_sandbox: WSL2 agent unreachable on vsock ({e}); re-booting and retrying"
                    );
                    drop(_guard);
                    drop(_permit);
                    evict_dead_distro(&version, flavor, &h).await;
                    continue;
                }
                Err(e) => return Err(e),
            };

            // No sync-out: the sandbox wrote straight through 9p to the
            // Windows-host workspace, so chat-side `tools/files.rs` reads and
            // `version_back` already see the writes — nothing to copy back.
            return result;
        }
    }

    async fn shutdown(&self) {
        let mut distros = DISTROS.lock().await;
        for (flavor, h) in distros.drain() {
            stop_agent(&h).await;
            let _ = run_wsl(&["--terminate", &h.distro]).await;
            tracing::info!(
                key = %flavor,
                distro = %h.distro,
                "code_sandbox: WSL2 distro stopped on shutdown"
            );
        }
    }

    /// Version-aware evict (Plan 5 Phase 3 drain-on-swap): tear down
    /// ONLY the `(version, flavor)` distro the drain task observed
    /// finishing.
    async fn evict_artifact(
        &self,
        mount_dir: &Path,
        flavor: &str,
        version: &str,
    ) -> EvictOutcome {
        let key = distro_key(version, flavor);
        if let Some(h) = DISTROS.lock().await.remove(&key) {
            stop_agent(&h).await;
            let _ = run_wsl(&["--unregister", &h.distro]).await;
        } else {
            // Cold but maybe still registered from a prior run.
            let distro = Self::distro_name(flavor, version);
            if distro_registered(&distro).await {
                let _ = run_wsl(&["--unregister", &distro]).await;
            }
        }
        // Delete the cached `<arch>-<flavor>.tar.zst` next to mount_dir
        // (the per-version cache subdir the version manager passed).
        let version_cache_dir = mount_dir.parent().unwrap_or(mount_dir);
        let suffix = format!("-{flavor}.tar.zst");
        let mut bytes_freed = 0u64;
        let mut was_cached = false;
        if let Ok(rd) = std::fs::read_dir(version_cache_dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(&suffix))
                {
                    was_cached = true;
                    if let Ok(m) = std::fs::metadata(&p) {
                        bytes_freed += m.len();
                    }
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
        EvictOutcome { bytes_freed, was_cached }
    }

    async fn exec_raw_argv(
        &self,
        argv: Vec<String>,
        rootfs_squashfs: &Path,
        timeout: std::time::Duration,
    ) -> Result<super::RawExecResult, AppError> {
        // Mirror of mac_vm.rs::exec_raw_argv. The trait param is named
        // `rootfs_squashfs` because Mac/Linux test fixtures publish a
        // squashfs; on Windows `wsl --import` only accepts tarball
        // formats (tar / tar.gz / tar.zst), so we look for a sibling
        // `.tar.zst` next to the squashfs. The build-test-rootfs.sh
        // helper publishes both formats side by side.
        let tarball = resolve_tarball_for_rootfs(rootfs_squashfs)?;
        let distro = self.ensure_test_distro(&tarball).await?;
        let req = ExecRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: 0,
            bwrap_path: GUEST_BWRAP_PATH.to_string(),
            argv,
            timeout_ms: timeout
                .as_millis()
                .min(u64::MAX as u128) as u64,
            // Per mac_vm test pattern: leave seccomp + cgroup unset so
            // tier-4 tests can opt into either via their own argv (e.g.
            // an explicit `--seccomp <fd>` flag) without the seam
            // injecting one.
            seccomp_fd: None,
            cgroup: None,
            // Tier-4 raw-argv harness never exercises live progress.
            progress: false,
            collect_artifacts: Vec::new(),
        };
        let secs = timeout.as_secs().max(1);
        let _permit = distro
            .sem
            .acquire()
            .await
            .expect("test distro semaphore never closed");
        distro.inflight.fetch_add(1, Ordering::SeqCst);
        let _guard = InflightGuard(distro.clone());
        let stream = hvsocket::connect(distro.vm_id, distro.vsock_port)
            .await
            .map_err(|e| {
                AppError::internal_with_id(format!(
                    "connect to test distro vsock:{}: {e}",
                    distro.vsock_port
                ))
            })?;
        let run = super::vm_client::run_on_stream(stream, req, secs).await?;
        Ok(super::RawExecResult {
            exit_code: run.exit_code,
            stdout: run.stdout.into_bytes(),
            stderr: run.stderr.into_bytes(),
            timed_out: run.timed_out,
        })
    }

    async fn open_long_lived_session(
        &self,
        state: &CodeSandboxState,
        flavor: &str,
    ) -> Result<Option<super::vm_long_lived::LongLivedSession>, AppError> {
        // Ensure the per-(version, flavor) distro is provisioned + the
        // agent is up (cold-start path identical to one-shot `run`).
        // MUST fetch the `.tar.zst` packaging, not the squashfs: `wsl
        // --import` only accepts a tarball, and `ensure_distro` passes
        // this straight to it — the squashfs-defaulting `ensure_fetched`
        // would fail every long-lived/MCP spawn with `bsdtar:
        // Unrecognized archive format`.
        let cache = state.rootfs.cache_dir();
        let fetched = state.rootfs.ensure_fetched_format(
            &cache,
            flavor,
            RootfsFormat::TarZst,
            Box::new(|_| {}),
        )
        .await
        .map_err(|e| AppError::internal_with_id(format!("rootfs fetch failed: {e}")))?;
        let tarball = fetched.installed_path;
        let version = fetched.version;
        let h = self.ensure_distro(state, flavor, &tarball, &version).await?;

        // Hold an inflight count for the session's lifetime so the
        // distro reaper waits for live MCP sessions to drain before
        // evicting (same gate the one-shot exec path uses).
        h.inflight.fetch_add(1, Ordering::SeqCst);
        let guard = InflightGuard(h.clone());
        *h.last_used.lock().await = Instant::now();

        let vm_id = self.vm_id()?;
        let stream = hvsocket::connect(vm_id, h.vsock_port).await?;

        let session = super::vm_long_lived::open_long_lived_with_guard(
            stream,
            Some(Box::new(guard)),
        );
        Ok(Some(session))
    }

    /// WSL2: the long-lived MCP bwrap argv binds `/workspace/mcp/<server_id>`
    /// (an in-distro ext4 path) as `/home/sandboxuser`. Create that dir in the
    /// distro and rsync the host workspace into it. Without this, bwrap fails
    /// with "Can't find source path /workspace/mcp/<id>" and the MCP child
    /// never starts.
    ///
    /// NOTE: this is the built-in MCP server's OWN scratch workspace (not the
    /// user's file workspace), and it still uses the in-distro-ext4 + rsync
    /// model. The one-shot `execute_command` path moved to a direct 9p bind
    /// (rsync removed); converting this MCP-session path to a direct 9p bind is
    /// a separate follow-up — it also needs `mcp_spawn::build_guest_mcp_argv`'s
    /// hard-coded `/workspace/mcp/<id>` bind source changed to the 9p path.
    async fn prepare_mcp_vm_workspace(
        &self,
        state: &CodeSandboxState,
        flavor: &str,
        server_id: uuid::Uuid,
    ) -> Result<(), AppError> {
        // Resolve the same pinned rootfs version `open_long_lived_session`
        // used, so we target the matching `ziee-sandbox-<flavor>-v<version>`
        // distro. This is a cache hit: the session that drove us here already
        // fetched the rootfs (distro_name took `(flavor, version)` as of
        // Plan 5 Phase 3, when versioned distros were introduced).
        let cache = state.rootfs.cache_dir();
        let fetched = state.rootfs.ensure_fetched_format(
            &cache,
            flavor,
            RootfsFormat::TarZst,
            Box::new(|_| {}),
        )
        .await
        .map_err(|e| AppError::internal_with_id(format!("rootfs fetch failed: {e}")))?;
        let distro = Self::distro_name(flavor, &fetched.version);
        let host_workspace = state
            .workspace_root
            .join("mcp")
            .join(server_id.to_string());
        let _ = std::fs::create_dir_all(&host_workspace);
        let host_mnt = win_to_wsl_path(&host_workspace);
        // Bind path is `/workspace/mcp/<server_id>` (see
        // mcp_spawn::build_guest_mcp_argv). chmod 1777 so the sandboxed
        // uid 1001 can write into its own /home/sandboxuser (rsync -a would
        // otherwise carry the host dir's perms).
        let dest = format!("/workspace/mcp/{server_id}");
        let script = format!(
            "mkdir -p '{dest}' && rsync -a --delete '{src}/' '{dest}/' && chmod 1777 '{dest}'",
            dest = dest,
            src = host_mnt,
        );
        run_in_distro(&distro, &script).await.map_err(|e| {
            AppError::internal_with_id(format!("mcp vm workspace sync-in failed: {e}"))
        })
    }
}

/// Test-only WSL2 distro pool keyed by tarball path. Used by
/// `exec_raw_argv` so the 30+ tier-4/6 tests in a `cargo test`
/// invocation share one warm distro per (process, rootfs) rather than
/// paying the `wsl --import` + provision cost (~30s) per test.
///
/// Distinct from the production `DISTROS` registry (which is keyed by
/// flavor and reaped on idle). Test distros live for the lifetime of the
/// process; the `kill_on_drop` on the agent `Child` cleans up at exit,
/// and `wsl --unregister` of the test distro happens lazily on the next
/// run if a previous run crashed (the import path is idempotent).
static TEST_DISTROS: Lazy<Mutex<HashMap<PathBuf, Arc<DistroHandle>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Per-tarball test-distro import serialization. Mirrors the per-flavor
/// `BOOT_LOCKS` for production distros.
static TEST_BOOT_LOCKS: Lazy<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

async fn test_boot_lock_for(tarball: &Path) -> Arc<Mutex<()>> {
    TEST_BOOT_LOCKS
        .lock()
        .await
        .entry(tarball.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Derive the WSL distro name used by `exec_raw_argv` for a given
/// tarball path. Distinct namespace (`ziee-sandbox-test-*`) from the
/// production `ziee-sandbox-<flavor>-v<schema>` so a dev who has both
/// warm in the same WSL host can't collide.
fn test_distro_name(tarball: &Path) -> String {
    // Hash the absolute path so the name is stable across `cargo test`
    // runs, distro-name-safe (no path separators / spaces), and short
    // enough that `wsl --import` doesn't choke (WSL caps at 64 chars).
    use sha2::{Digest, Sha256};
    let abs = std::fs::canonicalize(tarball).unwrap_or_else(|_| tarball.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(abs.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    // 12 hex chars is 48 bits — collision-free across any realistic
    // dev's test corpus and keeps the distro name well under WSL's
    // 64-char limit.
    let short = hex::encode(&digest[..6]);
    // Test distros use a hardcoded "test" version suffix — they're
    // tarball-keyed (TEST_DISTROS) so they don't need to coexist with
    // production pinned-version distros (DISTROS).
    format!("ziee-sandbox-test-{short}-vtest")
}

/// Resolve the tarball path to import from a path the trait gave us.
/// The trait parameter is named `rootfs_squashfs` because Mac/Linux test
/// fixtures are squashfs files; the Windows test fixture script must
/// publish a sibling `.tar.zst` (WSL2 cannot import squashfs directly).
fn resolve_tarball_for_rootfs(rootfs_path: &Path) -> Result<PathBuf, AppError> {
    // Already a tarball — pass through.
    if let Some(name) = rootfs_path.file_name().and_then(|n| n.to_str())
        && (name.ends_with(".tar.zst")
            || name.ends_with(".tar.gz")
            || name.ends_with(".tar"))
    {
        return Ok(rootfs_path.to_path_buf());
    }
    // Squashfs path — look for the sibling `.tar.zst` (preferred), then
    // `.tar.gz`, then `.tar`. The build-test-rootfs.sh script publishes
    // `.tar.zst` alongside the squashfs.
    if let Some(stem) = rootfs_path.file_stem().and_then(|s| s.to_str())
        && let Some(parent) = rootfs_path.parent()
    {
        for ext in &[".tar.zst", ".tar.gz", ".tar"] {
            let candidate = parent.join(format!("{stem}{ext}"));
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    Err(AppError::internal_error(format!(
        "WSL2 exec_raw_argv: no sibling tarball found for {} \
         (expected `<stem>.tar.zst` / `.tar.gz` / `.tar` next to the squashfs). \
         The Windows backend cannot `wsl --import` a squashfs directly.",
        rootfs_path.display()
    )))
}

impl Wsl2Backend {
    /// Boot (or reuse) a WSL2 distro from `tarball` and start the
    /// `ziee-sandbox-agent` inside on an AF_VSOCK port. Returns the
    /// shared `DistroHandle`. Cached per-tarball for the lifetime of
    /// the process — subsequent calls hit the cache.
    ///
    /// Used only by `exec_raw_argv` (the tier-4/6 test seam). The
    /// production `ensure_distro` path is unaffected; this test path
    /// has its own registry (`TEST_DISTROS`) and distro namespace
    /// (`ziee-sandbox-test-*`) so the two cannot collide.
    async fn ensure_test_distro(
        &self,
        tarball: &Path,
    ) -> Result<Arc<DistroHandle>, AppError> {
        // Fast path: warm distro (don't hold the boot lock across import).
        let key = std::fs::canonicalize(tarball).unwrap_or_else(|_| tarball.to_path_buf());
        if let Some(h) = TEST_DISTROS.lock().await.get(&key) {
            return Ok(h.clone());
        }
        let boot_lock = test_boot_lock_for(&key).await;
        let _boot = boot_lock.lock().await;
        if let Some(h) = TEST_DISTROS.lock().await.get(&key) {
            return Ok(h.clone());
        }

        if !tarball.exists() {
            return Err(AppError::internal_error(format!(
                "WSL2 exec_raw_argv: test rootfs tarball not found: {} \
                 (run `scripts/build-test-rootfs.sh` or `just test-prereqs`)",
                tarball.display()
            )));
        }

        let distro = test_distro_name(&key);

        // 1. Import the distro if it isn't already registered. The import
        //    is idempotent across runs — if a prior run crashed mid-test,
        //    the distro is still registered and we skip straight to
        //    provisioning. The provision sentinel inside the distro
        //    short-circuits if it's already set up.
        if !distro_registered(&distro).await {
            // Each test distro gets its own ext4 vhdx under the system
            // temp dir. We don't use the production `state.rootfs.cache_dir()`
            // because there's no CodeSandboxState in the test path —
            // and we want test artifacts isolated from the user's app
            // data dir anyway.
            let import_dir = std::env::temp_dir()
                .join("ziee-sandbox-test")
                .join(&distro);
            std::fs::create_dir_all(&import_dir).map_err(|e| {
                AppError::internal_with_id(format!("create test WSL import dir: {e}"))
            })?;
            run_wsl(&[
                "--import",
                &distro,
                &import_dir.to_string_lossy(),
                &tarball.to_string_lossy(),
                "--version",
                "2",
            ])
            .await
            .map_err(|e| {
                AppError::internal_with_id(format!("wsl --import {distro}: {e}"))
            })?;
        }

        // 2. Run the same provision steps the production path uses
        //    (apt-install bwrap+rsync, write AppArmor profile, sysctl,
        //    wsl.conf, identity files, agent binary). Idempotent —
        //    sentinel check short-circuits on warm distros.
        self.provision_distro(&distro).await?;

        // 2b. Kill stale agents inside the distro (see the matching
        //     comment in `ensure_distro` — same reasoning applies here:
        //     `cargo test` invocations on Tier 6 spawn one ziee.exe
        //     server per test, but the in-distro agent outlives them).
        let _ = run_in_distro(
            &distro,
            "pkill -9 -f /usr/local/bin/ziee-sandbox-agent 2>/dev/null; \
             pkill -9 ziee-sandbox 2>/dev/null; \
             sleep 0.2; true",
        )
        .await;

        // 3. Start the agent on a fresh AF_VSOCK port inside the
        //    utility VM. Same path the production hot path uses, so a
        //    failure here surfaces the exact problem production would
        //    hit.
        let vsock_port = NEXT_VSOCK_PORT.fetch_add(1, Ordering::Relaxed);
        let vm_id = self.vm_id()?;
        let agent = tokio::process::Command::new("wsl.exe")
            .args([
                "-d",
                &distro,
                "-u",
                "root",
                "--cd",
                "/",
                "--",
                GUEST_AGENT_PATH,
                "--listen",
                &format!("vsock:{vsock_port}"),
            ])
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                AppError::internal_with_id(format!("spawn WSL2 test agent: {e}"))
            })?;

        // Wait for the agent to start accepting on vsock. See the matching
        // 60s comment in `ensure_distro` — same vmcompute warmup race.
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut last_err: Option<String> = None;
        loop {
            match hvsocket::connect(vm_id, vsock_port).await {
                Ok(_) => break,
                Err(e) => last_err = Some(e.to_string()),
            }
            if Instant::now() > deadline {
                // WSA 10060 (timeout) typically means the port-template
                // GUID is not registered under HKLM\…\GuestCommunicationServices,
                // OR it's registered but vmcompute hasn't picked it up
                // (needs `wsl --shutdown`). The bundled
                // `scripts/register-sandbox-vsock-ports.ps1` does both
                // in one admin invocation.
                return Err(AppError::internal_with_id(format!(
                    "WSL2 test agent did not start listening on vsock:{vsock_port} \
                     within 30s. Last connect error: {}\n\
                     \n\
                     If you see WSA error 10060, the AF_HYPERV port \
                     template GUID is not registered. Run from an admin \
                     PowerShell:\n\
                     \n  \
                     scripts/register-sandbox-vsock-ports.ps1\n\
                     \n  \
                     (registers ports 10001..10100 + runs `wsl --shutdown`)",
                    last_err.unwrap_or_else(|| "<no error captured>".to_string())
                )));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }

        // Test path gets a generous concurrent-exec cap — tier-4 tests
        // are mostly sequential but a few parallelize. 4 mirrors the
        // mac_vm test pool default.
        let handle = Arc::new(DistroHandle {
            agent: Mutex::new(agent),
            distro,
            vsock_port,
            vm_id,
            last_used: Mutex::new(Instant::now()),
            inflight: AtomicUsize::new(0),
            sem: Semaphore::new(4),
        });
        TEST_DISTROS.lock().await.insert(key, handle.clone());
        Ok(handle)
    }
}

/// Stop the in-distro agent cleanly + kill the Windows-side relay.
///
/// There is no `PR_SET_PDEATHSIG` across the WSL boundary
/// ([microsoft/WSL#1037]): killing the Windows `wsl.exe` relay does NOT, by
/// itself, terminate the agent process inside the distro — the agent is
/// reachable on a separate TCP socket and can outlive the relay. Without
/// in-distro cooperation that's an orphan-process leak (HIGH-3 audit
/// finding). The clean fix is a 2-frame handshake: send `Frame::Shutdown`,
/// the agent receives it + `process::exit(0)`s, which makes bwrap's argv
/// `--die-with-parent` take care of any in-flight children. The relay
/// `start_kill` is then a backstop in case the Shutdown round-trip times
/// out (agent already wedged, network broken, …).
async fn stop_agent(h: &DistroHandle) {
    // Best-effort clean shutdown over the existing vsock transport. Tight
    // timeout — we'd rather fall through to the relay kill than block the
    // reaper.
    let shutdown = tokio::time::timeout(Duration::from_secs(2), async {
        let mut stream = hvsocket::connect(h.vm_id, h.vsock_port).await.ok()?;
        use tokio::io::AsyncWriteExt;
        let _ = stream
            .write_all(&sandbox_vm_protocol::encode(
                &sandbox_vm_protocol::Frame::Shutdown,
            ))
            .await;
        let _ = stream.shutdown().await; // half-close write side; agent reads EOF
        Some(())
    })
    .await;
    if shutdown.is_err() {
        tracing::warn!(
            distro = %h.distro,
            "code_sandbox: WSL2 agent did not accept Shutdown within 2s; \
             falling back to relay kill"
        );
    }

    let mut agent = h.agent.lock().await;
    let _ = agent.start_kill();
    let _ = agent.wait().await;
}

/// Remove a dead/unreachable distro from the registry (only if it's still the
/// current handle for that (version, flavor) — don't clobber a concurrent
/// fresh boot) and stop its agent, so the next `ensure_distro` re-boots
/// (mirrors macOS B1). Idempotent.
async fn evict_dead_distro(version: &str, flavor: &str, dead: &Arc<DistroHandle>) {
    let key = distro_key(version, flavor);
    {
        let mut distros = DISTROS.lock().await;
        if distros.get(&key).is_some_and(|h| Arc::ptr_eq(h, dead)) {
            distros.remove(&key);
        }
    }
    stop_agent(dead).await;
    // Terminate (not unregister) so a re-boot reuses the imported filesystem.
    let _ = run_wsl(&["--terminate", &dead.distro]).await;
}

/// `true` if `distro` appears in `wsl.exe -l -q`. WSL emits UTF-16LE, so decode
/// it before matching.
async fn distro_registered(distro: &str) -> bool {
    match tokio::process::Command::new("wsl.exe")
        .args(["-l", "-q"])
        .output()
        .await
    {
        Ok(out) => decode_wsl_output(&out.stdout)
            .lines()
            .any(|l| l.trim() == distro),
        Err(_) => false,
    }
}

/// `test -f <path>` inside the distro (idempotency probe for provisioning).
async fn wsl_test_f(distro: &str, path: &str) -> bool {
    tokio::process::Command::new("wsl.exe")
        .args(["-d", distro, "-u", "root", "--", "test", "-f", path])
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `wslpath -a <winpath>` inside the distro to get the `/mnt/...` path.
async fn wslpath(distro: &str, win: &Path) -> Result<String, AppError> {
    let out = tokio::process::Command::new("wsl.exe")
        .args(["-d", distro, "-u", "root", "--", "wslpath", "-a"])
        .arg(win.as_os_str())
        .output()
        .await
        .map_err(|e| AppError::internal_with_id(format!("wslpath: {e}")))?;
    if !out.status.success() {
        return Err(AppError::internal_error(format!(
            "wslpath failed for {}",
            win.display()
        )));
    }
    Ok(decode_wsl_output(&out.stdout).trim().to_string())
}

/// Run a top-level `wsl.exe <args>` (no `-d`), erroring on non-zero exit.
async fn run_wsl(args: &[&str]) -> Result<(), AppError> {
    let out = tokio::process::Command::new("wsl.exe")
        .args(args)
        .output()
        .await
        .map_err(|e| AppError::internal_with_id(format!("wsl.exe {args:?}: {e}")))?;
    if !out.status.success() {
        return Err(AppError::internal_with_id(format!(
            "wsl.exe {args:?} failed: {}",
            decode_wsl_output(&out.stderr)
        )));
    }
    Ok(())
}

/// Parse the first non-blank line of `wsl --version` for a (major, minor,
/// patch) triple. The Windows binary prints e.g.:
///   `WSL version: 2.6.1.0`
///   `Kernel version: 6.6.87.2-1`
/// The fourth component (`.0` build) is ignored. Old WSL versions don't
/// implement `--version` and exit non-zero — that case bubbles up as Err.
fn probe_wsl_version() -> Result<(u32, u32, u32), String> {
    let out = std::process::Command::new("wsl.exe")
        .args(["--version"])
        .output()
        .map_err(|e| format!("wsl --version: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "wsl --version failed: {}",
            decode_wsl_output(&out.stderr).trim()
        ));
    }
    let text = decode_wsl_output(&out.stdout);
    for line in text.lines() {
        let line = line.trim();
        // Match the version line robustly across localized prefixes ("WSL
        // version:", "Versión de WSL:", …) by scanning each line for a
        // dotted-integer triple.
        if let Some(v) = extract_version_triple(line) {
            return Ok(v);
        }
    }
    Err(format!("no version triple found in: {text:?}"))
}

fn extract_version_triple(line: &str) -> Option<(u32, u32, u32)> {
    // Find a substring that looks like `<u32>.<u32>.<u32>` (allow trailing
    // `.<build>` which we ignore).
    let mut chars = line.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if !c.is_ascii_digit() {
            continue;
        }
        let mut end = start;
        for (i, ch) in line[start..].char_indices() {
            if ch.is_ascii_digit() || ch == '.' {
                end = start + i + ch.len_utf8();
            } else {
                break;
            }
        }
        let candidate = &line[start..end];
        let mut parts = candidate.split('.');
        if let (Some(a), Some(b), Some(c), _) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        {
            if let (Ok(major), Ok(minor), Ok(patch)) = (a.parse(), b.parse(), c.parse()) {
                return Some((major, minor, patch));
            }
        }
    }
    None
}

/// CVE-2025-53788 patched-version gate. Fix landed on two release channels:
/// 2.5.10 (the older one) and 2.6.1. Anything on either channel at or above
/// its fix is acceptable; anything below either is rejected.
fn wsl_version_is_patched((major, minor, patch): (u32, u32, u32)) -> bool {
    if major != 2 {
        // 1.x / 3+: out of scope; we already rejected by the v2 check.
        return major > 2;
    }
    match minor {
        // ≥ 2.6 series: 2.6.1+.
        m if m >= WSL_MIN_VERSION_26.1 => {
            (minor, patch) >= (WSL_MIN_VERSION_26.1, WSL_MIN_VERSION_26.2)
        }
        // 2.5 series: 2.5.10+.
        m if m == WSL_MIN_VERSION_25.1 => patch >= WSL_MIN_VERSION_25.2,
        // < 2.5: not patched.
        _ => false,
    }
}

/// `true` if `%USERPROFILE%\.wslconfig` has `networkingMode = mirrored` in
/// some form. Best-effort tokenizer (no full INI parser needed for one knob).
fn user_wslconfig_uses_mirrored_mode() -> bool {
    let userprofile = match std::env::var("USERPROFILE") {
        Ok(p) => p,
        Err(_) => return false,
    };
    let path = std::path::Path::new(&userprofile).join(".wslconfig");
    let text = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("networkingmode") && lower.contains("mirrored") {
            return true;
        }
    }
    false
}

/// WSL's top-level commands (`-l`, `--status`) emit UTF-16LE; in-distro command
/// stdout is UTF-8. Decode UTF-16LE when it looks like it (alternating
/// NUL-byte pattern), else fall back to lossy UTF-8.
fn decode_wsl_output(bytes: &[u8]) -> String {
    let looks_utf16 = bytes.len() >= 4
        && bytes.len() % 2 == 0
        && bytes
            .iter()
            .skip(1)
            .step_by(2)
            .take(8)
            .filter(|&&b| b == 0)
            .count()
            >= 2;
    if looks_utf16 {
        let u16s: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u16s)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── extract_version_triple ────────────────────────────────────────

    #[test]
    fn extract_version_triple_canonical_wsl_output() {
        assert_eq!(extract_version_triple("WSL version: 2.6.1.0"), Some((2, 6, 1)));
        assert_eq!(extract_version_triple("WSL version: 2.5.10.0"), Some((2, 5, 10)));
        assert_eq!(extract_version_triple("WSL version: 2.7.3.0"), Some((2, 7, 3)));
    }

    #[test]
    fn extract_version_triple_no_trailing_build() {
        assert_eq!(extract_version_triple("WSL version: 2.6.1"), Some((2, 6, 1)));
    }

    #[test]
    fn extract_version_triple_localized_prefix() {
        // Spanish/French/etc localized WSL output also has the triple.
        assert_eq!(extract_version_triple("Versión de WSL: 2.6.1.0"), Some((2, 6, 1)));
        assert_eq!(extract_version_triple("Version de WSL\u{a0}: 2.5.10.0"), Some((2, 5, 10)));
    }

    #[test]
    fn extract_version_triple_two_components_not_enough() {
        assert_eq!(extract_version_triple("WSL version: 2.6"), None);
    }

    #[test]
    fn extract_version_triple_no_digits() {
        assert_eq!(extract_version_triple("Kernel version: linux"), None);
        assert_eq!(extract_version_triple(""), None);
    }

    #[test]
    fn extract_version_triple_picks_first_triple_on_line() {
        // Kernel lines look like `6.6.87.2-1` — also a valid triple match.
        // Function returns the first dotted-integer triple found, which is
        // expected (caller scans line-by-line and gates by `wsl_version_is_patched`
        // which rejects major != 2).
        assert_eq!(extract_version_triple("Kernel version: 6.6.87.2-1"), Some((6, 6, 87)));
    }

    // ─── wsl_version_is_patched ────────────────────────────────────────

    #[test]
    fn wsl_version_is_patched_on_26_channel() {
        // 2.6.0 — just below fix on the 2.6 channel.
        assert!(!wsl_version_is_patched((2, 6, 0)));
        // 2.6.1 — exact fix.
        assert!(wsl_version_is_patched((2, 6, 1)));
        // 2.6.2 — above fix.
        assert!(wsl_version_is_patched((2, 6, 2)));
        // 2.7.3 — newer channel still considered patched.
        assert!(wsl_version_is_patched((2, 7, 3)));
    }

    #[test]
    fn wsl_version_is_patched_on_25_channel() {
        // 2.5.9 — below fix on the 2.5 channel.
        assert!(!wsl_version_is_patched((2, 5, 9)));
        // 2.5.10 — exact fix.
        assert!(wsl_version_is_patched((2, 5, 10)));
        // 2.5.11 — above fix.
        assert!(wsl_version_is_patched((2, 5, 11)));
    }

    #[test]
    fn wsl_version_is_patched_below_25() {
        assert!(!wsl_version_is_patched((2, 0, 0)));
        assert!(!wsl_version_is_patched((2, 3, 99)));
        assert!(!wsl_version_is_patched((2, 4, 5)));
    }

    #[test]
    fn wsl_version_is_patched_major_outside_2() {
        // Pre-WSL2 — refused by the v2 default check upstream; gate returns
        // false here too. The probe doesn't claim WSL1 is patched.
        assert!(!wsl_version_is_patched((1, 0, 0)));
        // Hypothetical WSL3 — out of scope of CVE-2025-53788, treated as patched
        // (newer kernel branch).
        assert!(wsl_version_is_patched((3, 0, 0)));
    }

    // ─── decode_wsl_output ─────────────────────────────────────────────

    #[test]
    fn decode_wsl_output_handles_utf16_le() {
        // "OK" in UTF-16LE: 0x4F 0x00 0x4B 0x00. Add some padding to trip
        // the heuristic.
        let bytes = b"O\0K\0\n\0a\0b\0";
        assert_eq!(decode_wsl_output(bytes), "OK\nab");
    }

    #[test]
    fn decode_wsl_output_handles_utf8() {
        let bytes = b"hello world\n";
        assert_eq!(decode_wsl_output(bytes), "hello world\n");
    }

    #[test]
    fn decode_wsl_output_short_input_is_utf8() {
        // < 4 bytes — heuristic bails to UTF-8.
        assert_eq!(decode_wsl_output(b"ab"), "ab");
        assert_eq!(decode_wsl_output(b""), "");
    }

    #[test]
    fn decode_wsl_output_odd_length_is_utf8() {
        // Odd length can't be UTF-16; falls back to lossy UTF-8.
        let bytes = b"abc";
        assert_eq!(decode_wsl_output(bytes), "abc");
    }

    #[test]
    fn decode_wsl_output_invalid_utf8_is_lossy() {
        // 0xFF is invalid UTF-8 start byte; lossy decoder substitutes U+FFFD.
        let bytes = &[b'a', 0xFF, b'b'];
        let out = decode_wsl_output(bytes);
        assert!(out.contains('a') && out.contains('b'));
        assert!(out.contains('\u{FFFD}'));
    }

    // ─── user_wslconfig_uses_mirrored_mode (parser core) ───────────────
    //
    // The real fn reads from %USERPROFILE%\.wslconfig — a host side effect.
    // We test the parser logic via a thin shim that takes the file text.

    fn wslconfig_text_uses_mirrored_mode(text: &str) -> bool {
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("networkingmode") && lower.contains("mirrored") {
                return true;
            }
        }
        false
    }

    #[test]
    fn wslconfig_mirrored_classic_form() {
        assert!(wslconfig_text_uses_mirrored_mode(
            "[wsl2]\nnetworkingMode = mirrored\n"
        ));
    }

    #[test]
    fn wslconfig_mirrored_no_spaces() {
        assert!(wslconfig_text_uses_mirrored_mode(
            "[wsl2]\nnetworkingMode=mirrored\n"
        ));
    }

    #[test]
    fn wslconfig_mirrored_uppercase_value() {
        assert!(wslconfig_text_uses_mirrored_mode(
            "[wsl2]\nnetworkingMode = Mirrored\n"
        ));
    }

    #[test]
    fn wslconfig_nat_mode_is_not_mirrored() {
        assert!(!wslconfig_text_uses_mirrored_mode(
            "[wsl2]\nnetworkingMode = nat\n"
        ));
    }

    #[test]
    fn wslconfig_no_networking_key_is_not_mirrored() {
        assert!(!wslconfig_text_uses_mirrored_mode(
            "[wsl2]\nmemory = 8GB\nprocessors = 4\n"
        ));
    }

    #[test]
    fn wslconfig_commented_out_is_not_mirrored() {
        assert!(!wslconfig_text_uses_mirrored_mode(
            "[wsl2]\n# networkingMode = mirrored\n; networkingMode = mirrored\n"
        ));
    }

    #[test]
    fn wslconfig_empty_or_missing_is_not_mirrored() {
        assert!(!wslconfig_text_uses_mirrored_mode(""));
        assert!(!wslconfig_text_uses_mirrored_mode("   \n   \n"));
    }

    // ─── WSL_MIN_VERSION constants stay aligned with the gate ──────────

    #[test]
    fn min_version_constants_are_themselves_patched() {
        // Sanity: bumping the constants should preserve the property that
        // each fix-version satisfies the patched gate.
        assert!(wsl_version_is_patched(WSL_MIN_VERSION_25));
        assert!(wsl_version_is_patched(WSL_MIN_VERSION_26));
    }

    // ─── MED-3 regression: no future maintainer accidentally
    //     re-introduces the WSLENV credential-leak pattern.
    //
    // The historical design used `WSLENV=ZIEE_PASSWD:ZIEE_GROUP` to
    // propagate the synthetic identity into the distro. That leaked the
    // contents into the in-distro environment, where a sandboxed
    // process could read it via `/proc/<wsl.exe-relay-pid>/environ`
    // (the relay PID is reachable from inside the distro by enumerating
    // /proc — the audit's reachability proof). The fix replaced the env
    // path with `write_file_into_distro`, which pipes content via stdin
    // (no environment crossing).
    //
    // This test fails fast if a future commit re-introduces the WSLENV
    // pattern, in either of two forms:
    //   1. The literal env-var name `"WSLENV"` appearing outside the
    //      audit/doc comments that intentionally describe the historic
    //      footgun (allowlist of 4 known mentions in this file).
    //   2. A `.env("WSLENV", …)` or `.env_remove("WSLENV")` call (any
    //      direct env manipulation of WSLENV from this module).
    //
    // The allowlist of expected mentions is hard-coded; if you legitimately
    // add a new audit-doc reference, bump the count. The principle is
    // "every WSLENV mention is INTENTIONAL and reviewed."
    #[test]
    fn med3_wslenv_credential_leak_regression() {
        let full_src = include_str!("wsl2.rs");
        // Scan only the production code section (everything before the
        // `#[cfg(test)] mod tests` marker). The test mod itself contains
        // WSLENV strings in assertion messages + forbidden-pattern
        // literals; scanning it would conflate "this regression test
        // exists" with "the regression has re-occurred".
        //
        // Normalize CRLF→LF first so the split works regardless of
        // whether git or the editor preserved Windows line endings.
        let normalized = full_src.replace("\r\n", "\n");
        let prod_src = normalized
            .split_once("#[cfg(test)]\nmod tests {")
            .map(|(prod, _)| prod)
            .expect("test mod marker present");

        // Forbidden: any active code path that sets WSLENV on a Command.
        for forbidden in &[
            ".env(\"WSLENV\"",
            ".env_remove(\"WSLENV\"",
            ".envs([(\"WSLENV\"",
            "(\"WSLENV\", ",
        ] {
            assert!(
                !prod_src.contains(forbidden),
                "MED-3 regression: wsl2.rs production code contains `{forbidden}` — \
                 propagating identity via WSLENV leaks credentials into \
                 the in-distro environment. Use `write_file_into_distro` \
                 to pipe content via stdin instead. See the audit comment \
                 at the top of `provision_distro` and the doc on \
                 `write_file_into_distro` for the correct pattern."
            );
        }

        // The string `WSLENV` may legitimately appear in doc comments
        // that explain the historic pattern. Count occurrences in the
        // production section only; fail if it grows beyond the known
        // allowlist — any new mention must be reviewed for intent.
        const ALLOWED_DOC_MENTIONS: usize = 6;
        let mention_count = prod_src.matches("WSLENV").count();
        assert!(
            mention_count <= ALLOWED_DOC_MENTIONS,
            "MED-3 regression: wsl2.rs production code has {mention_count} \
             occurrences of `WSLENV` but only {ALLOWED_DOC_MENTIONS} are \
             expected (all in doc/audit comments that describe the historic \
             credential-leak pattern). If a new mention is intentional doc \
             text, bump ALLOWED_DOC_MENTIONS in this test. If it's active \
             code, that's the regression — use `write_file_into_distro` \
             instead."
        );

        // Synthetic identity bodies must continue to flow through
        // `write_file_into_distro` (the safe stdin-pipe alternative).
        assert!(
            prod_src.contains("write_file_into_distro(distro, GUEST_PASSWD, SYNTHETIC_PASSWD"),
            "MED-3 regression: synthetic passwd no longer written via \
             write_file_into_distro. Revert to stdin-pipe (see the doc \
             on write_file_into_distro)."
        );
        assert!(
            prod_src.contains("write_file_into_distro(distro, GUEST_GROUP, SYNTHETIC_GROUP"),
            "MED-3 regression: synthetic group no longer written via \
             write_file_into_distro. Revert to stdin-pipe (see the doc \
             on write_file_into_distro)."
        );
    }
}
