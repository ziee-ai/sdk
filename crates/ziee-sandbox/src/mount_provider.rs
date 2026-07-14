//! Generic sandbox mount-provider seam (feature #3, Part B0).
//!
//! `execute_command` runs inside the server, but features like desktop
//! host-folder mounting need to contribute extra bind mounts WITHOUT the
//! `code_sandbox` core knowing anything about them. This module is the
//! feature-agnostic seam: any module — in-crate, or (the whole point) the
//! **desktop crate** that embeds this server as a lib — registers a
//! [`SandboxMountProvider`] at boot via [`register_sandbox_mount_provider`].
//! `execute_command` then collects every provider's mounts, applies a generic
//! sanity guard, and threads them through the existing
//! [`StagedMount`] → `build_bwrap_argv` bind plumbing.
//!
//! Registration is a **runtime handoff** (mirrors the `additional_handlers`
//! arg of `start_server_with_routes`), NOT a link-time `linkme` slice: the
//! desktop crate is a separate binary that links this server as a library, and
//! the proven desktop→server injection idiom in this codebase is explicit
//! runtime registration.
//!
//! There is a SINGLE mount descriptor ([`MountSpec`] is a type alias over
//! [`StagedMount`]) and a single bind path, so provider mounts and the
//! workflow runner's staged mounts both flow through the same argv builder.

use std::sync::{Arc, OnceLock, RwLock};

use async_trait::async_trait;

use ziee_core::AppError;
use crate::types::SandboxContext;
use crate::workflow_staging::StagedMount;

/// One mount a provider asks the sandbox to bind for a given execution.
///
/// Alias over [`StagedMount`] on purpose — provider mounts and workflow staged
/// mounts share one descriptor and one bind path. `host_path` is the path on
/// the host where the server runs (the per-backend layer translates it to a
/// guest-reachable source on macOS/WSL2); `sandbox_path` is the in-sandbox
/// target (host mounts use `/mnt/<full host path>`); `mode` picks
/// `--ro-bind` vs `--bind`.
pub type MountSpec = StagedMount;

/// A source of extra sandbox mounts, resolved per `execute_command`.
#[async_trait]
pub trait SandboxMountProvider: Send + Sync {
    /// Mounts to apply for `ctx`. An empty vec (or an `Err`) contributes
    /// nothing and the command still runs.
    async fn mounts_for(&self, ctx: &SandboxContext) -> Result<Vec<MountSpec>, AppError>;
}

fn registry() -> &'static RwLock<Vec<Arc<dyn SandboxMountProvider>>> {
    static REG: OnceLock<RwLock<Vec<Arc<dyn SandboxMountProvider>>>> = OnceLock::new();
    REG.get_or_init(|| RwLock::new(Vec::new()))
}

/// Register a mount provider. Call once at boot, before serving — typically
/// from the desktop crate's `host_mount` module. With no provider registered
/// (standalone/remote-web server) the seam is inert and `execute_command`
/// behaves exactly as before.
///
/// NOTE: Currently only the desktop crate registers a provider. The
/// standalone server build never calls this, so mark it allowed here.
#[allow(dead_code)]
pub fn register_sandbox_mount_provider(provider: Arc<dyn SandboxMountProvider>) {
    registry()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .push(provider);
}

/// Whether any provider is registered — the "host mounts are possible on this
/// deployment" capability (desktop registers one; remote web does not).
///
/// NOTE: Same status as `register_sandbox_mount_provider` — only the
/// desktop crate calls this. Standalone build never reaches it.
#[allow(dead_code)]
pub fn has_providers() -> bool {
    !registry()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty()
}

/// Collect mounts from every provider for `ctx`, then apply the generic
/// server-side sanity guard. Returns `(applied, notes)` — `notes` explains any
/// spec that was dropped (missing source, protected target) so the caller can
/// surface them to the model.
pub async fn collect_and_sanitize(ctx: &SandboxContext) -> (Vec<MountSpec>, Vec<String>) {
    // Clone the Arcs out under the lock; never hold a std guard across await.
    let providers: Vec<Arc<dyn SandboxMountProvider>> =
        registry().read().unwrap_or_else(|e| e.into_inner()).clone();
    let mut raw = Vec::new();
    for p in providers {
        match p.mounts_for(ctx).await {
            Ok(mut specs) => raw.append(&mut specs),
            Err(e) => {
                tracing::warn!(error = %e, "sandbox mount provider errored; skipping its mounts")
            }
        }
    }
    sanitize(raw)
}

/// Defense-in-depth guard applied to EVERY provider mount, independent of any
/// per-feature policy the provider already enforced. A buggy or hostile
/// provider must not be able to shadow the rootfs, `/proc`, or the workspace
/// home, nor bind a source that doesn't exist on the host.
fn sanitize(specs: Vec<MountSpec>) -> (Vec<MountSpec>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for m in specs {
        if let Err(reason) = check_target(&m.sandbox_path) {
            notes.push(format!("mount '{}' rejected: {reason}", m.sandbox_path));
            continue;
        }
        if !m.host_path.exists() {
            notes.push(format!(
                "mounted folder '{}' is unavailable and was skipped",
                m.sandbox_path
            ));
            continue;
        }
        out.push(m);
    }
    (out, notes)
}

/// Protected in-sandbox prefixes a provider mount may never land on.
const PROTECTED_TARGETS: &[&str] = &[
    "/usr", "/etc", "/bin", "/sbin", "/lib", "/lib64", "/proc", "/dev", "/sys", "/var", "/root",
    "/run", "/tmp",
];

fn check_target(p: &str) -> Result<(), String> {
    if !p.starts_with('/') {
        return Err("sandbox path must be absolute".into());
    }
    if p.contains("..") {
        return Err("sandbox path must not contain '..'".into());
    }
    // The home/workspace roots themselves are off-limits (binding over them
    // would clobber the writable workspace). Subpaths under them are allowed
    // (the workflow runner uses /home/sandboxuser/<subdir>).
    if p == "/" || p == "/home" || p == "/home/sandboxuser" {
        return Err("sandbox path collides with a reserved mount point".into());
    }
    for prot in PROTECTED_TARGETS {
        if p == *prot || p.starts_with(&format!("{prot}/")) {
            return Err(format!("sandbox path is under protected {prot}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow_staging::StageMode;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn spec(host: PathBuf, sandbox: &str) -> MountSpec {
        MountSpec {
            mode: StageMode::ReadOnly,
            host_path: host,
            sandbox_path: sandbox.to_string(),
        }
    }

    #[test]
    fn rejects_protected_and_relative_targets() {
        for bad in [
            "/usr", "/usr/bin", "/etc/passwd", "/proc/1", "/dev/null", "/", "/home",
            "/home/sandboxuser", "relative/path", "/mnt/../etc",
        ] {
            assert!(
                check_target(bad).is_err(),
                "expected {bad} to be rejected as a mount target"
            );
        }
    }

    #[test]
    fn allows_mnt_and_workspace_subdirs() {
        for ok in ["/mnt/Users/me/data", "/mnt/C/data/x", "/home/sandboxuser/workflow"] {
            assert!(check_target(ok).is_ok(), "expected {ok} to be an allowed target");
        }
    }

    #[test]
    fn sanitize_drops_missing_source_with_note() {
        let dir = tempdir().unwrap();
        let present = dir.path().join("data");
        std::fs::create_dir_all(&present).unwrap();

        let (kept, notes) = sanitize(vec![
            spec(present.clone(), "/mnt/data"),
            spec(dir.path().join("gone"), "/mnt/gone"),
            spec(present.clone(), "/usr"), // protected target
        ]);

        assert_eq!(kept.len(), 1, "only the present, valid-target mount survives");
        assert_eq!(kept[0].sandbox_path, "/mnt/data");
        assert_eq!(notes.len(), 2, "one note for missing source, one for protected target");
        assert!(notes.iter().any(|n| n.contains("unavailable")));
        assert!(notes.iter().any(|n| n.contains("rejected")));
    }
}
