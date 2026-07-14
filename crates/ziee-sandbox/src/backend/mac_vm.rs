//! macOS backend: bwrap runs inside a per-flavor libkrun microVM (Plan 1 §2).
//!
//! Architecture (see also `sandbox-vm-launcher` + `sandbox-guest-agent`):
//!   - One warm **VM per flavor**, booted lazily on first use by spawning the
//!     `ziee-sandbox-vm-launcher` child process (NOT an in-process fork —
//!     `krun_start_enter` `exit()`s and post-fork only async-signal-safe calls
//!     are legal).
//!   - The launcher boots libkrun with: the guest root (agent + bwrap), the
//!     flavor squashfs as a virtio-blk disk, the workspace root via virtio-fs,
//!     and a vsock port bridged to a host unix socket.
//!   - The guest `ziee-sandbox-agent` listens on that vsock port. Per
//!     `execute_command`, this backend connects to the unix socket and sends
//!     the **bwrap argv built by `build_bwrap_argv`** (guest paths) — so the
//!     hardening flags are identical to the Linux backend.
//!
//! ⚠️ **Validation status:** this file cannot be compiled on Linux (no macOS
//! toolchain / libkrun). It is grounded in the real libkrun API + the protocol
//! crate but must be compiled + validated on macOS. Points flagged `MAC-TODO`
//! need first-run attention. See `MACOS-RUNBOOK.md` in the standalone
//! `ziee-ai/sandbox-rootfs` repo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use sandbox_vm_protocol::{CgroupLimits, ExecRequest, PROTOCOL_VERSION};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, Semaphore};

use super::SandboxBackend;
use ziee_core::AppError;
use crate::sandbox_config::CodeSandboxConfig;
use crate::resource_limits_cache;
use crate::provider::{EnsureOutcome, EvictOutcome};
use crate::sandbox::{self, SandboxRunResult};
use crate::types::{
    CgroupMode, CodeSandboxState, HardeningCapabilities, HostCapabilities, PidNsMode,
    SandboxContext, SeccompMode,
};

// ── Guest contract (must match sandbox-guest-agent constants) ────────────────
const GUEST_VSOCK_PORT: u32 = 1024;
const GUEST_ROOTFS_MOUNT: &str = "/sandbox-rootfs";
const GUEST_WORKSPACE_MOUNT: &str = "/workspace";
// Host-folder mounts (feature #3): each shared host folder appears in the guest
// at `/host-mounts/<i>` (mounted by the agent); bwrap binds it to
// `/mnt/<full host path>`. MUST match the agent + launcher constants.
const GUEST_EXTRA_MOUNTS_DIR: &str = "/host-mounts";
// Base dir for the guest-local artifact-collection dirs (macOS libkrun
// virtio-fs CREATE-EPERM workaround). Each RW StagedMount gets its own
// `<base>/<i>` dir UNDER the guest's already-writable `/tmp` tmpfs (so the
// agent can `mkdir` it without touching the RO guest root); bwrap binds THAT
// (not the broken virtio-fs path), and after exit the agent streams the files
// back over the vsock protocol. The host writes them into the real host
// artifact dir. The path is carried per-exec in `ExecRequest::collect_artifacts`.
const GUEST_ARTIFACTS_OUT_DIR: &str = "/tmp/ziee-artifacts-out";
const GUEST_BWRAP_PATH: &str = "/usr/bin/bwrap";
const GUEST_AGENT_PATH: &str = "/usr/bin/ziee-sandbox-agent";
// Fixed fd the agent dup2's the seccomp BPF pipe to in the bwrap child; the
// argv built here references it via `--seccomp <fd>`. Out of the stdio range.
const GUEST_SECCOMP_FD: i32 = 10;
// Guest synthetic identity files — baked into the guest root image (MAC-TODO:
// ensure the guest root ships these; build_bwrap_argv binds them over
// /etc/passwd + /etc/group).
const GUEST_PASSWD: &str = "/etc/ziee-sandbox-passwd";
const GUEST_GROUP: &str = "/etc/ziee-sandbox-group";
/// Empty regular file baked into the guest root, used as the bind source
/// for the `DANGEROUS_DOTFILES` masks. MUST be a normal file — see
/// `build_bwrap_argv::mask_path` doc for why `/dev/null` doesn't work.
const GUEST_EMPTY: &str = "/etc/ziee-sandbox-empty";

// VM sizing + per-VM concurrency cap now live in the runtime-tunable
// `code_sandbox_settings` row (Plan 1 §6). Defaults (mirroring the prior
// consts: 2 vCPU, 2048 MiB, 3 concurrent execs) come from the SQL DEFAULTs
// in migration 42 + `resource_limits_cache::defaults`. `ensure_vm` reads
// the snapshot when booting a new VM; existing warm VMs keep the sizing
// they were booted with (admin tunes apply to the NEXT cold boot).

/// Monotonic request id (B4) — avoids the cgroup-path collisions a timestamp
/// id risked under concurrency.
static REQ_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A booted, warm per-flavor VM.
struct VmHandle {
    child: Mutex<tokio::process::Child>,
    socket_path: PathBuf,
    last_used: Mutex<Instant>,
    /// In-flight exec count — the reaper never evicts a VM with a running
    /// command (a long command keeps the VM alive past the idle threshold).
    inflight: AtomicUsize,
    /// Bounds concurrent execs in this VM (Ga) so they can't OOM the guest.
    sem: Semaphore,
}

/// Decrements `inflight` on drop — so a cancelled `run()` future (aborted chat
/// turn) doesn't leak the count and wedge the reaper (B2).
struct InflightGuard(Arc<VmHandle>);
impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Background reaper: started once; evicts idle, not-in-use VMs.
static REAPER_STARTED: AtomicBool = AtomicBool::new(false);

fn ensure_reaper() {
    if REAPER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            // Resolve the configured idle-evict threshold on every tick so
            // an admin's PUT takes effect within ~1 minute. `0` = never evict.
            let idle_evict_secs = resource_limits_cache::snapshot_or_defaults()
                .vm_idle_evict_secs
                .max(0) as u64;
            if idle_evict_secs == 0 {
                continue;
            }
            let mut vms = VMS.lock().await;
            let mut evict = Vec::new();
            // `key` is the `<version>/<flavor>` composite (Plan 5 Phase 3).
            for (key, h) in vms.iter() {
                if h.inflight.load(Ordering::SeqCst) == 0
                    && h.last_used.lock().await.elapsed().as_secs() >= idle_evict_secs
                {
                    evict.push(key.clone());
                }
            }
            for key in evict {
                if let Some(h) = vms.remove(&key) {
                    let mut child = h.child.lock().await;
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    let _ = std::fs::remove_file(&h.socket_path);
                    tracing::info!(key = %key, "code_sandbox: macOS VM evicted (idle)");
                }
            }
        }
    });
}

/// Per-flavor warm VMs. Boot is serialized by holding this lock across the
/// (rare) launch, which doubles as single-flight — concurrent first-calls for a
/// flavor produce one VM. MAC-TODO: if cross-flavor boot contention matters,
/// switch to a per-flavor OnceCell like `runtime_mount::READY`.
static VMS: Lazy<Mutex<HashMap<String, Arc<VmHandle>>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Per-flavor boot serialization (B3) — held only during a boot, NOT during
/// warm reuse, so booting flavor A doesn't block running an already-warm flavor
/// B (the global VMS lock is released across the ≤30 s boot).
static BOOT_LOCKS: Lazy<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Composite registry key for `VMS` + `BOOT_LOCKS` so two pinned
/// rootfs versions of the same flavor (the new pin + a draining old
/// pin during a swap) get separate VM slots. Plan 5 Phase 3.
fn vm_key(version: &str, flavor: &str) -> String {
    format!("{version}/{flavor}")
}

async fn boot_lock_for(version: &str, flavor: &str) -> Arc<Mutex<()>> {
    BOOT_LOCKS
        .lock()
        .await
        .entry(vm_key(version, flavor))
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Test-only VM pool keyed by rootfs squashfs path. Used by
/// `exec_raw_argv` so the 50+ tier-4/6 tests in a `cargo test`
/// invocation share one libkrun VM per (process, rootfs) rather than
/// paying the ~2s boot cost per test.
static TEST_VMS: Lazy<Mutex<HashMap<PathBuf, Arc<VmHandle>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Boot (or reuse) a libkrun VM with `rootfs_squashfs` as the
/// virtio-blk disk and a temp dir as the virtio-fs workspace. Used
/// only by `exec_raw_argv`. Each unique rootfs gets its own VM;
/// subsequent calls hit the cache.
async fn ensure_test_vm(rootfs_squashfs: &Path) -> Result<Arc<VmHandle>, AppError> {
    {
        let vms = TEST_VMS.lock().await;
        if let Some(h) = vms.get(rootfs_squashfs) {
            return Ok(h.clone());
        }
    }

    let dir = runtime_dir()
        .map_err(|e| AppError::internal_with_id(format!("test runtime dir: {e}")))?;
    // Per-rootfs socket name so multiple test rootfs files can coexist.
    let key: String = rootfs_squashfs
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("test")
        .to_string();
    // macOS AF_UNIX SUN_PATH max is 104 bytes; `/var/folders/<5>/<29>/T/
    // ziee-sandbox-<pid>/test-vm-<full-rootfs-filename>.sock` blows past
    // that and `connect()` fails EAI_OVERFLOW with "path too long".
    // `stat()` still works (it doesn't go through sockaddr_un), which
    // historically masked the bug — the harness used to poll `exists()`
    // and time out without ever attempting connect. Hash the key into a
    // short stable digest so the full sockaddr_un fits.
    let key_digest = short_key_digest(&key);
    let socket_path = dir.join(format!("vm-{key_digest}.sock"));
    let _ = std::fs::remove_file(&socket_path);
    if socket_path.as_os_str().len() > 100 {
        return Err(AppError::internal_error(format!(
            "test VM socket path is {} bytes (AF_UNIX limit 104). \
             Set TMPDIR to a shorter prefix.",
            socket_path.as_os_str().len()
        )));
    }

    // Workspace dir for the VM's virtio-fs share. Shared across tests
    // in this pool entry — tier 4 tests don't write to /workspace.
    let workspace_host_path = dir.join(format!("test-vm-{key}-workspace"));
    std::fs::create_dir_all(&workspace_host_path)
        .map_err(|e| AppError::internal_with_id(format!("mkdir test workspace: {e}")))?;

    let cfg = serde_json::json!({
        "num_vcpus": 1,
        "ram_mib": 512,
        "root_path": MacVmBackend::guest_root_path().to_string_lossy(),
        "sandbox_disk_path": rootfs_squashfs.to_string_lossy(),
        "workspace_host_path": workspace_host_path.to_string_lossy(),
        "vsock_socket_path": socket_path.to_string_lossy(),
        "vsock_port": GUEST_VSOCK_PORT,
        "agent_exec_path": GUEST_AGENT_PATH,
    });
    let cfg_path = dir.join(format!("test-vm-{key}.json"));
    std::fs::write(&cfg_path, serde_json::to_vec(&cfg).unwrap())
        .map_err(|e| AppError::internal_with_id(format!("write test VM config: {e}")))?;

    // Spawn launcher with stderr piped so we can scan for the agent's
    // "listening on vsock port" readiness marker. The socket existing
    // ≠ the agent listening; connecting too early gets EOF.
    let mut child = tokio::process::Command::new(MacVmBackend::launcher_path())
        .arg(&cfg_path)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::internal_with_id(format!("spawn test VM launcher: {e}")))?;

    // Wait for socket to appear AND agent to log readiness.
    let stderr = child.stderr.take().expect("piped stderr");
    let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel::<()>(1);
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut signaled = false;
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("[test-vm:{key}] {line}");
            if !signaled && line.contains("listening on vsock port") {
                let _ = ready_tx.send(()).await;
                signaled = true;
            }
        }
    });

    // libkrun's `krun_add_vsock_port2(listen=true)` bridges a host-side
    // unix-socket pseudo-endpoint to the guest's vsock listener. On macOS
    // 1.18.1 the endpoint is NOT a filesystem-visible socket — `stat()`
    // returns ENOENT, but `connect(AF_UNIX, path)` succeeds and is
    // proxied through to the guest's vsock listen. The old poll on
    // `socket_path.exists()` therefore never resolved.
    //
    // Poll by attempting an `UnixStream::connect` instead: success means
    // libkrun's bridge is wired AND the agent on the guest side is
    // listening (the agent's vsock_listen is what backs the connect).
    // Once that round-trips, also drain the agent's "listening on vsock
    // port" stderr marker (may arrive before or after the first
    // successful connect — either order is fine).
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if tokio::net::UnixStream::connect(&socket_path).await.is_ok() {
            break;
        }
        if Instant::now() > deadline {
            return Err(AppError::internal_error(
                "test VM launcher: vsock bridge did not accept a connection within 30s \
                 (libkrun host-side connect proxy never came up)",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::timeout(Duration::from_secs(30), ready_rx.recv())
        .await
        .map_err(|_| {
            AppError::internal_error(
                "test VM launcher: agent did not log 'listening on vsock port' within 30s",
            )
        })?;

    let handle = Arc::new(VmHandle {
        child: Mutex::new(child),
        socket_path,
        last_used: Mutex::new(Instant::now()),
        inflight: AtomicUsize::new(0),
        // 4 concurrent execs per test VM — tier-4 tests are mostly
        // sequential so this is generous.
        sem: Semaphore::new(4),
    });
    TEST_VMS
        .lock()
        .await
        .insert(rootfs_squashfs.to_path_buf(), handle.clone());
    Ok(handle)
}

/// Per-server-instance private runtime dir (mode 0700) for VM control sockets +
/// launch configs. Replaces predictable, world-traversable /tmp paths (S1/S2):
/// only the server's uid can reach the (unauthenticated) control socket, and an
/// attacker can't pre-create a symlink at a guessable path.
fn runtime_dir() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("ziee-sandbox-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// 8-hex-char digest of `key` — stable across runs, ~5 collisions per
/// billion. Used to compress long natural keys (rootfs filenames,
/// `version-flavor` tuples) into a fragment that keeps the full
/// socket path under macOS's 104-byte AF_UNIX SUN_PATH cap. SHA-256
/// is overkill but already in-tree; we just take the first 8 hex
/// chars for a 4-byte digest.
fn short_key_digest(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    let full = format!("{:x}", h.finalize());
    full[..8].to_string()
}

/// True if `launcher` can find `libkrun.1.dylib` via its rpath search
/// from the current location. Checks the two app-bundle conventions
/// (`<exe-dir>/../lib` and `<exe-dir>/../Frameworks`) — matches the
/// `LC_RPATH` entries that `build_helper/sandbox_runtime.rs` installs
/// via `install_name_tool` on the bundled launcher. The raw cargo-
/// built launcher has `/opt/homebrew/lib` instead, which is *not*
/// `@executable_path/...`-relative, so this returns false for it
/// even if homebrew is installed — that's correct because:
///   1. dyld evaluates rpath search lazily at LOAD time, and
///   2. the homebrew rpath only works on dev machines that have
///      `brew install libkrun` — production deployments don't.
/// Falling through to the embedded extracted launcher is always the
/// safer choice when this returns false.
fn launcher_can_load_libkrun(launcher: &Path) -> bool {
    let Some(exe_dir) = launcher.parent() else { return false; };
    let lib = exe_dir.join("..").join("lib").join("libkrun.1.dylib");
    let fw = exe_dir.join("..").join("Frameworks").join("libkrun.1.dylib");
    lib.exists() || fw.exists()
}

pub struct MacVmBackend;

impl MacVmBackend {
    pub fn new() -> Self {
        Self
    }

    /// Resolve the bundled launcher binary + guest root. Resolution order:
    ///   1. Env var (`ZIEE_SANDBOX_VM_LAUNCHER` / `ZIEE_SANDBOX_GUEST_ROOT`)
    ///      — explicit dev override.
    ///   2. Sibling of the running executable (the legacy app-bundle layout
    ///      where the launcher lives next to the server binary). For the
    ///      guest root, the legacy default `/opt/ziee/sandbox-guest-root`.
    ///   3. Embedded bundle, extracted to the user's cache dir on first
    ///      sandbox call. This is the self-contained-binary path: the
    ///      server binary ships the launcher + dylibs + guest-root as
    ///      bytes (see `code_sandbox::embedded`); on first use we unpack
    ///      to `dirs::cache_dir()/ziee/sandbox-runtime/<sha256>/`.
    fn launcher_path() -> PathBuf {
        if let Ok(p) = std::env::var("ZIEE_SANDBOX_VM_LAUNCHER") {
            return PathBuf::from(p);
        }
        let sibling = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
            .map(|dir| dir.join("ziee-sandbox-vm-launcher"));
        // The sibling-of-current-exe path is the production app-bundle
        // layout (launcher next to server inside Contents/MacOS). In
        // dev / cargo-test environments a cargo-built launcher may
        // also exist at <workspace>/target/debug/ziee-sandbox-vm-launcher
        // because the launcher is a workspace member — that copy is
        // NOT post-processed by `build_helper/sandbox_runtime.rs` and
        // its `@rpath/libkrun.1.dylib` resolves to non-existent paths
        // (libkrun is at <embedded>/lib/, not <workspace>/target/lib/).
        // Picking it up makes libkrun fail with EINVAL → vsock-never-
        // appears → 30s timeout. Only trust the sibling if libkrun
        // is actually reachable via its rpath search.
        if let Some(p) = sibling.as_ref()
            && p.exists()
            && launcher_can_load_libkrun(p)
        {
            return p.clone();
        }
        // Coupling D seam: the embedded bundle (`embedded.rs`) stays in the
        // ziee server crate (its `include_bytes!` reads the SERVER
        // `CARGO_MANIFEST_DIR`); reach its `ensure()` through the injected
        // `GuestAgentProvider` on the engine-owned global state.
        match crate::config::get_state()
            .ok_or_else(|| "code_sandbox state not initialized".to_string())
            .and_then(|s| s.guest_agent.ensure().map(|ex| ex.launcher.clone()))
        {
            Ok(launcher) => launcher,
            Err(e) => {
                tracing::warn!(
                    "code_sandbox: launcher resolution falling back to bare path; \
                     embedded bundle extraction failed: {e}"
                );
                sibling.unwrap_or_else(|| PathBuf::from("ziee-sandbox-vm-launcher"))
            }
        }
    }

    fn guest_root_path() -> PathBuf {
        if let Ok(p) = std::env::var("ZIEE_SANDBOX_GUEST_ROOT") {
            return PathBuf::from(p);
        }
        let legacy = PathBuf::from("/opt/ziee/sandbox-guest-root");
        if legacy.exists() {
            return legacy;
        }
        // Coupling D seam (see `launcher_path`): reach the embedded bundle's
        // `ensure()` through the injected `GuestAgentProvider`.
        match crate::config::get_state()
            .ok_or_else(|| "code_sandbox state not initialized".to_string())
            .and_then(|s| s.guest_agent.ensure().map(|ex| ex.guest_root.clone()))
        {
            Ok(guest_root) => guest_root,
            Err(e) => {
                tracing::warn!(
                    "code_sandbox: guest-root resolution falling back to legacy path; \
                     embedded bundle extraction failed: {e}"
                );
                legacy
            }
        }
    }

    /// Get the warm VM for `(version, flavor)`, booting one
    /// (single-flight) if needed. Plan 5 Phase 3: keyed on
    /// `(version, flavor)` so the old-pin VM keeps serving its
    /// in-flight execs while a new-pin VM boots alongside.
    async fn ensure_vm(
        &self,
        state: &CodeSandboxState,
        flavor: &str,
        sandbox_disk: &Path,
        version: &str,
        // Host folders shared read-only into this VM (feature #3), ordered:
        // index i → virtio-fs tag `host-mount-<i>` → guest `/host-mounts/<i>`.
        extra_mount_paths: &[String],
        // Stable digest of `extra_mount_paths`; folded into the VM slot key so a
        // changed mount set boots a fresh VM (the old one idle-evicts). Empty
        // for the common no-host-mounts path (byte-identical to before).
        mount_digest: &str,
    ) -> Result<Arc<VmHandle>, AppError> {
        let base_key = vm_key(version, flavor);
        let key = if mount_digest.is_empty() {
            base_key
        } else {
            format!("{base_key}/{mount_digest}")
        };
        // Fast path: warm VM (don't hold the lock across a boot).
        if let Some(h) = VMS.lock().await.get(&key) {
            return Ok(h.clone());
        }
        // Serialize boot for THIS (version, flavor) only (B3). Global
        // VMS lock is NOT held across the ≤30 s boot, so other slots
        // stay responsive. (Mount-set variants of the same flavor share this
        // boot lock → their cold boots serialize; each still checks its own
        // `key` after acquiring, so they get distinct VM slots.)
        let boot_lock = boot_lock_for(version, flavor).await;
        let _boot = boot_lock.lock().await;
        if let Some(h) = VMS.lock().await.get(&key) {
            return Ok(h.clone());
        }

        let dir = runtime_dir().map_err(|e| AppError::internal_with_id(format!("runtime dir: {e}")))?;
        // Socket path must fit in macOS AF_UNIX SUN_PATH (104 bytes).
        // Two pinned versions of the same flavor still can't collide on
        // a shared `vm-<flavor>.sock` during a swap-drain, so hash the
        // (version, flavor) key into a short stable digest. See
        // `short_key_digest` for the reasoning — `connect()` fails
        // EAI_OVERFLOW on the un-hashed path; `stat()` still works
        // (historically masked the bug).
        let key_digest = short_key_digest(&format!("{version}-{flavor}-{mount_digest}"));
        let socket_path = dir.join(format!("vm-{key_digest}.sock"));
        let _ = std::fs::remove_file(&socket_path);
        if socket_path.as_os_str().len() > 100 {
            return Err(AppError::internal_error(format!(
                "VM socket path is {} bytes (AF_UNIX limit 104). \
                 Set TMPDIR to a shorter prefix.",
                socket_path.as_os_str().len()
            )));
        }

        // Read runtime-tunable VM sizing + concurrency cap (§6). Boot-time
        // snapshot — once the VM is up, an admin's PUT applies to the NEXT
        // cold boot of THIS flavor, not the warm one.
        let limits = resource_limits_cache::get(state.limits.as_ref()).await?;

        let cfg = serde_json::json!({
            "num_vcpus": limits.mac_vm_vcpus.max(1) as u32,
            "ram_mib": limits.mac_vm_ram_mib.max(256) as u32,
            "root_path": Self::guest_root_path().to_string_lossy(),
            "sandbox_disk_path": sandbox_disk.to_string_lossy(),
            "workspace_host_path": state.workspace_root.to_string_lossy(),
            "vsock_socket_path": socket_path.to_string_lossy(),
            "vsock_port": GUEST_VSOCK_PORT,
            "agent_exec_path": GUEST_AGENT_PATH,
            // Host folders shared read-only into this VM (feature #3). Empty for
            // the no-host-mounts path. Ordered: index i → tag host-mount-<i>.
            "extra_mounts": extra_mount_paths,
        });
        let cfg_path = dir.join(format!("vm-{key_digest}.json"));
        std::fs::write(&cfg_path, serde_json::to_vec(&cfg).unwrap()).map_err(|e| {
            AppError::internal_with_id(format!("write VM launch config: {e}"))
        })?;

        // Gap #4: clear the env so the VMM process does not inherit the
        // server's secrets (DATABASE_URL/JWT/API keys). The launcher needs no
        // env — its config is the JSON file arg and libkrun is found via rpath.
        // Pipe stderr so we can scan for the agent's "listening on vsock port"
        // log line — vm.sock appearing on the HOST is libkrun's bridge being
        // ready, NOT the in-guest agent. Connecting before the guest agent
        // listens gets an immediate EOF. (Same race + fix as the test-VM
        // helper `ensure_test_vm`.)
        let mut child = tokio::process::Command::new(Self::launcher_path())
            .arg(&cfg_path)
            .env_clear()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AppError::internal_with_id(format!("spawn VM launcher: {e}")))?;

        // Reader task: forward stderr to ours (preserves diagnostic output)
        // AND signal when the agent logs readiness.
        let stderr = child.stderr.take().expect("piped stderr");
        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel::<()>(1);
        let flavor_for_log = flavor.to_string();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stderr).lines();
            let mut signaled = false;
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[vm:{flavor_for_log}] {line}");
                if !signaled && line.contains("listening on vsock port") {
                    let _ = ready_tx.send(()).await;
                    signaled = true;
                }
            }
        });

        // Wait first for the host bridge socket, then for the in-guest agent.
        // libkrun's `krun_add_vsock_port2(listen=true)` host endpoint isn't
        // a filesystem-visible socket on macOS 1.18.1 — `stat()` returns
        // ENOENT, but `connect(AF_UNIX, path)` succeeds and is proxied to
        // the guest's vsock listener. Poll by attempting a connect rather
        // than checking file existence. (Same fix as `ensure_test_vm`.)
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if tokio::net::UnixStream::connect(&socket_path).await.is_ok() {
                break;
            }
            if Instant::now() > deadline {
                return Err(AppError::internal_error(
                    "VM launcher: vsock bridge did not accept a connection within 30s \
                     (libkrun host-side connect proxy never came up)",
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tokio::time::timeout(Duration::from_secs(30), ready_rx.recv())
            .await
            .map_err(|_| {
                AppError::internal_error(
                    "VM launcher: agent did not log 'listening on vsock port' within 30s",
                )
            })?;

        let handle = Arc::new(VmHandle {
            child: Mutex::new(child),
            socket_path,
            last_used: Mutex::new(Instant::now()),
            inflight: AtomicUsize::new(0),
            sem: Semaphore::new(limits.vm_max_concurrent_execs.max(1) as usize),
        });
        VMS.lock().await.insert(key, handle.clone());
        ensure_reaper();
        Ok(handle)
    }
}

/// Map a host workspace path to its guest equivalent under the virtio-fs share.
fn guest_workspace_path(state: &CodeSandboxState, host_ws: &Path) -> PathBuf {
    let rel = host_ws.strip_prefix(&state.workspace_root).unwrap_or(host_ws);
    Path::new(GUEST_WORKSPACE_MOUNT).join(rel)
}

#[async_trait]
impl SandboxBackend for MacVmBackend {
    fn probe_host(&self, _cfg: &CodeSandboxConfig) -> Option<HostCapabilities> {
        // libkrun (via Hypervisor.framework) ships only on Apple Silicon, so
        // gate boot on aarch64 + a reachable launcher binary. The launcher
        // itself dlopens libkrun + checks Hypervisor.framework at start; here
        // we just keep init cheap (sub-10 ms, no dlopen, no fs scans).
        if std::env::consts::ARCH != "aarch64" {
            tracing::error!(
                "code_sandbox: macOS backend requires Apple Silicon (aarch64); \
                 host is {}. Sandbox MCP row will NOT be registered.",
                std::env::consts::ARCH
            );
            return None;
        }
        let launcher = Self::launcher_path();
        if !launcher.exists() {
            tracing::error!(
                "code_sandbox: macOS VM launcher not found at {} \
                 (set ZIEE_SANDBOX_VM_LAUNCHER for dev). Sandbox MCP row will \
                 NOT be registered.",
                launcher.display()
            );
            return None;
        }
        // Placeholder caps: the VM run path rebuilds real *guest* caps in
        // `ensure_rootfs_ready` + `run`; the only downstream consumer of this
        // value on the Linux backend (`runtime_mount`'s PID-ns probe) isn't on
        // the macOS code path.
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
        // Shared fetch coordination (OS-independent). Mounting happens inside
        // the guest, so there is no host mount dir — the guest mounts the
        // squashfs at GUEST_ROOTFS_MOUNT.
        let cache = state.rootfs.cache_dir();
        let outcome = state.rootfs.ensure_fetched(&cache, flavor, Box::new(|_| {}))
            .await
            .map_err(|e| AppError::internal_with_id(format!("rootfs fetch failed: {e}")))?;

        let guest_caps = HardeningCapabilities {
            bwrap_path: PathBuf::from(GUEST_BWRAP_PATH),
            pid_namespace: PidNsMode::Strict,
            // caps.cgroup drives the *host* CgroupScope, unused in the VM — the
            // guest agent applies cgroup v2 from ExecRequest.cgroup. See run().
            cgroup: CgroupMode::None,
            // The caps.seccomp field is unused for the --seccomp flag (that's
            // the build_bwrap_argv seccomp_fd arg). The guest agent builds +
            // applies the shared seccomp filter itself; see run().
            seccomp: SeccompMode::NotLinked,
        };
        let artifact_id = outcome.artifact_id;
        let artifact_version = outcome.version.clone();
        // Register with the version-manager mount registry so the
        // Phase 3 drain task can find this VM-backed mount when the
        // admin changes the pin. The mount_dir is the guest-side
        // path; the host backend will tear down the actual VM in
        // `evict_artifact`.
        crate::registry::register_mount(
            artifact_id,
            &artifact_version,
            std::env::consts::ARCH,
            flavor,
            PathBuf::from(GUEST_ROOTFS_MOUNT),
        );
        Ok(EnsureOutcome {
            caps: Arc::new(guest_caps),
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
        // Host folders are virtio-fs-shared into the VM (feature #3) and bound
        // by bwrap; workspace-internal extra mounts resolve via /workspace.
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

        // Locate (fetch if needed) the flavor squashfs; idempotent on
        // cache hit. Capture `version` so `ensure_vm` keys its slot on
        // `(version, flavor)` — two pinned versions of the same flavor
        // coexist during a swap-drain cycle (Plan 5 Phase 3).
        let cache = state.rootfs.cache_dir();
        let fetched = state.rootfs.ensure_fetched(&cache, flavor, Box::new(|_| {}))
            .await
            .map_err(|e| AppError::internal_with_id(format!("rootfs fetch failed: {e}")))?;
        let disk = fetched.installed_path;
        let version = fetched.version;

        // Runtime-configurable resource caps (Plan 1 §6). Snapshot once per
        // exec — both the host argv (prlimit) and the guest cgroup (via
        // ExecRequest.cgroup) read from the same row, so they stay coherent.
        let limits = resource_limits_cache::get(state.limits.as_ref()).await?;

        // Host-folder mounts (feature #3): a host mount's source is OUTSIDE the
        // workspace root, so it's virtio-fs-shared into the VM by the launcher
        // (tag host-mount-<i>), mounted by the agent at /host-mounts/<i>, and
        // bound by bwrap to its /mnt/<full host path> target. Sort
        // deterministically so index↔tag↔guest-path is stable, and key the VM
        // on the mount set so adding/removing a folder boots a fresh VM (the old
        // one idle-evicts). Workspace-internal extra mounts (workflow runner;
        // Linux-only in practice) translate to their /workspace/... guest path.
        let mut host_folders: Vec<&StagedMount> = extra_mounts
            .iter()
            .filter(|m| !m.host_path.starts_with(&state.workspace_root))
            .collect();
        host_folders.sort_by(|a, b| a.host_path.cmp(&b.host_path));
        let host_folder_paths: Vec<String> = host_folders
            .iter()
            .map(|m| m.host_path.to_string_lossy().into_owned())
            .collect();
        let mount_digest = if host_folder_paths.is_empty() {
            String::new()
        } else {
            short_key_digest(&host_folder_paths.join("\u{0}"))
        };
        use crate::workflow_staging::StageMode;
        let mut guest_mounts: Vec<StagedMount> = Vec::with_capacity(extra_mounts.len());
        for (i, m) in host_folders.iter().enumerate() {
            guest_mounts.push(StagedMount {
                mode: m.mode,
                host_path: PathBuf::from(format!("{GUEST_EXTRA_MOUNTS_DIR}/{i}")),
                sandbox_path: m.sandbox_path.clone(),
            });
        }
        // Workspace-internal mounts. RO mounts map to their /workspace/... guest
        // path (virtio-fs READS work). RW mounts (the workflow runner's per-step
        // `artifacts/<step>` dirs) CANNOT use the virtio-fs path: a sandboxed
        // `open(O_CREAT)` there fails EPERM on libkrun. Instead each RW mount
        // gets a guest-local tmpfs dir (`/artifacts-out/<idx>`) that the agent
        // provisions; bwrap binds THAT, and after exit the agent streams the
        // files back over vsock. We remember `idx → host_path` so the receiving
        // side writes them into the real host artifact dir (the host's own fs).
        let mut collect_artifacts: Vec<String> = Vec::new();
        let mut artifact_host_dirs: Vec<PathBuf> = Vec::new();
        for m in extra_mounts
            .iter()
            .filter(|m| m.host_path.starts_with(&state.workspace_root))
        {
            match m.mode {
                StageMode::ReadOnly => {
                    guest_mounts.push(StagedMount {
                        mode: m.mode,
                        host_path: guest_workspace_path(state, &m.host_path),
                        sandbox_path: m.sandbox_path.clone(),
                    });
                }
                StageMode::ReadWrite => {
                    let idx = collect_artifacts.len();
                    let guest_tmpfs = format!("{GUEST_ARTIFACTS_OUT_DIR}/{idx}");
                    collect_artifacts.push(guest_tmpfs.clone());
                    artifact_host_dirs.push(m.host_path.clone());
                    guest_mounts.push(StagedMount {
                        mode: m.mode,
                        host_path: PathBuf::from(guest_tmpfs),
                        sandbox_path: m.sandbox_path.clone(),
                    });
                }
            }
        }

        // Build the bwrap argv with GUEST paths so the agent can exec it
        // verbatim. The hardening flags are identical to the Linux backend.
        let guest_caps = HardeningCapabilities {
            bwrap_path: PathBuf::from(GUEST_BWRAP_PATH),
            pid_namespace: PidNsMode::Strict,
            cgroup: CgroupMode::None,
            seccomp: SeccompMode::NotLinked,
        };
        let guest_ctx = SandboxContext {
            conversation_id: ctx.conversation_id,
            user_id: ctx.user_id,
            workspace: guest_workspace_path(state, &ctx.workspace),
            files: ctx.files.clone(),
            // Carry the caller-injected extra RO binds (the /lit view) forward
            // onto the guest argv — byte-identical to the pre-lift inline block,
            // which computed the same host path from `ctx.conversation_id`.
            extra_ro_binds: ctx.extra_ro_binds.clone(),
        };
        let secs = timeout_secs.unwrap_or(limits.timeout_secs.max(1) as u64);
        // The argv references the guest seccomp fd; the agent builds the same
        // shared-policy BPF and pipes it to that fd. Passing GUEST_WORKSPACE_MOUNT
        // as the attachment root (Gb) makes attachment binds resolve to guest
        // paths under /workspace, not the host workspace_root.
        // Live-progress: when a sink is requested, tell the agent to provision
        // the FIFO guest-side (`progress: true`) and bind its guest-local path
        // (NOT a virtio-fs path — libkrun's virtio-fs has an mkfifo EPERM bug)
        // onto `/ziee/progress`. The agent reads it + forwards each line as a
        // `Frame::ProcessProgress`, which `run_on_stream` routes to `progress_tx`.
        let want_progress = progress_tx.is_some();
        let progress_fifo_src = want_progress
            .then(|| sandbox_vm_protocol::PROGRESS_GUEST_FIFO_PATH);
        let req = ExecRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: REQ_COUNTER.fetch_add(1, Ordering::Relaxed),
            bwrap_path: GUEST_BWRAP_PATH.to_string(),
            argv: sandbox::build_bwrap_argv(
                &guest_caps,
                Path::new(GUEST_WORKSPACE_MOUNT),
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
            // In-guest cgroup v2 limits (the agent applies them; prlimit in
            // the argv is the backstop). Source from §6 config: memory /
            // pids / cpu mirror the host argv literals on the same row.
            cgroup: Some(CgroupLimits {
                memory_max_bytes: limits.memory_max_bytes as u64,
                memory_swap_max_bytes: limits.memory_swap_max_bytes as u64,
                pids_max: limits.pids_max as u64,
                cpu_max: limits.cpu_max.clone(),
            }),
            // RW (artifact) mounts → guest-local tmpfs dirs the agent streams
            // back. Empty for the no-RW-mount path → byte-identical exec.
            collect_artifacts: collect_artifacts.clone(),
        };

        // Up to 2 attempts: a dead/unreachable VM (connect fails — the command
        // never ran, so retry is safe) is evicted + re-booted once (B1). A
        // failure AFTER connect is NOT retried (the command may have started).
        let mut attempt = 0;
        loop {
            attempt += 1;
            let vm = self
                .ensure_vm(state, flavor, &disk, &version, &host_folder_paths, &mount_digest)
                .await?;
            // Bound concurrency per VM (Ga) and mark in-flight so the reaper
            // won't evict mid-command (with a Drop guard so a cancelled future
            // can't leak the count — B2).
            let _permit = vm.sem.acquire().await.expect("VM semaphore never closed");
            vm.inflight.fetch_add(1, Ordering::SeqCst);
            let _guard = InflightGuard(vm.clone());
            *vm.last_used.lock().await = Instant::now();

            match UnixStream::connect(&vm.socket_path).await {
                Ok(stream) => {
                    let result = super::vm_client::run_on_stream_collecting(
                        stream,
                        req.clone(),
                        secs,
                        progress_tx.clone(),
                        &artifact_host_dirs,
                    )
                    .await;
                    *vm.last_used.lock().await = Instant::now();
                    return result;
                }
                Err(e) if attempt < 2 => {
                    tracing::warn!(flavor, version = %version, "code_sandbox: VM unreachable ({e}); re-booting and retrying");
                    drop(_guard);
                    drop(_permit);
                    evict_dead_vm(&version, flavor, &vm).await;
                    continue;
                }
                Err(e) => {
                    return Err(AppError::internal_with_id(format!("connect VM socket: {e}")));
                }
            }
        }
    }

    async fn shutdown(&self) {
        let mut vms = VMS.lock().await;
        for (key, handle) in vms.drain() {
            let mut child = handle.child.lock().await;
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = std::fs::remove_file(&handle.socket_path);
            tracing::info!(key = %key, "code_sandbox: macOS VM stopped on shutdown");
        }
        // Also tear down test-scoped VMs (tier 4 helper pool).
        let mut test_vms = TEST_VMS.lock().await;
        for (_, handle) in test_vms.drain() {
            let mut child = handle.child.lock().await;
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = std::fs::remove_file(&handle.socket_path);
        }
    }

    async fn exec_raw_argv(
        &self,
        argv: Vec<String>,
        rootfs_squashfs: &Path,
        timeout: Duration,
    ) -> Result<super::RawExecResult, AppError> {
        let vm = ensure_test_vm(rootfs_squashfs).await?;
        let req = ExecRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: REQ_COUNTER.fetch_add(1, Ordering::Relaxed),
            bwrap_path: GUEST_BWRAP_PATH.to_string(),
            argv,
            timeout_ms: timeout.as_millis().min(u64::MAX as u128) as u64,
            // The agent's seccomp pipe path uses its embedded BPF when
            // seccomp_fd is Some; we leave it None so tests can supply
            // --seccomp <fd> themselves if they want, by appending a
            // bwrap arg that opens an FD. Most tier 4 seccomp tests
            // synthesize this via the agent's pipe — see the helper.
            seccomp_fd: None,
            cgroup: None,
            // Tier-4 raw-argv harness never exercises live progress.
            progress: false,
            collect_artifacts: Vec::new(),
        };
        let secs = timeout.as_secs().max(1);
        let _permit = vm.sem.acquire().await.expect("VM semaphore never closed");
        vm.inflight.fetch_add(1, Ordering::SeqCst);
        let _guard = InflightGuard(vm.clone());
        let stream = UnixStream::connect(&vm.socket_path)
            .await
            .map_err(|e| AppError::internal_with_id(format!("connect test-VM socket: {e}")))?;
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
        // Make sure the flavor's rootfs is on disk and a VM is warm
        // (cold-start path identical to one-shot `run`).
        let cache = state.rootfs.cache_dir();
        let fetched = state.rootfs.ensure_fetched(&cache, flavor, Box::new(|_| {}))
            .await
            .map_err(|e| AppError::internal_with_id(format!("rootfs fetch failed: {e}")))?;
        let disk = fetched.installed_path;
        let version = fetched.version;
        // Long-lived MCP session VM: no host-folder mounts.
        let vm = self.ensure_vm(state, flavor, &disk, &version, &[], "").await?;

        // Hold an inflight count for the session's lifetime so the
        // reaper waits for live MCP sessions to drain before evicting.
        vm.inflight.fetch_add(1, Ordering::SeqCst);
        let guard = InflightGuard(vm.clone());
        *vm.last_used.lock().await = Instant::now();

        let stream = UnixStream::connect(&vm.socket_path)
            .await
            .map_err(|e| AppError::internal_with_id(format!("connect VM socket: {e}")))?;

        let session = super::vm_long_lived::open_long_lived_with_guard(
            stream,
            Some(Box::new(guard)),
        );
        Ok(Some(session))
    }

    /// Version-aware evict (Plan 5 Phase 3 drain-on-swap): tear down
    /// ONLY the `(version, flavor)` VM the drain task observed
    /// finishing. Leaves the new-pin VM alive.
    async fn evict_artifact(
        &self,
        mount_dir: &Path,
        flavor: &str,
        version: &str,
    ) -> EvictOutcome {
        let key = vm_key(version, flavor);
        if let Some(handle) = VMS.lock().await.remove(&key) {
            let mut child = handle.child.lock().await;
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = std::fs::remove_file(&handle.socket_path);
        }
        // Delete the per-version squashfs.
        let version_cache_dir = mount_dir.parent().unwrap_or(mount_dir);
        let suffix = format!("-{flavor}.squashfs");
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
}

/// Remove a dead/unreachable VM from the registry (only if it's still the
/// current handle for `(version, flavor)` — don't clobber a concurrent fresh
/// boot) and kill its launcher, so the next `ensure_vm` re-boots (B1).
/// Idempotent.
async fn evict_dead_vm(version: &str, flavor: &str, dead: &Arc<VmHandle>) {
    let key = vm_key(version, flavor);
    {
        let mut vms = VMS.lock().await;
        if vms.get(&key).is_some_and(|h| Arc::ptr_eq(h, dead)) {
            vms.remove(&key);
        }
    }
    let mut child = dead.child.lock().await;
    let _ = child.start_kill();
    let _ = child.wait().await;
    let _ = std::fs::remove_file(&dead.socket_path);
}
