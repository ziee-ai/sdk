use schemars::JsonSchema;
use serde::Serialize;

/// Cached server update-availability status (admin endpoint).
#[derive(Clone, Debug, Serialize, JsonSchema, Default)]
pub struct UpdateStatusResponse {
    /// The running server version (`CARGO_PKG_VERSION`).
    pub current_version: String,
    /// Latest version seen on GitHub, or null if not yet checked / disabled.
    pub latest_version: Option<String>,
    /// True when `latest_version` is newer than `current_version`.
    pub update_available: bool,
    /// GitHub release page for the latest version.
    pub release_url: Option<String>,
    /// Release notes (markdown) for the latest version.
    pub notes: Option<String>,
    /// RFC3339 timestamp of the last successful check, or null if never.
    pub checked_at: Option<String>,
    /// Whether update checks are enabled in config (false → air-gapped or the
    /// embedded desktop server).
    pub enabled: bool,
}
