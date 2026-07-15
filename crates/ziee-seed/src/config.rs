//! Domain-neutral seed-engine configuration. An app composes this into its own
//! config (e.g. a `seed:` section) and passes it to [`crate::run`].

use serde::Deserialize;

fn default_true() -> bool {
    true
}

/// Deploy-level seed configuration. Deliberately NOT an admin settings-table row: the
/// seed runs at BOOT before the server serves, so a runtime UI toggle can't gate it
/// (chicken-and-egg). Mirrors a deploy config flag (like a feature kill switch), not a
/// runtime settings row.
#[derive(Debug, Clone, Deserialize)]
pub struct SeedConfig {
    /// Master switch. Default ON — the default seed-if-empty path never clobbers, so it
    /// is safe to run on every boot.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Global authoritative reconcile: re-sync declared fields on owned rows AND delete
    /// ledger-owned rows absent from the YAML. Default OFF (opt-in) because it can revert
    /// admin edits. A per-section `mode`/`reset` directive overrides this.
    #[serde(default)]
    pub reconcile: bool,
    /// Operator overlay: a single file OR a directory of `*.yaml`/`*.yml` merged in
    /// lexical order, deep-merged over the app's embedded default by `(section, name)`.
    /// When unset, the engine falls back to the `SEED_FILE` env var.
    #[serde(default)]
    pub overlay_path: Option<String>,
}

impl Default for SeedConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reconcile: false,
            overlay_path: None,
        }
    }
}
