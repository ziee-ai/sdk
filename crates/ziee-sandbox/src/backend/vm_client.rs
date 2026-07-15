// When compiled ONLY for the Linux `test` build (backend/mod.rs adds `test` to
// this module's cfg so the `write_artifact` path-confinement test runs on
// Linux), the transport fns here have no caller. Silence the resulting
// dead_code warnings — this attribute is inert on the real macOS/Windows
// builds where those fns ARE used.
#![cfg_attr(
    all(test, not(any(target_os = "macos", target_os = "windows"))),
    allow(dead_code)
)]

//! Host-side client for the in-guest `ziee-sandbox-agent`, shared by the macOS
//! (libkrun → vsock-bridged unix socket) and Windows (WSL2 → localhost TCP)
//! backends. Sends one `ExecRequest` over an already-connected stream and
//! collects the streamed `Stdout`/`Stderr`/`Exit` frames into a
//! `SandboxRunResult`, applying the same 1 MiB output cap as the Linux backend
//! and a host-side read-timeout backstop (a wedged agent can't hang the turn).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sandbox_vm_protocol::{encode, Decoder, ExecRequest, Frame};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use ziee_core::AppError;
use crate::sandbox::{SandboxRunResult, OUTPUT_CAP_BYTES};

/// Run one command on a connected control stream. Transport-agnostic: the
/// caller (mac unix socket / WSL2 TCP) connects and hands the stream in.
///
/// The simple wrapper: NO live-progress sink and NO artifact write-back (the
/// chat/MCP + WSL2 + test raw-exec paths). The workflow sandbox step instead
/// calls [`run_on_stream_collecting`] directly with a `progress_tx` sink and
/// the RW-mount → host-dir mapping.
pub async fn run_on_stream<S>(
    stream: S,
    req: ExecRequest,
    timeout_secs: u64,
) -> Result<SandboxRunResult, AppError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    run_on_stream_collecting(stream, req, timeout_secs, None, &[]).await
}

/// Like [`run_on_stream`] but, in addition to streaming stdout/stderr, receives
/// any `ArtifactFile` frames the guest emits (the macOS libkrun virtio-fs
/// CREATE-EPERM workaround) and writes each into the matching host artifact dir.
///
/// `artifact_host_dirs[i]` is the real host directory backing
/// `ExecRequest::collect_artifacts[i]` (the guest tmpfs the agent walks). A
/// frame's `rel_path` is joined under that dir; the host writes its OWN fs (no
/// virtio-fs involved), so the file actually lands and `collect_step_artifacts`
/// then finds it.
pub async fn run_on_stream_collecting<S>(
    mut stream: S,
    req: ExecRequest,
    timeout_secs: u64,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    artifact_host_dirs: &[PathBuf],
) -> Result<SandboxRunResult, AppError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let started = Instant::now();
    stream
        .write_all(&encode(&Frame::Exec(req)))
        .await
        .map_err(|e| AppError::internal_error(format!("send exec to guest: {e}")))?;

    let mut decoder = Decoder::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut stdout_truncated = false;
    let mut stderr_truncated = false;
    let mut exit_code = -1;
    let mut timed_out = false;

    // The agent enforces the per-exec timeout in-guest and should always send
    // Exit, but if it wedges, bound the host wait at the exec budget + grace.
    let read_budget = Duration::from_secs(timeout_secs + 30);
    loop {
        let n = match tokio::time::timeout(read_budget, stream.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(AppError::internal_error(format!("read guest stream: {e}"))),
            Err(_) => {
                timed_out = true;
                break;
            }
        };
        if n == 0 {
            break; // stream closed
        }
        decoder.feed(&buf[..n]);
        let mut done = false;
        loop {
            match decoder.next_frame() {
                Ok(Some(Frame::Stdout(b))) => append_capped(&mut stdout, &b, &mut stdout_truncated),
                Ok(Some(Frame::Stderr(b))) => append_capped(&mut stderr, &b, &mut stderr_truncated),
                Ok(Some(Frame::Exit(s))) => {
                    exit_code = s.code;
                    timed_out = s.timed_out;
                    done = true;
                    break;
                }
                Ok(Some(Frame::ArtifactFile {
                    mount_index,
                    rel_path,
                    data,
                })) => {
                    write_artifact(artifact_host_dirs, mount_index, &rel_path, &data);
                }
                Ok(Some(Frame::Exec(_))) => {} // not expected from the guest
                Ok(Some(Frame::Shutdown)) => {} // host-only frame; ignore if echoed
                // Live-progress line from the guest agent's FIFO reader
                // (workflow sandbox step). Forward the raw bytes (one
                // newline-trimmed line) to the sink. Ignored when no sink is
                // wired (stray frame on a non-progress exec).
                Ok(Some(Frame::ProcessProgress { bytes, .. })) => {
                    if let Some(tx) = progress_tx.as_ref() {
                        let _ = tx.send(bytes);
                    }
                }
                // Other long-lived frames don't belong on a one-shot Exec
                // connection; the guest only emits them when the host
                // sent StartProcess first. Ignore defensively.
                Ok(Some(
                    Frame::StartProcess(_)
                    | Frame::Started(_)
                    | Frame::Stdin { .. }
                    | Frame::ProcessStdout { .. }
                    | Frame::ProcessStderr { .. }
                    | Frame::ProcessExit(_)
                    | Frame::KillProcess(_)
                    | Frame::Ping
                    | Frame::Pong,
                )) => {}
                Ok(None) => break,
                Err(e) => return Err(AppError::internal_error(format!("guest protocol error: {e}"))),
            }
        }
        if done {
            break;
        }
    }

    Ok(SandboxRunResult {
        exit_code,
        stdout: lossy(stdout, stdout_truncated),
        stderr: lossy(stderr, stderr_truncated),
        stdout_truncated,
        stderr_truncated,
        duration_ms: started.elapsed().as_millis() as u64,
        timed_out,
    })
}

/// Write one streamed artifact into its host dir. `rel_path` is guest-supplied
/// (the bytes a sandboxed process wrote), so it is UNTRUSTED: reject absolute
/// paths, any `..` component, and confine the resolved write under the host
/// dir. Best-effort — a malformed entry is logged + skipped, never fatal (the
/// runner's `collect_step_artifacts` re-walks the host dir afterward and also
/// re-checks path safety).
fn write_artifact(host_dirs: &[PathBuf], mount_index: u32, rel_path: &str, data: &[u8]) {
    let Some(base) = host_dirs.get(mount_index as usize) else {
        tracing::warn!(
            mount_index,
            "vm_client: ArtifactFile for unknown mount index; dropping"
        );
        return;
    };
    let rel = Path::new(rel_path);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir))
    {
        tracing::warn!(%rel_path, "vm_client: rejecting unsafe artifact rel_path");
        return;
    }
    let dest = base.join(rel);
    // Defense-in-depth: the joined path must still be under base.
    if !dest.starts_with(base) {
        tracing::warn!(%rel_path, "vm_client: artifact escaped its mount dir; dropping");
        return;
    }
    if let Some(parent) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(path = %parent.display(), "vm_client: mkdir artifact parent failed: {e}");
            return;
        }
    }
    if let Err(e) = std::fs::write(&dest, data) {
        tracing::warn!(path = %dest.display(), "vm_client: write artifact failed: {e}");
    } else {
        tracing::debug!(path = %dest.display(), bytes = data.len(), "vm_client: wrote streamed artifact");
    }
}

#[cfg(test)]
mod tests {
    use super::write_artifact;
    use std::path::PathBuf;

    /// Security: a guest-supplied `rel_path` is UNTRUSTED. Absolute paths,
    /// `..` traversal, and any post-join escape must write NOTHING outside the
    /// host artifact dir. Only a benign relative path lands under base.
    #[test]
    fn write_artifact_confines_untrusted_rel_path_under_base() {
        let base_tmp = tempfile::tempdir().unwrap();
        let base: PathBuf = base_tmp.path().to_path_buf();
        // A sentinel dir OUTSIDE base that a traversal would target.
        let outside_tmp = tempfile::tempdir().unwrap();
        let host_dirs = vec![base.clone()];

        // Each of these must be rejected (nothing written anywhere).
        for evil in [
            "../../etc/passwd",
            "/etc/passwd",
            "a/../../b",
            format!("{}/pwned", outside_tmp.path().display()).as_str(), // absolute escape
        ] {
            write_artifact(&host_dirs, 0, evil, b"malicious");
        }

        // base must still be empty (no traversal wrote into or through it).
        let entries: Vec<_> = std::fs::read_dir(&base).unwrap().collect();
        assert!(
            entries.is_empty(),
            "no rejected artifact may create anything under base: {entries:?}"
        );
        // The outside sentinel dir must be untouched by the escaping writes.
        let outside_entries: Vec<_> =
            std::fs::read_dir(outside_tmp.path()).unwrap().collect();
        assert!(
            outside_entries.is_empty(),
            "a traversal must not write outside base"
        );

        // An out-of-range mount_index is dropped (no panic, nothing written).
        write_artifact(&host_dirs, 99, "ok.txt", b"x");
        assert!(std::fs::read_dir(&base).unwrap().next().is_none());

        // A benign nested relative path is written UNDER base.
        write_artifact(&host_dirs, 0, "sub/dir/f.txt", b"good bytes");
        let landed = base.join("sub/dir/f.txt");
        assert!(landed.exists(), "benign artifact must be written under base");
        assert!(landed.starts_with(&base));
        assert_eq!(std::fs::read(&landed).unwrap(), b"good bytes");
    }
}

fn append_capped(buf: &mut Vec<u8>, chunk: &[u8], truncated: &mut bool) {
    if *truncated {
        return;
    }
    if buf.len() + chunk.len() > OUTPUT_CAP_BYTES {
        let remain = OUTPUT_CAP_BYTES - buf.len();
        buf.extend_from_slice(&chunk[..remain]);
        *truncated = true;
    } else {
        buf.extend_from_slice(chunk);
    }
}

fn lossy(buf: Vec<u8>, truncated: bool) -> String {
    let mut s = String::from_utf8_lossy(&buf).into_owned();
    if truncated {
        s.push_str(&format!("\n[output truncated at {OUTPUT_CAP_BYTES} bytes]\n"));
    }
    s
}
