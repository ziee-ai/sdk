//! Host side of the long-lived multi-process session protocol.
//!
//! Both VM backends (macOS libkrun, Windows WSL2) want to run sandboxed
//! MCP stdio servers — long-lived JSON-RPC subprocesses — over the in-VM
//! `ziee-sandbox-agent`. The protocol extension defined in
//! `sandbox-vm-protocol` (tags 6–14) lets one vsock connection multiplex
//! many such processes, identified by a host-assigned `u64` handle.
//!
//! Architecturally the session has:
//!   - a single **writer task** owning the stream's write half; every
//!     concurrent sender goes through an `mpsc::UnboundedSender<Frame>`
//!     so writes stay serialised.
//!   - a single **reader task** owning the read half; it decodes frames
//!     and demuxes by handle into per-handle channels held by the
//!     [`HandleSlot`] registry.
//!   - one [`HandleSlot`] per live process, holding the one-shot ack +
//!     exit channels and the per-process stdout `mpsc` the reader feeds.
//!   - one [`ProcessIo`] per live process, returned from
//!     [`LongLivedSession::spawn`]. It implements `AsyncRead +
//!     AsyncWrite` so it can be split into halves and passed to rmcp's
//!     `AsyncRwTransport::new_client(read, write)` directly. Dropping
//!     `ProcessIo` sends `KillProcess` on its way out — so dropping the
//!     rmcp service tears the sandboxed child down.
//!
//! The session itself stays open across MCP tool calls; one MCP server
//! → one session for its lifetime. Multiple servers on the same flavor's
//! warm VM share one VM but get distinct sessions (one vsock connection
//! each).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use sandbox_vm_protocol::{
    encode, Decoder, ExitStatus, Frame, KillProcessRequest, ProcessExitStatus, ProcessRequest,
    StartedAck, PROTOCOL_VERSION,
};
use tokio::io::{
    duplex, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use ziee_core::AppError;

/// Buffer between the caller's `ProcessIo` and the per-handle pump
/// tasks. 64 KiB matches the agent's `READ_CHUNK`, so a single guest
/// stdout chunk fits without back-pressure churn.
const DUPLEX_BUF_BYTES: usize = 64 * 1024;

/// Hard cap on how long [`LongLivedSession::spawn`] waits for the agent's
/// `Started` ack. A healthy bwrap-exec takes single-digit milliseconds;
/// 10 s is "the agent is wedged" territory.
const STARTED_ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-handle state held by the reader task's registry. The
/// `started_tx` / `exit_tx` halves are consumed (taken) when the
/// corresponding frame arrives; `stdout_tx` is the channel the reader
/// forwards `ProcessStdout` chunks onto.
#[allow(dead_code)]
struct HandleSlot {
    started_tx: Option<oneshot::Sender<StartedAck>>,
    stdout_tx: mpsc::UnboundedSender<Vec<u8>>,
    exit_tx: Option<oneshot::Sender<ExitStatus>>,
}

/// A long-lived multiplexed session over one agent connection. Hold it
/// for the lifetime of the MCP servers using its underlying VM
/// connection; drop it to tear down.
pub struct LongLivedSession {
    writer_tx: mpsc::UnboundedSender<Frame>,
    next_handle: Arc<AtomicU64>,
    registry: Arc<Mutex<HashMap<u64, HandleSlot>>>,
    reader_task: Option<JoinHandle<()>>,
    writer_task: Option<JoinHandle<()>>,
    /// Backend-supplied opaque guard, dropped when the session is dropped.
    /// Used by the VM backends to decrement a per-flavor inflight counter so
    /// the VM-evict reaper waits for live MCP sessions to drain. Linux and
    /// the unsupported backend leave it `None`.
    _vm_keepalive: Option<Box<dyn std::any::Any + Send + Sync>>,
}

impl LongLivedSession {
    /// Number of currently live processes on this session. Intended for the
    /// VM idle-evict gate (a session with live processes must keep the VM
    /// warm); exercised by tests today.
    // TODO: consult this in the VM idle-evict decision path so a session with
    // live processes is never torn down. Test-only consumer for now.
    #[allow(dead_code)] // consumed only by #[cfg(test)]; dead in non-test builds
    pub fn live_process_count(&self) -> usize {
        self.registry.lock().unwrap().len()
    }

    /// Spawn one process in the sandbox. Builds a `ProcessRequest` from
    /// the caller's bwrap argv, sends `StartProcess`, awaits `Started`,
    /// and returns a `ProcessIo` whose `AsyncRead` is the child's
    /// stdout and `AsyncWrite` is its stdin.
    pub async fn spawn(
        &self,
        bwrap_path: String,
        argv: Vec<String>,
        seccomp_fd: Option<i32>,
        cgroup: Option<sandbox_vm_protocol::CgroupLimits>,
    ) -> Result<ProcessIo, AppError> {
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);

        let (caller_half, internal_half) = duplex(DUPLEX_BUF_BYTES);
        let (internal_rd, internal_wr) = tokio::io::split(internal_half);

        // Per-handle stdout pipeline: reader → mpsc → stdout-writer task → internal_wr → caller_half.read
        let (stdout_tx, stdout_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(run_stdout_writer(stdout_rx, internal_wr));

        // Per-handle stdin pump: caller_half.write → internal_rd → Frame::Stdin{handle, bytes} via writer_tx
        let stdin_writer_tx = self.writer_tx.clone();
        tokio::spawn(run_stdin_pump(internal_rd, stdin_writer_tx, handle));

        let (started_tx, started_rx) = oneshot::channel::<StartedAck>();
        let (exit_tx, exit_rx) = oneshot::channel::<ExitStatus>();

        // Register BEFORE sending StartProcess so the reader can't race
        // a Started frame against our slot insert.
        {
            let mut reg = self.registry.lock().unwrap();
            reg.insert(
                handle,
                HandleSlot {
                    started_tx: Some(started_tx),
                    stdout_tx,
                    exit_tx: Some(exit_tx),
                },
            );
        }

        let req = ProcessRequest {
            protocol_version: PROTOCOL_VERSION,
            handle,
            bwrap_path,
            argv,
            seccomp_fd,
            cgroup,
        };
        self.writer_tx
            .send(Frame::StartProcess(req))
            .map_err(|_| AppError::internal_error("vm long-lived session writer is closed"))?;

        // Wait for ack with a hard timeout — a wedged agent must not hang us.
        let ack = match tokio::time::timeout(STARTED_ACK_TIMEOUT, started_rx).await {
            Ok(Ok(ack)) => ack,
            Ok(Err(_)) => {
                self.registry.lock().unwrap().remove(&handle);
                return Err(AppError::internal_error(
                    "vm long-lived session ack channel dropped — reader task exited",
                ));
            }
            Err(_) => {
                self.registry.lock().unwrap().remove(&handle);
                return Err(AppError::internal_error(format!(
                    "vm long-lived spawn ack timed out after {}s",
                    STARTED_ACK_TIMEOUT.as_secs()
                )));
            }
        };
        if !ack.ok {
            self.registry.lock().unwrap().remove(&handle);
            return Err(AppError::internal_error(format!(
                "vm long-lived spawn rejected by agent: {}",
                ack.err.as_deref().unwrap_or("unspecified")
            )));
        }

        Ok(ProcessIo {
            caller_half,
            handle,
            writer_tx: self.writer_tx.clone(),
            exit_rx: Some(exit_rx),
            killed: false,
        })
    }
}

impl Drop for LongLivedSession {
    fn drop(&mut self) {
        // Closing the writer_tx isn't enough — there may still be live
        // ProcessIo holding clones. We can only signal "no more new
        // spawns from THIS handle" by dropping our writer_tx clone. The
        // tasks tear down once every clone goes away (ProcessIo Drop
        // sends KillProcess + drops its writer_tx).
        if let Some(h) = self.reader_task.take() {
            h.abort();
        }
        if let Some(h) = self.writer_task.take() {
            h.abort();
        }
    }
}

/// Open a long-lived session over an already-connected agent stream.
///
/// Spawns the reader + writer tasks and returns the session immediately.
/// Subsequent [`LongLivedSession::spawn`] calls send `StartProcess` frames
/// over the same connection.
#[allow(dead_code)]
pub fn open_long_lived<S>(stream: S) -> LongLivedSession
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    open_long_lived_with_guard(stream, None)
}

/// Same as [`open_long_lived`] but lets the caller wedge an opaque
/// drop-guard into the session. The guard is held until the session is
/// dropped — used by the VM backends to hold a per-flavor inflight
/// guard so the VM-evict reaper doesn't tear the VM down underneath a
/// live MCP session.
#[allow(dead_code)]
pub fn open_long_lived_with_guard<S>(
    stream: S,
    vm_keepalive: Option<Box<dyn std::any::Any + Send + Sync>>,
) -> LongLivedSession
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (rd, wr) = tokio::io::split(stream);

    let (writer_tx, writer_rx) = mpsc::unbounded_channel::<Frame>();
    let registry: Arc<Mutex<HashMap<u64, HandleSlot>>> = Arc::new(Mutex::new(HashMap::new()));

    let writer_task = tokio::spawn(run_writer(writer_rx, wr));
    let reader_task = tokio::spawn(run_reader(rd, registry.clone()));

    LongLivedSession {
        writer_tx,
        next_handle: Arc::new(AtomicU64::new(1)),
        registry,
        reader_task: Some(reader_task),
        writer_task: Some(writer_task),
        _vm_keepalive: vm_keepalive,
    }
}

/// Drains the writer channel into the network. Exits when every
/// `writer_tx` clone is dropped.
#[allow(dead_code)]
async fn run_writer<W: AsyncWrite + Unpin>(mut rx: mpsc::UnboundedReceiver<Frame>, mut wr: W) {
    while let Some(frame) = rx.recv().await {
        if wr.write_all(&encode(&frame)).await.is_err() {
            break;
        }
    }
    let _ = wr.flush().await;
}

/// Reads frames off the network and demuxes into the registry. Exits on
/// EOF / read error or when the writer side has closed.
#[allow(dead_code)]
async fn run_reader<R: AsyncRead + Unpin>(mut rd: R, registry: Arc<Mutex<HashMap<u64, HandleSlot>>>) {
    let mut decoder = Decoder::new();
    let mut buf = vec![0u8; DUPLEX_BUF_BYTES];
    loop {
        let n = match rd.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("vm long-lived: reader io error: {e}");
                break;
            }
        };
        decoder.feed(&buf[..n]);
        loop {
            match decoder.next_frame() {
                Ok(Some(Frame::Started(ack))) => {
                    let handle = ack.handle;
                    let started_tx_opt = registry.lock().unwrap().get_mut(&handle).and_then(|slot| slot.started_tx.take());
                    if let Some(tx) = started_tx_opt {
                        let _ = tx.send(ack);
                    } else {
                        tracing::warn!("vm long-lived: Started for unknown/already-acked handle {handle}");
                    }
                }
                Ok(Some(Frame::ProcessStdout { handle, bytes })) => {
                    let stdout_tx_opt = registry.lock().unwrap().get(&handle).map(|s| s.stdout_tx.clone());
                    if let Some(tx) = stdout_tx_opt {
                        let _ = tx.send(bytes);
                    }
                }
                Ok(Some(Frame::ProcessStderr { handle, bytes })) => {
                    // MCP children write JSON-RPC on stdout; stderr is
                    // informational (logs / panics). Surface to tracing
                    // at debug so it's available when troubleshooting
                    // without dominating normal logs.
                    let s = String::from_utf8_lossy(&bytes);
                    tracing::debug!(handle, %s, "vm long-lived: child stderr");
                }
                Ok(Some(Frame::ProcessExit(ProcessExitStatus { handle, status }))) => {
                    // Take BOTH exit_tx and the whole slot so the stdout
                    // mpsc closes, the per-handle stdout-writer task
                    // drains + exits, internal_wr drops, and the
                    // caller_half observes EOF on read.
                    let slot = registry.lock().unwrap().remove(&handle);
                    if let Some(mut slot) = slot {
                        if let Some(tx) = slot.exit_tx.take() {
                            let _ = tx.send(status);
                        }
                    }
                }
                Ok(Some(Frame::Pong)) => { /* heartbeat ack — drop */ }
                Ok(Some(other)) => {
                    tracing::warn!("vm long-lived: ignoring unexpected frame: {other:?}");
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("vm long-lived: protocol decode error: {e}");
                    return;
                }
            }
        }
    }

    // Reader is exiting; close every live slot so any awaiting caller
    // gets a clean "agent gone" signal instead of hanging forever.
    let drained: Vec<_> = registry.lock().unwrap().drain().collect();
    for (_handle, mut slot) in drained {
        if let Some(tx) = slot.started_tx.take() {
            // Synthesise a refusal so spawn() returns an error.
            let _ = tx.send(StartedAck {
                handle: 0,
                ok: false,
                err: Some("agent connection closed before Started ack".into()),
            });
        }
        if let Some(tx) = slot.exit_tx.take() {
            let _ = tx.send(ExitStatus { code: -1, timed_out: false });
        }
        drop(slot.stdout_tx); // EOF the caller_half
    }
}

/// Drains a per-handle stdout channel into the duplex internal write
/// half. When the channel closes (the handle was removed from the
/// registry on ProcessExit, or the reader exited), `wr` drops here →
/// caller_half sees EOF.
async fn run_stdout_writer(mut rx: mpsc::UnboundedReceiver<Vec<u8>>, mut wr: tokio::io::WriteHalf<DuplexStream>) {
    while let Some(chunk) = rx.recv().await {
        if wr.write_all(&chunk).await.is_err() {
            break;
        }
    }
    let _ = wr.shutdown().await;
}

/// Reads the caller's writes (the child's stdin) and forwards as
/// `Frame::Stdin{handle, bytes}` to the session writer. EOF on the
/// duplex read half is forwarded as an empty `bytes` frame so the
/// agent can close the child's stdin (e.g. `cat` blocking on EOF).
async fn run_stdin_pump(mut rd: tokio::io::ReadHalf<DuplexStream>, writer_tx: mpsc::UnboundedSender<Frame>, handle: u64) {
    let mut buf = vec![0u8; DUPLEX_BUF_BYTES];
    loop {
        match rd.read(&mut buf).await {
            Ok(0) => {
                // EOF on caller stdin → tell agent to close child stdin.
                let _ = writer_tx.send(Frame::Stdin { handle, bytes: Vec::new() });
                break;
            }
            Ok(n) => {
                let bytes = buf[..n].to_vec();
                if writer_tx.send(Frame::Stdin { handle, bytes }).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// Caller-facing handle to one long-lived process. Implements
/// `AsyncRead` (child stdout) + `AsyncWrite` (child stdin) so it slots
/// into rmcp's `AsyncRwTransport::new_client(read, write)` via
/// `tokio::io::split(process_io)`. Drop sends `KillProcess` so leaving
/// scope tears the sandboxed child down.
pub struct ProcessIo {
    caller_half: DuplexStream,
    handle: u64,
    writer_tx: mpsc::UnboundedSender<Frame>,
    /// `take()` consumed when `wait_exit` is called; afterwards it's None.
    #[allow(dead_code)]
    exit_rx: Option<oneshot::Receiver<ExitStatus>>,
    /// Once true, Drop does NOT send KillProcess again (the process
    /// already exited or the caller already killed it).
    killed: bool,
}

impl std::fmt::Debug for ProcessIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessIo")
            .field("handle", &self.handle)
            .field("killed", &self.killed)
            .finish()
    }
}

impl ProcessIo {
    /// The handle this process was registered under. Mainly for logs / tests.
    #[allow(dead_code)] // consumed only by #[cfg(test)]; dead in non-test builds
    pub fn handle(&self) -> u64 {
        self.handle
    }

    /// Take the exit-receiver. Can only be called once.
    #[allow(dead_code)]
    pub fn take_exit_rx(&mut self) -> Option<oneshot::Receiver<ExitStatus>> {
        self.exit_rx.take()
    }

    /// Send an explicit KillProcess (idempotent) and mark Drop as no-op.
    /// Useful in tests where you want to assert kill semantics without
    /// the Drop side-effect.
    #[allow(dead_code)] // consumed only by #[cfg(test)]; dead in non-test builds
    pub fn kill(&mut self) {
        if !self.killed {
            self.killed = true;
            let _ = self
                .writer_tx
                .send(Frame::KillProcess(KillProcessRequest { handle: self.handle }));
        }
    }
}

impl AsyncRead for ProcessIo {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.caller_half).poll_read(cx, buf)
    }
}

impl AsyncWrite for ProcessIo {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.caller_half).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.caller_half).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.caller_half).poll_shutdown(cx)
    }
}

impl Drop for ProcessIo {
    fn drop(&mut self) {
        if !self.killed {
            let _ = self
                .writer_tx
                .send(Frame::KillProcess(KillProcessRequest { handle: self.handle }));
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tier-1 tests for the host long-lived session. Uses
    //! `tokio::io::duplex` to stand in for the vsock connection — one
    //! end becomes the "agent" we hand-simulate, the other end becomes
    //! the session's stream. No bwrap, no rootfs.

    use super::*;
    use sandbox_vm_protocol::{Decoder, Frame};
    use tokio::io::AsyncWriteExt;

    /// Spin up a session backed by a duplex pair; return the session
    /// plus the "agent" side (which the test drives manually).
    fn pair() -> (LongLivedSession, DuplexStream) {
        let (host_side, agent_side) = duplex(DUPLEX_BUF_BYTES);
        (open_long_lived(host_side), agent_side)
    }

    /// Pretend to be the agent: read every frame the host sent and
    /// optionally respond. The closure decides what to echo back
    /// based on each inbound frame.
    async fn drive_agent(
        stream: DuplexStream,
        mut on_frame: impl FnMut(Frame, &mut Vec<Frame>) -> bool + Send + 'static,
    ) {
        let (mut rd, mut wr) = tokio::io::split(stream);
        let task = tokio::spawn(async move {
            let mut decoder = Decoder::new();
            let mut buf = vec![0u8; 64 * 1024];
            'outer: loop {
                let n = match rd.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                decoder.feed(&buf[..n]);
                while let Ok(Some(frame)) = decoder.next_frame() {
                    let mut outbound = Vec::new();
                    let done = on_frame(frame, &mut outbound);
                    for f in outbound {
                        if wr.write_all(&encode(&f)).await.is_err() {
                            break 'outer;
                        }
                    }
                    if done {
                        break 'outer;
                    }
                }
            }
        });
        let _ = task.await;
    }

    #[tokio::test]
    async fn spawn_returns_err_when_agent_rejects() {
        let (session, agent) = pair();
        // Drive the agent to immediately reject every StartProcess.
        let agent_task = tokio::spawn(drive_agent(agent, |frame, out| {
            if let Frame::StartProcess(req) = frame {
                out.push(Frame::Started(StartedAck {
                    handle: req.handle,
                    ok: false,
                    err: Some("intentional test reject".into()),
                }));
            }
            false
        }));

        let res = session.spawn("/usr/bin/bwrap".into(), vec![], None, None).await;
        assert!(res.is_err(), "expected spawn to error on agent reject");
        let msg = format!("{:?}", res.unwrap_err());
        assert!(msg.contains("intentional test reject"), "msg = {msg}");
        drop(session);
        let _ = agent_task.await;
    }

    #[tokio::test]
    async fn spawn_returns_processio_then_echoes_stdout() {
        let (session, agent) = pair();
        // Agent acks then echoes anything written to stdin back as stdout.
        let agent_task = tokio::spawn(drive_agent(agent, |frame, out| match frame {
            Frame::StartProcess(req) => {
                out.push(Frame::Started(StartedAck { handle: req.handle, ok: true, err: None }));
                false
            }
            Frame::Stdin { handle, bytes } => {
                if bytes.is_empty() {
                    // EOF — emit ProcessExit and end the agent loop.
                    out.push(Frame::ProcessExit(ProcessExitStatus {
                        handle,
                        status: ExitStatus { code: 0, timed_out: false },
                    }));
                    true
                } else {
                    out.push(Frame::ProcessStdout { handle, bytes });
                    false
                }
            }
            _ => false,
        }));

        let mut io = session
            .spawn("/usr/bin/bwrap".into(), vec![], None, None)
            .await
            .expect("spawn ok");

        io.write_all(b"hello").await.unwrap();
        io.shutdown().await.unwrap();

        // Read up to EOF and verify we got "hello" back.
        let mut got = Vec::new();
        io.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"hello");

        // Don't double-kill in Drop — process already exited.
        io.kill();
        drop(session);
        let _ = agent_task.await;
    }

    #[tokio::test]
    async fn drop_sends_kill_process() {
        let (session, agent) = pair();
        let (kill_seen_tx, kill_seen_rx) = oneshot::channel::<u64>();
        let mut kill_seen_tx = Some(kill_seen_tx);
        let agent_task = tokio::spawn(drive_agent(agent, move |frame, out| match frame {
            Frame::StartProcess(req) => {
                out.push(Frame::Started(StartedAck { handle: req.handle, ok: true, err: None }));
                false
            }
            Frame::KillProcess(KillProcessRequest { handle }) => {
                if let Some(tx) = kill_seen_tx.take() {
                    let _ = tx.send(handle);
                }
                true
            }
            _ => false,
        }));

        let io = session
            .spawn("/usr/bin/bwrap".into(), vec![], None, None)
            .await
            .expect("spawn ok");
        let h = io.handle();
        drop(io); // should send KillProcess { handle: h }

        let seen = tokio::time::timeout(Duration::from_secs(2), kill_seen_rx)
            .await
            .expect("kill arrived")
            .expect("oneshot");
        assert_eq!(seen, h, "agent saw KillProcess for the right handle");
        drop(session);
        let _ = agent_task.await;
    }

    #[tokio::test]
    async fn spawn_returns_err_when_agent_disconnects_before_ack() {
        // Test the "agent connection closed" path: agent reads the
        // StartProcess then drops its stream. The reader task exits,
        // walks the registry, and synthesises a refusing Started ack
        // so spawn() returns Err instead of hanging forever.
        let (session, agent) = pair();
        let agent_task = tokio::spawn(async move {
            let mut agent = agent;
            let mut buf = vec![0u8; 4096];
            // Read one batch (the StartProcess frame should be in here)
            // then drop the stream.
            let _ = agent.read(&mut buf).await;
            drop(agent);
        });

        let res = session.spawn("/usr/bin/bwrap".into(), vec![], None, None).await;
        assert!(res.is_err(), "spawn should err when agent disconnects pre-ack");
        let msg = format!("{:?}", res.err().unwrap());
        assert!(
            msg.contains("agent connection closed") || msg.contains("ack channel dropped"),
            "unexpected err msg: {msg}"
        );
        drop(session);
        let _ = agent_task.await;
    }

    #[tokio::test]
    async fn live_process_count_tracks_spawn_and_exit() {
        let (session, agent) = pair();
        let agent_task = tokio::spawn(drive_agent(agent, move |frame, out| match frame {
            Frame::StartProcess(req) => {
                out.push(Frame::Started(StartedAck { handle: req.handle, ok: true, err: None }));
                false
            }
            Frame::Stdin { handle, bytes } if bytes.is_empty() => {
                out.push(Frame::ProcessExit(ProcessExitStatus {
                    handle,
                    status: ExitStatus { code: 0, timed_out: false },
                }));
                false
            }
            _ => false,
        }));

        assert_eq!(session.live_process_count(), 0);
        let mut io1 = session.spawn("/usr/bin/bwrap".into(), vec![], None, None).await.unwrap();
        let mut io2 = session.spawn("/usr/bin/bwrap".into(), vec![], None, None).await.unwrap();
        assert_eq!(session.live_process_count(), 2);

        // EOF io1 → ProcessExit → registry removal
        io1.shutdown().await.unwrap();
        // ProcessExit is async; give the reader a moment.
        for _ in 0..50 {
            if session.live_process_count() == 1 { break }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(session.live_process_count(), 1);

        io2.shutdown().await.unwrap();
        for _ in 0..50 {
            if session.live_process_count() == 0 { break }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(session.live_process_count(), 0);

        // Mark both killed so Drop doesn't try to KillProcess (they exited).
        io1.kill();
        io2.kill();
        drop(session);
        let _ = agent_task.await;
    }
}
