//! The FOUR-part capability manifest (Chunk D design-gate 1).
//!
//! Mode-gating in the app is today FOUR separate, scattered mechanisms that a
//! single per-app manifest keyed by [`DeploymentMode`] replaces:
//!
//! | # | Today (app-side)                                             | Manifest field           |
//! |---|--------------------------------------------------------------|--------------------------|
//! | 1 | backend hard-coded module vec `create_desktop_modules`       | [`backend_modules`]      |
//! | 2 | frontend `CORE_MODULE_BLOCKLIST` Set (`loader.desktop.ts`)    | [`frontend_blocklist`]   |
//! | 3 | scattered `config.<feature>.enabled = true` overrides        | [`config_overrides`]     |
//! | 4 | `setMultiUserMode(false)` (`desktop/ui/main.tsx`)            | [`multi_user`]           |
//!
//! [`backend_modules`]: CapabilityManifest::backend_modules
//! [`frontend_blocklist`]: CapabilityManifest::frontend_blocklist
//! [`config_overrides`]: CapabilityManifest::config_overrides
//! [`multi_user`]: CapabilityManifest::multi_user
//!
//! ## What lives here vs. app-side
//!
//! The manifest **STRUCTURE + mode keying** are reusable and live in this
//! harness crate. The manifest **CONTENTS** (which modules ziee registers on
//! desktop, which frontend module ids ziee's web bundle drops, ziee's exact
//! feature overrides) stay **app-side** — the app constructs one
//! [`CapabilityManifest`] per mode from its own domain vocabulary. This mirrors
//! Chunk D's "stays app-side" list: the `create_desktop_modules` vec, the
//! `CORE_MODULE_BLOCKLIST` contents, the `config.<feature>.enabled` set, and
//! branding all remain in the app; only the *shape* is factored out.
//!
//! ## How the frontend consumes it
//!
//! Parts 2 + 4 are read by the browser (the forked desktop loader). Rather than
//! keep a *second*, hand-forked source of truth (`loader.desktop.ts`), the app
//! serves the manifest's [`FrontendManifest`] projection over a small endpoint;
//! the desktop UI's boot reads it and applies the blocklist + multi-user flag.
//! One manifest, one source of truth, both surfaces.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The deployment mode a [`CapabilityManifest`] is keyed by.
///
/// A single app ships two manifests (one per mode); the harness selects the
/// desktop one, the standalone server entrypoint selects [`Server`].
///
/// [`Server`]: DeploymentMode::Server
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeploymentMode {
    /// Multi-user server deployment (the web app): every capability is
    /// operator-opt-in, users authenticate individually.
    Server,
    /// Single-user desktop deployment (the Tauri app): opt-in capabilities
    /// default on, one auto-provisioned owner, permanent session.
    Desktop,
}

impl DeploymentMode {
    /// The default multi-user posture for the mode (server = multi-user,
    /// desktop = single-user). Callers override via
    /// [`CapabilityManifest::multi_user`] when a deployment is unusual.
    pub fn default_multi_user(self) -> bool {
        matches!(self, DeploymentMode::Server)
    }
}

/// The one per-app, per-mode manifest that replaces the four scattered
/// mode-gating mechanisms (see the module docs).
///
/// Construct one per mode via [`CapabilityManifest::new`] and the builder
/// setters; the harness reads it to drive boot, and the app serves its
/// [`FrontendManifest`] projection to the desktop UI.
#[derive(Clone, Debug)]
pub struct CapabilityManifest {
    mode: DeploymentMode,
    /// Part 1 — the backend modules the app registers for this mode (the
    /// former `create_desktop_modules` vec, expressed as stable module names).
    backend_modules: Vec<String>,
    /// Part 2 — frontend module ids dropped from the web bundle for this mode
    /// (the former `CORE_MODULE_BLOCKLIST` Set).
    frontend_blocklist: Vec<String>,
    /// Part 3 — `config.<feature>.enabled` overrides applied for this mode
    /// (the scattered `sandbox_cfg.enabled = true`, `bio_cfg.enabled = true`,
    /// `web_search_cfg.enabled = true`, … forced-on flags).
    config_overrides: BTreeMap<String, bool>,
    /// Part 4 — whether the deployment is multi-user (the former
    /// `setMultiUserMode(...)` call). Defaults from the mode.
    multi_user: bool,
}

impl CapabilityManifest {
    /// A fresh manifest for `mode` with the mode's default multi-user posture
    /// and empty capability sets (the app fills them in).
    pub fn new(mode: DeploymentMode) -> Self {
        Self {
            mode,
            backend_modules: Vec::new(),
            frontend_blocklist: Vec::new(),
            config_overrides: BTreeMap::new(),
            multi_user: mode.default_multi_user(),
        }
    }

    /// Part 1 — register the backend module names the app enables for this mode.
    pub fn with_backend_modules<I, S>(mut self, modules: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.backend_modules = modules.into_iter().map(Into::into).collect();
        self
    }

    /// Part 2 — the frontend module ids the web bundle drops for this mode.
    pub fn with_frontend_blocklist<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.frontend_blocklist = ids.into_iter().map(Into::into).collect();
        self
    }

    /// Part 3 — force `config.<feature>.enabled = value` for this mode.
    pub fn with_config_override(mut self, feature: impl Into<String>, enabled: bool) -> Self {
        self.config_overrides.insert(feature.into(), enabled);
        self
    }

    /// Part 4 — override the multi-user posture (defaults from the mode).
    pub fn with_multi_user(mut self, multi_user: bool) -> Self {
        self.multi_user = multi_user;
        self
    }

    /// The mode this manifest is keyed by.
    pub fn mode(&self) -> DeploymentMode {
        self.mode
    }

    /// Part 1 — the backend module names to register.
    pub fn backend_modules(&self) -> &[String] {
        &self.backend_modules
    }

    /// Part 2 — the frontend module ids to drop from the web bundle.
    pub fn frontend_blocklist(&self) -> &[String] {
        &self.frontend_blocklist
    }

    /// Part 3 — the `config.<feature>.enabled` overrides.
    pub fn config_overrides(&self) -> &BTreeMap<String, bool> {
        &self.config_overrides
    }

    /// Part 4 — whether the deployment is multi-user.
    pub fn multi_user(&self) -> bool {
        self.multi_user
    }

    /// The forced-on/off state of `feature`, or `None` if this manifest does
    /// not override it (leaving the config's own default in force).
    pub fn config_override(&self, feature: &str) -> Option<bool> {
        self.config_overrides.get(feature).copied()
    }

    /// The browser-facing projection (parts 2 + 4) the app serves to the
    /// desktop UI loader, so the frontend has ONE source of truth instead of a
    /// hand-forked `loader.desktop.ts`.
    pub fn frontend(&self) -> FrontendManifest {
        FrontendManifest {
            mode: self.mode,
            module_blocklist: self.frontend_blocklist.clone(),
            multi_user: self.multi_user,
        }
    }
}

/// The browser-facing slice of a [`CapabilityManifest`] (parts 2 + 4). The app
/// serves this; the desktop UI applies it at boot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrontendManifest {
    /// The deployment mode (informational for the client).
    pub mode: DeploymentMode,
    /// Module ids to drop from the loaded set (was `CORE_MODULE_BLOCKLIST`).
    pub module_blocklist: Vec<String>,
    /// Multi-user flag (was the `setMultiUserMode(...)` argument).
    pub multi_user: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_mode_defaults_to_multi_user() {
        let m = CapabilityManifest::new(DeploymentMode::Server);
        assert!(m.multi_user());
        assert_eq!(m.mode(), DeploymentMode::Server);
    }

    #[test]
    fn desktop_mode_defaults_to_single_user() {
        let m = CapabilityManifest::new(DeploymentMode::Desktop);
        assert!(!m.multi_user());
    }

    #[test]
    fn desktop_manifest_captures_all_four_parts() {
        // A desktop manifest expressed the way ziee-desktop would build it —
        // contents are illustrative (the real vocabulary stays app-side).
        let m = CapabilityManifest::new(DeploymentMode::Desktop)
            .with_backend_modules(["remote_access", "magic_link", "host_mount"])
            .with_frontend_blocklist(["server-update"])
            .with_config_override("code_sandbox", true)
            .with_config_override("bio_mcp", true)
            .with_config_override("web_search", true);

        // (1) backend modules
        assert_eq!(m.backend_modules().len(), 3);
        // (2) frontend blocklist flows into the frontend projection
        assert_eq!(m.frontend().module_blocklist, vec!["server-update"]);
        // (3) config overrides force features on; unset features stay None
        assert_eq!(m.config_override("code_sandbox"), Some(true));
        assert_eq!(m.config_override("memory"), None);
        // (4) single-user posture inherited from the mode, exported to frontend
        assert!(!m.frontend().multi_user);
    }

    #[test]
    fn multi_user_override_wins_over_mode_default() {
        let m = CapabilityManifest::new(DeploymentMode::Desktop).with_multi_user(true);
        assert!(m.multi_user());
    }
}
