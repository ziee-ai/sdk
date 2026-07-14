//! In-memory mount registry + install-cache wipe primitives — the DB-FREE tail
//! carved out of the ziee server's `version_manager`. The DB half (GitHub
//! releases, pins, artifact rows) stays in the server crate and re-imports this
//! module's types/functions (via `ziee_sandbox::registry`), so the swap-drain +
//! delete-artifact paths keep operating on the SAME process-global registry.
//!
//! Nothing here touches the DB (`sqlx::query`) or names a ziee `crate::modules::…`
//! symbol — verified by the build-DB-free grep gate.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use uuid::Uuid;

/// Subdirs that get wiped on a **major** version bump (Trigger A) or
/// on a per-conversation **flavor switch** (Trigger B).
///
/// Curated to exactly the package-manager install targets where ABI
/// mismatches across rootfs majors crash (Python wheels baked against
/// the old glibc/Python ABI, node-native modules, cargo binaries, R
/// libraries). User-generated files (`*.py`, `*.csv`, `plot.png`,
/// virtualenvs under arbitrary names, etc.) are deliberately
/// preserved.
pub const WIPE_ON_MAJOR_BUMP: &[&str] = &[
    ".local",        // pip --user, npm prefix, cargo install --root binaries
    ".cache",        // pip cache, uv cache, hf cache, build caches
    ".npm",          // npm install scratch
    ".npm-global",   // npm -g
    ".cargo",        // cargo registry + installed binaries
    ".rustup",       // rust toolchains
    ".pyenv",        // pyenv shims (if anyone installs into HOME)
    "node_modules",  // local node deps (top-level only — don't recursively walk for nested ones)
];

/// Sentinel filename dropped at the workspace root after a major-bump
/// or flavor-switch wipe. The next `execute_command` reads + unlinks
/// it and prepends a system note to the tool result.
pub const SENTINEL_ROOTFS_UPGRADED: &str = ".rootfs-upgraded";

/// Sentinel filename dropped after a per-conversation flavor-switch
/// wipe (narrower message than the rootfs-upgrade one).
pub const SENTINEL_FLAVOR_CHANGED: &str = ".flavor-changed";

/// Sentinel payload — written as JSON for forward extensibility.
/// Only the version manager's wipe walker + the chat-extension's
/// sentinel consumer ever touch this.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WipeSentinel {
    pub old: String,
    pub new: String,
    pub at: chrono::DateTime<chrono::Utc>,
}

/// One live mount + its inflight counters. The version manager holds
/// these in a static map keyed by `artifact_id`; the per-backend
/// `evict_artifact` calls operate against the `mount_dir`.
pub struct MountedArtifact {
    pub artifact_id: Uuid,
    pub version: String,
    pub arch: String,
    pub flavor: String,
    pub mount_dir: PathBuf,
    inflight_exec: AtomicUsize,
    inflight_mcp: AtomicUsize,
    /// Notified whenever `inflight_exec + inflight_mcp` changes.
    /// Drain tasks `notified().await` until both counters read zero.
    drained: Notify,
}

impl MountedArtifact {
    /// Live count (exec + MCP). Sequentially-consistent so a drain
    /// task that wakes on `notified()` sees the right value.
    pub fn inflight(&self) -> usize {
        self.inflight_exec.load(Ordering::SeqCst)
            + self.inflight_mcp.load(Ordering::SeqCst)
    }

    /// Per-class breakdown for the admin UI's "draining" row chip.
    pub fn inflight_breakdown(&self) -> (usize, usize) {
        (
            self.inflight_exec.load(Ordering::SeqCst),
            self.inflight_mcp.load(Ordering::SeqCst),
        )
    }

    /// Wait on the `drained` Notify until BOTH inflight counters read
    /// zero. Drain tasks `await` this; in-flight execs + MCP transports
    /// just need to `drop` their guards (which calls `notify_waiters`)
    /// and the drain task wakes naturally.
    pub async fn wait_until_drained(&self) {
        loop {
            if self.inflight() == 0 {
                return;
            }
            // Subscribe BEFORE the recheck so we never miss the wake.
            let waker = self.drained.notified();
            if self.inflight() == 0 {
                return;
            }
            waker.await;
        }
    }
}

/// Class of usage the inflight guard represents. Tracked separately so
/// the admin UI can show "5 execs + 1 MCP server are pinning v0.1.0".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflightKind {
    Exec,
    Mcp,
}

/// RAII guard: increment on construction, decrement + notify on drop.
/// `sandbox::run_in_sandbox` holds one for the exec; `mcp_spawn`'s
/// `McpSandboxTransport` holds one for the MCP server's lifetime.
pub struct InflightGuard {
    artifact: Arc<MountedArtifact>,
    kind: InflightKind,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        let counter = match self.kind {
            InflightKind::Exec => &self.artifact.inflight_exec,
            InflightKind::Mcp => &self.artifact.inflight_mcp,
        };
        // Decrement THIS class, then only notify drain waiters when
        // BOTH classes have hit zero. Audit B6: notifying on every
        // decrement made drain task spin uselessly (it'd wake, see
        // inflight() > 0, and loop). The drain loop's secondary
        // re-check (`if artifact.inflight() == 0` after notify) means
        // a spurious wake is correctness-safe but pure overhead.
        counter.fetch_sub(1, Ordering::SeqCst);
        if self.artifact.inflight() == 0 {
            self.artifact.drained.notify_waiters();
        }
    }
}

/// In-memory registry of live mounts. Keyed by artifact_id so a
/// per-conversation exec can look up its mount in O(1) and the
/// pin-swap drain task can iterate every stale-version entry.
pub static MOUNTED_ARTIFACTS: once_cell::sync::Lazy<
    dashmap::DashMap<Uuid, Arc<MountedArtifact>>,
> = once_cell::sync::Lazy::new(dashmap::DashMap::new);

/// Audit B2: dedup drain tasks. If `set_pin_with_drain` is called
/// twice in quick succession (rapid-fire admin clicks; two admin
/// sessions concurrently flipping the pin) we'd otherwise spawn two
/// drain tasks for the same artifact_id, both racing on
/// `evict_artifact`, `MOUNTED_ARTIFACTS.remove`, and the wipe walker.
/// The set is checked + populated atomically via `DashSet::insert` so
/// only the first caller spawns the task.
pub static DRAINING_ARTIFACTS: once_cell::sync::Lazy<dashmap::DashSet<Uuid>> =
    once_cell::sync::Lazy::new(dashmap::DashSet::new);

/// Register (or refresh) the in-memory tracking for an artifact that
/// was just mounted. Idempotent: a second call with the same
/// `artifact_id` returns the existing `Arc<MountedArtifact>` so
/// inflight counters carry across a re-mount.
pub fn register_mount(
    artifact_id: Uuid,
    version: &str,
    arch: &str,
    flavor: &str,
    mount_dir: PathBuf,
) -> Arc<MountedArtifact> {
    MOUNTED_ARTIFACTS
        .entry(artifact_id)
        .or_insert_with(|| {
            Arc::new(MountedArtifact {
                artifact_id,
                version: version.to_string(),
                arch: arch.to_string(),
                flavor: flavor.to_string(),
                mount_dir,
                inflight_exec: AtomicUsize::new(0),
                inflight_mcp: AtomicUsize::new(0),
                drained: Notify::new(),
            })
        })
        .clone()
}

/// Take an inflight guard against an already-registered artifact.
/// Caller MUST hold the guard for the entirety of the use (exec
/// duration / MCP transport lifetime). Returns `None` if the artifact
/// isn't in the registry — caller should treat that as "no mount yet"
/// (e.g. a stray call before `runtime_mount::ensure_rootfs_ready`).
pub fn acquire_inflight(artifact_id: Uuid, kind: InflightKind) -> Option<InflightGuard> {
    let artifact = MOUNTED_ARTIFACTS.get(&artifact_id)?.value().clone();
    let counter = match kind {
        InflightKind::Exec => &artifact.inflight_exec,
        InflightKind::Mcp => &artifact.inflight_mcp,
    };
    // Increment ONLY. The drain task waits for inflight == 0; an
    // increment cannot make that condition true, so notifying here
    // (audit B7) only causes the drain loop to wake and immediately
    // sleep again — pointless wakeup on every exec.
    counter.fetch_add(1, Ordering::SeqCst);
    Some(InflightGuard { artifact, kind })
}

/// Look up an already-registered artifact by id (used by drain tasks).
#[allow(dead_code)]
pub fn mounted_artifact(id: Uuid) -> Option<Arc<MountedArtifact>> {
    MOUNTED_ARTIFACTS.get(&id).map(|e| e.value().clone())
}

/// Snapshot of every live mount — read by the admin UI's "draining"
/// row chips. Cheap to call (clones the `Arc`s, not the structs).
pub fn list_mounted_artifacts() -> Vec<Arc<MountedArtifact>> {
    MOUNTED_ARTIFACTS.iter().map(|e| e.value().clone()).collect()
}

/// What the wipe walker did. Surfaced via the tracing log for
/// post-hoc admin visibility (the actual per-path detail is too
/// noisy for a single log line). Callers outside the version_manager
/// only care about the structured tracing fields, not the strict-type.
#[derive(Debug, Default, Clone)]
pub struct WipeResult {
    pub conversation_dirs: usize,
    pub mcp_server_dirs: usize,
    pub subdirs_removed: usize,
}

/// Walk a workspace_root and `rm -rf` the curated install-cache
/// subdirs inside every per-conversation and per-MCP-server workspace
/// directory. Drops a `.rootfs-upgraded` sentinel in each affected
/// workspace so the next `execute_command` (or next MCP tool call)
/// can prepend a system note to its tool result.
///
/// Skips `attachments/` and `identity/` (shared-state dirs that are
/// neither per-conversation nor per-MCP-server).
pub fn wipe_install_caches_in_root(
    workspace_root: &std::path::Path,
    sentinel: &WipeSentinel,
) -> WipeResult {
    let mut result = WipeResult::default();
    if !workspace_root.is_dir() {
        return result;
    }
    let sentinel_json = serde_json::to_string(sentinel).unwrap_or_default();

    // Layer 1: per-conversation dirs (children of workspace_root) +
    //          the `mcp/` subtree.
    let entries = match std::fs::read_dir(workspace_root) {
        Ok(e) => e,
        Err(_) => return result,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Reject symlinks at the workspace_root level — an operator (or
        // attacker with workspace-write) could plant `<wr>/00000000-...
        // -000evil` as a symlink to `/etc` and the walker would
        // recurse + wipe inside the symlink target. Audit B13/B14.
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        // Layer 2: MCP per-server dirs under `<workspace_root>/mcp/`.
        if name == "mcp" {
            let mcp_dirs = match std::fs::read_dir(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            for mcp_entry in mcp_dirs.flatten() {
                let mcp_path = mcp_entry.path();
                let mcp_meta = match std::fs::symlink_metadata(&mcp_path) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if mcp_meta.file_type().is_symlink() || !mcp_meta.is_dir() {
                    continue;
                }
                // Require the MCP server dir name to parse as a Uuid
                // — server IDs are deterministic v5 / v4 Uuids, and
                // anything else is operator-created garbage we should
                // not recurse into.
                let mcp_name = match mcp_path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => continue,
                };
                if Uuid::parse_str(mcp_name).is_err() {
                    continue;
                }
                let n = wipe_subdirs_in(&mcp_path, &sentinel_json);
                result.mcp_server_dirs += 1;
                result.subdirs_removed += n;
            }
            continue;
        }

        // Skip shared subsystem dirs (not per-conversation):
        //   `attachments/` is shared staging for bind-mounted user
        //   attachments; `identity/` is the shared synthetic
        //   passwd/group.
        if name == "attachments" || name == "identity" {
            continue;
        }

        // Per-conversation dir names MUST be valid Uuids. Audit B14:
        // without this an operator-planted `<wr>/etc-symlink-target`
        // would be treated as a conv dir and recursed into.
        if Uuid::parse_str(name).is_err() {
            continue;
        }

        let n = wipe_subdirs_in(&path, &sentinel_json);
        result.conversation_dirs += 1;
        result.subdirs_removed += n;
    }
    result
}

/// Per-workspace wipe primitive: `rm -rf` each subdir in
/// `WIPE_ON_MAJOR_BUMP` that exists, then drop a `.rootfs-upgraded`
/// sentinel. Returns the count of subdirs that were actually removed.
fn wipe_subdirs_in(workspace_dir: &std::path::Path, sentinel_json: &str) -> usize {
    let mut removed = 0;
    for sub in WIPE_ON_MAJOR_BUMP {
        let target = workspace_dir.join(sub);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => {
                let r = if target.is_dir() {
                    std::fs::remove_dir_all(&target)
                } else {
                    std::fs::remove_file(&target)
                };
                if r.is_ok() {
                    removed += 1;
                } else if let Err(e) = r {
                    tracing::warn!(
                        path = %target.display(),
                        "workspace_cleanup: failed to remove {sub}: {e}"
                    );
                }
            }
            Err(_) => continue, // missing — fine
        }
    }
    // Drop the sentinel (best-effort).
    let sentinel_path = workspace_dir.join(SENTINEL_ROOTFS_UPGRADED);
    if let Err(e) = std::fs::write(&sentinel_path, sentinel_json) {
        tracing::warn!(
            path = %sentinel_path.display(),
            "workspace_cleanup: failed to drop sentinel: {e}"
        );
    }
    removed
}

/// Per-conversation flavor-switch wipe (Trigger B). Called
/// synchronously from `tools/execute.rs` when the LLM changes the
/// flavor mid-conversation. Wipes only THIS one workspace dir's
/// install-cache subdirs and drops a `.flavor-changed` sentinel.
pub fn wipe_install_caches_for_conversation(
    workspace_dir: &std::path::Path,
    old_flavor: &str,
    new_flavor: &str,
) -> WipeResult {
    let mut result = WipeResult::default();
    if !workspace_dir.is_dir() {
        return result;
    }
    let sentinel = WipeSentinel {
        old: old_flavor.to_string(),
        new: new_flavor.to_string(),
        at: chrono::Utc::now(),
    };
    let sentinel_json = serde_json::to_string(&sentinel).unwrap_or_default();
    let n = wipe_subdirs_in(workspace_dir, &sentinel_json);
    // Overwrite the sentinel name to the flavor-specific one (the
    // helper drops a `.rootfs-upgraded`; rename to
    // `.flavor-changed` for this trigger).
    let _ = std::fs::rename(
        workspace_dir.join(SENTINEL_ROOTFS_UPGRADED),
        workspace_dir.join(SENTINEL_FLAVOR_CHANGED),
    );
    result.conversation_dirs = 1;
    result.subdirs_removed = n;
    result
}

/// Read + unlink the most-recent wipe sentinel in `workspace_dir`,
/// formatted as a human-readable system-note string suitable for
/// prepending to the tool result. Returns `None` if no sentinel is
/// present.
///
/// Looks for `.rootfs-upgraded` first (major-bump), then
/// `.flavor-changed` (per-conversation flavor switch). Both are
/// removed after reading so the next call doesn't re-prepend the
/// same message.
pub fn consume_workspace_sentinel(workspace_dir: &std::path::Path) -> Option<String> {
    for (filename, is_major) in [
        (SENTINEL_ROOTFS_UPGRADED, true),
        (SENTINEL_FLAVOR_CHANGED, false),
    ] {
        let path = workspace_dir.join(filename);
        let body = match std::fs::read_to_string(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let _ = std::fs::remove_file(&path);
        let sentinel: WipeSentinel = match serde_json::from_str(&body) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let msg = if is_major {
            format!(
                "Sandbox upgraded from v{} to v{} (major bump). \
                 Package-manager caches (.local, .cache, .npm, ...) were cleared; \
                 reinstall (pip / npm / ...) anything you need. \
                 Your files in /workspace are intact.",
                sentinel.old, sentinel.new
            )
        } else {
            format!(
                "Sandbox flavor changed from {} to {} in this conversation. \
                 Package-manager caches were cleared; reinstall anything you need. \
                 Your files in /workspace are intact.",
                sentinel.old, sentinel.new
            )
        };
        return Some(msg);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wipe_walker_drops_install_caches_and_keeps_user_files() {
        let workspace_root = tempfile::tempdir().unwrap();
        let conv_a = workspace_root.path().join("00000000-0000-0000-0000-00000000000a");
        let conv_b = workspace_root.path().join("00000000-0000-0000-0000-00000000000b");
        std::fs::create_dir_all(conv_a.join(".local")).unwrap();
        std::fs::create_dir_all(conv_a.join(".cache/pip")).unwrap();
        std::fs::write(conv_a.join("notes.md"), "user file").unwrap();
        std::fs::write(conv_a.join("output.csv"), "x,y\n").unwrap();
        std::fs::create_dir_all(conv_b.join(".npm")).unwrap();
        std::fs::write(conv_b.join("plot.png"), b"PNG").unwrap();
        // Shared subsystem dirs the walker must skip.
        std::fs::create_dir_all(workspace_root.path().join("attachments")).unwrap();
        std::fs::create_dir_all(workspace_root.path().join("identity")).unwrap();
        // Per-MCP-server workspace.
        let mcp_server =
            workspace_root.path().join("mcp").join("11111111-1111-1111-1111-111111111111");
        std::fs::create_dir_all(mcp_server.join(".local/lib")).unwrap();
        std::fs::write(mcp_server.join("server-state.json"), "{}").unwrap();

        let sentinel = WipeSentinel {
            old: "0.9.0".to_string(),
            new: "1.0.0".to_string(),
            at: chrono::Utc::now(),
        };
        let result = wipe_install_caches_in_root(workspace_root.path(), &sentinel);

        // Counts.
        assert_eq!(result.conversation_dirs, 2);
        assert_eq!(result.mcp_server_dirs, 1);
        assert!(result.subdirs_removed >= 3); // .local, .cache, .npm (.local from mcp)

        // Conversation workspaces — install caches gone, user files intact.
        assert!(!conv_a.join(".local").exists());
        assert!(!conv_a.join(".cache").exists());
        assert!(conv_a.join("notes.md").exists());
        assert!(conv_a.join("output.csv").exists());
        assert!(!conv_b.join(".npm").exists());
        assert!(conv_b.join("plot.png").exists());

        // MCP server workspace.
        assert!(!mcp_server.join(".local").exists());
        assert!(mcp_server.join("server-state.json").exists());

        // Sentinels dropped.
        assert!(conv_a.join(SENTINEL_ROOTFS_UPGRADED).exists());
        assert!(conv_b.join(SENTINEL_ROOTFS_UPGRADED).exists());
        assert!(mcp_server.join(SENTINEL_ROOTFS_UPGRADED).exists());
    }

    #[test]
    fn flavor_switch_wipes_only_caller_conversation() {
        let workspace_root = tempfile::tempdir().unwrap();
        let conv_a = workspace_root.path().join("00000000-0000-0000-0000-00000000000a");
        let conv_b = workspace_root.path().join("00000000-0000-0000-0000-00000000000b");
        std::fs::create_dir_all(conv_a.join(".local/lib")).unwrap();
        std::fs::create_dir_all(conv_b.join(".local/lib")).unwrap();

        let result = wipe_install_caches_for_conversation(&conv_a, "minimal", "full");
        assert_eq!(result.conversation_dirs, 1);
        assert!(result.subdirs_removed >= 1);

        // A wiped, B untouched.
        assert!(!conv_a.join(".local").exists());
        assert!(conv_b.join(".local/lib").exists());

        // Sentinel uses the flavor-changed name.
        assert!(conv_a.join(SENTINEL_FLAVOR_CHANGED).exists());
        assert!(!conv_a.join(SENTINEL_ROOTFS_UPGRADED).exists());
    }

    #[test]
    fn consume_workspace_sentinel_reads_unlinks_returns_message() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = WipeSentinel {
            old: "0.1.0".to_string(),
            new: "1.0.0".to_string(),
            at: chrono::Utc::now(),
        };
        let json_text = serde_json::to_string(&sentinel).unwrap();
        std::fs::write(dir.path().join(SENTINEL_ROOTFS_UPGRADED), &json_text).unwrap();

        let note = consume_workspace_sentinel(dir.path()).expect("sentinel present");
        assert!(note.contains("v0.1.0"));
        assert!(note.contains("v1.0.0"));
        assert!(note.contains("major bump"));
        assert!(!dir.path().join(SENTINEL_ROOTFS_UPGRADED).exists());

        // Second call: sentinel unlinked, no message.
        assert!(consume_workspace_sentinel(dir.path()).is_none());
    }

    #[test]
    fn consume_workspace_sentinel_handles_flavor_switch() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = WipeSentinel {
            old: "minimal".to_string(),
            new: "full".to_string(),
            at: chrono::Utc::now(),
        };
        let json_text = serde_json::to_string(&sentinel).unwrap();
        std::fs::write(dir.path().join(SENTINEL_FLAVOR_CHANGED), &json_text).unwrap();

        let note = consume_workspace_sentinel(dir.path()).expect("sentinel present");
        assert!(note.contains("minimal"));
        assert!(note.contains("full"));
        assert!(note.contains("flavor"));
        assert!(!dir.path().join(SENTINEL_FLAVOR_CHANGED).exists());
    }

    #[test]
    fn inflight_guard_round_trip() {
        let id = Uuid::new_v4();
        let _registry_guard =
            register_mount(id, "0.1.0", "x86_64", "minimal", std::path::PathBuf::from("/tmp"));
        let artifact = mounted_artifact(id).unwrap();
        assert_eq!(artifact.inflight(), 0);

        let exec = acquire_inflight(id, InflightKind::Exec).unwrap();
        assert_eq!(artifact.inflight(), 1);
        let mcp = acquire_inflight(id, InflightKind::Mcp).unwrap();
        assert_eq!(artifact.inflight(), 2);
        assert_eq!(artifact.inflight_breakdown(), (1, 1));

        drop(exec);
        assert_eq!(artifact.inflight(), 1);
        drop(mcp);
        assert_eq!(artifact.inflight(), 0);

        // Cleanup so a parallel test on this registry doesn't see leftover.
        MOUNTED_ARTIFACTS.remove(&id);
    }
}
