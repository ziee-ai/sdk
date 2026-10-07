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
//! Requires the same live Postgres the other harness tests need (`DATABASE_URL`
//! or the shared-cluster default at 127.0.0.1:54321); the probe app's template
//! DB carries zero migrations, so nothing app-specific has to exist.

#![cfg(unix)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};

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
