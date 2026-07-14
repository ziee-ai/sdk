//! End-to-end smoke test for the libkrun microVM stack on macOS.
//!
//! Drives the launcher binary directly (bypassing the server) and runs a
//! single Exec round-trip through the agent. Validates MACOS-RUNBOOK §6:
//!   (1) VM boots, (2) vsock direction (host dials), (3) /dev/vda mounts
//!   the squashfs, (4) virtio-fs tag "workspace" mounts, (5) exit code +
//!   stdout round-trip via Frame::Exec → Stdout → Exit.
//!
//! Run on Apple Silicon after `brew install slp/krun/libkrun`, building the
//! launcher (codesigned with com.apple.security.hypervisor), cross-building
//! the guest agent (aarch64-musl, static), and assembling the guest root:
//!
//!   cargo test --release --test boot_smoke -- --ignored --nocapture
//!
//! Paths are overridable via env vars; defaults match the dev cache layout
//! under `.ziee-cache/` at the repo root.

#![cfg(target_os = "macos")]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use sandbox_vm_protocol::{encode, Decoder, ExecRequest, Frame, PROTOCOL_VERSION};

const SOCK_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(15);
const EXEC_TIMEOUT_MS: u64 = 10_000;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn repo_cache(rel: &str) -> String {
    // Canonicalize: macOS unix-socket sun_path is 104 bytes max, and
    // launcher_dir + "/../../.ziee-cache/..." easily overshoots that.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let raw = std::path::PathBuf::from(format!("{manifest_dir}/../../.ziee-cache"));
    let canon = std::fs::canonicalize(&raw).unwrap_or(raw);
    canon.join(rel).to_string_lossy().into_owned()
}

#[test]
#[ignore = "needs libkrun + codesigned launcher + assembled guest root + dummy squashfs"]
fn boot_smoke_echo_round_trip() {
    let launcher = env_or(
        "ZIEE_LAUNCHER_BIN",
        &format!("{}/target/release/ziee-sandbox-vm-launcher", env!("CARGO_MANIFEST_DIR")),
    );
    let guest_root = env_or("ZIEE_SANDBOX_GUEST_ROOT", &repo_cache("guest-root"));
    let sandbox_disk = env_or("ZIEE_SANDBOX_DISK", &repo_cache("test-validation/dummy-sandbox.sqfs"));
    let workspace_root = env_or("ZIEE_WORKSPACE", &repo_cache("test-validation/workspace"));
    let cache_dir = env_or("ZIEE_TEST_CACHE", &repo_cache("test-validation"));

    std::fs::create_dir_all(&workspace_root).expect("workspace mkdir");
    std::fs::create_dir_all(&cache_dir).expect("cache mkdir");

    // The launcher needs com.apple.security.hypervisor (and library-validation
    // disabled to load /opt/homebrew/lib/libkrun.dylib from a non-system path);
    // without it krun_start_enter returns -22 (EINVAL). `cargo build` clears
    // any prior ad-hoc signature, so re-apply on every test run. Idempotent.
    let entitlements = format!("{}/entitlements.plist", env!("CARGO_MANIFEST_DIR"));
    let sign = std::process::Command::new("codesign")
        .args(["--entitlements", &entitlements, "--force", "-s", "-", &launcher])
        .output()
        .expect("codesign exec");
    assert!(
        sign.status.success(),
        "codesign failed: {}",
        String::from_utf8_lossy(&sign.stderr)
    );

    let sock_path = format!("{cache_dir}/vm.sock");
    let _ = std::fs::remove_file(&sock_path); // stale → krun_add_vsock_port2 EEXIST

    let cfg_path = format!("{cache_dir}/launch.json");
    let cfg = serde_json::json!({
        "num_vcpus": 1,
        "ram_mib": 256,
        "root_path": guest_root,
        "sandbox_disk_path": sandbox_disk,
        "workspace_host_path": workspace_root,
        "vsock_socket_path": sock_path,
        "vsock_port": 1024,
        "agent_exec_path": "/usr/bin/ziee-sandbox-agent",
    });
    std::fs::write(&cfg_path, serde_json::to_vec_pretty(&cfg).unwrap())
        .expect("write launch.json");

    // Spawn the launcher. Pipe stderr so we can scan for the agent's
    // "listening on vsock port" readiness line — vm.sock appearing on the
    // host side is libkrun's bridge being ready, NOT the guest vsock
    // listener; connecting in that window produces an immediate EOF
    // because there's no destination yet. We mirror stderr to our own
    // stderr from a reader thread so `--nocapture` still shows agent logs.
    let mut child = std::process::Command::new(&launcher)
        .arg(&cfg_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn launcher");
    let child_stderr = child.stderr.take().expect("piped stderr");
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let mut signaled = false;
        let reader = BufReader::new(child_stderr);
        for line in reader.lines().map_while(Result::ok) {
            eprintln!("{line}");
            if !signaled && line.contains("listening on vsock port") {
                let _ = ready_tx.send(());
                signaled = true;
            }
        }
    });
    let _guard = ChildGuard(child);

    // Wait for libkrun to create the host socket AND the agent to start
    // listening on the guest vsock port. The order matters: connecting
    // before the agent listens gets an immediate EOF from the bridge.
    let deadline = Instant::now() + SOCK_TIMEOUT;
    loop {
        if std::path::Path::new(&sock_path).exists() {
            break;
        }
        assert!(Instant::now() < deadline, "vsock socket {sock_path} did not appear within {SOCK_TIMEOUT:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
    ready_rx
        .recv_timeout(SOCK_TIMEOUT)
        .expect("agent did not log 'listening on vsock port' within timeout");

    // Connect via blocking unix socket — matches the Python reference probe
    // exactly (the earlier tokio version's read blocked indefinitely; not
    // root-caused, but the synchronous path is what the test needs anyway).
    let mut stream = UnixStream::connect(&sock_path).expect("connect vsock bridge");
    stream.set_read_timeout(Some(READ_TIMEOUT)).expect("set read timeout");

    let req = Frame::Exec(ExecRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: 1,
        bwrap_path: "/bin/busybox".to_string(),
        argv: vec!["echo".into(), "hello from guest".into()],
        timeout_ms: EXEC_TIMEOUT_MS,
        seccomp_fd: None,
        cgroup: None,
    });
    stream.write_all(&encode(&req)).expect("write Exec");

    let mut decoder = Decoder::new();
    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    let mut exit: Option<sandbox_vm_protocol::ExitStatus> = None;
    let mut buf = [0u8; 64 * 1024];
    while exit.is_none() {
        let n = match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => panic!(
                "read failed: {e}. stdout={:?} stderr={:?}",
                String::from_utf8_lossy(&stdout_buf),
                String::from_utf8_lossy(&stderr_buf),
            ),
        };
        decoder.feed(&buf[..n]);
        while let Some(frame) = decoder.next_frame().expect("decode") {
            match frame {
                Frame::Stdout(b) => stdout_buf.extend_from_slice(&b),
                Frame::Stderr(b) => stderr_buf.extend_from_slice(&b),
                Frame::Exit(s) => exit = Some(s),
                other => panic!("unexpected frame: {other:?}"),
            }
        }
    }

    let exit = exit.expect("Exit frame");
    let stdout = String::from_utf8_lossy(&stdout_buf);
    let stderr = String::from_utf8_lossy(&stderr_buf);
    eprintln!("--- exit: {exit:?}");
    eprintln!("--- stdout: {stdout:?}");
    eprintln!("--- stderr: {stderr:?}");

    assert_eq!(exit.code, 0, "expected exit 0, got {exit:?} (stderr={stderr:?})");
    assert!(!exit.timed_out, "command timed out unexpectedly");
    assert!(
        stdout.contains("hello from guest"),
        "expected 'hello from guest' in stdout, got {stdout:?}"
    );
}

/// Kill the child on Drop so a panic mid-test doesn't orphan the VM.
struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
