//! `portrace_probe_server` — the harness's own e2e server for the
//! GAP-harness-port-pick-toctou regression test (`tests/portrace.rs`).
//!
//! A deliberately trivial HTTP server whose ONLY job is to exercise the
//! socket-activation handoff: when `ZIEE_LISTEN_FD` is set (Unix), it adopts
//! the inherited, already-bound listener exactly like a real app would via
//! `ziee_framework::bind_listener`; otherwise it binds `PROBE_PORT`. It serves
//! `/api/health` → 200 `{"status":"ok"}` (the path `HarnessApp::health_path`
//! defaults to) and stays alive until the harness reaps it — so
//! `TestHarness::start`'s readiness poll and this process drive each other
//! precisely like a real spawned test server does.
//!
//! It deliberately does NOT depend on `ziee-framework`: the framework's own
//! contract is pinned by `ziee-framework/tests/listen_fd.rs`, and the harness
//! crate stays a lean dev-support crate. This binary is the harness-side half
//! (fd reachable across `exec` + `ZIEE_LISTEN_FD` in the child's env) and
//! implements the same 3-line adoption here so the test can stay in the
//! harness crate.

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Adopt the inherited listener (`ZIEE_LISTEN_FD`), or bind `PROBE_PORT`.
async fn listener() -> std::io::Result<tokio::net::TcpListener> {
    #[cfg(unix)]
    {
        use std::os::fd::FromRawFd;
        if let Ok(fd) = std::env::var("ZIEE_LISTEN_FD") {
            let fd: i32 = fd.parse().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("probe server: ZIEE_LISTEN_FD={fd:?} is not a valid fd number"),
                )
            })?;
            // SAFETY: the fd was bound and kept open by the harness, which set
            // ZIEE_LISTEN_FD — it is ours by contract (the framework validates
            // with fcntl first; the harness handoff is the only producer).
            let std_listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
            std_listener.set_nonblocking(true)?;
            return tokio::net::TcpListener::from_std(std_listener);
        }
    }
    let port: u16 = std::env::var("PROBE_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "probe server: neither ZIEE_LISTEN_FD nor PROBE_PORT is set",
            )
        })?;
    tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let listener = listener().await?;
    let addr = listener.local_addr()?;
    eprintln!("portrace probe server: serving on {addr}");
    loop {
        let (mut sock, _peer) = listener.accept().await?;
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let (status, body) = if request.starts_with("GET /api/health") {
                ("200 OK", "{\"status\":\"ok\"}")
            } else {
                ("404 Not Found", "not found")
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.shutdown().await;
        });
    }
}
