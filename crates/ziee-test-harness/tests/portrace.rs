//! GAP-harness-port-pick-toctou regression: the SDK fixes the race the comic
//! app used to retry around.
//!
//! The harness now reserves the spawned server's port by binding 127.0.0.1:0
//! and KEEPING the listener open, and hands that already-bound listener to the
//! child across `exec` (`ZIEE_LISTEN_FD`); the child serves on it instead of
//! binding the configured address. This test proves the whole chain end-to-end
//! through the REAL `TestHarness::start`, with the harness's own throwaway
//! probe server (src/bin/portrace_probe_server.rs) as the app:
//!
//!   1. at `plan_spawn` time (the old pick→spawn window), an independent bind
//!      of the picked port must FAIL with AddrInUse — the harness holds it
//!      (with the old probe-and-drop pick it succeeded, which is the RED line
//!      this test exists to catch);
//!   2. the spawned server is healthy on that SAME port — the fd survived
//!      `exec` and the child is serving on the inherited listener;
//!   3. while the server runs, an independent bind still fails — the child now
//!      holds the port (no gap in ownership at any instant);
//!   4. after Drop, the port is free again — the hold was real, not a leak.
//!
//! The second test pins a second-order defect of the same handoff: the
//! inherited listener's FD_CLOEXEC must be cleared in the CHILD's `pre_exec`
//! hook, never in this (multi-threaded test) parent — clearing it here would
//! leak the listener's socket into every unrelated process any other thread
//! forks while the parent holds it. The test installs the harness's test-only
//! spawn seam (which runs exactly in that former leak window, while the parent
//! still holds the listener), spawns an unrelated sibling there, and asserts
//! the sibling owns no copy of the server's socket (inode-compared via
//! `/proc/<pid>/fd`).
//!
//! Requires the same live Postgres the other harness tests need (`DATABASE_URL`
//! or the shared-cluster default at 127.0.0.1:54321); the probe app's template
//! DB carries zero migrations, so nothing app-specific has to exist.

#![cfg(unix)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ziee_test_harness::{HarnessApp, SpawnFacts, SpawnPlan, TestHarness, Variant};

/// The probe server's fallback port (used only when `ZIEE_LISTEN_FD` is
/// absent — which is exactly the temporarily-reverted old behaviour).
const PROBE_PORT_ENV: &str = "PROBE_PORT";

/// The harness's own throwaway app: one route-free probe server that proves
/// the port handoff. Its `plan_spawn` is the observation point inside the
/// pick→spawn window — exactly where the old probe-and-drop pick was
/// stealable.
struct PortraceProbeApp {
    probe_result: PathBuf,
}

impl HarnessApp for PortraceProbeApp {
    type Options = ();

    fn template_db_base(&self, _variant: Variant) -> String {
        "sdk_portrace_probe".to_string()
    }

    fn migration_dirs(&self, _variant: Variant, _manifest_dir: &Path) -> Vec<PathBuf> {
        // Zero migrations: the template DB needs no schema for a health probe.
        vec![]
    }

    fn plan_spawn(&self, _opts: &Self::Options, facts: &SpawnFacts) -> SpawnPlan {
        // THE observation point for the race: at this instant the harness has
        // "picked" `facts.server_port` but not yet spawned the child. An
        // independent bind must FAIL with AddrInUse — the harness holds the
        // port the whole time. With the old pick-and-drop it succeeded (the
        // port was free until the child bound it), which is the RED line.
        let outcome = match TcpListener::bind(("127.0.0.1", facts.server_port)) {
            Ok(_) => "unexpectedly-free".to_string(),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => "addr-in-use".to_string(),
            Err(e) => format!("other:{e}"),
        };
        std::fs::write(&self.probe_result, &outcome)
            .expect("write port-race probe result");

        SpawnPlan {
            config_yaml: format!(
                "postgresql:\n  use_embedded: false\nserver:\n  host: 127.0.0.1\n  port: {}\n",
                facts.server_port
            ),
            binary_name: "portrace_probe_server".to_string(),
            extra_argv: vec![],
            extra_env: vec![(PROBE_PORT_ENV.to_string(), facts.server_port.to_string())],
            keep_alive: vec![],
        }
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime")
}

#[test]
fn the_picked_port_is_held_by_the_harness_and_then_the_child() {
    // Referencing CARGO_BIN_EXE_* is what makes cargo build the probe server
    // and place it at `target/debug/portrace_probe_server` — the exact path
    // `TestHarness::start`'s binary walk looks in.
    let probe_bin = env!("CARGO_BIN_EXE_portrace_probe_server");
    assert!(
        Path::new(probe_bin).exists(),
        "probe server binary {probe_bin} must be built"
    );

    let temp_dir = tempfile::tempdir().expect("tempdir for probe result");
    let probe_result = temp_dir.path().join("plan-spawn-probe.txt");
    let app = PortraceProbeApp {
        probe_result: probe_result.clone(),
    };
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let server = runtime().block_on(async {
        TestHarness::new(app, manifest_dir, Variant::Server)
            .start(())
            .await
    });

    // 1) The window probe: the harness held the port between pick and spawn.
    let outcome = std::fs::read_to_string(&probe_result)
        .expect("plan_spawn must have written the probe result");
    assert_eq!(
        outcome,
        "addr-in-use",
        "the picked port must be HELD by the harness the whole pick→spawn \
         window (an independent bind must hit AddrInUse); got {outcome:?} — \
         with the old probe-and-drop pick this bind succeeded"
    );

    // 2) The child is healthy on that SAME port: the inherited listener works.
    let port: u16 = server
        .base_url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|s| s.parse().ok())
        .expect("base_url carries the reserved port");
    let health_url = format!("{}/api/health", server.base_url);
    let health = runtime().block_on(async {
        reqwest::get(&health_url).await.expect("GET /api/health")
    });
    assert_eq!(
        health.status().as_u16(),
        200,
        "probe server must be healthy on the inherited listener"
    );

    // 3) While the server runs, the port belongs to the CHILD — an independent
    //    bind still fails, so there is no instant at which it was free.
    let held = TcpListener::bind(("127.0.0.1", port)).expect_err(
        "the port must be held by the running server — a concurrent bind must \
         fail with AddrInUse",
    );
    assert_eq!(
        held.kind(),
        std::io::ErrorKind::AddrInUse,
        "the running server holds {port}: {held}"
    );

    // 4) Drop: the hold is released, not leaked.
    drop(server);
    let rebound = TcpListener::bind(("127.0.0.1", port))
        .expect("after Drop the port must be free again (the hold was real)");
    assert_eq!(
        rebound.local_addr().expect("rebound local_addr").port(),
        port,
        "the port really was free again"
    );
}

/// Every `socket:[inode]` target under `/proc/<pid>/fd`.
fn socket_inodes(pid: u32) -> Vec<String> {
    let dir = format!("/proc/{pid}/fd");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir}: {e}")) {
        let entry = entry.unwrap_or_else(|e| panic!("readdir {dir}: {e}"));
        let target = std::fs::read_link(entry.path())
            .unwrap_or_else(|e| panic!("readlink {}: {e}", entry.path().display()));
        let target = target.to_string_lossy().into_owned();
        if target.starts_with("socket:[") {
            out.push(target);
        }
    }
    out.sort();
    out
}

/// Second-order regression of the same handoff: clearing FD_CLOEXEC on the
/// reserved listener must happen in the CHILD's `pre_exec` hook, after fork —
/// never in this parent. This process is a multi-threaded libtest test runner;
/// between a parent-side `F_SETFD` and the parent's spawn+close of the fd, ANY
/// other thread's `fork`+`exec` (another test's server spawn, any `Command`)
/// inherits the listener's socket and holds it open for its whole life.
///
/// The harness's test-only spawn seam runs exactly inside that window — after
/// the `ZIEE_LISTEN_FD` handoff setup, before the child spawn, while this
/// parent still holds the reserved listener (between "bind" and "spawn"). The
/// seam forks an unrelated `sleep` sibling; the test then compares socket
/// inodes between the intended server (`/proc/<server-pid>/fd`) and the
/// sibling (`/proc/<sibling-pid>/fd`) and asserts the sibling owns NO copy of
/// the server's socket. With the fix the parent never clears FD_CLOEXEC, so
/// the sibling's `exec` closes the descriptor and it has no sockets at all —
/// GREEN. With the parent-side clear restored, the sibling inherits the socket
/// and keeps it open: the inode shows up in its fd table — RED.
#[test]
fn unrelated_sibling_in_the_leak_window_does_not_inherit_the_listener_socket() {
    let temp_dir = tempfile::tempdir().expect("tempdir for the sibling-leak probe");
    let probe_result = temp_dir.path().join("plan-spawn-probe.txt");
    let sibling_pid_file = temp_dir.path().join("sibling.pid");

    // The sibling is THIS test's direct child. Kill + reap it on every exit
    // path (green AND red) so the leak test never leaves a `sleep` behind.
    struct SiblingReaper(Arc<Mutex<Option<std::process::Child>>>);
    impl Drop for SiblingReaper {
        fn drop(&mut self) {
            if let Some(mut child) = self.0.lock().expect("sibling cell").take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    let sibling_cell: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
    let seam_cell = Arc::clone(&sibling_cell);
    let seam_pid_file = sibling_pid_file.clone();

    let mut harness = TestHarness::new(
        PortraceProbeApp { probe_result },
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        Variant::Server,
    );
    // THE SEAM — the unrelated sibling is forked at the exact instant the old
    // parent-side FD_CLOEXEC clear used to leak the listener: after the
    // handoff setup, before the child spawn, while this parent still holds the
    // reserved listener.
    harness.set_spawn_seam(move || {
        let child = std::process::Command::new("/bin/sleep")
            .arg("300")
            .spawn()
            .expect("spawn the unrelated sibling in the leak window");
        std::fs::write(&seam_pid_file, child.id().to_string())
            .expect("write the unrelated-sibling pid");
        *seam_cell.lock().expect("sibling cell") = Some(child);
    });

    let server = runtime().block_on(harness.start(()));
    let _reaper = SiblingReaper(Arc::clone(&sibling_cell));

    // The seam ran synchronously inside `start`, before the server was
    // spawned; its sibling is alive and its pid is recorded.
    let sibling_pid: u32 = std::fs::read_to_string(&sibling_pid_file)
        .expect("the seam must have written the sibling pid")
        .trim()
        .parse()
        .expect("parse the sibling pid");
    let server_pid = server.process_id();
    assert_ne!(
        server_pid, sibling_pid,
        "the sibling must be an unrelated process, not the server"
    );

    // The socket inodes the INTENDED server owns right now: its inherited
    // listener (plus any transient accepted connection).
    let server_sockets = socket_inodes(server_pid);
    assert!(
        !server_sockets.is_empty(),
        "the harness-spawned server {server_pid} must own the inherited listener \
         socket — it was handed the port"
    );

    // The unrelated sibling must hold NO fd pointing at any of those inodes.
    let sibling_sockets = socket_inodes(sibling_pid);
    for s in &sibling_sockets {
        assert!(
            !server_sockets.contains(s),
            "unrelated sibling {sibling_pid} holds the intended server's socket \
             {s} — the parent-side FD_CLOEXEC clear leaked the reserved listener \
             into an unrelated child, which now keeps the port open for its \
             whole life"
        );
    }
    // A plain `sleep` sibling holds no sockets at all.
    assert!(
        sibling_sockets.is_empty(),
        "unrelated sibling {sibling_pid} holds sockets it was never given: \
         {sibling_sockets:?}"
    );
}
