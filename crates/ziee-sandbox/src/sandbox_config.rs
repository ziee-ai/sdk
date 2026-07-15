//! `CodeSandboxConfig` — the code_sandbox built-in MCP server's YAML config
//! block. Moved here (out of the ziee server's `core::config`) so the engine
//! owns the config value type; the server re-exports it via
//! `pub use ziee_sandbox::sandbox_config::CodeSandboxConfig` so its monolithic
//! `Config { code_sandbox: Option<CodeSandboxConfig> }` field + `resolve_paths`
//! access are unchanged.

use serde::Deserialize;

/// Configuration for the code_sandbox built-in MCP server.
///
/// Disabled by default so dev environments without bwrap / rootfs boot cleanly.
/// Flip `enabled` to true after bwrap is installed and the rootfs is mounted.
#[derive(Debug, Deserialize, Clone)]
pub struct CodeSandboxConfig {
    /// Master switch. When false, the module's `init()` returns early
    /// (no boot probes, no MCP row upsert, no reaper task).
    #[serde(default)]
    pub enabled: bool,
    /// Path to the cached rootfs squashfs files. Default
    /// `<app.data_dir>/sandbox-rootfs/` (filled by `resolve_paths`).
    /// Bind-mounted read-only into every bwrap call.
    #[serde(default)]
    pub rootfs_path: Option<String>,
    /// Per-conversation sandbox workspaces root. Default
    /// `<app.data_dir>/sandboxes/`. Was previously derived ad-hoc in
    /// `code_sandbox/mod.rs` with no override.
    #[serde(default)]
    pub workspace_root: Option<String>,
    /// Delegated cgroup v2 parent. Empty string → rlimits-only mode
    /// (no per-call cgroup scope; rlimits still enforce memory + procs).
    #[serde(default)]
    pub cgroup_parent: String,
    /// When true (default), the FIRST `execute_command` for a flavor
    /// whose rootfs isn't cached yet prompts the user for consent (via
    /// an MCP elicitation) before starting the multi-hundred-MB
    /// download. Set false to always auto-download silently.
    #[serde(default = "default_require_download_consent")]
    pub require_download_consent: bool,
    /// Flavors whose advertised size is below this threshold (MiB) skip
    /// the consent prompt and download silently — so the small
    /// `minimal` rootfs stays frictionless while a large `full` rootfs
    /// always asks. Only consulted when `require_download_consent` is true.
    #[serde(default = "default_auto_download_under_mb")]
    pub auto_download_under_mb: u64,
    /// Audit H-8: refuse to register the sandbox MCP server on Windows when
    /// WSL2 `networkingMode = mirrored` is enabled in `.wslconfig`. In
    /// mirrored mode the Windows host's 127.0.0.1 (Postgres, the Ziee API
    /// itself, browser DevTools, …) is reachable from inside the distro,
    /// and `--share-net` carries that into the sandbox. The previous
    /// behavior was a warn-log only. Operators who genuinely want this
    /// configuration must opt in with `allow_wsl2_mirrored_mode: true`.
    /// No-op on Linux/macOS.
    #[serde(default)]
    #[allow(dead_code)]
    pub allow_wsl2_mirrored_mode: bool,
    /// Audit H-4: refuse to register when the cloud instance metadata
    /// service (169.254.169.254) is reachable from the host AND `--share-net`
    /// would expose it to LLM-generated code. On EC2/GCE/Azure, IMDS hands
    /// out IAM/role credentials that the sandboxed workload can curl + ship
    /// to whatever egress the LLM is told to use. Operators on a cloud host
    /// who genuinely want this configuration (e.g. behind IMDSv2 with a
    /// hop-limit of 1 + sandboxed bash unable to set the v2 token header)
    /// must opt in with `allow_cloud_imds_reachable: true`. No-op on hosts
    /// where IMDS is unreachable (the common dev / on-prem case).
    #[serde(default)]
    pub allow_cloud_imds_reachable: bool,
    /// Public base origin for file download links handed to MCP clients on
    /// a different host (e.g. a reverse-proxy / tunnel URL like
    /// `https://3000--….coder…`). When set, BOTH `get_resource_link`
    /// (user attachments + workspace artifacts) and the MCP artifact-save
    /// pipeline's tool-to-tool download URLs root links here instead of the
    /// loopback origin — see `public_file_origin`, the shared resolver.
    /// The built-in MCP server's own dial URL stays on 127.0.0.1 (see
    /// `loopback_host`); this only affects returned link origins, never the
    /// dial URL. Note: `get_resource_link` additionally requires
    /// `enabled: true` (it reads the initialized sandbox state), whereas the
    /// artifact-save pipeline honors this field regardless of `enabled`.
    /// Empty/unset → loopback behavior: download URLs use the 127.0.0.1
    /// loopback and no longer derive from `server.host`. So an operator who
    /// binds `server.host` to a non-loopback, externally-reachable address
    /// and needs a remote MCP server to fetch artifacts MUST set this.
    #[serde(default)]
    pub public_base_url: Option<String>,
}

impl Default for CodeSandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            rootfs_path: None,
            workspace_root: None,
            cgroup_parent: String::new(),
            require_download_consent: default_require_download_consent(),
            auto_download_under_mb: default_auto_download_under_mb(),
            allow_wsl2_mirrored_mode: false,
            allow_cloud_imds_reachable: false,
            public_base_url: None,
        }
    }
}

impl CodeSandboxConfig {
    /// `rootfs_path` after `Config::resolve_paths` has run. Panics if
    /// called on an unresolved config (programmer error — `load_from`
    /// always resolves).
    pub fn rootfs_path(&self) -> &str {
        self.rootfs_path
            .as_deref()
            .expect("rootfs_path filled by Config::resolve_paths")
    }
    #[allow(dead_code)]
    pub fn workspace_root(&self) -> &str {
        self.workspace_root
            .as_deref()
            .expect("workspace_root filled by Config::resolve_paths")
    }

    /// Origin (`scheme://host[:port]`, no trailing slash, no path) for file
    /// download links handed to MCP clients. Returns `public_base_url` when
    /// it is set + non-empty; otherwise the caller's already-pinned loopback
    /// origin.
    ///
    /// This is the single source of truth for the origin used by BOTH
    /// `get_resource_link` (user attachments + workspace artifacts) and the
    /// MCP artifact-save pipeline's tool-to-tool download URLs. Keeping both
    /// paths on this one helper guarantees the LLM is never handed a
    /// `127.0.0.1` / `0.0.0.0` link for a file when the deployment has a
    /// reachable `public_base_url` configured.
    ///
    /// `loopback_origin` must already be the canonical loopback the caller
    /// derived via `loopback_host` (i.e. `http://127.0.0.1:{port}`) — never
    /// `0.0.0.0`, a wildcard, or the configured bind host, which are not
    /// routable destinations.
    pub fn public_file_origin(&self, loopback_origin: &str) -> String {
        match self.public_base_url.as_deref() {
            Some(base) if !base.trim().is_empty() => base.trim().trim_end_matches('/').to_string(),
            _ => loopback_origin.trim_end_matches('/').to_string(),
        }
    }
}

fn default_require_download_consent() -> bool {
    true
}

fn default_auto_download_under_mb() -> u64 {
    100
}

#[cfg(test)]
mod public_file_origin_tests {
    use super::CodeSandboxConfig;

    fn cfg(public_base_url: Option<&str>) -> CodeSandboxConfig {
        CodeSandboxConfig {
            public_base_url: public_base_url.map(str::to_string),
            ..Default::default()
        }
    }

    const LOOPBACK: &str = "http://127.0.0.1:8080";

    #[test]
    fn uses_public_base_url_when_set() {
        let c = cfg(Some("https://tunnel.example.com"));
        assert_eq!(c.public_file_origin(LOOPBACK), "https://tunnel.example.com");
    }

    #[test]
    fn trims_trailing_slash_and_surrounding_whitespace() {
        let c = cfg(Some("  https://tunnel.example.com/  "));
        assert_eq!(c.public_file_origin(LOOPBACK), "https://tunnel.example.com");
    }

    #[test]
    fn falls_back_to_loopback_when_unset() {
        let c = cfg(None);
        assert_eq!(c.public_file_origin(LOOPBACK), "http://127.0.0.1:8080");
    }

    #[test]
    fn falls_back_when_empty_or_whitespace() {
        assert_eq!(cfg(Some("")).public_file_origin(LOOPBACK), LOOPBACK);
        assert_eq!(cfg(Some("   ")).public_file_origin(LOOPBACK), LOOPBACK);
    }

    #[test]
    fn never_emits_wildcard_when_caller_passes_pinned_loopback() {
        // The helper trusts the caller's pinned loopback (always 127.0.0.1 via
        // loopback_host). Regardless of public_base_url being absent, the
        // result must never carry a wildcard / unroutable bind address.
        let c = cfg(None);
        let origin = c.public_file_origin("http://127.0.0.1:9000");
        assert!(origin.starts_with("http://127.0.0.1"), "origin: {origin}");
        assert!(!origin.contains("0.0.0.0"), "origin: {origin}");
    }
}
