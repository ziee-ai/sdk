use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use once_cell::sync::Lazy;
use uuid::Uuid;

// Chunk C1: the JSON-RPC 2.0 envelope types moved to `ziee_framework::mcp` and
// are re-exported here so every `code_sandbox::types::JsonRpc*` importer across
// the built-in MCP servers (files/memory/web_search/lit_search/…/control/
// elicitation) resolves unchanged (decision N2 shim).
pub use ziee_framework::mcp::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};

use crate::sandbox_config::CodeSandboxConfig;
use crate::models::ConversationFile;

/// Per-conversation flavor lock. Populated by the FIRST
/// `execute_command` call in a conversation; subsequent calls in the
/// same conversation default to that flavor unless the LLM
/// explicitly requests a different one. A switch within a
/// conversation is logged (so it's auditable) but allowed — it just
/// triggers a fresh mount for the new flavor (both stay live).
pub static CONVERSATION_FLAVOR: Lazy<DashMap<Uuid, String>> = Lazy::new(DashMap::new);

/// Flavor metadata surfaced via the `list_sandbox_environments` MCP
/// tool (Phase 5). The list lives in code rather than the rootfs
/// because the binary needs to advertise flavors BEFORE any rootfs
/// is mounted; future iteration may move human-readable descriptions
/// into a `<rootfs>/.ziee-sandbox-rootfs-flavor.json` sentinel.
pub struct FlavorMetadata {
    pub flavor: &'static str,
    pub description: &'static str,
    pub approximate_size_mb: u64,
}

pub const KNOWN_FLAVORS: &[FlavorMetadata] = &[
    FlavorMetadata {
        flavor: "minimal",
        description: "Shell + coreutils + curl + jq + git + python3 (interpreter only).",
        approximate_size_mb: 57,
    },
    FlavorMetadata {
        flavor: "full",
        description: "minimal + numpy + pandas + torch + R 4.4 + tidyverse + Node 24 (npx) + uv (uvx) + ts-node.",
        approximate_size_mb: 853,
    },
];

/// Per-call context built by the HTTP handler before dispatching a tool.
#[derive(Debug, Clone)]
pub struct SandboxContext {
    pub conversation_id: Uuid,
    pub user_id: Uuid,
    pub workspace: PathBuf,
    pub files: Arc<Vec<ConversationFile>>,
    /// Caller-injected extra read-only bind mounts, appended verbatim to the
    /// `--ro-bind-try` set by `build_bwrap_argv` at the SAME position the
    /// (former inline) `/lit` bind occupied — AFTER attachment + workflow RO
    /// binds. This is the seam that keeps the build-DB-free sandbox engine free
    /// of any `crate::modules::lit_search` reverse-dependency: the ziee-side
    /// HTTP handler (`handlers::build_context`, which CAN reach `lit_search`)
    /// computes the per-conversation `/lit` full-text view bind and stores it
    /// here; the engine's argv builder appends it opaquely. Empty on every path
    /// that had no `/lit` bind (MCP-spawn ctx, tests), so the emitted argv is
    /// byte-identical to the pre-lift builder.
    pub extra_ro_binds: Vec<(String, String)>,
}

/// Host-level capabilities — known at server boot. These don't depend
/// on the sandbox rootfs being mounted, so they're probed unconditionally
/// in `code_sandbox::init()` and stored in `CodeSandboxState.host_caps`.
///
/// `bwrap_path` is required (not `Option`) — if bwrap is missing,
/// `init()` skips the MCP row entirely; this struct is never constructed.
#[derive(Debug, Clone)]
pub struct HostCapabilities {
    pub bwrap_path: PathBuf,
    pub cgroup: CgroupMode,
    pub seccomp: SeccompMode,
}

/// Full hardening capabilities — only complete after the rootfs is
/// lazily mounted on first `execute_command`. Built by merging
/// `HostCapabilities` with the rootfs-dependent `pid_namespace` probe.
/// Cached for the rest of the server's lifetime in `runtime_mount::READY`.
#[derive(Debug, Clone)]
pub struct HardeningCapabilities {
    pub bwrap_path: PathBuf,
    pub pid_namespace: PidNsMode,
    pub cgroup: CgroupMode,
    pub seccomp: SeccompMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidNsMode {
    /// `--unshare-pid --proc /proc` works (bare-metal Linux).
    Strict,
    /// PID-ns probe failed; fall back to `--dev-bind /proc /proc`
    /// (host PIDs visible inside sandbox — info leak, no escape).
    DevBindFallback,
    /// Neither mode works. Sandbox is forced off.
    Disabled,
}

#[derive(Debug, Clone)]
pub enum CgroupMode {
    /// Delegated parent cgroup is writable; per-call scope will be
    /// created at `<parent>/sandbox-<conv_id>-<nanos>/`.
    Delegated(PathBuf),
    /// No cgroup write access. rlimits-only mode.
    None,
}

#[derive(Debug, Clone)]
pub enum SeccompMode {
    /// Filter compiled once at boot; per-call we pipe these bytes to
    /// bwrap's `--seccomp <fd>`.
    Loaded(Arc<Vec<u8>>),
    /// `code_sandbox_seccomp` feature not compiled in.
    NotLinked,
    /// Feature on, but libseccomp failed at runtime. Logged at boot.
    Disabled,
}

/// Global per-process sandbox state, populated at `init()`.
///
/// De-`pool`-ed by the engine carve: the DB pin resolution + resource-limits DB
/// read + embedded guest-agent staging are injected through the three provider
/// seams (`crate::provider`) so this state — and the whole engine — names no
/// ziee-server DB/module symbol. The ziee server builds the concrete
/// `ZieeRootfsProvider` / `ZieeResourceLimitsProvider` / `ZieeGuestAgentProvider`
/// at `mod.rs::init` and stores them here.
pub struct CodeSandboxState {
    pub config: CodeSandboxConfig,
    /// Loopback URL the registered MCP server row points at.
    /// Cached so we don't recompute on every call.
    pub loopback_url: String,
    /// Workspace root: `<data_dir>/sandboxes/`. Per-conversation
    /// subdirs are created on demand under here.
    pub workspace_root: PathBuf,
    /// Cheap, rootfs-independent capabilities probed at boot. The
    /// rootfs-dependent `pid_namespace` field of `HardeningCapabilities`
    /// is populated lazily on the first `execute_command` call and
    /// cached in `runtime_mount::READY`; per-call code paths fetch
    /// the full caps via `state.rootfs.ensure_rootfs_ready(flavor).await?`.
    pub host_caps: HostCapabilities,
    /// Rootfs lifecycle seam (fetch / mount / evict / shutdown). Was the
    /// DB `pool`; the ziee `ZieeRootfsProvider` holds the pool + delegates
    /// to `runtime_mount` / `runtime_fetch`.
    pub rootfs: Arc<dyn crate::provider::RootfsProvider>,
    /// Injected DB read behind `resource_limits_cache::get`.
    pub limits: Arc<dyn crate::provider::ResourceLimitsProvider>,
    /// Embedded guest-agent staging (macOS libkrun bundle / Windows WSL2 agent).
    pub guest_agent: Arc<dyn crate::provider::GuestAgentProvider>,
}

// Manual `Debug` (the injected trait-object providers aren't `Debug`).
impl std::fmt::Debug for CodeSandboxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeSandboxState")
            .field("config", &self.config)
            .field("loopback_url", &self.loopback_url)
            .field("workspace_root", &self.workspace_root)
            .field("host_caps", &self.host_caps)
            .finish_non_exhaustive()
    }
}

// =====================================================================
// ConversationIdHeader — axum extractor for the `x-conversation-id`
// request header. Equivalent in behavior to the inline parser in
// `handlers::extract_conversation_id`; we expose it as a proper
// extractor for any future handler that wants typed-extraction
// ergonomics. The existing handlers stay on the inline parser so the
// per-conversation mutex acquisition order doesn't reshuffle.
// =====================================================================

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;

/// Extractor for the optional `x-conversation-id` request header.
///
/// Wraps `Option<Uuid>` rather than `Uuid` because the MCP manager
/// only sets the header when a conversation context exists
/// (`client/manager.rs:88-93`). Requests without a conversation
/// context — `initialize` during MCP discovery, `tools/list` stateless
/// catalog queries — MUST succeed. Requests that actually need the
/// context (`tools/call`) validate the inner Option themselves and
/// reject with a JSON-RPC error if it's None.
///
/// A malformed (non-UUID) header is still rejected at extractor time
/// with 400 — that's a real client bug, not a missing-context case.
#[derive(Debug, Clone, Copy)]
pub struct ConversationIdHeader(pub Option<Uuid>);

impl<S: Send + Sync> FromRequestParts<S> for ConversationIdHeader {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let raw = match parts.headers.get("x-conversation-id") {
            Some(v) => v,
            None => return Ok(ConversationIdHeader(None)),
        };
        let s = raw
            .to_str()
            .map_err(|_| (StatusCode::BAD_REQUEST, "x-conversation-id is not ASCII"))?;
        let uuid = Uuid::parse_str(s.trim())
            .map_err(|_| (StatusCode::BAD_REQUEST, "x-conversation-id is not a uuid"))?;
        Ok(ConversationIdHeader(Some(uuid)))
    }
}

// =====================================================================
// Tier 1 unit tests — JSON-RPC envelope
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // The JSON-RPC envelope tests (round-trip / default-jsonrpc / string-id /
    // canonical error codes / result-xor-error serialize) moved with the types
    // to `ziee_framework::mcp` (Chunk C1).

    /// Mirrors the per-conversation flavor-lock decision in
    /// `tools/execute.rs::execute_command_with_mounts` (the FIRST call
    /// pins the flavor; a later call in the SAME conversation requesting
    /// a DIFFERENT flavor is detected as a switch and re-pins). This is
    /// the "flavor-switch within a conversation" decision that drives the
    /// install-cache wipe — previously only the wipe primitive
    /// (`version_manager::flavor_switch_wipes_only_caller_conversation`)
    /// was tested, never the lock transition that decides WHEN to wipe.
    ///
    /// Returns `(was_switch, locked_after)` exactly as execute.rs computes
    /// them, so a regression in the entry/or_insert/insert sequence fails
    /// here rather than silently mis-wiping (or never wiping) a real run.
    fn resolve_flavor_lock(conv: Uuid, requested: &str) -> (bool, String) {
        let locked = CONVERSATION_FLAVOR
            .entry(conv)
            .or_insert_with(|| requested.to_string())
            .clone();
        let was_switch = locked != requested;
        if was_switch {
            CONVERSATION_FLAVOR.insert(conv, requested.to_string());
        }
        let after = CONVERSATION_FLAVOR.get(&conv).map(|v| v.clone()).unwrap();
        (was_switch, after)
    }

    #[test]
    fn conversation_flavor_lock_pins_then_switches() {
        let conv_a = Uuid::new_v4();
        let conv_b = Uuid::new_v4();

        // 1) First call in a conversation pins the requested flavor and is
        //    NOT a switch (nothing to wipe yet).
        let (switch1, locked1) = resolve_flavor_lock(conv_a, "minimal");
        assert!(!switch1, "first call must not be a flavor switch");
        assert_eq!(locked1, "minimal");

        // 2) Same flavor again in the same conversation: still no switch.
        let (switch2, locked2) = resolve_flavor_lock(conv_a, "minimal");
        assert!(!switch2, "same-flavor re-request must not switch");
        assert_eq!(locked2, "minimal");

        // 3) Different flavor in the SAME conversation: detected as a
        //    switch, and the lock re-pins to the new flavor so the NEXT
        //    call sees it as the baseline.
        let (switch3, locked3) = resolve_flavor_lock(conv_a, "full");
        assert!(switch3, "different flavor in same conversation must switch");
        assert_eq!(locked3, "full");
        let (switch4, locked4) = resolve_flavor_lock(conv_a, "full");
        assert!(!switch4, "post-switch baseline must be the new flavor");
        assert_eq!(locked4, "full");

        // 4) A DIFFERENT conversation is independent — conv_a's switch to
        //    "full" must not leak into conv_b's first pin.
        let (switch_b, locked_b) = resolve_flavor_lock(conv_b, "minimal");
        assert!(!switch_b, "a different conversation pins independently");
        assert_eq!(locked_b, "minimal");

        // Clean up the process-global map so sibling tests are unaffected.
        CONVERSATION_FLAVOR.remove(&conv_a);
        CONVERSATION_FLAVOR.remove(&conv_b);
    }

    // ─── ConversationIdHeader extractor ──────────────────────────

    fn make_parts(headers: Vec<(&str, &str)>) -> axum::http::request::Parts {
        let mut builder = axum::http::Request::builder().uri("/");
        for (k, v) in headers {
            builder = builder.header(k, v);
        }
        let (parts, _) = builder.body(()).unwrap().into_parts();
        parts
    }

    #[tokio::test]
    async fn conversation_id_header_parses_uuid() {
        let mut parts = make_parts(vec![(
            "x-conversation-id",
            "11111111-2222-3333-4444-555555555555",
        )]);
        let ConversationIdHeader(opt) =
            ConversationIdHeader::from_request_parts(&mut parts, &()).await.unwrap();
        assert_eq!(
            opt.unwrap().to_string(),
            "11111111-2222-3333-4444-555555555555"
        );
    }

    #[tokio::test]
    async fn conversation_id_header_missing_returns_none() {
        // initialize + tools/list calls land WITHOUT a conversation
        // context. The extractor must succeed; per-call methods that
        // need the context will reject downstream.
        let mut parts = make_parts(vec![]);
        let ConversationIdHeader(opt) =
            ConversationIdHeader::from_request_parts(&mut parts, &()).await.unwrap();
        assert!(opt.is_none());
    }

    #[tokio::test]
    async fn conversation_id_header_rejects_garbage() {
        // A malformed header IS a client bug — reject at extractor time.
        let mut parts = make_parts(vec![("x-conversation-id", "not-a-uuid")]);
        let err = ConversationIdHeader::from_request_parts(&mut parts, &())
            .await
            .expect_err("garbage must reject");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

}
