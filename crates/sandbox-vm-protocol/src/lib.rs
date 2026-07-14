//! Host ↔ guest control protocol for the code-sandbox microVM backend
//! (macOS libkrun and WSL2). The host (server) builds the **full bwrap
//! argv** with `sandbox::build_bwrap_argv` using *guest* paths and sends it to
//! the in-VM guest agent, which simply execs it and streams the output back —
//! so `build_bwrap_argv` stays the single source of truth for hardening across
//! every backend; the guest agent is a dumb executor.
//!
//! ## Wire format
//!
//! A stream of length-prefixed frames, each: `[u8 tag][u32 BE len][payload]`.
//! Structured frames (`Exec`, `Exit`, `StartProcess`, `Started`,
//! `ProcessExit`, `KillProcess`) carry a JSON payload; one-shot output
//! frames (`Stdout`, `Stderr`) carry raw bytes; long-lived per-process
//! byte frames (`Stdin`, `ProcessStdout`, `ProcessStderr`) carry an
//! 8-byte big-endian `u64` handle followed by raw bytes so many
//! processes can multiplex over a single vsock connection. Tags are
//! unique across both directions so a single decoder serves host and guest.
//!
//! ## Two-mode connection
//!
//! Each connection enters exactly one of two modes based on the first
//! structured frame the guest receives:
//!
//! - **One-shot** — first frame is `Exec`. The agent runs the command,
//!   streams `Stdout`/`Stderr`, sends `Exit`, closes. Backward-compatible
//!   with every existing release.
//! - **Long-lived** — first frame is `StartProcess`. The agent enters a
//!   multi-process loop and the host can send further `StartProcess`,
//!   `Stdin`, `KillProcess`, `Ping`, `Shutdown` frames on the same
//!   connection. Used by the MCP-in-sandbox feature to keep a single
//!   per-server vsock open across many MCP tool calls.
//!
//! This crate is **pure** (no IO): each side does its own (async) socket IO and
//! uses [`encode`] / [`Decoder`] for framing. That keeps the contract trivially
//! unit-testable and dependency-light.

use serde::{Deserialize, Serialize};

/// Wire-protocol version. Bumped any time the on-wire shape of `Frame`,
/// `ExecRequest`, or `ProcessRequest` changes in a way that would break an
/// older peer. Host + agent ship together from the same release, so a
/// mismatch always indicates an operator running a stale agent binary
/// against a fresh server (or vice versa). Defense-in-depth — surfaces the
/// mismatch loudly instead of running against undefined semantics.
///
/// Long-lived process frames (tags 6–14) were added as a purely additive
/// extension — older agents that don't understand them reject with
/// `UnknownTag`, so PROTOCOL_VERSION stays at 1.
pub const PROTOCOL_VERSION: u32 = 1;

/// Path the sandbox step's code writes live-progress lines to (the value of
/// `$ZIEE_PROGRESS`). `build_bwrap_argv` binds the host/guest progress FIFO
/// onto this path inside the bwrap mount namespace and `--setenv`s it. Fixed so
/// the argv builder + the agent + the runtime contract all agree.
pub const PROGRESS_SANDBOX_PATH: &str = "/ziee/progress";

/// Guest-local FIFO path the agent creates and the host-built bwrap argv binds
/// to [`PROGRESS_SANDBOX_PATH`]. MUST live on a guest-local filesystem (the
/// agent's `/tmp` tmpfs) — libkrun virtio-fs has an `O_CREAT`/`mkfifo` EPERM
/// bug on macOS, so the FIFO cannot live on the virtio-fs-shared workspace.
/// The Linux backend uses its own per-exec host path instead (the FIFO is
/// host-local there); this constant is only the VM-backend (agent) location.
pub const PROGRESS_GUEST_FIFO_PATH: &str = "/tmp/.ziee-progress.fifo";

/// Per-line cap on a progress FIFO `write()`. A single FIFO write below
/// `PIPE_BUF` (4096 on Linux) is atomic, so concurrent writers can't interleave
/// partial lines. Lines longer than this are dropped (not forwarded) rather
/// than risking a torn/interleaved frame. Shared host+agent so both readers
/// enforce the same boundary.
pub const PROGRESS_MAX_LINE_BYTES: usize = 4096;

/// Request to run one command in the sandbox. `argv` is the complete bwrap
/// argv produced by the host (already pointing at *guest* paths for the rootfs
/// mount + workspace); the agent execs `bwrap_path` with it verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecRequest {
    /// Must equal [`PROTOCOL_VERSION`] of the agent that receives it. Older
    /// peers send no field → serde defaults to `0` → mismatch → agent rejects.
    #[serde(default)]
    pub protocol_version: u32,
    /// Correlates the response frames with this request (the agent handles
    /// concurrent requests on separate connections, so this is mostly for logs).
    pub request_id: u64,
    /// Absolute path to `bwrap` inside the guest.
    pub bwrap_path: String,
    /// Full bwrap argv (the output of `build_bwrap_argv`, guest paths). When
    /// `seccomp_fd` is set, the argv already contains `--seccomp <that fd>`.
    pub argv: Vec<String>,
    /// Wall-clock budget; the agent SIGKILLs the bwrap process group on expiry.
    pub timeout_ms: u64,
    /// If set, the agent builds the shared seccomp BPF and pipes it to this fd
    /// in the bwrap child (the argv references it via `--seccomp <fd>`). `None`
    /// → no seccomp (e.g. a host that can't build the guest-arch filter). The
    /// macOS/Windows backends set this so the guest applies the same filter the
    /// Linux host does.
    #[serde(default)]
    pub seccomp_fd: Option<i32>,
    /// If set, the agent creates an in-guest cgroup v2 scope with these limits
    /// and attaches the bwrap process to it (defense-in-depth; the in-argv
    /// prlimit is the always-on backstop). `None` → rely on prlimit only. The
    /// host owns the policy so it stays single-source (and config-driven later).
    #[serde(default)]
    pub cgroup: Option<CgroupLimits>,
    /// If true, the agent creates the progress FIFO at
    /// [`PROGRESS_GUEST_FIFO_PATH`] before spawning bwrap, reads it line-by-line,
    /// and forwards each line as a [`Frame::ProcessProgress`] (using
    /// `request_id` as the handle). The host-built argv already binds that FIFO
    /// onto [`PROGRESS_SANDBOX_PATH`] + sets `$ZIEE_PROGRESS`. `false` (default)
    /// → no FIFO, no `$ZIEE_PROGRESS` (every chat/MCP exec; only the workflow
    /// sandbox step opts in). Older agents lacking the field see `false`.
    #[serde(default)]
    pub progress: bool,
    /// Guest-local artifact-collection dirs (macOS libkrun virtio-fs CREATE-EPERM
    /// workaround). For each entry, the agent provisions a **guest-local writable
    /// dir** (under the guest's `/tmp` tmpfs) at that path BEFORE running bwrap
    /// (so a sandboxed `open(O_CREAT)` succeeds — virtio-fs RW binds fail CREATE
    /// with EPERM on libkrun's broken credential switching; see
    /// `docker/sbx-releases#51`, `containers/podman#27679`). The
    /// bwrap argv binds these tmpfs dirs (not virtio-fs paths) as the RW mounts.
    /// AFTER the command exits, the agent walks each dir and streams every regular
    /// file back as an [`Frame::ArtifactFile`] (with `mount_index` = the entry's
    /// position in this vec) BEFORE [`Frame::Exit`]; the host writes them into the
    /// real host artifact dir. Empty for non-macOS backends and chat-side execs →
    /// byte-identical behavior.
    #[serde(default)]
    pub collect_artifacts: Vec<String>,
}

/// cgroup v2 resource limits the guest agent applies per exec. Values mirror
/// the Linux host's `cgroup::CgroupScope` defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CgroupLimits {
    /// `memory.max` (bytes).
    pub memory_max_bytes: u64,
    /// `memory.swap.max` (bytes; 0 disables swap).
    pub memory_swap_max_bytes: u64,
    /// `pids.max`.
    pub pids_max: u64,
    /// `cpu.max` ("<quota> <period>" in µs; "100000 100000" = 1 CPU).
    pub cpu_max: String,
}

impl CgroupLimits {
    /// The default policy — must match `cgroup::CgroupScope` on the Linux host
    /// (512 MiB / no swap / 256 PIDs / 1 CPU). Will become config-driven with
    /// Plan 1 §6 (runtime-configurable limits).
    pub fn default_policy() -> Self {
        Self {
            memory_max_bytes: 512 * 1024 * 1024,
            memory_swap_max_bytes: 0,
            pids_max: 256,
            cpu_max: "100000 100000".to_string(),
        }
    }
}

/// Terminal status of an `ExecRequest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitStatus {
    /// Process exit code (`-1` if killed by signal / timeout).
    pub code: i32,
    /// True if the agent killed it because `timeout_ms` elapsed.
    pub timed_out: bool,
}

/// Request to start one long-lived process inside the sandbox. Unlike
/// `ExecRequest`, the agent does **not** wait for completion before
/// returning; it acks with `Started` and then streams chunks until the
/// process exits or the host sends `KillProcess`. The host can issue
/// multiple `StartProcess` frames over the same connection and
/// multiplex them by `handle`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRequest {
    /// Must equal [`PROTOCOL_VERSION`] of the receiving agent.
    #[serde(default)]
    pub protocol_version: u32,
    /// Host-assigned identifier (must be unique among live processes on
    /// this connection). All subsequent per-process frames carry this.
    pub handle: u64,
    /// Absolute path to `bwrap` inside the guest.
    pub bwrap_path: String,
    /// Full bwrap argv (the output of `build_mcp_sandbox_argv`, guest
    /// paths). When `seccomp_fd` is set, the argv already contains
    /// `--seccomp <that fd>`.
    pub argv: Vec<String>,
    /// Same semantics as on `ExecRequest`.
    #[serde(default)]
    pub seccomp_fd: Option<i32>,
    /// Same semantics as on `ExecRequest`.
    #[serde(default)]
    pub cgroup: Option<CgroupLimits>,
}

/// Ack for `StartProcess`. `ok=false` with `err=Some(msg)` indicates the
/// agent could not spawn (bad path, exec failure, handle collision); the
/// host treats it as a terminal error for that handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartedAck {
    pub handle: u64,
    pub ok: bool,
    #[serde(default)]
    pub err: Option<String>,
}

/// Per-process termination. `status` matches the one-shot `ExitStatus`
/// shape (code + `timed_out`) so consumers can share logic; the agent
/// only sets `timed_out=true` if a per-process timeout was honored
/// (currently MCP processes have no per-frame timeout — the host owns
/// teardown via `KillProcess`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessExitStatus {
    pub handle: u64,
    pub status: ExitStatus,
}

/// Host → guest: kill the process registered under `handle` (SIGKILL).
/// The agent emits a `ProcessExit` once the wait task observes the death.
/// Silent no-op if `handle` is unknown — race with natural exit is fine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillProcessRequest {
    pub handle: u64,
}

/// A single protocol frame, either direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    // ---- one-shot exec (tags 1..=5) ----
    /// host → guest: run this command.
    Exec(ExecRequest),
    /// guest → host: a chunk of stdout (one-shot exec).
    Stdout(Vec<u8>),
    /// guest → host: a chunk of stderr (one-shot exec).
    Stderr(Vec<u8>),
    /// guest → host: the command finished.
    Exit(ExitStatus),
    /// host → guest: clean-shutdown request. The agent acknowledges by exiting
    /// its own process; any in-flight bwrap children then die via the argv's
    /// `--die-with-parent`. Used by the WSL2 backend so a `wsl --terminate`
    /// can be preceded by a clean in-distro stop (without it, the agent can
    /// outlive the Windows-side `wsl.exe` relay because there is no
    /// `PR_SET_PDEATHSIG` across the WSL boundary; [microsoft/WSL#1037]).
    Shutdown,

    // ---- long-lived multi-process (tags 6..=14) ----
    /// host → guest: spawn a new process, register it under `handle`.
    StartProcess(ProcessRequest),
    /// guest → host: ack a `StartProcess` (success or per-handle error).
    Started(StartedAck),
    /// host → guest: write `bytes` to the stdin of process `handle`.
    /// Empty `bytes` means "EOF — close stdin"; the host MUST send
    /// exactly one EOF per process to signal end of input.
    Stdin { handle: u64, bytes: Vec<u8> },
    /// guest → host: stdout chunk for process `handle`.
    ProcessStdout { handle: u64, bytes: Vec<u8> },
    /// guest → host: stderr chunk for process `handle`.
    ProcessStderr { handle: u64, bytes: Vec<u8> },
    /// guest → host: process `handle` exited.
    ProcessExit(ProcessExitStatus),
    /// host → guest: SIGKILL process `handle`.
    KillProcess(KillProcessRequest),
    /// either direction: liveness probe. The receiver responds with `Pong`.
    /// Used to keep long-lived MCP sessions warm + detect dead peers.
    Ping,
    /// either direction: response to `Ping`.
    Pong,
    /// guest → host: a live-progress line for process `handle`, read from the
    /// sandbox step's `$ZIEE_PROGRESS` FIFO. Same byte-frame shape as
    /// `ProcessStdout`, but the host parses it as `progress.v1`, not step
    /// output. (Tag 16 — additive; ArtifactFile already claimed tag 15 on main.)
    ProcessProgress { handle: u64, bytes: Vec<u8> },

    // ---- one-shot artifact write-back (tag 15) ----
    /// guest → host: one collected artifact file from a guest-local
    /// artifact-collection tmpfs dir (see `ExecRequest::collect_artifacts`).
    /// Emitted in the one-shot path AFTER the command exits and BEFORE
    /// `Exit`. `mount_index` is the dir's index in `collect_artifacts`;
    /// `rel_path` is the file's path relative to that dir (forward-slash,
    /// no leading slash); `data` is the file's full contents (capped by the
    /// guest at [`MAX_FRAME_PAYLOAD`] per file). The host writes `data` to
    /// `<host_artifact_dir for mount_index>/<rel_path>` on its OWN
    /// filesystem (which works — only the guest's virtio-fs CREATE is broken).
    /// Wire payload: `[u32 BE mount_index][u32 BE rel_path_len][rel_path utf8][data]`.
    ArtifactFile {
        mount_index: u32,
        rel_path: String,
        data: Vec<u8>,
    },
}

const TAG_EXEC: u8 = 1;
const TAG_STDOUT: u8 = 2;
const TAG_STDERR: u8 = 3;
const TAG_EXIT: u8 = 4;
const TAG_SHUTDOWN: u8 = 5;
const TAG_START_PROCESS: u8 = 6;
const TAG_STARTED: u8 = 7;
const TAG_STDIN: u8 = 8;
const TAG_PROCESS_STDOUT: u8 = 9;
const TAG_PROCESS_STDERR: u8 = 10;
const TAG_PROCESS_EXIT: u8 = 11;
const TAG_KILL_PROCESS: u8 = 12;
const TAG_PING: u8 = 13;
const TAG_PONG: u8 = 14;
const TAG_ARTIFACT_FILE: u8 = 15;
const TAG_PROGRESS: u8 = 16;

/// Length of the `u64 BE` handle prefix on `Stdin`/`ProcessStdout`/
/// `ProcessStderr` payloads.
const HANDLE_BYTES: usize = 8;

/// Header bytes (tag + u32 length) before each payload.
const HEADER_LEN: usize = 1 + 4;

/// Hard cap on a single frame's payload so a corrupt/hostile length prefix
/// can't make a peer allocate unbounded memory. 8 MiB comfortably exceeds any
/// realistic output chunk (the host re-caps total output separately).
pub const MAX_FRAME_PAYLOAD: usize = 8 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolError {
    UnknownTag(u8),
    FrameTooLarge(usize),
    BadJson,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::UnknownTag(t) => write!(f, "unknown frame tag {t}"),
            ProtocolError::FrameTooLarge(n) => write!(f, "frame payload too large ({n} bytes)"),
            ProtocolError::BadJson => write!(f, "malformed JSON frame payload"),
        }
    }
}
impl std::error::Error for ProtocolError {}

/// Encode a frame to its wire bytes.
pub fn encode(frame: &Frame) -> Vec<u8> {
    let (tag, payload): (u8, Vec<u8>) = match frame {
        Frame::Exec(req) => (TAG_EXEC, serde_json::to_vec(req).expect("ExecRequest serializes")),
        Frame::Stdout(b) => (TAG_STDOUT, b.clone()),
        Frame::Stderr(b) => (TAG_STDERR, b.clone()),
        Frame::Exit(s) => (TAG_EXIT, serde_json::to_vec(s).expect("ExitStatus serializes")),
        Frame::Shutdown => (TAG_SHUTDOWN, Vec::new()),
        Frame::StartProcess(req) => (
            TAG_START_PROCESS,
            serde_json::to_vec(req).expect("ProcessRequest serializes"),
        ),
        Frame::Started(ack) => (
            TAG_STARTED,
            serde_json::to_vec(ack).expect("StartedAck serializes"),
        ),
        Frame::Stdin { handle, bytes } => (TAG_STDIN, encode_handle_bytes(*handle, bytes)),
        Frame::ProcessStdout { handle, bytes } => {
            (TAG_PROCESS_STDOUT, encode_handle_bytes(*handle, bytes))
        }
        Frame::ProcessStderr { handle, bytes } => {
            (TAG_PROCESS_STDERR, encode_handle_bytes(*handle, bytes))
        }
        Frame::ProcessExit(exit) => (
            TAG_PROCESS_EXIT,
            serde_json::to_vec(exit).expect("ProcessExitStatus serializes"),
        ),
        Frame::KillProcess(req) => (
            TAG_KILL_PROCESS,
            serde_json::to_vec(req).expect("KillProcessRequest serializes"),
        ),
        Frame::Ping => (TAG_PING, Vec::new()),
        Frame::Pong => (TAG_PONG, Vec::new()),
        Frame::ProcessProgress { handle, bytes } => {
            (TAG_PROGRESS, encode_handle_bytes(*handle, bytes))
        }
        Frame::ArtifactFile {
            mount_index,
            rel_path,
            data,
        } => (
            TAG_ARTIFACT_FILE,
            encode_artifact_file(*mount_index, rel_path, data),
        ),
    };
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.push(tag);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    out
}

/// Pack `[u64 BE handle][raw bytes…]` for the long-lived per-process
/// byte frames (`Stdin`, `ProcessStdout`, `ProcessStderr`).
fn encode_handle_bytes(handle: u64, bytes: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(HANDLE_BYTES + bytes.len());
    payload.extend_from_slice(&handle.to_be_bytes());
    payload.extend_from_slice(bytes);
    payload
}

/// Pack `[u32 BE mount_index][u32 BE rel_path_len][rel_path utf8][data]`
/// for the `ArtifactFile` frame.
fn encode_artifact_file(mount_index: u32, rel_path: &str, data: &[u8]) -> Vec<u8> {
    let rel = rel_path.as_bytes();
    let mut payload = Vec::with_capacity(8 + rel.len() + data.len());
    payload.extend_from_slice(&mount_index.to_be_bytes());
    payload.extend_from_slice(&(rel.len() as u32).to_be_bytes());
    payload.extend_from_slice(rel);
    payload.extend_from_slice(data);
    payload
}

/// Inverse of [`encode_artifact_file`]. Returns `BadJson` on a truncated /
/// malformed payload (re-using the existing payload-shape error).
fn decode_artifact_file(payload: &[u8]) -> Result<(u32, String, Vec<u8>), ProtocolError> {
    if payload.len() < 8 {
        return Err(ProtocolError::BadJson);
    }
    let mount_index = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let rel_len = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
    let rel_start: usize = 8;
    let rel_end = rel_start
        .checked_add(rel_len)
        .ok_or(ProtocolError::BadJson)?;
    if payload.len() < rel_end {
        return Err(ProtocolError::BadJson);
    }
    let rel_path =
        String::from_utf8(payload[rel_start..rel_end].to_vec()).map_err(|_| ProtocolError::BadJson)?;
    let data = payload[rel_end..].to_vec();
    Ok((mount_index, rel_path, data))
}

/// Inverse of [`encode_handle_bytes`]: split out the handle prefix and
/// return `(handle, rest)`. Returns `BadJson` (re-using the existing
/// payload-shape error) if the payload is shorter than 8 bytes.
fn decode_handle_bytes(payload: &[u8]) -> Result<(u64, Vec<u8>), ProtocolError> {
    if payload.len() < HANDLE_BYTES {
        return Err(ProtocolError::BadJson);
    }
    let mut handle_bytes = [0u8; HANDLE_BYTES];
    handle_bytes.copy_from_slice(&payload[..HANDLE_BYTES]);
    let handle = u64::from_be_bytes(handle_bytes);
    let rest = payload[HANDLE_BYTES..].to_vec();
    Ok((handle, rest))
}

/// Incremental frame decoder: feed it bytes as they arrive on the socket and
/// pull complete frames out. Holds a buffer of not-yet-consumed bytes.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Append freshly-read bytes.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Pop the next complete frame, if one is fully buffered. Returns
    /// `Ok(None)` when more bytes are needed.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, ProtocolError> {
        if self.buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let tag = self.buf[0];
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if len > MAX_FRAME_PAYLOAD {
            return Err(ProtocolError::FrameTooLarge(len));
        }
        if self.buf.len() < HEADER_LEN + len {
            return Ok(None); // payload not fully arrived yet
        }
        let payload = self.buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        // Drop the consumed header + payload.
        self.buf.drain(..HEADER_LEN + len);

        let frame = match tag {
            TAG_EXEC => Frame::Exec(
                serde_json::from_slice(&payload).map_err(|_| ProtocolError::BadJson)?,
            ),
            TAG_STDOUT => Frame::Stdout(payload),
            TAG_STDERR => Frame::Stderr(payload),
            TAG_EXIT => {
                Frame::Exit(serde_json::from_slice(&payload).map_err(|_| ProtocolError::BadJson)?)
            }
            TAG_SHUTDOWN => Frame::Shutdown,
            TAG_START_PROCESS => Frame::StartProcess(
                serde_json::from_slice(&payload).map_err(|_| ProtocolError::BadJson)?,
            ),
            TAG_STARTED => Frame::Started(
                serde_json::from_slice(&payload).map_err(|_| ProtocolError::BadJson)?,
            ),
            TAG_STDIN => {
                let (handle, bytes) = decode_handle_bytes(&payload)?;
                Frame::Stdin { handle, bytes }
            }
            TAG_PROCESS_STDOUT => {
                let (handle, bytes) = decode_handle_bytes(&payload)?;
                Frame::ProcessStdout { handle, bytes }
            }
            TAG_PROCESS_STDERR => {
                let (handle, bytes) = decode_handle_bytes(&payload)?;
                Frame::ProcessStderr { handle, bytes }
            }
            TAG_PROCESS_EXIT => Frame::ProcessExit(
                serde_json::from_slice(&payload).map_err(|_| ProtocolError::BadJson)?,
            ),
            TAG_KILL_PROCESS => Frame::KillProcess(
                serde_json::from_slice(&payload).map_err(|_| ProtocolError::BadJson)?,
            ),
            TAG_PING => Frame::Ping,
            TAG_PONG => Frame::Pong,
            TAG_PROGRESS => {
                let (handle, bytes) = decode_handle_bytes(&payload)?;
                Frame::ProcessProgress { handle, bytes }
            }
            TAG_ARTIFACT_FILE => {
                let (mount_index, rel_path, data) = decode_artifact_file(&payload)?;
                Frame::ArtifactFile {
                    mount_index,
                    rel_path,
                    data,
                }
            }
            other => return Err(ProtocolError::UnknownTag(other)),
        };
        Ok(Some(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_exec() -> Frame {
        Frame::Exec(ExecRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: 42,
            bwrap_path: "/usr/bin/bwrap".to_string(),
            argv: vec!["--clearenv".into(), "--".into(), "/bin/bash".into(), "-lc".into(), "echo hi".into()],
            timeout_ms: 600_000,
            seccomp_fd: Some(10),
            cgroup: Some(CgroupLimits::default_policy()),
            progress: false,
            collect_artifacts: vec!["/artifacts-out/0".into()],
        })
    }

    #[test]
    fn roundtrips_every_frame() {
        for frame in [
            sample_exec(),
            Frame::Stdout(b"hello world".to_vec()),
            Frame::Stderr(vec![0, 1, 2, 255, 254]),
            Frame::Exit(ExitStatus { code: 0, timed_out: false }),
            Frame::Exit(ExitStatus { code: -1, timed_out: true }),
            Frame::Shutdown,
            Frame::ArtifactFile {
                mount_index: 0,
                rel_path: "upper.txt".into(),
                data: b"ABC".to_vec(),
            },
            Frame::ArtifactFile {
                mount_index: 3,
                rel_path: "nested/dir/file.bin".into(),
                data: vec![0, 1, 2, 255, 254],
            },
            Frame::ArtifactFile {
                mount_index: 1,
                rel_path: "empty".into(),
                data: Vec::new(),
            },
        ] {
            let mut d = Decoder::new();
            d.feed(&encode(&frame));
            let decoded = d.next_frame().unwrap().expect("a full frame");
            assert_eq!(decoded, frame);
            assert!(d.next_frame().unwrap().is_none(), "no trailing frame");
        }
    }

    #[test]
    fn roundtrips_process_progress() {
        let frame = Frame::ProcessProgress {
            handle: 7,
            bytes: br#"{"type":"bar","fraction":0.42}"#.to_vec(),
        };
        let mut d = Decoder::new();
        d.feed(&encode(&frame));
        assert_eq!(d.next_frame().unwrap(), Some(frame));
        assert!(d.next_frame().unwrap().is_none());
    }

    #[test]
    fn decodes_multiple_frames_from_one_buffer() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&encode(&Frame::Stdout(b"a".to_vec())));
        bytes.extend_from_slice(&encode(&Frame::Stderr(b"b".to_vec())));
        bytes.extend_from_slice(&encode(&Frame::Exit(ExitStatus { code: 7, timed_out: false })));

        let mut d = Decoder::new();
        d.feed(&bytes);
        assert_eq!(d.next_frame().unwrap(), Some(Frame::Stdout(b"a".to_vec())));
        assert_eq!(d.next_frame().unwrap(), Some(Frame::Stderr(b"b".to_vec())));
        assert_eq!(d.next_frame().unwrap(), Some(Frame::Exit(ExitStatus { code: 7, timed_out: false })));
        assert_eq!(d.next_frame().unwrap(), None);
    }

    #[test]
    fn handles_partial_then_completed_frame() {
        let full = encode(&Frame::Stdout(b"chunky".to_vec()));
        let (head, tail) = full.split_at(3); // split mid-header/payload
        let mut d = Decoder::new();
        d.feed(head);
        assert_eq!(d.next_frame().unwrap(), None, "incomplete → None");
        d.feed(tail);
        assert_eq!(d.next_frame().unwrap(), Some(Frame::Stdout(b"chunky".to_vec())));
    }

    #[test]
    fn rejects_oversized_frame() {
        // Craft a header claiming a payload bigger than the cap.
        let mut bytes = vec![TAG_STDOUT];
        bytes.extend_from_slice(&((MAX_FRAME_PAYLOAD as u32) + 1).to_be_bytes());
        let mut d = Decoder::new();
        d.feed(&bytes);
        assert_eq!(d.next_frame(), Err(ProtocolError::FrameTooLarge(MAX_FRAME_PAYLOAD + 1)));
    }

    #[test]
    fn rejects_unknown_tag() {
        let mut bytes = vec![99u8];
        bytes.extend_from_slice(&0u32.to_be_bytes());
        let mut d = Decoder::new();
        d.feed(&bytes);
        assert_eq!(d.next_frame(), Err(ProtocolError::UnknownTag(99)));
    }

    fn sample_process_request() -> Frame {
        Frame::StartProcess(ProcessRequest {
            protocol_version: PROTOCOL_VERSION,
            handle: 7,
            bwrap_path: "/usr/bin/bwrap".into(),
            argv: vec!["--clearenv".into(), "--".into(), "/usr/bin/python3".into()],
            seccomp_fd: Some(11),
            cgroup: Some(CgroupLimits::default_policy()),
        })
    }

    #[test]
    fn roundtrips_every_long_lived_frame() {
        for frame in [
            sample_process_request(),
            Frame::Started(StartedAck { handle: 7, ok: true, err: None }),
            Frame::Started(StartedAck {
                handle: 8,
                ok: false,
                err: Some("exec failed: ENOENT".into()),
            }),
            Frame::Stdin { handle: 7, bytes: b"hello\n".to_vec() },
            Frame::Stdin { handle: 7, bytes: Vec::new() }, // EOF marker
            Frame::ProcessStdout { handle: 7, bytes: b"line\n".to_vec() },
            Frame::ProcessStderr { handle: 7, bytes: vec![0u8, 255u8] },
            Frame::ProcessExit(ProcessExitStatus {
                handle: 7,
                status: ExitStatus { code: 0, timed_out: false },
            }),
            Frame::KillProcess(KillProcessRequest { handle: 9 }),
            Frame::Ping,
            Frame::Pong,
        ] {
            let mut d = Decoder::new();
            d.feed(&encode(&frame));
            let decoded = d.next_frame().unwrap().expect("a full frame");
            assert_eq!(decoded, frame);
            assert!(d.next_frame().unwrap().is_none(), "no trailing frame");
        }
    }

    #[test]
    fn handle_prefix_is_eight_be_bytes() {
        // The handle MUST be 8 BE bytes followed by raw payload — verify
        // byte layout directly so changes don't silently break the wire.
        let frame = Frame::ProcessStdout { handle: 0x0102_0304_0506_0708, bytes: vec![0xAA, 0xBB] };
        let wire = encode(&frame);
        // tag(1) + len(4) + handle(8) + bytes(2) = 15
        assert_eq!(wire.len(), 1 + 4 + 8 + 2);
        assert_eq!(wire[0], TAG_PROCESS_STDOUT);
        assert_eq!(&wire[1..5], &10u32.to_be_bytes()); // payload length
        assert_eq!(&wire[5..13], &0x0102_0304_0506_0708u64.to_be_bytes());
        assert_eq!(&wire[13..], &[0xAAu8, 0xBB]);
    }

    #[test]
    fn artifact_file_byte_layout_is_index_len_path_data() {
        // [u32 mount_index][u32 rel_path_len][rel_path][data]
        let frame = Frame::ArtifactFile {
            mount_index: 0x0102_0304,
            rel_path: "a/b".into(),
            data: vec![0xAA, 0xBB],
        };
        let wire = encode(&frame);
        // tag(1) + len(4) + index(4) + pathlen(4) + path(3) + data(2) = 18
        assert_eq!(wire.len(), 1 + 4 + 4 + 4 + 3 + 2);
        assert_eq!(wire[0], TAG_ARTIFACT_FILE);
        assert_eq!(&wire[1..5], &((4 + 4 + 3 + 2) as u32).to_be_bytes()); // payload len = 13
        assert_eq!(&wire[5..9], &0x0102_0304u32.to_be_bytes()); // mount_index
        assert_eq!(&wire[9..13], &3u32.to_be_bytes()); // rel_path_len
        assert_eq!(&wire[13..16], b"a/b");
        assert_eq!(&wire[16..], &[0xAAu8, 0xBB]);
    }

    #[test]
    fn rejects_artifact_file_shorter_than_header() {
        // A TAG_ARTIFACT_FILE frame with a 3-byte payload (< the 8-byte
        // index+len header) → decoder must reject, not panic.
        let payload = vec![0u8, 1, 2];
        let mut bytes = vec![TAG_ARTIFACT_FILE];
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&payload);
        let mut d = Decoder::new();
        d.feed(&bytes);
        assert_eq!(d.next_frame(), Err(ProtocolError::BadJson));
    }

    #[test]
    fn rejects_artifact_file_truncated_rel_path() {
        // rel_path_len claims 100 bytes but the payload only has 2 after
        // the header → reject.
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u32.to_be_bytes()); // mount_index
        payload.extend_from_slice(&100u32.to_be_bytes()); // rel_path_len (lie)
        payload.extend_from_slice(b"ab"); // only 2 bytes
        let mut bytes = vec![TAG_ARTIFACT_FILE];
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&payload);
        let mut d = Decoder::new();
        d.feed(&bytes);
        assert_eq!(d.next_frame(), Err(ProtocolError::BadJson));
    }

    #[test]
    fn rejects_handle_frame_shorter_than_eight_bytes() {
        // Hand-craft a TAG_STDIN frame with a 3-byte payload (less than the
        // 8-byte handle prefix) → decoder must reject, not panic.
        let payload = vec![0u8, 1, 2];
        let mut bytes = vec![TAG_STDIN];
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&payload);
        let mut d = Decoder::new();
        d.feed(&bytes);
        assert_eq!(d.next_frame(), Err(ProtocolError::BadJson));
    }

    #[test]
    fn one_shot_and_long_lived_tags_do_not_collide() {
        // Tags 1..=5 are one-shot; 6..=14 are long-lived. Verify all 14
        // are distinct (catches a copy-paste bug if a new tag duplicates
        // an old one). The decoder uses match-on-u8 so a collision would
        // silently miscategorize frames.
        let tags = [
            TAG_EXEC, TAG_STDOUT, TAG_STDERR, TAG_EXIT, TAG_SHUTDOWN,
            TAG_START_PROCESS, TAG_STARTED, TAG_STDIN, TAG_PROCESS_STDOUT,
            TAG_PROCESS_STDERR, TAG_PROCESS_EXIT, TAG_KILL_PROCESS,
            TAG_PING, TAG_PONG, TAG_ARTIFACT_FILE,
        ];
        let mut seen = std::collections::HashSet::new();
        for t in tags {
            assert!(seen.insert(t), "tag {t} appears twice");
        }
    }

    #[test]
    fn interleaved_long_lived_frames_decode_independently() {
        // Two processes share one connection; their stdout chunks arrive
        // interleaved. The decoder must surface them in order with the
        // correct handles.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&encode(&Frame::ProcessStdout { handle: 1, bytes: b"A".to_vec() }));
        bytes.extend_from_slice(&encode(&Frame::ProcessStdout { handle: 2, bytes: b"B".to_vec() }));
        bytes.extend_from_slice(&encode(&Frame::ProcessStdout { handle: 1, bytes: b"AA".to_vec() }));
        bytes.extend_from_slice(&encode(&Frame::ProcessExit(ProcessExitStatus {
            handle: 2,
            status: ExitStatus { code: 0, timed_out: false },
        })));

        let mut d = Decoder::new();
        d.feed(&bytes);
        assert_eq!(
            d.next_frame().unwrap(),
            Some(Frame::ProcessStdout { handle: 1, bytes: b"A".to_vec() })
        );
        assert_eq!(
            d.next_frame().unwrap(),
            Some(Frame::ProcessStdout { handle: 2, bytes: b"B".to_vec() })
        );
        assert_eq!(
            d.next_frame().unwrap(),
            Some(Frame::ProcessStdout { handle: 1, bytes: b"AA".to_vec() })
        );
        assert_eq!(
            d.next_frame().unwrap(),
            Some(Frame::ProcessExit(ProcessExitStatus {
                handle: 2,
                status: ExitStatus { code: 0, timed_out: false }
            }))
        );
        assert_eq!(d.next_frame().unwrap(), None);
    }

    #[test]
    fn legacy_process_request_decodes_with_version_zero() {
        // A peer that predates the new ProcessRequest sends no
        // `protocol_version` field. serde defaults to 0; agent rejects.
        let legacy_json = serde_json::json!({
            "handle": 1,
            "bwrap_path": "/usr/bin/bwrap",
            "argv": [],
        });
        let payload = serde_json::to_vec(&legacy_json).unwrap();
        let mut bytes = vec![TAG_START_PROCESS];
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&payload);
        let mut d = Decoder::new();
        d.feed(&bytes);
        let frame = d.next_frame().unwrap().expect("legacy ProcessRequest decodes");
        match frame {
            Frame::StartProcess(req) => assert_eq!(req.protocol_version, 0),
            other => panic!("expected StartProcess, got {other:?}"),
        }
    }

    #[test]
    fn legacy_exec_request_decodes_with_version_zero() {
        // A peer that predates PROTOCOL_VERSION sends an ExecRequest with no
        // `protocol_version` field. With `#[serde(default)]` this deserializes
        // to 0 (≠ PROTOCOL_VERSION) so the agent can reject it loudly.
        let legacy_json = serde_json::json!({
            "request_id": 1,
            "bwrap_path": "/usr/bin/bwrap",
            "argv": [],
            "timeout_ms": 0,
        });
        let payload = serde_json::to_vec(&legacy_json).unwrap();
        let mut bytes = vec![TAG_EXEC];
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&payload);
        let mut d = Decoder::new();
        d.feed(&bytes);
        let frame = d.next_frame().unwrap().expect("legacy ExecRequest decodes");
        match frame {
            Frame::Exec(req) => assert_eq!(req.protocol_version, 0),
            _ => panic!("expected Exec"),
        }
    }
}
