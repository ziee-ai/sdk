//! `ziee-sandbox-agent` — the in-VM init + control agent for the microVM
//! sandbox backend (macOS libkrun; later WSL2).
//!
//! libkrun runs this as the guest entrypoint (`krun_set_exec`). Its job:
//!   1. Mount the pieces the host wired in: `/proc`, the sandbox **squashfs**
//!      (added by the host via `krun_add_disk`, appears as a virtio-blk device),
//!      and the **workspace** (shared via `krun_add_virtiofs`).
//!   2. Listen on a vsock port (libkrun bridges it to a unix socket on the
//!      macOS host) and, per connection, run the host-supplied **bwrap argv**
//!      (built by `build_bwrap_argv` with guest paths) and stream stdout/stderr
//!      + the exit status back over `sandbox-vm-protocol` frames.
//!
//! The agent is a *dumb executor*: all hardening lives in the argv the host
//! sends, so `build_bwrap_argv` stays the single source of truth across
//! Linux/macOS/Windows backends.
//!
//! ## Guest contract (must match the host backend's libkrun configuration)
//!  - vsock port [`VSOCK_PORT`] — agent listens; host connects via the bridge.
//!  - [`ROOTFS_DEVICE`] — the squashfs block device; mounted read-only at
//!    [`ROOTFS_MOUNT`] (the host passes this as the rootfs dir to
//!    `build_bwrap_argv`, so the argv's `--ro-bind <rootfs>/usr /usr` resolves).
//!  - virtio-fs tag [`WORKSPACE_TAG`] — mounted at [`WORKSPACE_MOUNT`]; the
//!    host points the workspace bind in the argv at a path under it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use sandbox_vm_protocol::{
    encode, CgroupLimits, Decoder, ExitStatus, Frame, KillProcessRequest, ProcessExitStatus,
    ProcessRequest, StartedAck, PROGRESS_GUEST_FIFO_PATH, PROGRESS_MAX_LINE_BYTES, PROTOCOL_VERSION,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_vsock::{VsockAddr, VsockListener};

/// vsock port the agent listens on. libkrun bridges this to a host unix socket.
const VSOCK_PORT: u32 = 1024;
/// The squashfs disk the host adds via `krun_add_disk` (virtio-blk).
const ROOTFS_DEVICE: &str = "/dev/vda";
/// Where the agent mounts the squashfs; matches the rootfs dir the host passes
/// to `build_bwrap_argv`.
const ROOTFS_MOUNT: &str = "/sandbox-rootfs";
/// virtio-fs tag the host shares the workspace root under.
const WORKSPACE_TAG: &str = "workspace";
const WORKSPACE_MOUNT: &str = "/workspace";
/// Base dir for host-folder mounts (feature #3). A tmpfs is mounted here (the
/// guest root is RO), then each extra virtio-fs share at /host-mounts/<i>. MUST
/// match mac_vm.rs's `GUEST_EXTRA_MOUNTS_DIR`.
#[cfg(target_os = "linux")]
const EXTRA_MOUNTS_DIR: &str = "/host-mounts";
/// Tag prefix for host-folder shares. MUST match the launcher's
/// `EXTRA_MOUNT_TAG_PREFIX`.
#[cfg(target_os = "linux")]
const EXTRA_MOUNT_TAG_PREFIX: &str = "host-mount-";

/// Chunk size for streaming child stdout/stderr.
const READ_CHUNK: usize = 64 * 1024;

/// Per-file cap on a streamed artifact (mirrors the runner's
/// `PER_FILE_ARTIFACT_CAP_BYTES` so the guest never streams a file the host
/// would only reject; also keeps each frame under `MAX_FRAME_PAYLOAD`).
#[cfg(target_os = "linux")]
const ARTIFACT_PER_FILE_CAP_BYTES: u64 = 10 * 1024 * 1024;

/// Total cap across all streamed artifacts for one exec (mirrors the runner's
/// `PER_RUN_ARTIFACT_CAP_BYTES`). A defense-in-depth backstop — the host also
/// caps downstream.
#[cfg(target_os = "linux")]
const ARTIFACT_TOTAL_CAP_BYTES: u64 = 100 * 1024 * 1024;

// Non-Linux stub so the bin can compile in a cargo workspace check
// from a Mac/Windows host. Cross-compile to Linux for the real build.
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("ziee-sandbox-agent is built for Linux guest VMs only");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .init();

    init_mounts();
    cgroup_init();

    // Build the seccomp BPF once from the shared policy crate (identical to the
    // Linux host). Per-exec we pipe these bytes to bwrap's --seccomp fd. If the
    // build fails (a broken guest image), execs that request seccomp fail
    // closed rather than running unfiltered.
    let bpf: Option<Arc<Vec<u8>>> = match sandbox_seccomp::build_bpf() {
        Ok((bytes, unresolved)) => {
            if !unresolved.is_empty() {
                tracing::warn!(?unresolved, "agent: some seccomp DENY entries unresolved on this kernel");
            }
            Some(Arc::new(bytes))
        }
        Err(e) => {
            tracing::error!("agent: seccomp filter build FAILED: {e} — seccomp'd execs will fail closed");
            None
        }
    };

    // The control transport differs per backend (the agent is the single guest
    // executor for both): libkrun (macOS) bridges a vsock port to a host unix
    // socket; WSL2 (Windows) reaches the agent over localhost TCP (WSL2
    // auto-forwards). Default to vsock:1024 so the macOS launcher needs no arg.
    match parse_listen() {
        Listen::Vsock(port) => serve_vsock(port, bpf).await,
        Listen::Tcp(addr) => serve_tcp(&addr, bpf).await,
    }
}

/// Which control transport to listen on.
enum Listen {
    Vsock(u32),
    Tcp(String),
}

/// Parse `--listen vsock:<port>` / `--listen tcp:<addr>` from argv; default
/// `vsock:1024` (macOS/libkrun back-compat).
fn parse_listen() -> Listen {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--listen" {
            if let Some(spec) = args.next() {
                if let Some(port) = spec.strip_prefix("vsock:") {
                    return Listen::Vsock(port.parse().unwrap_or(VSOCK_PORT));
                }
                if let Some(addr) = spec.strip_prefix("tcp:") {
                    return Listen::Tcp(addr.to_string());
                }
            }
        }
    }
    Listen::Vsock(VSOCK_PORT)
}

#[cfg(target_os = "linux")]
async fn serve_vsock(port: u32, bpf: Option<Arc<Vec<u8>>>) {
    let listener = match VsockListener::bind(VsockAddr::new(libc::VMADDR_CID_ANY, port)) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("agent: failed to bind vsock port {port}: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!("ziee-sandbox-agent: listening on vsock port {port}");
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                // Audit H-2: reject any peer that isn't the host. We bind on
                // VMADDR_CID_ANY (you can't bind to a single remote cid in
                // AF_VSOCK), so on Windows WSL2's shared utility VM a sibling
                // distro that can `socket(AF_VSOCK)` could in principle reach
                // us. The host appears as cid=2 (VMADDR_CID_HOST) from inside
                // every libkrun guest and inside every WSL2 Linux distro;
                // anything else is by definition a sibling guest and must be
                // refused with the connection torn down so the workload it
                // would have asked us to spawn never happens. On Mac libkrun
                // the host bridge also presents as cid=2, so this filter is a
                // no-op on the legitimate path.
                let peer_cid = peer.cid();
                if peer_cid != libc::VMADDR_CID_HOST {
                    tracing::warn!(
                        "agent: refusing vsock connection from non-host peer (cid={peer_cid}, port={})",
                        peer.port()
                    );
                    drop(stream);
                    continue;
                }
                spawn_conn(stream, &bpf, format!("{peer:?}"))
            }
            Err(e) => {
                tracing::warn!("agent: vsock accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[cfg(target_os = "linux")]
async fn serve_tcp(addr: &str, bpf: Option<Arc<Vec<u8>>>) {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("agent: failed to bind tcp {addr}: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!("ziee-sandbox-agent: listening on tcp {addr}");
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => spawn_conn(stream, &bpf, format!("{peer}")),
            Err(e) => {
                tracing::warn!("agent: tcp accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

// spawn_conn calls install_seccomp + uses libc::pidfd_open; both Linux-only.
#[cfg(target_os = "linux")]
fn spawn_conn<S>(stream: S, bpf: &Option<Arc<Vec<u8>>>, peer: String)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    tracing::info!("agent: connection from {peer}");
    let bpf = bpf.clone();
    tokio::spawn(async move {
        if let Err(e) = handle_conn(stream, bpf).await {
            tracing::warn!("agent: connection handler error: {e}");
        }
    });
}

/// Best-effort mounts. Failures are logged, not fatal: an `execute_command`
/// against a missing rootfs returns a clear non-zero exit, which is more useful
/// than the agent refusing to start.
// libc::mount + MS_RDONLY are Linux-only — gating these two functions
// keeps `cargo check --workspace` happy on Mac/Windows hosts. main()
// has a parallel cfg gate.
#[cfg(target_os = "linux")]
fn init_mounts() {
    mount_fs("proc", "/proc", "proc", 0, None);
    // Guest root is mounted RO via virtio-fs (host-side gap #2). Provide a
    // writable /tmp so the seccomp BPF build (which uses `tempfile`) and any
    // other ephemeral guest work has somewhere to land.
    mount_fs("tmpfs", "/tmp", "tmpfs", 0, Some("size=16m,mode=1777"));

    // The two backends differ in HOW the rootfs reaches the guest:
    //
    //   - libkrun (macOS): the host attaches the squashfs as a virtio-blk
    //     disk at /dev/vda and shares the workspace over virtio-fs. We mount
    //     both here, and bwrap (rootfs = /sandbox-rootfs) chroots into the
    //     squashfs.
    //
    //   - WSL2 (Windows): there is no /dev/vda and no virtio-fs — the
    //     `wsl --import`ed distro filesystem IS the rootfs, already at `/`.
    //     bwrap still chroots into /sandbox-rootfs (shared argv), so we
    //     recursively bind `/` there. Without this, /sandbox-rootfs is empty,
    //     bwrap chroots into nothing, and any sandboxed exec (e.g. the python
    //     MCP server) fails to start — surfacing on the host as
    //     "connection closed: initialize response".
    //
    // Detect the backend by the presence of the libkrun rootfs disk.
    if std::path::Path::new(ROOTFS_DEVICE).exists() {
        mount_fs(ROOTFS_DEVICE, ROOTFS_MOUNT, "squashfs", libc::MS_RDONLY, None);
        mount_fs(WORKSPACE_TAG, WORKSPACE_MOUNT, "virtiofs", 0, None);
    } else {
        // WSL2: the imported distro filesystem IS the rootfs, already at `/`.
        //
        // Do NOT bind `/` into `/sandbox-rootfs`. Binding the mount-namespace
        // root — even NON-recursively — puts the namespace in a state where a
        // subsequent `unshare(CLONE_NEWUSER)` fails with EPERM, so bwrap's
        // `--unshare-user` (and thus every sandboxed exec) would break with
        // "No permissions to creating new namespace". Verified empirically on
        // the WSL2 kernel: `mount --bind / X` then `unshare --user` → EPERM.
        //
        // Instead the WSL2 backend points bwrap's rootfs directly at `/`
        // (see `Wsl2Backend::ensure_rootfs_ready`), so bwrap binds the distro's
        // real /usr, /etc, … into the sandbox. Here we only ensure /workspace
        // exists (the host rsyncs into it and the bwrap argv binds it as
        // /home/sandboxuser).
        let _ = std::fs::create_dir_all(WORKSPACE_MOUNT);
    }

    // Make /workspace world-writable so the sandboxed user (uid 1001
    // via --unshare-user in the bwrap argv) can create files inside
    // it. The virtio-fs share inherits its mode from the host dir,
    // which on Mac is owned by the running user (not uid 1001). Without
    // this chmod, bwrap's bind-target creation under /home/sandboxuser
    // (bound from /workspace) fails with EACCES.
    // Same for /tmp's mode-1777 above — kernel honors the explicit
    // tmpfs option; virtio-fs needs an explicit post-mount chmod.
    let c_ws = std::ffi::CString::new(WORKSPACE_MOUNT).unwrap();
    let chmod_rc = unsafe { libc::chmod(c_ws.as_ptr(), 0o1777) };
    if chmod_rc != 0 {
        tracing::warn!(
            "agent: chmod {WORKSPACE_MOUNT} 1777 failed: {} — sandboxed writes may fail",
            std::io::Error::last_os_error()
        );
    }

    // Host-folder mounts (feature #3): the launcher shared N folders as
    // read-only virtio-fs devices (tags host-mount-0..N-1) and told us N via
    // the ZIEE_EXTRA_MOUNTS env. The guest root is RO, so mount a tmpfs at
    // /host-mounts to hold the mountpoints, then mount each share at
    // /host-mounts/<i>. bwrap (argv built by the server) binds these to
    // /mnt/<full host path>. No-op when N == 0.
    if let Ok(n) = std::env::var("ZIEE_EXTRA_MOUNTS")
        .unwrap_or_default()
        .parse::<usize>()
    {
        if n > 0 {
            mount_fs("tmpfs", EXTRA_MOUNTS_DIR, "tmpfs", 0, Some("size=1m,mode=0755"));
            for i in 0..n {
                let tag = format!("{EXTRA_MOUNT_TAG_PREFIX}{i}");
                let target = format!("{EXTRA_MOUNTS_DIR}/{i}");
                let _ = std::fs::create_dir_all(&target);
                mount_fs(&tag, &target, "virtiofs", 0, None);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn mount_fs(src: &str, target: &str, fstype: &str, flags: libc::c_ulong, data: Option<&str>) {
    use std::ffi::CString;
    let c_src = CString::new(src).unwrap();
    let c_tgt = CString::new(target).unwrap();
    let c_fs = CString::new(fstype).unwrap();
    let c_data = data.map(|d| CString::new(d).unwrap());
    let data_ptr = c_data
        .as_ref()
        .map(|c| c.as_ptr() as *const libc::c_void)
        .unwrap_or(std::ptr::null());
    // SAFETY: all pointers are valid CStrings living for the call.
    let rc = unsafe { libc::mount(c_src.as_ptr(), c_tgt.as_ptr(), c_fs.as_ptr(), flags, data_ptr) };
    if rc != 0 {
        tracing::warn!(
            "agent: mount {src} -> {target} ({fstype}) failed: {}",
            std::io::Error::last_os_error()
        );
    } else {
        tracing::info!("agent: mounted {src} -> {target} ({fstype})");
    }
}

/// Handle one control connection. Dispatches on the first structured frame
/// the host sends:
/// - `Exec` → one-shot mode: run bwrap, stream output, send `Exit`, close.
///   Byte-identical with every prior release (audited path).
/// - `StartProcess` → long-lived mode: enter a multi-process loop; host can
///   issue further `StartProcess` / `Stdin` / `KillProcess` / `Ping` /
///   `Shutdown` frames until disconnect. Used by MCP-in-sandbox.
// handle_conn calls install_seccomp + uses Linux pidfd APIs.
#[cfg(target_os = "linux")]
async fn handle_conn<S>(stream: S, bpf: Option<Arc<Vec<u8>>>) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut rd, wr) = tokio::io::split(stream);

    // Read until we observe a structured frame (Exec, StartProcess, or
    // Shutdown). Unknown / pre-Exec frames are logged and skipped to
    // preserve the prior tolerant behaviour.
    let mut decoder = Decoder::new();
    let mut buf = [0u8; READ_CHUNK];
    enum FirstFrame {
        OneShot(sandbox_vm_protocol::ExecRequest),
        LongLived(ProcessRequest),
    }
    let first = loop {
        match decoder.next_frame() {
            Ok(Some(Frame::Exec(req))) => break FirstFrame::OneShot(req),
            Ok(Some(Frame::StartProcess(req))) => break FirstFrame::LongLived(req),
            Ok(Some(Frame::Shutdown)) => {
                // Clean stop requested by the host (WSL2 backend on
                // distro-evict; macOS could use it too). Exiting this
                // process triggers bwrap's `--die-with-parent` for any
                // in-flight children that other connections were running.
                // Status 0 is the explicit "clean shutdown" signal.
                tracing::info!("agent: shutdown requested by host; exiting");
                std::process::exit(0);
            }
            Ok(Some(other)) => {
                tracing::warn!("agent: ignoring unexpected pre-exec frame: {other:?}");
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("agent: protocol error: {e}");
                return Ok(());
            }
        }
        let n = rd.read(&mut buf).await?;
        if n == 0 {
            return Ok(()); // peer closed before sending a request
        }
        decoder.feed(&buf[..n]);
    };

    match first {
        FirstFrame::OneShot(req) => handle_one_shot(rd, wr, req, bpf).await,
        FirstFrame::LongLived(req) => handle_long_lived(rd, wr, decoder, req, bpf).await,
    }
}

/// Existing one-shot exec path — byte-identical with prior releases.
/// All hardening lives in the argv the host built; the agent execs it,
/// streams output, sends `Exit`, closes.
#[cfg(target_os = "linux")]
async fn handle_one_shot<R, W>(
    mut rd: R,
    wr: W,
    req: sandbox_vm_protocol::ExecRequest,
    bpf: Option<Arc<Vec<u8>>>,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    // Reject mismatched protocol versions loudly — defends against operators
    // running a stale agent binary against a fresh server (or vice versa).
    // `#[serde(default)]` on `protocol_version` means peers that predate the
    // field send `0`, which never matches the current PROTOCOL_VERSION.
    if req.protocol_version != PROTOCOL_VERSION {
        tracing::error!(
            request_id = req.request_id,
            peer_version = req.protocol_version,
            agent_version = PROTOCOL_VERSION,
            "agent: protocol version mismatch; rejecting"
        );
        return Ok(());
    }

    tracing::info!(
        request_id = req.request_id,
        argv_len = req.argv.len(),
        "agent: running bwrap"
    );

    // macOS libkrun virtio-fs CREATE-EPERM workaround: provision each requested
    // artifact-collection dir (under the guest's already-writable /tmp tmpfs)
    // BEFORE bwrap runs, so a sandboxed `open(O_CREAT)`/`mkdir` there succeeds
    // (a virtio-fs RW bind would fail CREATE with EPERM on libkrun). The bwrap
    // argv binds THESE dirs as the RW mounts; we walk + stream them back after
    // exit. No-op for the common (empty) case.
    provision_artifact_tmpfs(&req.collect_artifacts);

    // Single writer task owns the write half; stdout/stderr readers + the exit
    // funnel frames through this channel so concurrent writes are serialized.
    let (tx, mut frame_rx) = mpsc::unbounded_channel::<Frame>();
    let writer = tokio::spawn(async move {
        let mut wr = wr;
        while let Some(frame) = frame_rx.recv().await {
            if wr.write_all(&encode(&frame)).await.is_err() {
                break;
            }
        }
        let _ = wr.flush().await;
    });

    // Seccomp: if the host put `--seccomp <fd>` in the argv, pipe the shared
    // BPF to that fd in the bwrap child (mirrors the Linux host's SeccompPipe).
    if req.seccomp_fd.is_some() && bpf.is_none() {
        let _ = tx.send(Frame::Stderr(
            b"agent: seccomp requested but the guest filter failed to build\n".to_vec(),
        ));
        let _ = tx.send(Frame::Exit(ExitStatus { code: -1, timed_out: false }));
        drop(tx);
        let _ = writer.await;
        return Ok(());
    }

    let mut cmd = Command::new(&req.bwrap_path);
    cmd.args(&req.argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    // Install the seccomp pipe (read end dup2'd to the host-specified fd in the
    // bwrap child). Keep the read fd to close after spawn.
    let (seccomp_read_fd, seccomp_writer) = match (req.seccomp_fd, bpf.as_ref()) {
        (Some(fd), Some(bytes)) => match install_seccomp(&mut cmd, fd, bytes.clone()) {
            Ok((rfd, h)) => (Some(rfd), Some(h)),
            Err(e) => {
                let _ = tx.send(Frame::Stderr(format!("agent: seccomp setup failed: {e}\n").into_bytes()));
                let _ = tx.send(Frame::Exit(ExitStatus { code: -1, timed_out: false }));
                drop(tx);
                let _ = writer.await;
                return Ok(());
            }
        },
        _ => (None, None),
    };

    // Live-progress FIFO (workflow sandbox step). When the host set
    // `req.progress`, create the named pipe at the guest-local path (NOT
    // virtio-fs — libkrun's virtio-fs has an mkfifo EPERM bug) BEFORE spawning
    // bwrap, because the host-built argv `--bind`s it onto `/ziee/progress`. The
    // reader forwards each newline-trimmed line as a `Frame::ProcessProgress`
    // (using `request_id` as the handle); it ends when the bwrap child exits
    // (the guard's drop unlinks the FIFO + aborts the reader).
    let progress = if req.progress {
        match ProgressFifo::spawn(req.request_id, tx.clone()) {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!("agent: progress FIFO setup failed: {e}; bwrap bind will fail");
                None
            }
        }
    } else {
        None
    };

    let spawned = cmd.spawn();
    // The child holds its own dup of the read fd; close ours so the writer's
    // EOF is observed once it finishes.
    if let Some(rfd) = seccomp_read_fd {
        unsafe { libc::close(rfd) };
    }
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Frame::Stderr(format!("agent: failed to spawn bwrap: {e}\n").into_bytes()));
            let _ = tx.send(Frame::Exit(ExitStatus { code: -1, timed_out: false }));
            drop(tx);
            let _ = writer.await;
            return Ok(());
        }
    };

    // In-guest cgroup v2 (defense-in-depth; the prlimit wrapper in the argv is
    // the always-on backstop). Held until the end of the fn so Drop removes the
    // cgroup after the child exits.
    let _cgroup = match (&req.cgroup, child.id()) {
        (Some(limits), Some(pid)) => match GuestCgroup::create(req.request_id, limits) {
            Ok(cg) => {
                cg.attach(pid);
                Some(cg)
            }
            Err(e) => {
                tracing::warn!("agent: cgroup create failed: {e}; prlimit still applies");
                None
            }
        },
        _ => None,
    };

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let out_tx = tx.clone();
    let err_tx = tx.clone();
    let out_task = tokio::spawn(pump(stdout, out_tx, true));
    let err_task = tokio::spawn(pump(stderr, err_tx, false));

    // Wait for the command, racing three things: normal exit, the wall-clock
    // budget (→ SIGKILL; bwrap's --die-with-parent collapses the tree), and the
    // host disconnecting (B5 — turn aborted). On host disconnect we kill the
    // workload instead of letting it run to the full timeout.
    let budget = Duration::from_millis(req.timeout_ms.max(1));
    let (code, timed_out) = tokio::select! {
        res = child.wait() => match res {
            Ok(status) => (status.code().unwrap_or(-1), false),
            Err(_) => (-1, false),
        },
        _ = tokio::time::sleep(budget) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            (-1, true)
        }
        _ = wait_host_eof(&mut rd) => {
            tracing::info!(request_id = req.request_id, "agent: host disconnected; killing command");
            let _ = child.start_kill();
            let _ = child.wait().await;
            (-1, false)
        }
    };

    // Make sure all output is flushed before Exit.
    let _ = out_task.await;
    let _ = err_task.await;
    // Join the seccomp BPF writer (bwrap has consumed the filter by now); this
    // reaps the task instead of leaking it and surfaces a truncated-write log.
    if let Some(h) = seccomp_writer {
        let _ = h.await;
    }
    // Stop the progress reader + unlink the FIFO BEFORE dropping `tx`: the
    // bwrap child has exited so no further `$ZIEE_PROGRESS` writes arrive, and
    // we don't want an orphaned reader holding the writer half of `tx`.
    drop(progress);

    // macOS virtio-fs CREATE-EPERM workaround: walk each artifact-collection
    // tmpfs dir and stream its files back to the host (which writes them to the
    // real host artifact dir — the host's own fs, which works). Sent BEFORE
    // Exit so the host has every file before it tears down + collects. No-op
    // for the common (empty) case → byte-identical to prior releases.
    stream_collected_artifacts(&req.collect_artifacts, &tx);

    let _ = tx.send(Frame::Exit(ExitStatus { code, timed_out }));
    drop(tx);
    let _ = writer.await;
    Ok(())
}

/// Provision each artifact-collection dir so a sandboxed `open(O_CREAT)`/`mkdir`
/// succeeds (libkrun virtio-fs CREATE fails EPERM — these dirs live UNDER the
/// guest's already-writable `/tmp` tmpfs, NOT on the RO virtio-fs root, so the
/// `mkdir` lands and the bwrap RW bind to it can be created in place). chmod
/// 1777 so bwrap's uid-1001 workload can write files. Best-effort + Linux-only;
/// a failure just means the dir behaves like the (broken) virtio-fs bind would
/// have — no worse than before.
#[cfg(target_os = "linux")]
fn provision_artifact_tmpfs(dirs: &[String]) {
    for dir in dirs {
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!("agent: mkdir artifact dir {dir} failed: {e}");
            continue;
        }
        let c = match std::ffi::CString::new(dir.as_str()) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("agent: artifact dir {dir} has an interior NUL byte: {e}");
                continue;
            }
        };
        let rc = unsafe { libc::chmod(c.as_ptr(), 0o1777) };
        if rc != 0 {
            tracing::warn!(
                "agent: chmod {dir} 1777 failed: {} — sandboxed artifact writes may fail",
                std::io::Error::last_os_error()
            );
        } else {
            tracing::info!("agent: provisioned artifact dir {dir}");
        }
    }
}

/// Walk each artifact-collection tmpfs dir and send one `ArtifactFile` frame per
/// regular file. `mount_index` is the dir's position in `dirs`. Respects a
/// per-file + cumulative cap (mirrors the host runner's caps). Best-effort:
/// errors are logged + the file skipped; we never fail the exec over artifacts.
#[cfg(target_os = "linux")]
fn stream_collected_artifacts(dirs: &[String], tx: &mpsc::UnboundedSender<Frame>) {
    let mut total: u64 = 0;
    for (mount_index, dir) in dirs.iter().enumerate() {
        let root = std::path::Path::new(dir);
        if !root.is_dir() {
            continue;
        }
        walk_artifacts(root, root, mount_index as u32, tx, &mut total);
    }
}

/// Recursive walk for `stream_collected_artifacts`. Sends regular files;
/// rejects symlinks + oversize files; bails the whole walk once the cumulative
/// cap is crossed (the host re-caps downstream anyway).
#[cfg(target_os = "linux")]
fn walk_artifacts(
    root: &std::path::Path,
    cur: &std::path::Path,
    mount_index: u32,
    tx: &mpsc::UnboundedSender<Frame>,
    total: &mut u64,
) {
    let rd = match std::fs::read_dir(cur) {
        Ok(rd) => rd,
        Err(e) => {
            tracing::warn!("agent: read_dir artifact {} failed: {e}", cur.display());
            return;
        }
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("agent: stat artifact {} failed: {e}", path.display());
                continue;
            }
        };
        if md.file_type().is_symlink() {
            // Defense-in-depth: never follow/stream a symlink (the host also
            // rejects them).
            continue;
        }
        if md.is_dir() {
            walk_artifacts(root, &path, mount_index, tx, total);
            continue;
        }
        if !md.is_file() {
            continue;
        }
        if md.len() > ARTIFACT_PER_FILE_CAP_BYTES {
            tracing::warn!(
                "agent: skipping oversize artifact {} ({} bytes)",
                path.display(),
                md.len()
            );
            continue;
        }
        if total.saturating_add(md.len()) > ARTIFACT_TOTAL_CAP_BYTES {
            tracing::warn!(
                "agent: artifact total cap reached; not streaming {}",
                path.display()
            );
            return;
        }
        let rel = match path.strip_prefix(root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        // Forward-slash, no leading slash (the host joins it onto the host dir).
        let rel_path = rel.to_string_lossy().replace('\\', "/");
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("agent: read artifact {} failed: {e}", path.display());
                continue;
            }
        };
        *total = total.saturating_add(data.len() as u64);
        let _ = tx.send(Frame::ArtifactFile {
            mount_index,
            rel_path,
            data,
        });
    }
}

/// Long-lived multi-process loop. Each `StartProcess` registers a new
/// child in `registry`; subsequent `Stdin` / `KillProcess` frames look
/// the handle up and forward. On host disconnect every live child is
/// SIGKILL'd (mirrors the one-shot host-EOF cleanup) and the writer
/// drains before we return.
#[cfg(target_os = "linux")]
async fn handle_long_lived<R, W>(
    mut rd: R,
    wr: W,
    mut decoder: Decoder,
    first: ProcessRequest,
    bpf: Option<Arc<Vec<u8>>>,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut frame_rx) = mpsc::unbounded_channel::<Frame>();
    let writer = tokio::spawn(async move {
        let mut wr = wr;
        while let Some(frame) = frame_rx.recv().await {
            if wr.write_all(&encode(&frame)).await.is_err() {
                break;
            }
        }
        let _ = wr.flush().await;
    });

    // Shared registry of live processes. Per-process tasks (wait task)
    // remove their own entry on exit; the dispatcher reads for forwarding.
    let registry: Arc<Mutex<HashMap<u64, HandleEntry>>> = Arc::new(Mutex::new(HashMap::new()));

    // Spawn the first process.
    spawn_long_lived(first, &registry, &tx, &bpf).await;

    // Main dispatch loop: read frames from the host, dispatch by tag.
    let mut buf = [0u8; READ_CHUNK];
    let host_disconnected = loop {
        // Pull every fully-buffered frame before reading more bytes.
        loop {
            match decoder.next_frame() {
                Ok(Some(Frame::StartProcess(req))) => {
                    spawn_long_lived(req, &registry, &tx, &bpf).await;
                }
                Ok(Some(Frame::Stdin { handle, bytes })) => {
                    // Empty bytes = EOF (close stdin). Forward via the
                    // per-process stdin channel; if the receiver was
                    // already dropped (process exited), silently drop.
                    let stdin_tx = registry
                        .lock()
                        .unwrap()
                        .get(&handle)
                        .map(|e| e.stdin_tx.clone());
                    if let Some(s) = stdin_tx {
                        let _ = s.send(if bytes.is_empty() { None } else { Some(bytes) });
                    }
                }
                Ok(Some(Frame::KillProcess(KillProcessRequest { handle }))) => {
                    let pid = registry.lock().unwrap().get(&handle).map(|e| e.pid);
                    if let Some(pid) = pid {
                        kill_pid(pid);
                    }
                }
                Ok(Some(Frame::Ping)) => {
                    let _ = tx.send(Frame::Pong);
                }
                Ok(Some(Frame::Shutdown)) => {
                    tracing::info!("agent: shutdown requested mid-session; exiting");
                    // Kill every live process before we exit so output
                    // doesn't get cut off and then we go away anyway.
                    for (_, entry) in registry.lock().unwrap().drain() {
                        kill_pid(entry.pid);
                    }
                    std::process::exit(0);
                }
                Ok(Some(other)) => {
                    tracing::warn!("agent: long-lived: ignoring unexpected frame: {other:?}");
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("agent: long-lived protocol error: {e}");
                    break;
                }
            }
        }
        let n = rd.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            break true; // host disconnected
        }
        decoder.feed(&buf[..n]);
    };

    if host_disconnected {
        tracing::info!("agent: host disconnected; killing all live long-lived processes");
        for (_, entry) in registry.lock().unwrap().drain() {
            kill_pid(entry.pid);
        }
    }

    // Drop the dispatcher's tx clone so the writer task drains and exits
    // once every per-process pump finishes flushing.
    drop(tx);
    let _ = writer.await;
    Ok(())
}

/// Per-process state held in the long-lived registry.
#[cfg(target_os = "linux")]
struct HandleEntry {
    /// `Some(bytes)` → write `bytes` to the child's stdin; `None` → close stdin.
    stdin_tx: mpsc::UnboundedSender<Option<Vec<u8>>>,
    /// Child PID so the dispatcher can SIGKILL on `KillProcess` /
    /// host-disconnect without holding the `Child` (which is owned by
    /// the wait task).
    pid: u32,
}

/// SIGKILL a pid, swallowing `ESRCH` (already-exited race with the wait task).
#[cfg(target_os = "linux")]
fn kill_pid(pid: u32) {
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    if rc != 0 {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if errno != libc::ESRCH {
            tracing::warn!(
                "agent: kill pid {pid} failed: {}",
                std::io::Error::from_raw_os_error(errno)
            );
        }
    }
}

/// Spawn one long-lived bwrap process. On success: registers the handle
/// in `registry`, sends `Started { ok: true }`, and starts per-process
/// stdin / stdout / stderr / wait tasks. On failure: sends
/// `Started { ok: false, err: Some(...) }` and registers nothing.
#[cfg(target_os = "linux")]
async fn spawn_long_lived(
    req: ProcessRequest,
    registry: &Arc<Mutex<HashMap<u64, HandleEntry>>>,
    tx: &mpsc::UnboundedSender<Frame>,
    bpf: &Option<Arc<Vec<u8>>>,
) {
    let handle = req.handle;

    if req.protocol_version != PROTOCOL_VERSION {
        let _ = tx.send(Frame::Started(StartedAck {
            handle,
            ok: false,
            err: Some(format!(
                "protocol version mismatch (peer={}, agent={PROTOCOL_VERSION})",
                req.protocol_version
            )),
        }));
        return;
    }

    if req.seccomp_fd.is_some() && bpf.is_none() {
        let _ = tx.send(Frame::Started(StartedAck {
            handle,
            ok: false,
            err: Some("seccomp requested but the guest filter failed to build".into()),
        }));
        return;
    }

    // Refuse handle collisions explicitly so the host learns to retry
    // with a fresh ID rather than silently overwriting state.
    if registry.lock().unwrap().contains_key(&handle) {
        let _ = tx.send(Frame::Started(StartedAck {
            handle,
            ok: false,
            err: Some("handle already in use on this connection".into()),
        }));
        return;
    }

    let mut cmd = Command::new(&req.bwrap_path);
    cmd.args(&req.argv)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let (seccomp_read_fd, seccomp_writer) = match (req.seccomp_fd, bpf.as_ref()) {
        (Some(fd), Some(bytes)) => match install_seccomp(&mut cmd, fd, bytes.clone()) {
            Ok((rfd, h)) => (Some(rfd), Some(h)),
            Err(e) => {
                let _ = tx.send(Frame::Started(StartedAck {
                    handle,
                    ok: false,
                    err: Some(format!("seccomp setup failed: {e}")),
                }));
                return;
            }
        },
        _ => (None, None),
    };

    let spawned = cmd.spawn();
    if let Some(rfd) = seccomp_read_fd {
        unsafe { libc::close(rfd) };
    }
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Frame::Started(StartedAck {
                handle,
                ok: false,
                err: Some(format!("spawn bwrap: {e}")),
            }));
            return;
        }
    };

    let pid = match child.id() {
        Some(p) => p,
        None => {
            let _ = child.start_kill();
            let _ = tx.send(Frame::Started(StartedAck {
                handle,
                ok: false,
                err: Some("spawned child had no pid".into()),
            }));
            return;
        }
    };

    // In-guest cgroup (defense-in-depth; prlimit in the argv is the backstop).
    let cgroup = match &req.cgroup {
        Some(limits) => match GuestCgroup::create(handle, limits) {
            Ok(cg) => {
                cg.attach(pid);
                Some(cg)
            }
            Err(e) => {
                tracing::warn!("agent: cgroup create failed for handle {handle}: {e}");
                None
            }
        },
        None => None,
    };

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let stdin = child.stdin.take().expect("stdin piped");

    // Per-process stdin pump: drains the unbounded channel into the
    // child's stdin. `None` closes stdin (by dropping the writer).
    let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<Option<Vec<u8>>>();
    tokio::spawn(async move {
        let mut stdin = stdin;
        while let Some(msg) = stdin_rx.recv().await {
            match msg {
                Some(bytes) => {
                    if stdin.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                None => break, // EOF
            }
        }
        let _ = stdin.shutdown().await;
        drop(stdin);
    });

    // stdout/stderr pumps fan into the shared writer via tagged frames.
    let out_tx = tx.clone();
    let err_tx = tx.clone();
    tokio::spawn(pump_handle(stdout, out_tx, handle, true));
    tokio::spawn(pump_handle(stderr, err_tx, handle, false));

    registry.lock().unwrap().insert(handle, HandleEntry { stdin_tx, pid });
    let _ = tx.send(Frame::Started(StartedAck { handle, ok: true, err: None }));

    // Wait task: when the child exits, send ProcessExit and remove the
    // registry entry so future Stdin / KillProcess frames no-op cleanly.
    let exit_tx = tx.clone();
    let registry_for_wait = registry.clone();
    tokio::spawn(async move {
        let _cgroup = cgroup; // held until exit so Drop rmdir's the cgroup
        let status = child.wait().await;
        // Reap the seccomp BPF writer now that the child has consumed its
        // filter and exited — joins the task instead of leaking it.
        if let Some(h) = seccomp_writer {
            let _ = h.await;
        }
        let code = status.as_ref().ok().and_then(|s| s.code()).unwrap_or(-1);
        // Remove BEFORE sending the exit so any concurrent KillProcess
        // observes the missing entry instead of racing with a freshly
        // reused pid (kernel can recycle PIDs quickly under WSL2).
        registry_for_wait.lock().unwrap().remove(&handle);
        let _ = exit_tx.send(Frame::ProcessExit(ProcessExitStatus {
            handle,
            status: ExitStatus { code, timed_out: false },
        }));
    });
}

/// Stream a long-lived child pipe into tagged per-process protocol frames.
#[cfg(target_os = "linux")]
async fn pump_handle<R: AsyncReadExt + Unpin>(
    mut reader: R,
    tx: mpsc::UnboundedSender<Frame>,
    handle: u64,
    is_stdout: bool,
) {
    let mut buf = [0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let bytes = buf[..n].to_vec();
                let frame = if is_stdout {
                    Frame::ProcessStdout { handle, bytes }
                } else {
                    Frame::ProcessStderr { handle, bytes }
                };
                if tx.send(frame).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// Live-progress FIFO, guest side. Mirrors the Linux host's `ProgressFifo`:
/// create the named pipe at the fixed guest-local path, open the read end
/// `O_RDWR|O_NONBLOCK` (so it never sees EOF between the sandbox's writes), and
/// forward each newline-trimmed line as a `Frame::ProcessProgress` to the
/// connection's writer channel. Drop signals stop, aborts the reader, and
/// unlinks the FIFO so no orphaned reader survives the exec.
#[cfg(target_os = "linux")]
struct ProgressFifo {
    stop: Arc<std::sync::atomic::AtomicBool>,
    reader: Option<tokio::task::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl ProgressFifo {
    fn spawn(handle: u64, tx: mpsc::UnboundedSender<Frame>) -> std::io::Result<Self> {
        use std::os::fd::FromRawFd;

        let c_path = std::ffi::CString::new(PROGRESS_GUEST_FIFO_PATH).unwrap();
        // Remove any stale FIFO at the fixed path (prior exec on this guest).
        let _ = std::fs::remove_file(PROGRESS_GUEST_FIFO_PATH);
        // SAFETY: c_path is a valid NUL-terminated CString for the call.
        if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // O_RDWR keeps our own writer ref so the read end never hits EOF while
        // the sandbox opens/closes the FIFO per `echo`. O_NONBLOCK + AsyncFd =
        // readiness-driven async reads. O_CLOEXEC so the fd doesn't leak into
        // the bwrap child (it reaches the FIFO via the bind, not an inherited fd).
        // SAFETY: c_path valid; -1 on error (checked).
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let err = std::io::Error::last_os_error();
            let _ = std::fs::remove_file(PROGRESS_GUEST_FIFO_PATH);
            return Err(err);
        }

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_reader = stop.clone();
        // SAFETY: we just opened `fd` (O_CLOEXEC, O_NONBLOCK) and own it; the
        // reader task closes it via AsyncFd::Drop on exit.
        let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        let reader = tokio::spawn(progress_reader_loop(owned, tx, handle, stop_reader));
        Ok(Self {
            stop,
            reader: Some(reader),
        })
    }
}

#[cfg(target_os = "linux")]
impl Drop for ProgressFifo {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(h) = self.reader.take() {
            h.abort();
        }
        let _ = std::fs::remove_file(PROGRESS_GUEST_FIFO_PATH);
    }
}

/// Read newline-delimited progress lines off the FIFO fd and forward each
/// (newline-trimmed) line as a `Frame::ProcessProgress { handle, bytes }`. A
/// single FIFO `write()` under `PROGRESS_MAX_LINE_BYTES` (= PIPE_BUF) is atomic,
/// so lines don't interleave; over-cap lines are DROPPED. Ends when `stop` is
/// set (aborted on drop) or `tx` is closed.
#[cfg(target_os = "linux")]
async fn progress_reader_loop(
    fd: std::os::fd::OwnedFd,
    tx: mpsc::UnboundedSender<Frame>,
    handle: u64,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    use std::os::fd::AsRawFd;
    use tokio::io::unix::AsyncFd;

    let async_fd = match AsyncFd::new(fd) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("agent: progress AsyncFd::new failed: {e}");
            return;
        }
    };

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
        loop {
            let raw = async_fd.get_ref().as_raw_fd();
            // SAFETY: raw is valid (owned by async_fd); buf is a valid buffer.
            let n = unsafe {
                libc::read(raw, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
            };
            if n > 0 {
                for &byte in &buf[..n as usize] {
                    if byte == b'\n' {
                        if !overflowed && !pending.is_empty() {
                            let _ = tx.send(Frame::ProcessProgress {
                                handle,
                                bytes: std::mem::take(&mut pending),
                            });
                        } else {
                            pending.clear();
                        }
                        overflowed = false;
                    } else if !overflowed {
                        pending.push(byte);
                        if pending.len() > PROGRESS_MAX_LINE_BYTES {
                            pending.clear();
                            overflowed = true;
                        }
                    }
                }
                continue;
            }
            if n == 0 {
                guard.clear_ready();
                break;
            }
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
                guard.clear_ready();
                break;
            }
            if errno == libc::EINTR {
                continue;
            }
            tracing::warn!(
                "agent: progress read failed: {}; ending",
                std::io::Error::from_raw_os_error(errno)
            );
            return;
        }
    }
}

/// One-time cgroup v2 setup: ensure cgroup2 is mounted and enable the
/// controllers child cgroups will use. Best-effort — if it fails, per-exec
/// cgroup limits are simply unavailable and prlimit (in the bwrap argv) does
/// the enforcement, exactly like the Linux host's `CgroupMode::None` path.
#[cfg(target_os = "linux")]
fn cgroup_init() {
    if !std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists() {
        let _ = std::fs::create_dir_all("/sys/fs/cgroup");
        mount_fs("cgroup2", "/sys/fs/cgroup", "cgroup2", 0, None);
    }
    // The agent (in the root cgroup) is exempt from the no-internal-processes
    // rule, so enabling subtree_control on the root is allowed.
    if let Err(e) = std::fs::write("/sys/fs/cgroup/cgroup.subtree_control", "+memory +pids +cpu") {
        tracing::warn!(
            "agent: enabling cgroup controllers failed: {e}; per-exec cgroup \
             limits unavailable (the prlimit backstop still applies)"
        );
    }
}

/// Per-exec cgroup v2 scope under the guest root, mirroring the host's
/// `CgroupScope`. Drop removes it (empty cgroups rmdir cleanly).
struct GuestCgroup {
    path: std::path::PathBuf,
}

impl GuestCgroup {
    fn create(request_id: u64, limits: &CgroupLimits) -> std::io::Result<Self> {
        let path = std::path::PathBuf::from(format!("/sys/fs/cgroup/sb-{request_id}"));
        std::fs::create_dir(&path)?;
        // Per-controller writes are best-effort: a controller the guest kernel
        // didn't build still leaves the others enforcing. We do NOT fail the
        // whole call on a single controller miss, but every miss is logged so a
        // silently-unenforced limit (e.g. memory.max not applied) is detectable
        // rather than invisible.
        let write_limit = |attr: &str, value: &str| {
            if let Err(e) = std::fs::write(path.join(attr), value) {
                tracing::warn!(
                    "agent: cgroup {attr} not enforced for sb-{request_id}: {e} \
                     (controller likely unavailable in guest kernel)"
                );
            }
        };
        write_limit("memory.max", &limits.memory_max_bytes.to_string());
        write_limit("memory.swap.max", &limits.memory_swap_max_bytes.to_string());
        write_limit("pids.max", &limits.pids_max.to_string());
        write_limit("cpu.max", &limits.cpu_max);
        Ok(Self { path })
    }

    fn attach(&self, pid: u32) {
        if let Err(e) = std::fs::write(self.path.join("cgroup.procs"), pid.to_string()) {
            tracing::warn!("agent: cgroup attach pid {pid} failed: {e}");
        }
    }
}

impl Drop for GuestCgroup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.path);
    }
}

/// Pipe the seccomp BPF to a fd in the bwrap child, mirroring the Linux host's
/// `SeccompPipe`: create a pipe, write the bytes from a task, and `dup2` the
/// read end to `target_fd` (clearing `FD_CLOEXEC` so it survives execve into
/// bwrap, which reads it via `--seccomp <target_fd>`). Returns the parent's
/// read fd so the caller can close it after spawn.
#[cfg(target_os = "linux")]
fn install_seccomp(
    cmd: &mut Command,
    target_fd: i32,
    bpf: Arc<Vec<u8>>,
) -> std::io::Result<(i32, tokio::task::JoinHandle<()>)> {
    let mut fds: [libc::c_int; 2] = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let read_fd = fds[0];
    let write_fd = fds[1];

    // Write the BPF from a task (it may exceed the pipe buffer, so it MUST run
    // concurrently with the bwrap child that drains the read end — we can't
    // await it before spawn or a >pipe-buffer filter would deadlock). The
    // JoinHandle is returned so the caller awaits it during connection cleanup
    // instead of leaking a detached task whose truncation error would go
    // unobserved.
    let writer = tokio::spawn(async move {
        let bytes: &[u8] = bpf.as_ref();
        let mut off = 0;
        while off < bytes.len() {
            let n = unsafe {
                libc::write(
                    write_fd,
                    bytes[off..].as_ptr() as *const libc::c_void,
                    bytes.len() - off,
                )
            };
            if n > 0 {
                off += n as usize;
                continue;
            }
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if n < 0 && (errno == libc::EINTR || errno == libc::EAGAIN) {
                continue;
            }
            break;
        }
        unsafe { libc::close(write_fd) };
        if off < bytes.len() {
            tracing::error!(written = off, expected = bytes.len(), "agent: seccomp BPF write truncated");
        }
    });

    // SAFETY: dup2/fcntl are async-signal-safe; `read_fd` is valid through spawn.
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(read_fd, target_fd) < 0 {
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
    Ok((read_fd, writer))
}

/// Resolve when the host closes the control connection (read half hits EOF) or
/// errors — i.e. the turn was abandoned. The host sends only the `Exec` frame,
/// so any further read is EOF on disconnect.
async fn wait_host_eof<R: AsyncReadExt + Unpin>(rd: &mut R) {
    let mut b = [0u8; 256];
    loop {
        match rd.read(&mut b).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {} // unexpected extra data from the host; ignore
        }
    }
}

/// Stream a child pipe into protocol frames.
async fn pump<R: AsyncReadExt + Unpin>(mut reader: R, tx: mpsc::UnboundedSender<Frame>, is_stdout: bool) {
    let mut buf = [0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let chunk = buf[..n].to_vec();
                let frame = if is_stdout { Frame::Stdout(chunk) } else { Frame::Stderr(chunk) };
                if tx.send(frame).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use sandbox_vm_protocol::{Decoder, Frame, KillProcessRequest, ProcessRequest, StartedAck};
    use tokio::io::{duplex, AsyncWriteExt};

    /// Build a tiny PYTHON-FREE ProcessRequest that invokes `/bin/cat`
    /// directly (no bwrap). We point `bwrap_path` at `/bin/cat` so the
    /// agent simply execs cat with the given argv. cat with no args
    /// copies stdin → stdout; perfect for testing the long-lived
    /// stdin/stdout multiplex without depending on a rootfs.
    fn cat_request(handle: u64) -> ProcessRequest {
        ProcessRequest {
            protocol_version: PROTOCOL_VERSION,
            handle,
            bwrap_path: "/bin/cat".into(),
            argv: vec![],
            seccomp_fd: None,
            cgroup: None,
        }
    }

    /// Drive a duplex pair: host writes frames to `host_wr`, agent reads
    /// from `agent_rd`; agent writes to `agent_wr`, host reads from
    /// `host_rd`. The harness simulates the host side.
    async fn run_long_lived_with_frames(
        host_input: Vec<Frame>,
        wait_for: impl Fn(&[Frame]) -> bool + Send + 'static,
    ) -> Vec<Frame> {
        let (host_side, agent_side) = duplex(64 * 1024);
        let (mut host_rd, mut host_wr) = tokio::io::split(host_side);

        // Spawn the agent's long-lived handler on agent_side.
        let agent_task = tokio::spawn(async move {
            let _ = handle_conn(agent_side, None).await;
        });

        // Host sends every frame in order.
        tokio::spawn(async move {
            for f in host_input {
                let bytes = encode(&f);
                if host_wr.write_all(&bytes).await.is_err() {
                    break;
                }
            }
            // Close the writer so the agent observes EOF after the test
            // has read what it expected.
            let _ = host_wr.shutdown().await;
        });

        // Host reads frames into a buffer; stop when `wait_for` returns true.
        let mut decoder = Decoder::new();
        let mut collected = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            if wait_for(&collected) {
                break;
            }
            let n = host_rd.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            decoder.feed(&buf[..n]);
            while let Ok(Some(f)) = decoder.next_frame() {
                collected.push(f);
                if wait_for(&collected) {
                    break;
                }
            }
        }

        let _ = agent_task.await;
        collected
    }

    #[tokio::test]
    async fn long_lived_echoes_stdin_to_stdout_then_exits() {
        let frames = run_long_lived_with_frames(
            vec![
                Frame::StartProcess(cat_request(1)),
                Frame::Stdin { handle: 1, bytes: b"hello\n".to_vec() },
                Frame::Stdin { handle: 1, bytes: Vec::new() }, // EOF
            ],
            |frames| frames.iter().any(|f| matches!(f, Frame::ProcessExit(_))),
        )
        .await;

        let started = frames.iter().any(|f| matches!(f, Frame::Started(StartedAck { handle: 1, ok: true, .. })));
        assert!(started, "expected Started{{handle:1,ok:true}}, got {frames:?}");

        let stdout: Vec<u8> = frames
            .iter()
            .filter_map(|f| match f {
                Frame::ProcessStdout { handle: 1, bytes } => Some(bytes.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(stdout, b"hello\n", "stdout chunks did not match input");

        let exit = frames.iter().find_map(|f| match f {
            Frame::ProcessExit(ProcessExitStatus { handle: 1, status }) => Some(*status),
            _ => None,
        });
        assert!(matches!(exit, Some(s) if s.code == 0 && !s.timed_out));
    }

    #[tokio::test]
    async fn long_lived_two_handles_multiplex_independently() {
        let frames = run_long_lived_with_frames(
            vec![
                Frame::StartProcess(cat_request(1)),
                Frame::StartProcess(cat_request(2)),
                Frame::Stdin { handle: 1, bytes: b"AAA".to_vec() },
                Frame::Stdin { handle: 2, bytes: b"BBB".to_vec() },
                Frame::Stdin { handle: 1, bytes: Vec::new() }, // EOF h1
                Frame::Stdin { handle: 2, bytes: Vec::new() }, // EOF h2
            ],
            |frames| {
                let exits = frames
                    .iter()
                    .filter(|f| matches!(f, Frame::ProcessExit(_)))
                    .count();
                exits >= 2
            },
        )
        .await;

        let h1_stdout: Vec<u8> = frames
            .iter()
            .filter_map(|f| match f {
                Frame::ProcessStdout { handle: 1, bytes } => Some(bytes.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        let h2_stdout: Vec<u8> = frames
            .iter()
            .filter_map(|f| match f {
                Frame::ProcessStdout { handle: 2, bytes } => Some(bytes.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(h1_stdout, b"AAA");
        assert_eq!(h2_stdout, b"BBB");
    }

    #[tokio::test]
    async fn long_lived_kill_terminates_handle() {
        // Spawn cat that will never receive EOF and never exit on its
        // own; verify KillProcess produces a ProcessExit.
        let frames = run_long_lived_with_frames(
            vec![
                Frame::StartProcess(cat_request(7)),
                Frame::KillProcess(KillProcessRequest { handle: 7 }),
            ],
            |frames| frames.iter().any(|f| matches!(f, Frame::ProcessExit(_))),
        )
        .await;

        let exit = frames.iter().find_map(|f| match f {
            Frame::ProcessExit(ProcessExitStatus { handle: 7, status }) => Some(*status),
            _ => None,
        });
        assert!(exit.is_some(), "expected ProcessExit for handle 7, got {frames:?}");
    }

    #[tokio::test]
    async fn long_lived_ping_responds_with_pong() {
        let frames = run_long_lived_with_frames(
            vec![
                Frame::StartProcess(cat_request(1)),
                Frame::Ping,
                Frame::Stdin { handle: 1, bytes: Vec::new() }, // EOF → cat exits
            ],
            |frames| {
                let saw_pong = frames.iter().any(|f| matches!(f, Frame::Pong));
                let saw_exit = frames.iter().any(|f| matches!(f, Frame::ProcessExit(_)));
                saw_pong && saw_exit
            },
        )
        .await;
        assert!(frames.iter().any(|f| matches!(f, Frame::Pong)));
    }

    #[tokio::test]
    async fn long_lived_rejects_duplicate_handle() {
        let frames = run_long_lived_with_frames(
            vec![
                Frame::StartProcess(cat_request(1)),
                Frame::StartProcess(cat_request(1)), // collision
                Frame::Stdin { handle: 1, bytes: Vec::new() }, // EOF first cat
            ],
            |frames| {
                let oks = frames.iter().filter(|f| matches!(f, Frame::Started(s) if s.ok)).count();
                let errs = frames.iter().filter(|f| matches!(f, Frame::Started(s) if !s.ok)).count();
                let exits = frames.iter().filter(|f| matches!(f, Frame::ProcessExit(_))).count();
                oks >= 1 && errs >= 1 && exits >= 1
            },
        )
        .await;

        let errs: Vec<_> = frames
            .iter()
            .filter_map(|f| match f {
                Frame::Started(s) if !s.ok => Some(s.err.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(errs.len(), 1, "expected exactly one error Started, got {frames:?}");
        assert!(errs[0].as_deref().unwrap_or("").contains("already in use"));
    }

    #[tokio::test]
    async fn long_lived_rejects_protocol_version_mismatch() {
        // Build a ProcessRequest with the wrong version (mimics a stale host).
        let bad = ProcessRequest {
            protocol_version: 999, // not PROTOCOL_VERSION
            handle: 1,
            bwrap_path: "/bin/cat".into(),
            argv: vec![],
            seccomp_fd: None,
            cgroup: None,
        };
        let frames = run_long_lived_with_frames(
            vec![Frame::StartProcess(bad)],
            |frames| frames.iter().any(|f| matches!(f, Frame::Started(s) if !s.ok)),
        )
        .await;
        let err = frames.iter().find_map(|f| match f {
            Frame::Started(s) if !s.ok => s.err.clone(),
            _ => None,
        });
        assert!(err.unwrap_or_default().contains("protocol version mismatch"));
    }

    #[tokio::test]
    async fn kill_pid_swallows_esrch_for_dead_process() {
        // Pid 1 belongs to init on Linux; we lack the right to kill it,
        // but EPERM is *not* the ESRCH path we want to verify. Use a
        // freshly-spawned-then-reaped pid: spawn `/bin/true`, wait, then
        // try to kill its (now-reaped) pid. The OS returns ESRCH; our
        // helper must silently swallow it without panicking.
        let mut child = Command::new("/bin/true").spawn().expect("spawn /bin/true");
        let pid = child.id().expect("pid");
        let _ = child.wait().await;
        // Should not panic, should not log a warn (we can't easily
        // assert no-warn but the call returning cleanly is the contract).
        kill_pid(pid);
    }
}
