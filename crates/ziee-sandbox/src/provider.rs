//! The three injected seams that keep the sandbox engine build-DB-free and
//! free of any `crate::modules::…` (ziee server) reverse-dependency, plus the
//! vocabulary types they trade in (lifted verbatim out of the retained
//! `runtime_mount` / `runtime_fetch` ziee-server modules, which now re-import
//! them from here).
//!
//! - [`RootfsProvider`] — rootfs fetch + mount + evict + shutdown. The backends
//!   call `state.rootfs.*` instead of the ziee `runtime_mount` / `runtime_fetch`
//!   modules directly. The ziee `ZieeRootfsProvider` delegates back to them.
//! - [`ResourceLimitsProvider`] — the single injected DB read behind
//!   `resource_limits_cache::get`.
//! - [`GuestAgentProvider`] — the embedded guest-agent binary staging (macOS
//!   libkrun bundle + Windows WSL2 agent). The `include_bytes!` bodies must read
//!   the SERVER crate's `CARGO_MANIFEST_DIR`, so `embedded.rs` /
//!   `wsl2_agent_embedded.rs` stay in the ziee server crate; this seam lets the
//!   `#[cfg(macos)]` / `#[cfg(windows)]` backends reach them without naming
//!   `crate::embedded`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use ziee_core::AppError;

use crate::resource_limits::CodeSandboxResourceLimits;

// =====================================================================
// Rootfs-provider vocabulary (moved from `runtime_mount` / `runtime_fetch`)
// =====================================================================

/// What `ensure_rootfs_ready` returns. The caller (`run_in_sandbox`)
/// uses `caps` for build_bwrap_argv and `mount_dir` for the
/// `--ro-bind` source.
///
/// `fetch_info` is populated when THIS call did the auto-fetch; the
/// chat UI uses it to render a "fetched X (Y MB in Z s)" system
/// note. `None` when the flavor's squashfs was already in the cache.
///
/// `artifact_id` identifies the row in `code_sandbox_rootfs_artifacts`
/// that this mount corresponds to. Callers that wish to participate
/// in the drain-on-swap protocol acquire an `InflightGuard` against it
/// via `registry::acquire_inflight(artifact_id, kind)`.
#[derive(Debug, Clone)]
pub struct EnsureOutcome {
    pub caps: std::sync::Arc<crate::types::HardeningCapabilities>,
    pub mount_dir: PathBuf,
    pub fetch_info: Option<FetchOutcome>,
    pub artifact_id: Option<uuid::Uuid>,
}

/// Result of evicting a flavor from the cache.
#[derive(Debug, Clone, Copy)]
pub struct EvictOutcome {
    pub bytes_freed: u64,
    pub was_cached: bool,
}

#[derive(Debug, Clone)]
pub struct FetchProgress {
    pub phase: FetchPhase,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FetchPhase {
    Resolving,
    Downloading,
    VerifyingSha256,
    VerifyingCosign,
    Installing,
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub installed_path: PathBuf,
    pub bytes_downloaded: u64,
    pub duration_ms: u64,
    pub cosign_verified: bool,
    /// Semver string identifying which rootfs release this artifact
    /// belongs to. Surfaced via `fetch_info.version` in the chat UI.
    pub version: String,
    /// PK of the `code_sandbox_rootfs_artifacts` row corresponding to
    /// this fetch. Plumbed through so `runtime_mount` can register the
    /// mount + every caller can `registry::acquire_inflight`
    /// for the drain-on-swap protocol.
    pub artifact_id: uuid::Uuid,
}

/// Packaging variant. The squashfs is the universal artifact
/// (Linux squashfuse + macOS in-guest mount); the `.tar.zst` tarball
/// exists only for Windows `wsl --import` (which can't consume a
/// squashfs). Both are produced from the identical staged tree at
/// release time, so both share the rootfs content but ship in different
/// container formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootfsFormat {
    Squashfs,
    #[allow(dead_code)]
    TarZst,
}

#[derive(Debug, Clone)]
pub enum FetchError {
    /// Stable catch-all surfaced from the version_manager — the inner
    /// message carries the structured error code
    /// (`SANDBOX_ROOTFS_UNAVAILABLE`, `SANDBOX_ROOTFS_VERSION_MISSING`,
    /// …). Other variants were retired with the prior TOML resolver.
    Install(String),
    /// Download failed for network reasons.
    Download(String),
    /// sha256 sidecar disagreed with the downloaded artifact.
    Sha256Mismatch { expected: String, got: String },
    /// cosign keyless verification failed.
    CosignFailed(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Install(e) => write!(f, "{e}"),
            FetchError::Download(e) => write!(f, "download failed: {e}"),
            FetchError::Sha256Mismatch { expected, got } => {
                write!(f, "sha256 mismatch (expected {expected}, got {got})")
            }
            FetchError::CosignFailed(e) => write!(f, "cosign verification failed: {e}"),
        }
    }
}

// =====================================================================
// Seam 1 — RootfsProvider
// =====================================================================

/// Rootfs lifecycle, injected so the engine backends never name the ziee
/// server's `runtime_mount` / `runtime_fetch` modules (which read the DB pin +
/// own the per-flavor squashfuse mount statics). The ziee `ZieeRootfsProvider`
/// delegates each method back to those modules.
#[async_trait]
pub trait RootfsProvider: Send + Sync {
    /// Make the requested flavor's rootfs available (fetch if missing, mount),
    /// returning the full hardening caps + mount dir.
    async fn ensure_rootfs_ready(&self, flavor: &str) -> Result<EnsureOutcome, AppError>;

    /// The per-flavor cache dir (parent of the legacy `current` mount symlink).
    fn cache_dir(&self) -> PathBuf;

    /// Version-aware evict: tear down ONLY the `(version, flavor)` mount +
    /// remove the cached artifact, leaving siblings alone.
    async fn evict_by_version_flavor(
        &self,
        version_cache_dir: &Path,
        version: &str,
        flavor: &str,
    ) -> EvictOutcome;

    /// Resolve + download + verify + install the squashfs for `flavor`
    /// matching the pinned version.
    async fn ensure_fetched(
        &self,
        cache_dir: &Path,
        flavor: &str,
        progress: Box<dyn Fn(FetchProgress) + Send + Sync>,
    ) -> Result<FetchOutcome, FetchError>;

    /// Like [`ensure_fetched`](Self::ensure_fetched) but for a specific packaging
    /// (Windows WSL2 fetches [`RootfsFormat::TarZst`]).
    async fn ensure_fetched_format(
        &self,
        cache_dir: &Path,
        flavor: &str,
        format: RootfsFormat,
        progress: Box<dyn Fn(FetchProgress) + Send + Sync>,
    ) -> Result<FetchOutcome, FetchError>;

    /// Tear down every spawned squashfuse child (server-shutdown path).
    async fn shutdown(&self);
}

// =====================================================================
// Seam 2 — ResourceLimitsProvider
// =====================================================================

/// The single injected DB read behind `resource_limits_cache::get`.
/// `snapshot_or_defaults` / `invalidate` stay byte-identical DB-free.
#[async_trait]
pub trait ResourceLimitsProvider: Send + Sync {
    async fn load_from_db(&self) -> Result<CodeSandboxResourceLimits, AppError>;
}

// =====================================================================
// Seam 3 — GuestAgentProvider
// =====================================================================

/// Where the macOS launcher + guest root land after extraction of the embedded
/// `bundle.tar.zst`. Moved here (out of the ziee server's `embedded.rs`, which
/// keeps the `include_bytes!` body) so the engine's `mac_vm` backend + this
/// trait can name the return shape.
pub struct Extracted {
    pub launcher: PathBuf,
    pub guest_root: PathBuf,
}

/// Embedded guest-agent binary staging. The two `include_bytes!` bodies live in
/// the ziee server crate (`embedded.rs` / `wsl2_agent_embedded.rs`) because they
/// read the SERVER crate's `CARGO_MANIFEST_DIR`; the ziee `ZieeGuestAgentProvider`
/// forwards to them.
///
/// Methods are **sync** (mirroring the sync `embedded::ensure()` /
/// `wsl2_agent_embedded::ensure()` they wrap): the only call sites — mac_vm's
/// `launcher_path`/`guest_root_path` + wsl2's `agent_host_path` — are sync
/// associated functions, so an async seam would ripple through the (Linux-uncompilable)
/// VM backends. They return `&'static` handles, so the caller reaches them via
/// the engine-global `config::get_state()` without a lifetime tie to the state.
pub trait GuestAgentProvider: Send + Sync {
    /// macOS: stage the libkrun bundle (launcher + dylibs + guest root).
    /// Mirrors `embedded::ensure() -> Result<&'static Extracted, String>`.
    fn ensure(&self) -> Result<&'static Extracted, String>;

    /// Windows: stage the `ziee-sandbox-agent` ELF for `wsl --import`.
    /// Mirrors `wsl2_agent_embedded::ensure() -> Result<&'static PathBuf, String>`.
    fn ensure_wsl2(&self) -> Result<&'static PathBuf, String>;
}
