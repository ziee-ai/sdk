//! Workflow bundle staging into the per-conversation sandbox workspace.
//!
//! Per plan §11 + §4.5 staging contract: the workflow runner copies a
//! bundle's files (or a per-step `artifacts/` dir) onto the host's
//! existing per-conversation workspace under a named subdir, and the
//! sandbox call subsequently includes that subdir on its mount tree
//! (read-only for bundles, read-write for per-step artifact dirs).
//!
//! ### Scope of this phase (B2)
//! - Host-side staging (recursive `cp -R src → workspace/<subdir>/`).
//! - The `StageMode` enum that the runner records and that the bwrap
//!   layer will switch on in B4.
//! - A small `StagedMount` value that B4's runner threads into
//!   `HardeningArgvParams.extra_ro_binds` (read-only mode) or into a
//!   future RW-bind helper (read-write mode).
//!
//! ### Deferred to B4 (clearly marked here)
//! The plan §11 also calls for an `--bind` (read-write) mount path on
//! the existing argv builder. The current `HardeningArgvParams` only
//! carries `extra_ro_binds` (`--ro-bind-try`); adding a parallel
//! `extra_rw_binds` field affects the EXACT bwrap argv that ships in
//! production today (touching one of the most security-sensitive code
//! paths in the codebase). Doing it as part of B2 — where no caller
//! exists yet to validate the new field actually round-trips — risks a
//! regression with no observable behavior change to test against.
//!
//! B4 wires the bwrap-side extension at the moment it first acquires a
//! real caller (the SandboxDispatcher), so the change is tested
//! end-to-end the moment it lands.
//!
//! For B2 the security guarantees of existing `execute_command` calls
//! are unchanged — this module is purely a host-side prep layer.

use std::fs;
use std::path::{Path, PathBuf};

use ziee_core::AppError;
use crate::types::SandboxContext;

/// How a staged subdir is to be exposed to the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageMode {
    /// `--ro-bind <host_src> /home/sandboxuser/<subdir>/`. Used for the
    /// whole workflow bundle (workflow.yaml, scripts/, references/,
    /// outputs/).
    ReadOnly,
    /// `--bind <host_src> /home/sandboxuser/<subdir>/`. Used for
    /// per-step `artifacts/<step_id>/` so a sandbox script can WRITE
    /// files there for post-collection by the runner.
    ReadWrite,
}

/// One staged mount the runner asks the sandbox layer to wire up at
/// dispatch time. The host path is the on-disk dir under the
/// conversation workspace; the sandbox path is the conventional
/// `/home/sandboxuser/<subdir>/`.
#[derive(Debug, Clone)]
pub struct StagedMount {
    pub mode: StageMode,
    pub host_path: PathBuf,
    pub sandbox_path: String,
}

/// Stage one subdir of the conversation workspace by recursively
/// copying `src_path` into `<ctx.workspace>/<subdir_name>/`. Returns a
/// `StagedMount` the B4 runner will hand to the sandbox layer.
///
/// Why copy instead of bind-mount the original: bwrap can only
/// `--ro-bind` paths the host can reach unprivileged; copying into the
/// per-conversation workspace keeps the runner's whole input set on
/// the same filesystem as the workspace and keeps the security
/// boundary at the workspace root (the existing
/// `workspace_reaper` + per-conversation chmod machinery already
/// covers it).
///
/// NOTE: Written for the B2→B4 handoff. Currently only defines the
/// staging contract; the caller (B4 `SandboxDispatcher`) is deferred.
#[allow(dead_code)]
pub fn stage_workspace_subdir(
    ctx: &SandboxContext,
    subdir_name: &str,
    src_path: &Path,
    mode: StageMode,
) -> Result<StagedMount, AppError> {
    if subdir_name.is_empty()
        || subdir_name.contains('/')
        || subdir_name.contains('\\')
        || subdir_name.contains("..")
        || subdir_name.starts_with('.')
    {
        return Err(AppError::bad_request(
            "SANDBOX_STAGE_BAD_SUBDIR",
            format!(
                "stage_workspace_subdir: subdir_name '{subdir_name}' must be a simple basename"
            ),
        ));
    }
    if !src_path.exists() {
        return Err(AppError::internal_error(format!(
            "stage_workspace_subdir: src {} does not exist",
            src_path.display()
        )));
    }

    let dest = ctx.workspace.join(subdir_name);
    // Wipe any prior content so a re-stage (e.g. a workflow re-run
    // under the same run_id) doesn't see stale files from the prior
    // invocation.
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| {
            AppError::internal_error(format!(
                "stage_workspace_subdir: clear {}: {}",
                dest.display(),
                e
            ))
        })?;
    }
    fs::create_dir_all(&dest).map_err(|e| {
        AppError::internal_error(format!(
            "stage_workspace_subdir: mkdir {}: {}",
            dest.display(),
            e
        ))
    })?;
    copy_dir_recursive(src_path, &dest)?;

    Ok(StagedMount {
        mode,
        host_path: dest,
        sandbox_path: format!("/home/sandboxuser/{subdir_name}"),
    })
}

/// Plain recursive copy. Rejects symlinks defensively (defense-in-depth
/// behind `hub::bundle`'s extractor, which already rejects them at the
/// tar layer).
///
/// NOTE: Only reachable from `stage_workspace_subdir` (same B2 deferred
/// phase); once that caller is wired, remove this allow.
#[allow(dead_code)]
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(src).map_err(|e| {
        AppError::internal_error(format!(
            "stage_workspace_subdir: stat {}: {}",
            src.display(),
            e
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(AppError::bad_request(
            "SANDBOX_STAGE_SYMLINK",
            format!(
                "stage_workspace_subdir: symlinks are not staged ({})",
                src.display()
            ),
        ));
    }
    if metadata.is_dir() {
        fs::create_dir_all(dst).map_err(|e| {
            AppError::internal_error(format!(
                "stage_workspace_subdir: mkdir {}: {}",
                dst.display(),
                e
            ))
        })?;
        for entry in fs::read_dir(src).map_err(|e| {
            AppError::internal_error(format!(
                "stage_workspace_subdir: read_dir {}: {}",
                src.display(),
                e
            ))
        })? {
            let entry = entry.map_err(|e| {
                AppError::internal_error(format!(
                    "stage_workspace_subdir: entry: {e}"
                ))
            })?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            copy_dir_recursive(&from, &to)?;
        }
    } else if metadata.is_file() {
        fs::copy(src, dst).map_err(|e| {
            AppError::internal_error(format!(
                "stage_workspace_subdir: copy {} -> {}: {}",
                src.display(),
                dst.display(),
                e
            ))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Preserve the source mode (the bundle extractor already
            // applied per-kind mode policy — workflows preserve exec,
            // skills strip exec).
            let _ = fs::set_permissions(
                dst,
                fs::Permissions::from_mode(metadata.permissions().mode()),
            );
        }
    }
    // Anything else (devices, sockets, FIFOs) is silently skipped —
    // they can't appear in a bundle-extracted dir anyway.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;
    use uuid::Uuid;

    fn synth_ctx(workspace: PathBuf) -> SandboxContext {
        SandboxContext {
            conversation_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            workspace,
            files: Arc::new(Vec::new()),
            extra_ro_binds: Vec::new(),
        }
    }

    #[test]
    fn stages_a_dir_recursively() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let src = tmp.path().join("bundle");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        fs::write(src.join("sub/b.txt"), b"world").unwrap();

        let ctx = synth_ctx(workspace.clone());
        let mount =
            stage_workspace_subdir(&ctx, "workflow", &src, StageMode::ReadOnly).unwrap();

        assert_eq!(mount.mode, StageMode::ReadOnly);
        assert!(mount.host_path.starts_with(&workspace));
        assert!(workspace.join("workflow/a.txt").exists());
        assert!(workspace.join("workflow/sub/b.txt").exists());
        assert_eq!(mount.sandbox_path, "/home/sandboxuser/workflow");
    }

    #[test]
    fn rejects_unsafe_subdir_names() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let src = tmp.path().join("bundle");
        fs::create_dir_all(&src).unwrap();
        let ctx = synth_ctx(workspace);
        for bad in &["", "../etc", "a/b", "a\\b", ".hidden", "..", "a/../b"] {
            let err = stage_workspace_subdir(&ctx, bad, &src, StageMode::ReadOnly).unwrap_err();
            assert!(
                err.to_string().contains("basename") || err.to_string().contains("subdir"),
                "expected basename rejection for {bad:?}, got {err}"
            );
        }
    }

    #[test]
    fn restages_after_prior_content() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let src1 = tmp.path().join("bundle1");
        fs::create_dir_all(&src1).unwrap();
        fs::write(src1.join("a.txt"), b"first").unwrap();
        let src2 = tmp.path().join("bundle2");
        fs::create_dir_all(&src2).unwrap();
        fs::write(src2.join("b.txt"), b"second").unwrap();

        let ctx = synth_ctx(workspace.clone());
        stage_workspace_subdir(&ctx, "workflow", &src1, StageMode::ReadOnly).unwrap();
        assert!(workspace.join("workflow/a.txt").exists());
        stage_workspace_subdir(&ctx, "workflow", &src2, StageMode::ReadOnly).unwrap();
        // Re-stage wiped the prior content.
        assert!(!workspace.join("workflow/a.txt").exists());
        assert!(workspace.join("workflow/b.txt").exists());
    }
}
