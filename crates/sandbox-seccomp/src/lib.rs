//! Single source of truth for the code-sandbox **seccomp policy**.
//!
//! The policy (which syscalls to deny, and with which errno) must be identical
//! on the Linux host backend (`code_sandbox/probes.rs`) and inside the
//! macOS/Windows microVM (`sandbox-guest-agent`) — otherwise the VM guests
//! would silently run with weaker syscall filtering than the Linux host. Both
//! consume this crate so the lists can never drift.
//!
//! Shape: a **default-allow denylist** (deliberate — R/torch's large, evolving
//! syscall surface makes a default-deny allowlist a perpetual maintenance/
//! breakage burden). Most denied syscalls return `EPERM`; the probe-and-
//! fallback families (`clone3` + the new mount API) return **`ENOSYS`** so
//! glibc/runtimes fall back to the legacy syscall instead of failing (this is
//! what keeps threaded/forked R working under seccomp).

/// Denied with **EPERM** — no caller fallback path; a hard "operation not
/// permitted" is correct.
pub const DENY_EPERM: &[&str] = &[
    // Tracing / cross-process introspection
    "ptrace",
    "perf_event_open",
    "process_vm_readv",
    "process_vm_writev",
    "pidfd_send_signal",
    "pidfd_getfd",
    "pidfd_open",
    "kcmp",
    // Kernel modification
    "bpf",
    "userfaultfd",
    "kexec_load",
    "kexec_file_load",
    "init_module",
    "finit_module",
    "delete_module",
    // Legacy module API
    "create_module",
    "query_module",
    "get_kernel_syms",
    // Keyring
    "keyctl",
    "add_key",
    "request_key",
    // Mount family — old API
    "mount",
    "umount",
    "umount2",
    "pivot_root",
    "chroot",
    // Namespace manipulation
    "setns",
    "unshare",
    // File-handle resolution — the "Shocker" container-breakout primitive.
    "open_by_handle_at",
    "name_to_handle_at",
    // Time is NOT namespaced — block wall-clock tampering.
    "clock_settime",
    "clock_adjtime",
    "settimeofday",
    "stime",
    // Kernel ring buffer — leaks KASLR pointers.
    "syslog",
    // Swap / power
    "swapon",
    "swapoff",
    "reboot",
    // I/O ring — recent escape vectors via io_uring SQE rewrites
    "io_uring_setup",
    "io_uring_enter",
    "io_uring_register",
    // Direct hardware I/O ports
    "iopl",
    "ioperm",
    // Quotas + execution-domain switching + process accounting
    "quotactl",
    "personality",
    "acct",
    // NUMA memory-policy (best-effort in libs → EPERM tolerated)
    "migrate_pages",
    "mbind",
    "get_mempolicy",
    "set_mempolicy",
    "move_pages",
    // Misc obsolete / dangerous
    "lookup_dcookie",
    "nfsservctl",
    "uselib",
    "ustat",
    "vm86",
    "vm86old",
    "modify_ldt",
];

/// Denied with **ENOSYS** — callers probe these and fall back to the legacy
/// syscall only on `ENOSYS`. `EPERM` here would defeat the fallback and break
/// threaded/forked workloads (R `parallel::mclapply`, `data.table`, `torch` on
/// glibc ≥ 2.34). Matches Flatpak.
pub const DENY_ENOSYS: &[&str] = &[
    "clone3",
    "fsopen",
    "fsconfig",
    "fsmount",
    "move_mount",
    "open_tree",
    "mount_setattr",
];

/// Compile the seccomp filter to its BPF byte image (for bwrap `--seccomp <fd>`).
/// Per-name best-effort: syscalls absent on the running kernel are skipped (a
/// failed lookup must never disable the whole filter). Returns the BPF bytes
/// plus the names that didn't resolve (callers may log them).
#[cfg(target_os = "linux")]
pub fn build_bpf() -> Result<(Vec<u8>, Vec<String>), String> {
    use libseccomp::{ScmpAction, ScmpFilterContext, ScmpSyscall};

    let mut ctx =
        ScmpFilterContext::new_filter(ScmpAction::Allow).map_err(|e| format!("ctx: {e}"))?;
    let mut unresolved: Vec<String> = Vec::new();
    for (action, names) in [
        (ScmpAction::Errno(libc_eperm()), DENY_EPERM),
        (ScmpAction::Errno(libc_enosys()), DENY_ENOSYS),
    ] {
        for name in names {
            let sys = match ScmpSyscall::from_name(name) {
                Ok(s) => s,
                Err(_) => {
                    unresolved.push((*name).to_string());
                    continue;
                }
            };
            ctx.add_rule(action, sys)
                .map_err(|e| format!("add_rule {name}: {e}"))?;
        }
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut f = tempfile::tempfile().map_err(|e| format!("tempfile: {e}"))?;
    ctx.export_bpf(&mut f).map_err(|e| format!("export_bpf: {e}"))?;
    use std::io::{Read, Seek, SeekFrom};
    f.seek(SeekFrom::Start(0)).map_err(|e| format!("seek: {e}"))?;
    f.read_to_end(&mut buf).map_err(|e| format!("read: {e}"))?;
    Ok((buf, unresolved))
}

// Avoid a libc dep just for two errno constants.
#[cfg(target_os = "linux")]
fn libc_eperm() -> i32 {
    1
}
#[cfg(target_os = "linux")]
fn libc_enosys() -> i32 {
    38
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn build_produces_nonempty_bpf() {
        let (bpf, _unresolved) = build_bpf().expect("seccomp filter builds");
        assert!(!bpf.is_empty(), "exported BPF must be non-empty");
    }

    #[test]
    fn deny_lists_are_disjoint_and_classified_correctly() {
        assert!(!DENY_EPERM.is_empty());
        assert!(!DENY_ENOSYS.is_empty());
        for n in DENY_ENOSYS {
            assert!(!DENY_EPERM.contains(n), "{n} must not be in both lists");
        }
        for n in ["clone3", "fsopen", "fsconfig", "fsmount", "move_mount", "open_tree", "mount_setattr"] {
            assert!(DENY_ENOSYS.contains(&n), "{n} must be in DENY_ENOSYS");
        }
        assert!(DENY_EPERM.contains(&"open_by_handle_at"));
        assert!(DENY_EPERM.contains(&"name_to_handle_at"));
    }
}
