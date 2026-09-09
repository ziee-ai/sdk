use std::sync::Arc;

use once_cell::sync::OnceCell;
use schemars::JsonSchema;
use serde::Serialize;

use crate::types::CodeSandboxState;

/// Why `code_sandbox` is (or isn't) initialized, in machine-readable form.
///
/// `Ready` means `init()` reached the end and `get_state()` is `Some`. Every
/// other variant is a specific early-return reason recorded by `init()`; the
/// rootfs-versions admin endpoint surfaces it so the UI can degrade gracefully
/// (show the GitHub catalog + a precise notice) instead of a blanket error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SandboxAvailability {
    /// Fully initialized — the sandbox is registered and usable.
    Ready,
    /// `code_sandbox.enabled: false` in config (the default).
    DisabledInConfig,
    /// The host boot probe failed — on Linux this means `bwrap` is not on PATH.
    HostUnsupported,
    /// The cloud instance-metadata endpoint is reachable and
    /// `allow_cloud_imds_reachable` is not set, so registration was refused.
    CloudImdsRefused,
    /// The per-conversation workspace root could not be created.
    WorkspaceInitFailed,
    /// State exists but carries no DB pool (an in-process test edge case).
    PoolMissing,
    /// `init()` has not run yet, or bailed before recording a reason.
    NotInitialized,
}

impl SandboxAvailability {
    /// A precise, operator-facing explanation of WHY the sandbox is (or isn't)
    /// available — used to build the `SANDBOX_NOT_INITIALIZED` error so the
    /// message names the ACTUAL recorded cause instead of guessing.
    ///
    /// `init()` is a set-once `OnceCell`, so on a live server the status never
    /// changes after boot: there is no "not yet booted, try later" state to
    /// report — hence no such phrasing here.
    pub fn explain(self) -> &'static str {
        match self {
            SandboxAvailability::Ready => "the sandbox is initialized and ready",
            SandboxAvailability::DisabledInConfig => {
                "code_sandbox is disabled in config (code_sandbox.enabled: false)"
            }
            SandboxAvailability::HostUnsupported => {
                "the host does not support the sandbox (on Linux this means bwrap \
                 is not installed or not on PATH)"
            }
            SandboxAvailability::CloudImdsRefused => {
                "registration was refused because a cloud instance-metadata \
                 endpoint is reachable and allow_cloud_imds_reachable is not set"
            }
            SandboxAvailability::WorkspaceInitFailed => {
                "the per-conversation workspace root could not be created at boot"
            }
            SandboxAvailability::PoolMissing => {
                "the sandbox state carries no database pool (an in-process test edge case)"
            }
            SandboxAvailability::NotInitialized => {
                "code_sandbox has not been initialized (init() has not run for this process)"
            }
        }
    }
}

/// Build the standard `SANDBOX_NOT_INITIALIZED` (503) error with the ACTUAL
/// recorded reason from [`init_status`] embedded — so a caller sees the real
/// cause (a host gate, disabled-in-config, workspace-init failure, …) instead
/// of the old guessed "module disabled or not yet booted". One honest source of
/// truth shared by every `get_state()`-guard call site (SDK + server).
pub fn not_initialized_error() -> ziee_core::AppError {
    ziee_core::AppError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "SANDBOX_NOT_INITIALIZED",
        format!("code_sandbox is not available: {}", init_status().explain()),
    )
}

static STATE: OnceCell<Arc<CodeSandboxState>> = OnceCell::new();

/// Records WHY `init()` finished the way it did. Set once per process at every
/// `init()` exit (success or early return). Parallel to `STATE`: `STATE` is
/// `Some` iff this is `Ready`, but the reason is retained even when `STATE`
/// stays `None`, so a disabled/not-initialized deployment can explain itself.
static INIT_STATUS: OnceCell<SandboxAvailability> = OnceCell::new();

/// Record the module's init outcome. Idempotent: the first value wins (a second
/// call — e.g. a test harness re-running init — is ignored, matching `STATE`'s
/// first-write-wins semantics).
pub fn set_init_status(status: SandboxAvailability) {
    let _ = INIT_STATUS.set(status);
}

/// The recorded init outcome. Defaults to `NotInitialized` until `init()` has
/// set it (i.e. before boot).
pub fn init_status() -> SandboxAvailability {
    INIT_STATUS.get().copied().unwrap_or(SandboxAvailability::NotInitialized)
}

/// Set the global sandbox state. Called once at `code_sandbox::init()`.
/// Returns the existing state if already initialized; the second call
/// is logged at WARN level so test harnesses / hot-reload paths see a
/// clear signal that the new state was discarded.
pub fn init_state(state: CodeSandboxState) -> Arc<CodeSandboxState> {
    let arc = Arc::new(state);
    if STATE.set(arc.clone()).is_err() {
        tracing::warn!(
            "code_sandbox::init_state called more than once; \
             second call's state is discarded and the FIRST state \
             remains in effect. This typically happens in test \
             harnesses; in production it indicates a double init()."
        );
    }
    STATE.get().cloned().unwrap_or(arc)
}

/// Get the global sandbox state. Returns `None` until `init_state` has
/// been called (i.e. when `code_sandbox.enabled = false` or before boot).
pub fn get_state() -> Option<Arc<CodeSandboxState>> {
    STATE.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_serializes_snake_case() {
        // The UI reads these as a TS string union, so the wire form must be
        // snake_case (matching the module's other enums).
        let cases = [
            (SandboxAvailability::Ready, "\"ready\""),
            (SandboxAvailability::DisabledInConfig, "\"disabled_in_config\""),
            (SandboxAvailability::HostUnsupported, "\"host_unsupported\""),
            (SandboxAvailability::CloudImdsRefused, "\"cloud_imds_refused\""),
            (SandboxAvailability::WorkspaceInitFailed, "\"workspace_init_failed\""),
            (SandboxAvailability::PoolMissing, "\"pool_missing\""),
            (SandboxAvailability::NotInitialized, "\"not_initialized\""),
        ];
        for (variant, expected) in cases {
            assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
        }
    }

    #[test]
    fn init_status_defaults_to_not_initialized() {
        // NOTE: `INIT_STATUS` is a process-global OnceCell. This test only reads
        // the default; it never calls `set_init_status`, so it cannot perturb
        // (or be perturbed by) another test in the same binary. The set/first-
        // write-wins behavior is exercised by the integration harness (each test
        // is its own server subprocess) rather than mutated here.
        assert_eq!(init_status(), SandboxAvailability::NotInitialized);
    }

    // TEST-1 [acceptance INV-1]: every variant explains itself accurately, and
    // NO variant's reason contains the false "not yet booted" clause the old
    // message asserted (init() is set-once — it can never boot later).
    #[test]
    fn explain_gives_a_specific_honest_reason_per_variant() {
        let all = [
            SandboxAvailability::Ready,
            SandboxAvailability::DisabledInConfig,
            SandboxAvailability::HostUnsupported,
            SandboxAvailability::CloudImdsRefused,
            SandboxAvailability::WorkspaceInitFailed,
            SandboxAvailability::PoolMissing,
            SandboxAvailability::NotInitialized,
        ];
        for v in all {
            let reason = v.explain();
            assert!(!reason.is_empty(), "{v:?} must explain itself");
            assert!(
                !reason.contains("not yet booted"),
                "{v:?}: the false 'not yet booted' clause must be gone: {reason}"
            );
        }
        // The specific reason that misled the investigator (a missing host dep)
        // must name the host gate, not a guessed disjunction.
        let host = SandboxAvailability::HostUnsupported.explain();
        assert!(
            host.contains("bwrap") && (host.contains("PATH") || host.contains("host")),
            "HostUnsupported must name the bwrap/host gate: {host}"
        );
        // The disabled reason must NOT be reported for a host-gate failure.
        assert!(
            !host.contains("disabled"),
            "HostUnsupported must not guess 'disabled': {host}"
        );
    }
}
