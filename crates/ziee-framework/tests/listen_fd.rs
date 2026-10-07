//! `bind_listener` + the inherited-listener (`ZIEE_LISTEN_FD`) handoff.
//!
//! GAP-harness-port-pick-toctou closes the harness-side TOCTOU by handing the
//! child an ALREADY-BOUND listener across `exec` instead of a port NUMBER that
//! could be stolen between pick and bind. These tests are the FRAMEWORK half
//! of that contract: a process that receives `ZIEE_LISTEN_FD` must serve on
//! the inherited listener (not bind the configured address), and a
//! set-but-invalid value must be a loud error rather than a silent fall back.

#![cfg(unix)]

//! `bind_listener` + the inherited-listener (`ZIEE_LISTEN_FD`) handoff.
//!
//! GAP-harness-port-pick-toctou closes the harness-side TOCTOU by handing the
//! child an ALREADY-BOUND listener across `exec` instead of a port NUMBER that
//! could be stolen between pick and bind. These tests are the FRAMEWORK half
//! of that contract: a process that receives `ZIEE_LISTEN_FD` must serve on
//! the inherited listener (not bind the configured address), and a
//! set-but-invalid value must be a loud error rather than a silent fall back.

use std::net::TcpListener;
use std::os::fd::IntoRawFd;
use std::sync::Mutex;

use ziee_core::config::ServerConfig;
use ziee_framework::app_builder::bind_listener;

/// `std::env::set_var` is process-global; the two tests in this binary both
/// mutate `ZIEE_LISTEN_FD`, so they serialize on this lock (and each test
/// restores the prior value on every exit path via [`EnvRestore`]).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Restore exactly what `ZIEE_LISTEN_FD` was before the test touched it.
struct EnvRestore(Option<std::ffi::OsString>);
impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => std::env::set_var("ZIEE_LISTEN_FD", v),
            None => std::env::remove_var("ZIEE_LISTEN_FD"),
        }
    }
}

fn set_listen_fd(value: &str) -> EnvRestore {
    let prev = std::env::var_os("ZIEE_LISTEN_FD");
    std::env::set_var("ZIEE_LISTEN_FD", value);
    EnvRestore(prev)
}

/// A minimal valid `ServerConfig`. The configured host/port are deliberately
/// IRRELEVANT to the inherited path — the whole point is that the configured
/// address is NOT bound when a listener is inherited.
fn test_config() -> ServerConfig {
    serde_json::from_str(
        r#"{
            "postgresql": { "use_embedded": false },
            "server": { "host": "127.0.0.1", "port": 1, "api_prefix": "/api" },
            "jwt": {
                "secret": "0123456789abcdef0123456789abcdef-strong",
                "issuer": "test", "audience": "test-api",
                "access_token_expiry_hours": 24
            }
        }"#,
    )
    .expect("test config must deserialize")
}

/// A server started with `ZIEE_LISTEN_FD` serves on the INHERITED listener:
/// the same port the parent bound, not the configured one — and it can serve
/// real HTTP there.
#[tokio::test]
async fn serves_on_the_inherited_listener() {
    let _env_lock = ENV_LOCK.lock().unwrap();

    // The "parent": bind 127.0.0.1:0 and KEEP it open, then hand the fd over —
    // exactly the shape `ziee-test-harness::TestHarness::start` now uses when
    // it spawns a server (it just does it across `exec`; same process here).
    let parent_listener = TcpListener::bind("127.0.0.1:0").expect("parent binds");
    let port = parent_listener.local_addr().expect("parent local_addr").port();
    let fd = parent_listener.into_raw_fd();

    // `bind_listener` must ADOPT that fd instead of binding the config address
    // (port 1 in `test_config` — a bind there would fail, so an accidental
    // fall back is loudly caught by this very assertion).
    let _restore = set_listen_fd(&fd.to_string());
    let config = test_config();
    let listener = bind_listener(&config)
        .await
        .expect("bind_listener must adopt the inherited listener");
    assert_eq!(
        listener.local_addr().expect("adopted local_addr").port(),
        port,
        "must serve on the INHERITED listener's port, not the configured one"
    );

    // Prove it actually serves: the inherited listener is live and answering.
    let app = axum::Router::new().route(
        "/api/health",
        axum::routing::get(|| async { "{\"status\":\"ok\"}" }),
    );
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve on inherited listener");
    });
    let resp = reqwest::get(format!("http://127.0.0.1:{port}/api/health"))
        .await
        .expect("GET /api/health on the inherited listener");
    assert_eq!(resp.status().as_u16(), 200, "inherited listener answers");
    assert!(
        resp.text().await.expect("body").contains("\"ok\""),
        "body must be the health payload"
    );
    handle.abort();
    let _ = handle.await;
}

/// A set-but-INVALID `ZIEE_LISTEN_FD` is a clear error, never a silent fall
/// back to binding the configured address: a parent that intended a handoff
/// must not get an unexpectedly different port.
#[tokio::test]
async fn invalid_listen_fd_is_a_clear_error_not_a_silent_fallback() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let config = test_config();

    // Non-numeric value.
    {
        let _restore = set_listen_fd("not-a-fd");
        let err = bind_listener(&config).await.expect_err("non-numeric must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("ZIEE_LISTEN_FD"),
            "error must name the env var, got: {msg}"
        );
    }

    // An fd number no process on this box has open.
    {
        let _restore = set_listen_fd("99999999");
        let err = bind_listener(&config)
            .await
            .expect_err("nonexistent fd must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("ZIEE_LISTEN_FD") && msg.contains("not an open file descriptor"),
            "error must say the fd is invalid, got: {msg}"
        );
    }

    // A negative fd number.
    {
        let _restore = set_listen_fd("-3");
        let err = bind_listener(&config).await.expect_err("negative must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("ZIEE_LISTEN_FD") && msg.contains("negative"),
            "error must call out the negative fd, got: {msg}"
        );
    }
}

/// Unset is the previous behaviour: the configured address is bound. (Proves
/// the env-var path does not leak into ordinary boots.)
#[tokio::test]
async fn unset_binds_the_configured_address() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _restore = EnvRestore(std::env::var_os("ZIEE_LISTEN_FD"));
    std::env::remove_var("ZIEE_LISTEN_FD");

    let mut config = test_config();
    config.server.port = 0; // ephemeral: the OS picks it
    let listener = bind_listener(&config).await.expect("bind configured address");
    let addr = listener.local_addr().expect("local_addr");
    assert_eq!(
        addr.ip().to_string(),
        "127.0.0.1",
        "binds the configured host"
    );
}
