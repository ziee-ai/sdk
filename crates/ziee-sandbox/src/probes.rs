//! One-time boot probes for the sandbox.
//!
//! Probes run exactly once at `code_sandbox::init()` and the results
//! land in `HardeningCapabilities`. Every per-call code path reads
//! that cached struct — no shellouts on the hot path.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use crate::sandbox_config::CodeSandboxConfig;
use crate::types::{
    CgroupMode, HardeningCapabilities, HostCapabilities, PidNsMode, SeccompMode,
};

/// Probe everything that does NOT require the sandbox rootfs to be
/// mounted: bwrap on PATH, delegated cgroup, seccomp filter compile.
/// Cost: <10 ms. Returns `None` if bwrap is missing (in which case
/// `init()` skips MCP registration entirely).
///
/// Boot path. The rootfs-dependent half (`probe_pid_ns`) runs lazily
/// on the first `execute_command` call via
/// [`runtime_mount::ensure_rootfs_ready`].
pub fn probe_host_only(config: &CodeSandboxConfig) -> Option<HostCapabilities> {
    let bwrap_path = which_bwrap()?;
    let cgroup = probe_cgroup(&config.cgroup_parent);
    let seccomp = compile_seccomp_filter();
    Some(HostCapabilities { bwrap_path, cgroup, seccomp })
}

/// Promote `HostCapabilities` to the full `HardeningCapabilities` by
/// running the rootfs-dependent probes against the now-mounted rootfs.
/// Called from `runtime_mount::ensure_rootfs_ready` after squashfuse
/// has put the rootfs at `<rootfs_path>/usr`.
///
/// Returns `Err` if `probe_pid_ns` produces `Disabled` — that's the
/// "the sandbox cannot run on this host" signal that surfaces to the
/// caller as a structured MCP error.
pub fn probe_rootfs_dependent(
    config: &CodeSandboxConfig,
    host: &HostCapabilities,
) -> Result<HardeningCapabilities, String> {
    let pid_namespace = probe_pid_ns(&host.bwrap_path, config.rootfs_path());
    if matches!(pid_namespace, PidNsMode::Disabled) {
        return Err(format!(
            "PID-namespace probe failed against rootfs at {}; bwrap \
             cannot start a useful sandbox here. Check kernel config \
             (CONFIG_USER_NS=y, unprivileged_userns_clone=1).",
            config.rootfs_path()
        ));
    }
    let caps = HardeningCapabilities {
        bwrap_path: host.bwrap_path.clone(),
        pid_namespace,
        cgroup: host.cgroup.clone(),
        seccomp: host.seccomp.clone(),
    };
    log_hardening_summary(&caps);
    Ok(caps)
}

/// Convenience: probe both host-only and rootfs-dependent halves in one
/// call. Used by Tier-4 tests and the bootstrap script, NOT by the
/// production boot path (which only calls `probe_host_only`).
/// (Kept: sole constructor of `SeccompMode::NotLinked`, the stock-build
/// seccomp-off hardening state.)
#[allow(dead_code)]
pub fn probe_all(config: &CodeSandboxConfig) -> HardeningCapabilities {
    let host = match probe_host_only(config) {
        Some(h) => h,
        None => {
            tracing::error!(
                "code_sandbox: bwrap not found on PATH; sandbox will refuse \
                 to register. Install bubblewrap (apt install bubblewrap)."
            );
            return HardeningCapabilities {
                bwrap_path: PathBuf::from("bwrap"),
                pid_namespace: PidNsMode::Disabled,
                cgroup: CgroupMode::None,
                seccomp: SeccompMode::NotLinked,
            };
        }
    };
    probe_rootfs_dependent(config, &host).unwrap_or(HardeningCapabilities {
        bwrap_path: host.bwrap_path,
        pid_namespace: PidNsMode::Disabled,
        cgroup: host.cgroup,
        seccomp: host.seccomp,
    })
}

fn log_hardening_summary(caps: &HardeningCapabilities) {
    let summary = format!(
        "code_sandbox: hardening = {{ rlimits: on, bwrap: on, pid_ns: {pid_ns}, cgroup_v2: {cg}, seccomp: {sc} }}",
        pid_ns = match caps.pid_namespace {
            PidNsMode::Strict => "on",
            PidNsMode::DevBindFallback => "off-fallback-dev-bind",
            PidNsMode::Disabled => "DISABLED",
        },
        cg = match &caps.cgroup {
            CgroupMode::Delegated(_) => "on (delegated)",
            CgroupMode::None => "off-needs-delegation",
        },
        sc = match &caps.seccomp {
            SeccompMode::Loaded(_) => "on",
            SeccompMode::NotLinked => "off-feature-not-linked",
            SeccompMode::Disabled => "off-libseccomp-failed",
        },
    );
    tracing::info!("{summary}");
}

fn which_bwrap() -> Option<PathBuf> {
    for dir in std::env::var_os("PATH")?.to_string_lossy().split(':') {
        let p = Path::new(dir).join("bwrap");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Try strict mode (`--unshare-pid --proc /proc`) first; on failure
/// retry with `--dev-bind /proc /proc`. If neither works, sandbox is
/// disabled. Public-in-crate so `runtime_mount` can call it from the
/// lazy-init path against the just-mounted rootfs.
pub(crate) fn probe_pid_ns(bwrap_path: &Path, rootfs: &str) -> PidNsMode {
    if !bwrap_path.is_absolute() {
        return PidNsMode::Disabled;
    }
    let rootfs_usr = format!("{rootfs}/usr");
    if !Path::new(&rootfs_usr).exists() {
        // Rootfs not mounted; we can't run a probe. Disable gracefully.
        tracing::warn!(
            "code_sandbox: rootfs not present at {rootfs}; PID-ns probe skipped, sandbox disabled"
        );
        return PidNsMode::Disabled;
    }

    let strict = StdCommand::new(bwrap_path)
        .args([
            "--unshare-user",
            "--uid",
            "1001",
            "--gid",
            "1001",
            "--unshare-pid",
            "--share-net",
            "--new-session",
            "--die-with-parent",
            "--ro-bind",
            &rootfs_usr,
            "/usr",
            "--symlink",
            "usr/bin",
            "/bin",
            "--symlink",
            "usr/lib",
            "/lib",
            "--symlink",
            "usr/lib64",
            "/lib64",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--",
            "/bin/true",
        ])
        .output();

    if let Ok(o) = strict
        && o.status.success() {
            return PidNsMode::Strict;
        }

    // Fallback: same flags but bind /proc.
    let fallback = StdCommand::new(bwrap_path)
        .args([
            "--unshare-user",
            "--uid",
            "1001",
            "--gid",
            "1001",
            "--share-net",
            "--new-session",
            "--die-with-parent",
            "--ro-bind",
            &rootfs_usr,
            "/usr",
            "--symlink",
            "usr/bin",
            "/bin",
            "--symlink",
            "usr/lib",
            "/lib",
            "--symlink",
            "usr/lib64",
            "/lib64",
            "--dev-bind",
            "/proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--",
            "/bin/true",
        ])
        .output();

    match fallback {
        Ok(o) if o.status.success() => PidNsMode::DevBindFallback,
        _ => PidNsMode::Disabled,
    }
}

/// Detect a writable delegated cgroup parent. Empty config → None.
///
/// SECURITY: the parent path comes from `code_sandbox.cgroup_parent`
/// config. We canonicalize once at boot and require:
///   1. the canonical path is under `/sys/fs/cgroup/` — refuses to
///      operate on arbitrary filesystem paths even if the operator
///      misconfigures a symlink,
///   2. the path itself is NOT a symlink (a config-time symlink swap
///      could otherwise point us at an unrelated cgroup),
///   3. subtree_control is tokenized on whitespace (substring match
///      would accept the kernel's `-memory` denied-controller form).
fn probe_cgroup(parent_str: &str) -> CgroupMode {
    if parent_str.trim().is_empty() {
        return CgroupMode::None;
    }
    let raw_parent = PathBuf::from(parent_str);
    // Refuse symlinks at the parent path itself.
    if let Ok(meta) = std::fs::symlink_metadata(&raw_parent)
        && meta.file_type().is_symlink() {
            tracing::warn!(
                "code_sandbox: cgroup_parent {} is a symlink; refusing for safety",
                raw_parent.display()
            );
            return CgroupMode::None;
        }
    // Canonicalize and re-check the resolved path is under /sys/fs/cgroup.
    let parent = match std::fs::canonicalize(&raw_parent) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                "code_sandbox: cgroup_parent {} not accessible ({e}); rlimits-only mode",
                raw_parent.display()
            );
            return CgroupMode::None;
        }
    };
    if !parent.starts_with("/sys/fs/cgroup") {
        tracing::warn!(
            "code_sandbox: cgroup_parent resolved to {} which is NOT under \
             /sys/fs/cgroup; refusing",
            parent.display()
        );
        return CgroupMode::None;
    }
    let subtree = parent.join("cgroup.subtree_control");
    if !subtree.exists() {
        tracing::warn!(
            "code_sandbox: cgroup parent {} missing or not delegated; rlimits-only mode",
            parent.display()
        );
        return CgroupMode::None;
    }
    // Read subtree_control and tokenize properly. The kernel writes a
    // space-separated list of active-controller names (no `+`/`-`
    // prefix in the READ form, despite the WRITE syntax using prefixes).
    // A naive `contains("memory")` would match a hypothetical
    // `-memory` token (denied) or a substring like `memory_pressure`.
    let controllers = std::fs::read_to_string(&subtree).unwrap_or_default();
    let active: std::collections::HashSet<&str> = controllers.split_whitespace().collect();
    if !active.contains("memory") || !active.contains("pids") {
        tracing::warn!(
            "code_sandbox: cgroup parent {} subtree_control lacks memory+pids \
             (active={active:?}); rlimits-only mode",
            parent.display()
        );
        return CgroupMode::None;
    }
    // Sanity: can we mkdir + rmdir a probe child?
    let probe = parent.join(".sandbox-probe");
    let _ = std::fs::remove_dir(&probe); // ignore prior leftovers
    if std::fs::create_dir(&probe).is_ok() {
        let _ = std::fs::remove_dir(&probe);
        // Boot-time leak sweep: any pre-existing `sandbox-*` dirs
        // older than 1 hour are stale from previous server runs that
        // didn't get to clean up (crashes, SIGKILLs). Sweep them so
        // they don't accumulate across restarts.
        sweep_stale_cgroup_scopes(&parent);
        CgroupMode::Delegated(parent)
    } else {
        tracing::warn!(
            "code_sandbox: cgroup parent {} not writable by server uid; rlimits-only mode",
            parent.display()
        );
        CgroupMode::None
    }
}

/// Boot-time sweep of orphaned per-call cgroup scopes from prior
/// server runs. We delete `sandbox-*` subdirs older than 1 hour;
/// fresher ones are left alone to avoid racing a just-started in-flight
/// call from a hot-restart scenario.
fn sweep_stale_cgroup_scopes(parent: &std::path::Path) {
    use std::time::{Duration, SystemTime};
    const MIN_AGE: Duration = Duration::from_secs(3600);
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    let mut swept = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.starts_with("sandbox-") { continue }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() { continue }
        let age = meta.modified().ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .unwrap_or(Duration::ZERO);
        if age < MIN_AGE { continue }
        if std::fs::remove_dir(&path).is_ok() {
            swept += 1;
        }
    }
    if swept > 0 {
        tracing::info!(swept, "code_sandbox: swept stale cgroup scopes at boot");
    }
}

/// Compile the seccomp filter once at boot. Active only when BOTH
/// the `code_sandbox_seccomp` cargo feature is enabled AND the build
/// target is Linux. The feature is on by default; on Mac/Windows
/// the target gate trips so the filter is a no-op and seccomp logs
/// as `off-feature-not-linked` (the same surface as if the feature
/// had been turned off explicitly).
fn compile_seccomp_filter() -> SeccompMode {
    #[cfg(not(all(feature = "code_sandbox_seccomp", target_os = "linux")))]
    {
        SeccompMode::NotLinked
    }
    #[cfg(all(feature = "code_sandbox_seccomp", target_os = "linux"))]
    {
        match seccomp_impl::build() {
            Ok(bytes) => SeccompMode::Loaded(std::sync::Arc::new(bytes)),
            Err(e) => {
                tracing::warn!("code_sandbox: seccomp filter build failed: {e}");
                SeccompMode::Disabled
            }
        }
    }
}

#[cfg(all(feature = "code_sandbox_seccomp", target_os = "linux"))]
mod seccomp_impl {
    /// Compile the seccomp BPF from the shared policy crate. The DENY lists +
    /// EPERM/ENOSYS classification live in `sandbox-seccomp` (single source of
    /// truth — the in-VM guest agent uses the exact same crate, so the macOS/
    /// Windows guests apply an identical filter and can't drift from the host).
    pub fn build() -> Result<Vec<u8>, String> {
        let (bpf, unresolved) = sandbox_seccomp::build_bpf()?;
        if !unresolved.is_empty() {
            let total = sandbox_seccomp::DENY_EPERM.len() + sandbox_seccomp::DENY_ENOSYS.len();
            tracing::warn!(
                resolved = total - unresolved.len(),
                total,
                unresolved = ?unresolved,
                "code_sandbox: some seccomp DENY entries did not resolve on this \
                 kernel; they are silently skipped. Verify the kernel supports them \
                 (e.g. clone3 needs >=5.3)."
            );
        }
        Ok(bpf)
    }
}
