//! The two properties the per-test data-dir leak actually turned on, each proved
//! against the real kernel primitive rather than a model of it.
//!
//! These live in an integration test (not in-source) because both need resources a
//! unit test cannot fabricate: a live FUSE mount, and a process that dies by
//! `SIGKILL`. Both are Linux-gated — `/proc/mounts`, `squashfuse` and `flock`
//! semantics are what is under test, and the suite's parallel target is Linux.
//!
//! ## Why these two and not more
//!
//! The in-source unit tests already cover the predicate algebra (name shape, age
//! floor, lock/no-lock, symlink refusal, mount-table parsing). What they CANNOT
//! cover is the thing that actually broke:
//!
//!   * `reclaims_a_tree_containing_a_live_squashfuse_mount` is the only test that
//!     fails against the ORIGINAL code. A `remove_dir_all` over a mount point
//!     returns `ENOSYS` — so a test that exercises only clean, mount-free trees
//!     passes against the broken implementation, which is precisely how this
//!     shipped. It uniquely kills the mutation "delete the `unmount_all_under`
//!     call from `reclaim_data_dir`".
//!   * `a_sigkilled_owner_releases_its_lock_and_the_sweep_reclaims_it` is the only
//!     test that proves the SIGKILL half. The whole sweep rests on the claim that
//!     the kernel drops an `flock` when the owning process is killed outright; if
//!     that were false the sweep would reclaim nothing and every unit test would
//!     still pass. It uniquely kills "use a lock file's mere EXISTENCE, or a
//!     recorded pid, instead of `flock`".

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};

use ziee_test_harness::data_dir::{sweep_dir_root, unmount_all_under, PerTestDataDir};

/// True when `path` is listed as a mount point in the live mount table.
fn is_mounted(path: &Path) -> bool {
    std::fs::read_to_string("/proc/self/mounts")
        .map(|t| {
            t.lines()
                .filter_map(|l| l.split(' ').nth(1))
                .any(|mp| Path::new(mp) == path)
        })
        .unwrap_or(false)
}

fn have(bin: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .stdout(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Build a tiny squashfs image at `dest` from a freshly-created source tree.
///
/// Deliberately built at test time rather than checked in or borrowed from a cache:
/// the test must be reproducible on any box with the sandbox tooling the suite
/// already requires, and must not depend on a 300 MB artifact happening to be
/// present.
fn make_tiny_squashfs(scratch: &Path, dest: &Path) {
    let src = scratch.join("sqsrc");
    std::fs::create_dir_all(src.join("usr")).expect("mk squashfs source tree");
    std::fs::write(src.join("usr").join("marker"), b"rootfs marker").expect("write marker");
    let out = std::process::Command::new("mksquashfs")
        .arg(&src)
        .arg(dest)
        .args(["-noappend", "-no-progress", "-quiet"])
        .output()
        .expect("run mksquashfs");
    assert!(
        out.status.success(),
        "mksquashfs failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn reclaims_a_tree_containing_a_live_squashfuse_mount() {
    // The sandbox tiers already require these two on Linux; this test asserts on
    // them rather than self-skipping, so a box without them fails loudly instead of
    // reporting a green run that proved nothing.
    assert!(
        have("mksquashfs") && have("squashfuse"),
        "this test needs `mksquashfs` (squashfs-tools) and `squashfuse` — the same \
         host deps the sandbox test tiers already require. Install them; do not skip."
    );

    let scratch = tempfile::tempdir().expect("scratch");
    let data = PerTestDataDir::new().expect("create per-test data dir");
    let data_path = data.path().to_path_buf();

    // Reproduce the exact production shape: an ordinary symlink sibling (which the
    // old `remove_dir_all` DID delete) next to a digest dir holding a squashfs and
    // its mount point (which is where it died with ENOSYS).
    std::os::unix::fs::symlink(scratch.path(), data_path.join("bin")).expect("symlink sibling");
    let digest_dir = data_path.join("sandbox-rootfs").join("d".repeat(64));
    let mnt = digest_dir.join("mnt");
    std::fs::create_dir_all(&mnt).expect("mk digest dir + mnt");
    let squashfs = digest_dir.join("rootfs.squashfs");
    make_tiny_squashfs(scratch.path(), &squashfs);

    let mounted = std::process::Command::new("squashfuse")
        .arg(&squashfs)
        .arg(&mnt)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(mounted, "squashfuse could not mount the fixture image");
    assert!(is_mounted(&mnt), "fixture must really be mounted");
    assert!(
        mnt.join("usr").join("marker").exists(),
        "and really readable through the mount"
    );

    // The property. Against the original bare-`TempDir` implementation this is the
    // assertion that fails: the tree survives with `sandbox-rootfs/` intact.
    drop(data);

    assert!(
        !data_path.exists(),
        "the per-test data dir must be gone even though it contained a live mount; \
         this is the whole defect"
    );
    assert!(
        !is_mounted(&mnt),
        "and the mount table entry must be gone too — 703 stale entries is what \
         leaking these costs"
    );
}

#[test]
fn unmounting_is_scoped_to_the_tree_and_leaves_a_neighbour_alone() {
    assert!(
        have("mksquashfs") && have("squashfuse"),
        "needs mksquashfs + squashfuse (see the sibling test)"
    );
    let scratch = tempfile::tempdir().expect("scratch");

    // Two independent per-test dirs, each with its own mount — the shared-box case.
    // A sweep that unmounted by a loose textual prefix, or that unmounted everything
    // it could find, would take the neighbour's live mount with it.
    let mut mounts = Vec::new();
    let mut dirs = Vec::new();
    for i in 0..2 {
        let d = PerTestDataDir::new().expect("create");
        let mnt = d.path().join("mnt");
        std::fs::create_dir_all(&mnt).expect("mkdir mnt");
        let sq = scratch.path().join(format!("img{i}.squashfs"));
        make_tiny_squashfs(&scratch.path().join(format!("s{i}")), &sq);
        assert!(
            std::process::Command::new("squashfuse")
                .arg(&sq)
                .arg(&mnt)
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            "mount {i}"
        );
        mounts.push(mnt);
        dirs.push(d);
    }

    let victim = dirs.remove(0);
    let victim_mnt = mounts[0].clone();
    let neighbour_mnt = mounts[1].clone();
    let neighbour_path = dirs[0].path().to_path_buf();

    assert_eq!(
        unmount_all_under(victim.path()),
        0,
        "the victim's own tree must end with no mounts listed"
    );
    assert!(!is_mounted(&victim_mnt));
    assert!(
        is_mounted(&neighbour_mnt),
        "a CONCURRENT test's mount must be untouched"
    );
    assert!(neighbour_path.exists());

    drop(victim);
    assert!(
        is_mounted(&neighbour_mnt),
        "and still untouched after the victim is fully reclaimed"
    );
    // Clean up the neighbour through the same owned path.
    drop(dirs.remove(0));
    assert!(!is_mounted(&neighbour_mnt));
}

/// Fork a child that holds the data dir's liveness lock and blocks forever.
/// Returns the child pid; on return the lock is held SOLELY by that child.
///
/// `fork` + `_exit` rather than a spawned helper binary: the child touches only raw
/// syscalls before it blocks, so it is safe in a multi-threaded test process, and it
/// needs no second binary, no re-exec trick, and no assumption about the runner's argv.
///
/// The lock is taken in the PARENT, before the fork, and the parent's descriptor is
/// closed after it. Taking it in the child instead loses a race the test cannot win:
/// between the fork and the child's first instruction the directory has no lock file
/// at all, so a sweep in that window judges it unowned and deletes it — which is
/// exactly how the first version of this test failed, on correct production code.
fn fork_lock_holder(dir: &Path) -> libc::pid_t {
    use std::os::unix::io::IntoRawFd;

    let lock = std::fs::File::create(dir.join(ziee_test_harness::data_dir::LIVE_LOCK_FILE))
        .expect("create lock file");
    let fd = lock.into_raw_fd();

    // SAFETY: `fd` is a live descriptor we own; the child touches only
    // async-signal-safe syscalls and leaves via `_exit`, never returning into Rust
    // runtime teardown.
    unsafe {
        assert_eq!(
            libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB),
            0,
            "parent must take the lock before forking"
        );

        let pid = libc::fork();
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            // Die with the test process. Without this, cancelling the run (or any
            // parent crash) strands a child in `pause()` forever — holding both the
            // lock and, worse, a copy of the test binary's stdout, which keeps the
            // whole `cargo test` pipeline from ever reporting.
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            // Re-check after arming: if the parent died in the window above, the
            // signal was already delivered against the OLD parent and never fires.
            if libc::getppid() == 1 {
                libc::_exit(2);
            }
            // Drop every other INHERITED descriptor — including stdout/stderr.
            // `fork` duplicates the parent's open file DESCRIPTIONS, and an `flock`
            // lives on the description, so a child that kept them would hold the
            // liveness locks of every SIBLING test in this binary. Only the lock
            // descriptor may survive.
            let max = libc::sysconf(libc::_SC_OPEN_MAX).clamp(64, 4096) as libc::c_int;
            for other in 0..max {
                if other != fd {
                    libc::close(other);
                }
            }
            loop {
                libc::pause();
            }
        }
        // The child now holds the lock through its inherited copy of the same open
        // file description; releasing ours leaves it the sole holder.
        libc::close(fd);
        pid
    }
}

#[test]
fn a_sigkilled_owner_releases_its_lock_and_the_sweep_reclaims_it() {
    let scratch = tempfile::tempdir().expect("scratch");
    let dir = scratch.path().join("ziee-test-data-Killed1");
    std::fs::create_dir(&dir).expect("mkdir");
    // Put real content in it, so what is reclaimed is a tree and not an empty dir.
    std::fs::create_dir_all(dir.join("sandbox-rootfs").join("x")).expect("mk content");
    std::fs::write(dir.join("sandbox-rootfs").join("x").join("f"), b"bytes").expect("write");

    let pid = fork_lock_holder(&dir);

    // The lock is already held when `fork_lock_holder` returns, so this is a plain
    // assertion and not a poll: with the age floor at 0 the lock is the ONLY thing
    // that can exclude the directory, and it must exclude it on the very first pass.
    assert!(
        sweep_dir_root(scratch.path(), 0).considered.is_empty(),
        "a directory whose flock a live process holds must not even be considered"
    );
    assert!(
        dir.exists(),
        "and the live dir must not have been reclaimed"
    );

    // Now kill it the way a leaked run actually dies. No cleanup, no unwinding, no
    // destructor — only the kernel closing the descriptor.
    // SAFETY: `pid` is our own forked child, captured above.
    unsafe {
        assert_eq!(libc::kill(pid, libc::SIGKILL), 0, "kill our own child");
        let mut status = 0;
        assert_eq!(
            libc::waitpid(pid, &mut status, 0),
            pid,
            "reap our own child"
        );
    }

    let report = sweep_dir_root(scratch.path(), 0);
    assert!(
        report.considered.contains(&dir),
        "after SIGKILL the lock is released by the kernel and the dir must be \
         reclaimed; considered={:?}",
        report.considered
    );
    assert!(
        !dir.exists(),
        "and the tree must actually be gone — this is the half of the fix that \
         covers panic=abort, SIGKILL, OOM and binary timeouts"
    );
}

#[test]
fn the_startup_sweep_is_installed_not_merely_callable() {
    // Guards the CALL SITE, not the function: every other test here invokes the
    // sweep directly and would stay green if the hook inside the harness's own
    // startup were deleted.
    let src: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("lib.rs");
    let text = std::fs::read_to_string(&src).expect("read harness lib.rs");
    assert!(
        text.contains("data_dir::sweep_stale_test_data_dirs_once()"),
        "the once-per-process data-dir sweep must be called from the harness's \
         startup path in {}",
        src.display()
    );
}
