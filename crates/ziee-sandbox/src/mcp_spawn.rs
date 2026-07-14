//! High-level entry point for spawning a stdio MCP subprocess inside
//! the code_sandbox bwrap isolation. Returns a transport opaque to the
//! caller (`McpSandboxTransport`); the MCP client wires it into rmcp.
//!
//! Two paths converge here:
//! - **Linux** — spawn `bwrap` directly on the host with the MCP argv
//!   built by `sandbox::build_mcp_sandbox_argv`. Wrapped in rmcp's
//!   `TokioChildProcess` (handles kill-on-drop + stdio piping).
//! - **macOS / Windows** — ask the active `SandboxBackend` for a
//!   long-lived agent session; spawn one bwrap process inside the
//!   guest VM via that session; expose the per-process duplex stream
//!   as `AsyncRead + AsyncWrite` so rmcp can wrap it in
//!   `AsyncRwTransport::new_client`.
//!
//! Both paths use the *same* `build_mcp_sandbox_argv` helper, only the
//! "where bwrap runs" differs. Hardening (clearenv / user-ns / seccomp
//! pipe / cgroup / prlimit / dotfile masks / per-server workspace
//! bind) is identical.

use rmcp::transport::TokioChildProcess;
use uuid::Uuid;

use ziee_core::AppError;
use crate::backend;
use crate::backend::vm_long_lived;
// Referenced only by the Linux-gated `spawn_on_linux_host`
// (`sandbox::SeccompPipe`); the VM/WSL2 backends don't touch it.
#[cfg(target_os = "linux")]
use crate::sandbox;
use crate::types::CodeSandboxState;

/// Result of a successful sandboxed MCP spawn. The variant carries the
/// platform-specific lifecycle owner so dropping `McpSandboxTransport`
/// tears the sandboxed child down.
///
/// The `_inflight` field on each variant is the
/// [`version_manager::InflightGuard`] that keeps the swap-drain task
/// (Plan 5 Phase 3) waiting until this MCP server's transport drops.
/// Field order matters — Rust drops fields in declaration order, so
/// the transport-owner is listed first (so the bwrap child / VM
/// session torn down first) and the inflight guard last (decrements
/// the per-(version, arch, flavor) counter once the use is fully over).
pub enum McpSandboxTransport {
    /// Linux: already-spawned bwrap subprocess, ready to feed into
    /// rmcp via `().serve(transport)`. `TokioChildProcess` owns the
    /// `Child` and kills it on drop.
    LinuxBwrap {
        child: TokioChildProcess,
        _inflight: Option<crate::registry::InflightGuard>,
    },
    /// VM backend: long-lived agent session + the per-process duplex
    /// stream. The caller splits `io` into halves and passes them to
    /// `rmcp::transport::AsyncRwTransport::new_client(read, write)`.
    /// Drop order matters — `io` must drop BEFORE `session` so the
    /// `KillProcess` frame still has a writer to go through.
    VmSession {
        io: vm_long_lived::ProcessIo,
        session: vm_long_lived::LongLivedSession,
        _inflight: Option<crate::registry::InflightGuard>,
    },
}

/// Plumbing for one sandboxed MCP spawn. The caller
/// (`mcp::client::stdio::StdioMcpClient::connect_sandboxed`) supplies the
/// verbatim command; both the Linux and VM backends resolve it against
/// the rootfs PATH (no host bun/uv rewrite, no host-arch bind-mount).
pub struct McpSpawnRequest {
    /// Stable id of the MCP server row this spawn belongs to. Used to
    /// derive a unique per-server workspace under
    /// `<workspace_root>/mcp/<server_id>/`.
    pub server_id: Uuid,
    /// Verbatim `command` field from the MCP server config
    /// ("python3", "uvx", "node", "Rscript", …). Resolved against the
    /// rootfs PATH inside the sandbox on both backends.
    pub original_command: String,
    /// User-supplied args from `mcp_servers.args`.
    pub server_args: Vec<String>,
    /// Already-filtered env (BLOCKED_ENV_VARS stripped at the
    /// boundary). Injected as `--setenv` lines in the bwrap argv, NOT
    /// inherited from the host shell.
    pub extra_setenv: Vec<(String, String)>,
    /// Rootfs flavor (KNOWN_FLAVORS, e.g. `"minimal"` / `"full"`) to
    /// mount for this spawn — from the server's `sandbox_flavor`
    /// column. NOT NULL since migration 83 (defaults to `'full'`);
    /// the create handler validates against `KNOWN_FLAVORS` at
    /// write-time, so this is always a known flavor by the time it
    /// reaches the spawn path.
    pub flavor: String,
}

/// Resolve `command` for execution inside the sandbox. The sandbox runs
/// the command **verbatim** against the rootfs PATH — no rewrite to the
/// host's embedded uv/bun (those are host-arch and don't execute inside
/// the Linux sandbox/VM; the rootfs ships its own node/uv/python3/R).
/// `python` is normalised to `python3`; everything else passes through
/// and bwrap resolves it against `/usr/bin` in the mounted rootfs. Used
/// by both the Linux and VM backends.
///
/// Returns `(in_sandbox_command, prepended_args)`.
pub(crate) fn resolve_command_for_guest(
    command: &str,
) -> Result<(String, Vec<String>), AppError> {
    let cmd = if command == "python" { "python3" } else { command };
    Ok((cmd.to_string(), Vec::new()))
}

/// Spawn the MCP server inside the sandbox using whichever backend is
/// active. Returns an `McpSandboxTransport` the caller hands to rmcp.
pub async fn start_mcp_in_sandbox(
    state: &CodeSandboxState,
    req: McpSpawnRequest,
) -> Result<McpSandboxTransport, AppError> {
    use crate::registry::{self, InflightKind};

    // Per-server flavor (from `mcp_servers.sandbox_flavor`, default full).
    let flavor = req.flavor.clone();

    // Resolve the rootfs first so we know which artifact_id this
    // sandboxed MCP server is pinned to. ensure_rootfs_ready is
    // idempotent on the warm path. The InflightGuard we acquire
    // here keeps the drain-on-swap task from evicting the mount as
    // long as this MCP server's transport (held by the caller via
    // `McpSandboxTransport`) is alive.
    //
    // Go through the BACKEND (not `runtime_mount::ensure_rootfs_ready`
    // directly): the Linux backend delegates to runtime_mount (host
    // squashfuse mount), but the macOS/WSL2 VM backends mount the rootfs
    // INSIDE the guest. Calling runtime_mount directly here required host
    // squashfuse on every platform and made sandboxed MCP servers
    // unspawnable on macOS (SANDBOX_SQUASHFUSE_MISSING) even though the
    // one-shot `execute_command` path — which already routes through the
    // backend — works fine.
    let ensure = backend::active().ensure_rootfs_ready(state, &flavor).await?;
    let inflight = ensure
        .artifact_id
        .and_then(|id| registry::acquire_inflight(id, InflightKind::Mcp));

    // Backends opt in by overriding `open_long_lived_session`; Linux
    // inherits the default `Ok(None)` and falls through to the
    // host-bwrap path below.
    if let Some(session) = backend::active()
        .open_long_lived_session(state, &flavor)
        .await?
    {
        return spawn_in_vm_session(state, req, session, inflight).await;
    }

    #[cfg(target_os = "linux")]
    {
        return spawn_on_linux_host(state, req, inflight).await;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state, req, inflight);
        Err(AppError::internal_error(
            "MCP run-in-sandbox: no backend available on this host",
        ))
    }
}

/// VM-backend spawn. Builds the bwrap argv with **guest paths** and
/// sends it through the long-lived agent session as a `StartProcess`.
/// Returns the [`McpSandboxTransport::VmSession`] holding the session
/// alive and the per-process duplex stream.
///
/// Re-resolves the command against rootfs-native paths via
/// [`resolve_command_for_guest`] because the caller's `resolved_command`
/// points at the host's embedded uv binary, which is host-arch and
/// won't execute inside the Linux VM.
async fn spawn_in_vm_session(
    state: &CodeSandboxState,
    req: McpSpawnRequest,
    session: vm_long_lived::LongLivedSession,
    inflight: Option<crate::registry::InflightGuard>,
) -> Result<McpSandboxTransport, AppError> {
    // Re-resolve against the rootfs (rejects uvx/npx/node/deno on v1 VM).
    let (guest_command, guest_prepended) = resolve_command_for_guest(&req.original_command)?;

    // Make sure the per-server workspace exists on host so the VM's
    // virtio-fs share can see it as `/workspace/mcp/<server_id>/`,
    // then bwrap binds that as `/home/sandboxuser` inside the sandbox.
    let host_workspace = state
        .workspace_root
        .join("mcp")
        .join(req.server_id.to_string());
    std::fs::create_dir_all(&host_workspace).map_err(|e| {
        AppError::internal_error(format!("create mcp vm workspace: {e}"))
    })?;
    // Bwrap runs the sandbox as `--uid 1001 --gid 1001` (synthetic
    // sandboxuser). The host-side workspace is created by the server
    // process (a different uid), and the bind-mount inherits the
    // host inode's permissions — so without an explicit chmod, the
    // sandboxed uid 1001 can read but cannot write into its own
    // `/home/sandboxuser`. Set the sticky-bit "world-writable" mode
    // 0o1777 (same convention as the per-conversation workspace
    // tempdirs in the test harness). Single-user workspace, sticky
    // bit prevents the rare "two MCP servers share this UID" cross-
    // delete (defense-in-depth — workspaces are already per-server).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&host_workspace, std::fs::Permissions::from_mode(0o1777))
            .map_err(|e| AppError::internal_error(format!("chmod mcp vm workspace: {e}")))?;
    }

    // Ensure the bind source `/workspace/mcp/<server_id>` exists in the guest.
    // No-op on macOS (virtio-fs shares it); the WSL2 backend creates + rsyncs
    // the per-server workspace into the distro (no virtio-fs there).
    backend::active()
        .prepare_mcp_vm_workspace(state, &req.flavor, req.server_id)
        .await?;

    // Build the bwrap argv for the guest with the rootfs-resolved command.
    let guest_argv = build_guest_mcp_argv(&req, &guest_command, &guest_prepended)?;
    let bwrap_path = "/usr/bin/bwrap".to_string();

    let io = session
        .spawn(bwrap_path, guest_argv, None, None)
        .await?;
    Ok(McpSandboxTransport::VmSession {
        io,
        session,
        _inflight: inflight,
    })
}

/// Build the guest-side bwrap argv for a MCP spawn. Keeps a separate
/// (minimal, hand-rolled) argv from the Linux host path because:
///   - There's no `HardeningCapabilities` snapshot for the guest at
///     this point in the lifecycle (no plumbed `runtime_mount` on the
///     VM backends; the agent does its own mounts).
///   - The guest doesn't have an embedded uv/bun bind to plumb in.
///   - prlimit + the rootfs binds match the agent's
///     `build_bwrap_argv`-issued one-shot path.
///
/// `guest_command` and `guest_prepended` come from
/// [`resolve_command_for_guest`] — rootfs-native paths the bwrap argv
/// can actually exec inside the Linux VM.
fn build_guest_mcp_argv(
    req: &McpSpawnRequest,
    guest_command: &str,
    guest_prepended: &[String],
) -> Result<Vec<String>, AppError> {
    let mut argv: Vec<String> = vec![
        "--clearenv".into(),
        "--unshare-user".into(),
        "--uid".into(), "1001".into(),
        "--gid".into(), "1001".into(),
        "--unshare-uts".into(),
        "--unshare-ipc".into(),
        "--unshare-pid".into(),
        "--unshare-cgroup-try".into(),
        "--share-net".into(),
        "--die-with-parent".into(),
        "--new-session".into(),
        "--ro-bind".into(), "/sandbox-rootfs/usr".into(), "/usr".into(),
        "--symlink".into(), "usr/bin".into(), "/bin".into(),
        "--symlink".into(), "usr/sbin".into(), "/sbin".into(),
        "--symlink".into(), "usr/lib".into(), "/lib".into(),
        "--symlink".into(), "usr/lib64".into(), "/lib64".into(),
        "--dev".into(), "/dev".into(),
        "--tmpfs".into(), "/tmp".into(),
        "--proc".into(), "/proc".into(),
        // DNS + TLS trust roots — required for any MCP server that
        // talks out (pip-install, uvx, mcp-server-fetch). The
        // `--share-net` flag above shares the host network NAMESPACE,
        // but each child must still find a resolver + CA bundle to
        // make a TLS request. Source paths come from /sandbox-rootfs
        // (the squashfs) NOT the guest-root /etc — the guest-root is
        // the agent's filesystem (no resolv.conf), the sandbox rootfs
        // is what ships the deployment's /etc. ro-bind-try so a
        // rootfs without these files doesn't fail spawn.
        "--ro-bind-try".into(), "/sandbox-rootfs/etc/resolv.conf".into(), "/etc/resolv.conf".into(),
        "--ro-bind-try".into(), "/sandbox-rootfs/etc/ssl".into(), "/etc/ssl".into(),
        "--ro-bind-try".into(), "/sandbox-rootfs/etc/ca-certificates".into(), "/etc/ca-certificates".into(),
        "--bind".into(), format!("/workspace/mcp/{}", req.server_id), "/home/sandboxuser".into(),
        "--chdir".into(), "/home/sandboxuser".into(),
        "--setenv".into(), "HOME".into(), "/home/sandboxuser".into(),
        "--setenv".into(), "USER".into(), "sandboxuser".into(),
        "--setenv".into(), "PATH".into(),
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
        "--setenv".into(), "LANG".into(), "C.UTF-8".into(),
        "--setenv".into(), "LC_ALL".into(), "C.UTF-8".into(),
    ];
    for (k, v) in &req.extra_setenv {
        argv.push("--setenv".into());
        argv.push(k.clone());
        argv.push(v.clone());
    }
    argv.push("--".into());
    argv.push(guest_command.to_string());
    for a in guest_prepended { argv.push(a.clone()); }
    for a in &req.server_args { argv.push(a.clone()); }
    Ok(argv)
}

#[cfg(target_os = "linux")]
async fn spawn_on_linux_host(
    state: &CodeSandboxState,
    req: McpSpawnRequest,
    inflight: Option<crate::registry::InflightGuard>,
) -> Result<McpSandboxTransport, AppError> {
    use std::path::Path;
    use std::process::Stdio;
    use tokio::process::Command;

    use crate::resource_limits_cache;
    use crate::sandbox::{
        build_mcp_sandbox_argv, HardeningArgvParams, SyntheticIdentity,
    };
    use crate::types::SeccompMode;

    // Lazy-mount the flavor's rootfs (cheap on cache hit). Uses the
    // per-server `req.flavor` set by `StdioMcpClient::connect_sandboxed`
    // from `mcp_servers.sandbox_flavor`.
    let ensure = state.rootfs.ensure_rootfs_ready(&req.flavor).await?;
    let caps = ensure.caps.clone();
    let rootfs_dir = ensure.mount_dir;

    let synthetic = SyntheticIdentity::ensure_for(&state.workspace_root)?;

    // Per-server workspace: <workspace_root>/mcp/<server_id>/.
    // Bwrap runs as --uid 1001, but the host-side dir is owned by the
    // server's uid → without 0o1777 the sandboxed sandboxuser can't
    // write its own $HOME. Same bug as the VM path (which already
    // chmods in `spawn_in_vm_session`); without this `pip install`,
    // `uvx`, `npm install` and any other write inside the sandbox
    // fails with EACCES on Linux too.
    let server_workspace = state
        .workspace_root
        .join("mcp")
        .join(req.server_id.to_string());
    std::fs::create_dir_all(&server_workspace).map_err(|e| {
        AppError::internal_error(format!("create mcp server workspace: {e}"))
    })?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&server_workspace, std::fs::Permissions::from_mode(0o1777))
            .map_err(|e| AppError::internal_error(format!("chmod mcp server workspace: {e}")))?;
    }

    // Resolve the command against the rootfs PATH — no host bun/uv
    // bind-mount (the rootfs ships its own node/uv/python3/R). bwrap
    // execvp's a bare command name against the sandbox PATH, which
    // includes /usr/local/bin (where `uv` installs `uvx`).
    let (guest_command, guest_prepended) = resolve_command_for_guest(&req.original_command)?;
    let extra_ro_binds: Vec<(String, String)> = Vec::new();

    let limits = resource_limits_cache::get(state.limits.as_ref()).await?;

    // Per-spawn seccomp pipe (Linux only; piped to the child fd 7).
    // Drop the pipe AFTER spawn — until then the pre_exec dup2 needs
    // the read fd, and the writer task pumps the bytes.
    let seccomp_pipe = match &caps.seccomp {
        SeccompMode::Loaded(bpf) => Some(sandbox::SeccompPipe::install_pub(bpf.clone())?),
        _ => None,
    };

    let argv = build_mcp_sandbox_argv(
        &HardeningArgvParams {
            caps: &caps,
            rootfs_dir: &rootfs_dir,
            passwd_path: synthetic.passwd(),
            group_path: synthetic.group(),
            mask_path: synthetic.mask(),
            home_bind_source: &server_workspace,
            seccomp_fd: seccomp_pipe.as_ref().map(|p| p.target_fd_pub()),
            extra_setenv: &req.extra_setenv,
            extra_ro_binds: &extra_ro_binds,
            extra_rw_binds: &[],
            // Long-lived MCP servers don't surface live progress (no
            // workflow sandbox step routes through this path).
            progress_fifo_src: None,
        },
        Path::new(&guest_command),
        &{
            let mut all = guest_prepended.clone();
            all.extend(req.server_args.clone());
            all
        },
        &limits,
    );

    let mut cmd = Command::new(&caps.bwrap_path);
    cmd.args(&argv);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    if let Some(p) = seccomp_pipe.as_ref() {
        let source_fd = p.read_fd_pub();
        let target_fd = p.target_fd_pub();
        // SAFETY: dup2 + fcntl are async-signal-safe; source_fd is
        // owned by `SeccompPipe` and stays valid through spawn.
        unsafe {
            cmd.pre_exec(move || {
                if libc::dup2(source_fd, target_fd) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let flags = libc::fcntl(target_fd, libc::F_GETFD);
                if flags < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(target_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let child = TokioChildProcess::new(cmd).map_err(|e| {
        AppError::internal_error(format!("spawn bwrap for MCP server: {e}"))
    })?;
    // Spawn done — child has its own dup of read_fd; drop ours.
    drop(seccomp_pipe);

    Ok(McpSandboxTransport::LinuxBwrap {
        child,
        _inflight: inflight,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_mcp_argv_emits_clearenv_and_workspace_bind() {
        let req = McpSpawnRequest {
            server_id: Uuid::nil(),
            original_command: "python3".into(),
            server_args: vec!["-m".into(), "mymcp".into()],
            extra_setenv: vec![("MY_VAR".into(), "yes".into())],
            flavor: "full".into(),
        };
        let argv = build_guest_mcp_argv(&req, "python3", &[]).unwrap();

        assert!(argv.contains(&"--clearenv".to_string()));
        assert!(argv.contains(&"--die-with-parent".to_string()));
        assert!(argv.contains(&"--unshare-user".to_string()));
        assert!(argv.contains(&format!("/workspace/mcp/{}", Uuid::nil())));
        // env vars layered after the static block
        assert!(argv.windows(3).any(|w| w == ["--setenv", "MY_VAR", "yes"]));
        // guest_command appended after `--`, then guest_prepended (empty here),
        // then server_args.
        let dashdash = argv.iter().position(|s| s == "--").expect("-- terminator");
        assert_eq!(argv[dashdash + 1], "python3");
        assert_eq!(argv[dashdash + 2], "-m");
        assert_eq!(argv[dashdash + 3], "mymcp");
    }

    #[test]
    fn guest_mcp_argv_pid_namespace_is_strict() {
        // We must --unshare-pid AND --proc /proc inside the VM; the
        // VM has no fallback because the guest kernel always supports
        // PID namespaces. A regression here would leak host PIDs to
        // the sandboxed child.
        let req = McpSpawnRequest {
            server_id: Uuid::nil(),
            original_command: "python3".into(),
            server_args: vec![],
            extra_setenv: vec![],
            flavor: "full".into(),
        };
        let argv = build_guest_mcp_argv(&req, "python3", &[]).unwrap();
        assert!(argv.contains(&"--unshare-pid".to_string()));
        // --proc immediately followed by /proc
        let i = argv.iter().position(|s| s == "--proc").unwrap();
        assert_eq!(argv[i + 1], "/proc");
    }

    #[test]
    fn resolve_command_for_guest_passes_python_through() {
        let (cmd, args) = resolve_command_for_guest("python3").unwrap();
        assert_eq!(cmd, "python3");
        assert!(args.is_empty());
        let (cmd, args) = resolve_command_for_guest("python").unwrap();
        assert_eq!(cmd, "python3");
        assert!(args.is_empty());
    }

    #[test]
    fn resolve_command_for_guest_passes_through_any_command() {
        // The sandbox resolves commands against the rootfs PATH, so any
        // launcher (uvx/npx/node/Rscript/…) passes through verbatim with
        // no prepended args — it's up to the chosen flavor to provide it.
        for cmd in &["uvx", "npx", "node", "deno", "Rscript"] {
            let (resolved, args) = resolve_command_for_guest(cmd).unwrap();
            assert_eq!(&resolved, cmd);
            assert!(args.is_empty());
        }
    }
}
