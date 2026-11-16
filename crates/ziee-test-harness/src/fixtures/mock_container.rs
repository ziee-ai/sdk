//! Load-tolerant start for the auth mock containers ([`super::oauth_mock`],
//! [`super::ldap_mock`]) that also REMOVES a container whose start timed out —
//! a StartupTimeout must not leak a docker container.
//!
//! WHY THE GATE + RE-ATTEMPT: the fixture `start()` builds the image with
//! testcontainers' DEFAULT startup timeout — 60 s (`DEFAULT_STARTUP_TIMEOUT`,
//! testcontainers 0.27.3 `runners/async_runner.rs`), which bounds
//! `docker start` + the wait condition and is not configurable from outside
//! the fixture. On a loaded host (measured: loadavg 170-290 on the dental
//! gate host) that ceiling is hit: c295's gate lost 13 tests to
//! `WaitContainer(StartupTimeout)` at ~68 s, and a solo re-run of
//! `oauth_test::test_create_oauth_provider` (`--test-threads=1`) hit it again
//! (68.92 s), while the same rig's other solo re-runs of single container
//! tests took 7-53 s each — the start-time tail sits AT the ceiling. The
//! OAuth wait is a fixed 5 s sleep, so the rest is docker itself (a bare
//! `docker create` of the same image took 7.4 s at load 282 with no test
//! running). Every LDAP/OAuth test starts its own container, so the 5 LDAP
//! tests a gate scheduled together all started at the same instant and 4 of
//! them timed out.
//!
//! WHY THE LEAK + THIS FIX: on `WaitContainer(StartupTimeout)`,
//! `async_runner::start` returns the error WITHOUT ever handing back the
//! `ContainerAsync` — so testcontainers' Drop-based stop/remove never runs
//! and the (already created, often running) container stays behind. Each leak
//! adds dockerd/JVM load, which causes more StartupTimeouts: a feedback loop
//! (measured on a host running this fixture: 163 mock-oauth2-server + 35
//! test-openldap containers running from one gate run). The error carries no
//! container id (`WaitContainerError::StartupTimeout` is a unit variant), so
//! the timed-out container is identified by a UNIQUE label applied at create
//! time (docker labels are immutable — they must be set when the container is
//! created, and the label is unique PER START ATTEMPT, per process, so
//! removing by it can never touch another test's or another worktree's
//! container).
//!
//! What this module adds, without touching any assertion:
//! 1. a process-wide cap on concurrent container starts
//!    ([`MAX_CONCURRENT_STARTS`]) so one test binary does not pile N
//!    simultaneous `docker create/start` + JVM/slapd boots onto dockerd;
//! 2. a bounded re-attempt of the container START when — and only when —
//!    testcontainers reports `WaitContainer(StartupTimeout)`: the mock
//!    container did not come up in 60 s. That is provisioning of a test
//!    double, never the behaviour under test; any other start error (image
//!    missing, port mapping, docker refusing) fails on the first attempt,
//!    and the test body itself is never re-run;
//! 3. before every re-attempt AND on the last attempt's failure, removal of
//!    the timed-out container by its unique attempt label
//!    ([`remove_labeled_containers`]), through docker's API via
//!    testcontainers' public client (`docker_client_instance`, bollard);
//! 4. one `[auth-mock]` timing line per start (visible with `--nocapture` or
//!    on a failing test) so the real start cost under load is measured.
//!
//! LABEL CONTRACT for every start closure passed to [`start_mock`]: the
//! closure MUST apply [`ATTEMPT_LABEL_KEY`] with the `String` label argument
//! it receives to the container it creates (see `OAuthMockServer::start` and
//! `LdapMockServer::start`). A start closure that skips the label is silently
//! outside the cleanup: its timed-out container can never be found again and
//! becomes a leak.

use std::collections::HashMap;
use std::error::Error;
use std::future::Future;
use std::time::Instant;

use testcontainers::bollard::query_parameters::{
    ListContainersOptionsBuilder, RemoveContainerOptionsBuilder,
};
use testcontainers::core::error::WaitContainerError;
use testcontainers::TestcontainersError;
use tokio::sync::Semaphore;
use uuid::Uuid;

/// Label key applied to EVERY auth-mock container the harness starts. The
/// VALUE is unique per start attempt (a uuid), so a label-filtered removal
/// can only ever match the container of that one attempt.
pub const ATTEMPT_LABEL_KEY: &str = "ziee.auth-mock-attempt";

/// Concurrent container starts allowed per test process. `tokio::sync::Semaphore`
/// is runtime-agnostic, so one static gate serves every `#[tokio::test]`
/// runtime in the binary.
pub const MAX_CONCURRENT_STARTS: usize = 3;

/// Start attempts per mock (each bounded by testcontainers' 60 s startup
/// timeout), so a container gets up to ~3 minutes to come up under load.
pub const MAX_START_ATTEMPTS: u32 = 3;

/// Rounds a label-scoped removal retries (see [`remove_labeled_containers`]).
const REMOVE_ROUNDS: u32 = 3;

/// Pause between removal rounds.
const REMOVE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

static START_GATE: Semaphore = Semaphore::const_new(MAX_CONCURRENT_STARTS);

/// True only for testcontainers' startup-timeout error.
fn is_startup_timeout(err: &(dyn Error + 'static)) -> bool {
    matches!(
        err.downcast_ref::<TestcontainersError>(),
        Some(TestcontainersError::WaitContainer(
            WaitContainerError::StartupTimeout
        ))
    )
}

/// List the docker containers carrying `label` as the value of
/// [`ATTEMPT_LABEL_KEY`]. `all(true)` includes containers that never started
/// (a StartupTimeout can leave the container in `created` state).
async fn list_labeled_containers(label: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let docker = testcontainers::core::client::docker_client_instance().await?;
    let mut filters = HashMap::new();
    filters.insert("label", vec![format!("{ATTEMPT_LABEL_KEY}={label}")]);
    let options = ListContainersOptionsBuilder::new()
        .all(true)
        .filters(&filters)
        .build();
    let containers = docker.list_containers(Some(options)).await?;
    Ok(containers
        .into_iter()
        .filter_map(|summary| summary.id)
        .collect())
}

/// Remove every docker container carrying `label` as the value of
/// [`ATTEMPT_LABEL_KEY`] (force, so a running leaked mock is killed too).
/// The label is a uuid unique to one start attempt, so this can never touch
/// another test's or another worktree's container. Returns the number of
/// containers removed. A container already gone (404) is not an error.
///
/// The removal is retried in bounded rounds: on a loaded host a single docker
/// API call timed out at 60+ s MORE than once (measured in the dental full
/// Rust tier: `remove_container -> Timeout error`), and a removal that times
/// out is exactly the leak the fix exists to prevent — so a failed round
/// waits briefly and re-lists/removes by the SAME unique label. An empty
/// list ends the rounds; after [`REMOVE_ROUNDS`] rounds whatever is left is
/// logged and counted as not-removed.
pub async fn remove_labeled_containers(label: &str) -> Result<usize, Box<dyn Error>> {
    let docker = testcontainers::core::client::docker_client_instance().await?;
    let mut removed = 0usize;
    for round in 1..=REMOVE_ROUNDS {
        match list_labeled_containers(label).await {
            Ok(ids) if ids.is_empty() => return Ok(removed),
            Ok(ids) => {
                let mut round_failed = false;
                for id in ids {
                    let options = RemoveContainerOptionsBuilder::new().force(true).build();
                    match docker.remove_container(&id, Some(options)).await {
                        Ok(()) => removed += 1,
                        Err(
                            testcontainers::bollard::errors::Error::DockerResponseServerError {
                                status_code: 404,
                                ..
                            },
                        ) => {}
                        Err(e) => {
                            eprintln!(
                                "[auth-mock] remove round {round}: failed to remove \
                                 timed-out container {id} (label {label}): {e}"
                            );
                            round_failed = true;
                        }
                    }
                }
                if !round_failed {
                    return Ok(removed);
                }
            }
            Err(e) => {
                eprintln!("[auth-mock] remove round {round}: listing label {label} failed: {e}");
            }
        }
        if round < REMOVE_ROUNDS {
            tokio::time::sleep(REMOVE_RETRY_DELAY).await;
        }
    }
    Ok(removed)
}

/// Run `start` under the process-wide start gate, re-attempting it only on
/// `WaitContainer(StartupTimeout)` (see the module doc), up to
/// [`MAX_START_ATTEMPTS`] times.
///
/// `start` is a closure taking the UNIQUE ATTEMPT LABEL (a uuid string) —
/// every start must apply it to the container it creates (see the LABEL
/// CONTRACT above). After each failed attempt the timed-out container (which
/// testcontainers never hands back) is removed by that label BEFORE the next
/// attempt, and again after the last attempt fails.
pub async fn start_mock<T, F, Fut>(what: &'static str, start: F) -> Result<T, Box<dyn Error>>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, Box<dyn Error>>>,
{
    let queued_at = Instant::now();
    let _permit = START_GATE
        .acquire()
        .await
        .expect("auth-mock start gate is never closed");
    let queued = queued_at.elapsed();
    let mut attempt = 1;
    loop {
        let label = Uuid::new_v4().to_string();
        let started = Instant::now();
        match start(label.clone()).await {
            Ok(server) => {
                eprintln!(
                    "[auth-mock] {what} up: queued {} ms, attempt {attempt}, start {} ms",
                    queued.as_millis(),
                    started.elapsed().as_millis(),
                );
                return Ok(server);
            }
            Err(e) => {
                let elapsed = started.elapsed();
                // The timed-out container was created but never became a
                // ContainerAsync, so nobody else can stop it: remove it by
                // its unique attempt label before re-attempting (and on the
                // terminal failure), breaking the leak <-> load feedback loop.
                if let Err(cleanup_err) = remove_labeled_containers(&label).await {
                    eprintln!(
                        "[auth-mock] {what}: failed to remove timed-out container \
                         (label {label}): {cleanup_err}"
                    );
                }
                if attempt < MAX_START_ATTEMPTS && is_startup_timeout(e.as_ref()) {
                    eprintln!(
                        "[auth-mock] {what}: attempt {attempt} hit StartupTimeout after {} ms; \
                         removed the timed-out container and starting a fresh one",
                        elapsed.as_millis(),
                    );
                    attempt += 1;
                } else {
                    return Err(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn startup_timeout() -> Box<dyn Error> {
        Box::new(TestcontainersError::WaitContainer(
            WaitContainerError::StartupTimeout,
        ))
    }

    // The mock image is already pulled locally (the auth tests use it), so
    // create + start always work; JVM start-up is irrelevant because the
    // simulated start never waits for readiness.
    const OAUTH_IMAGE: &str = "ghcr.io/navikt/mock-oauth2-server";
    const OAUTH_TAG: &str = "2.1.10";

    /// Create + start a real container carrying `label` as
    /// [`ATTEMPT_LABEL_KEY`], retrying transient docker API failures: on a
    /// loaded host a single bollard call measured "Timeout error" at 60+ s
    /// (dental full Rust tier, 2026-11-15), which would otherwise flake the
    /// deterministic leak test instead of exercising the cleanup. A
    /// partially-started attempt leaves a labeled container behind (exactly
    /// the leak shape), which the cleanup still sees by the same label.
    async fn create_started_labeled(label: &str) -> Result<(), Box<dyn Error>> {
        for attempt in 1..=3u32 {
            let docker = testcontainers::core::client::docker_client_instance().await?;
            let body = testcontainers::bollard::models::ContainerCreateBody {
                image: Some(format!("{OAUTH_IMAGE}:{OAUTH_TAG}")),
                labels: Some(HashMap::from([(
                    ATTEMPT_LABEL_KEY.to_string(),
                    label.to_string(),
                )])),
                host_config: Some(testcontainers::bollard::models::HostConfig::default()),
                ..Default::default()
            };
            let created = match docker
                .create_container(
                    None::<testcontainers::bollard::query_parameters::CreateContainerOptions>,
                    body,
                )
                .await
            {
                Ok(created) => created,
                Err(_e) if attempt < 3 => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            match docker
                .start_container(
                    &created.id,
                    None::<testcontainers::bollard::query_parameters::StartContainerOptions>,
                )
                .await
            {
                Ok(()) => return Ok(()),
                Err(_e) if attempt < 3 => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
        unreachable!("bounded retry loop always returns or exits")
    }

    /// A StartupTimeout is re-attempted with a fresh start, and the start
    /// that comes up is returned.
    #[tokio::test]
    async fn startup_timeout_is_reattempted_until_a_start_succeeds() {
        let calls = AtomicU32::new(0);
        let got = start_mock("probe", |_label: String| async {
            let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
            if n < MAX_START_ATTEMPTS {
                Err(startup_timeout())
            } else {
                Ok(n)
            }
        })
        .await
        .expect("the last attempt succeeds");
        assert_eq!(got, MAX_START_ATTEMPTS);
        assert_eq!(calls.load(Ordering::SeqCst), MAX_START_ATTEMPTS);
    }

    /// The re-attempts are bounded: a container that never comes up fails
    /// with the StartupTimeout after exactly MAX_START_ATTEMPTS starts.
    #[tokio::test]
    async fn startup_timeout_reattempts_are_bounded() {
        let calls = AtomicU32::new(0);
        let labels = Mutex::new(Vec::new());
        let err = start_mock("probe", |label: String| {
            calls.fetch_add(1, Ordering::SeqCst);
            labels.lock().unwrap().push(label);
            async { Err::<(), _>(startup_timeout()) }
        })
        .await
        .expect_err("every attempt times out");
        assert!(is_startup_timeout(err.as_ref()));
        assert_eq!(calls.load(Ordering::SeqCst), MAX_START_ATTEMPTS);
        // every attempt got its own unique label (the handle removals rely on)
        assert_eq!(labels.lock().unwrap().len(), MAX_START_ATTEMPTS as usize);
    }

    /// Any other start error is NOT re-attempted: it fails on the first call.
    #[tokio::test]
    async fn other_start_errors_fail_on_the_first_attempt() {
        let calls = AtomicU32::new(0);
        let err = start_mock("probe", |_label: String| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<(), Box<dyn Error>>("image not found".into())
        })
        .await
        .expect_err("a non-timeout error is returned");
        assert_eq!(err.to_string(), "image not found");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// THE deterministic leak regression test: each start attempt creates a
    /// REAL labeled container (docker create + start of the already-pulled
    /// mock image) and then returns `WaitContainer(StartupTimeout)` WITHOUT
    /// ever handing back a ContainerAsync — the exact production leak shape
    /// (testcontainers' timeout drops the future; when it fires before
    /// `start_container` completes there is no ContainerAsync, so nothing
    /// stops or removes the container). Every attempt gets a unique label;
    /// afterwards ZERO docker containers may carry any of them.
    ///
    /// Non-vacuity: each attempt verifies, BEFORE returning, that at least
    /// one container carries its label — so the final ZERO is a real removal
    /// and not a "the container never existed".
    #[tokio::test]
    async fn startup_timeout_leaves_zero_labeled_containers() {
        let labels_ts = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen_ts = Arc::new(Mutex::new(Vec::<usize>::new()));
        let labels_for_closure = labels_ts.clone();
        let seen_for_closure = seen_ts.clone();

        let err = start_mock("probe", move |label: String| {
            let labels_captured = labels_for_closure.clone();
            let seen_captured = seen_for_closure.clone();
            async move {
                labels_captured.lock().unwrap().push(label.clone());
                // Simulate the timed-out start: create + start a container
                // carrying the attempt label, then fail with the same error
                // testcontainers would have returned, leaving no handle to
                // it (the leak condition).
                create_started_labeled(&label).await?;
                // It must be findable by its label at this point (all=true
                // covers the not-yet-started state too).
                let still_there = list_labeled_containers(&label).await?;
                seen_captured.lock().unwrap().push(still_there.len());
                Err::<(), _>(startup_timeout())
            }
        })
        .await
        .expect_err("every attempt times out");
        assert!(
            is_startup_timeout(err.as_ref()),
            "unexpected start-mock error (docker API failure under load?): {err}",
        );

        let labels = labels_ts.lock().unwrap().clone();
        let seen = seen_ts.lock().unwrap().clone();
        assert_eq!(labels.len(), MAX_START_ATTEMPTS as usize);
        assert_eq!(seen.len(), MAX_START_ATTEMPTS as usize);
        for (i, &count) in seen.iter().enumerate() {
            assert!(
                count >= 1,
                "attempt {} recorded no labeled container mid-start — \
                 the test is vacuous, it did not exercise a real leak",
                i + 1,
            );
        }
        // THE assertion: ZERO containers carrying any attempt's label remain.
        for label in labels.iter() {
            let leftovers = list_labeled_containers(label)
                .await
                .unwrap_or_else(|e| panic!("listing label {label} after start_mock failed: {e}"));
            assert!(
                leftovers.is_empty(),
                "attempt label {label} still has containers {leftovers:?} \
                 after start_mock returned — the timed-out container leaked",
            );
        }
    }
}
