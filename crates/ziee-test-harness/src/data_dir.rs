//! The per-test `app.data_dir`: owned, unmounted, and actually removed.
//!
//! ## The defect this exists to close
//!
//! The per-test data dir used to be a bare [`tempfile::TempDir`]. `TempDir`'s
//! destructor is `let _ = fs::remove_dir_all(path)` — it discards the error. That
//! is fine for a tree of ordinary files and catastrophic for this one, because a
//! test that runs anything in the sandbox leaves a **squashfuse mount** at
//! `<data_dir>/sandbox-rootfs/<digest>/mnt`, and `remove_dir_all` cannot descend
//! through a mount point. It fails with `ENOSYS` ("Function not implemented"),
//! having already deleted every sibling it reached first.
//!
//! So the leak was never confined to killed runs. It was the **happy path**: a
//! perfectly green test unmounted nothing, failed to remove its own data dir, and
//! said nothing. Measured on the shared box before this change: 885 orphaned
//! `ziee-test-data-*` trees holding **176 GB**, and — the part that costs more than
//! disk — **703 leaked entries in `/proc/mounts`**, most of them stale
//! (`Transport endpoint is not connected`, the daemon long dead but the mount
//! table entry immortal).
//!
//! The shape of the residue is the proof: every one of those trees had its `bin`,
//! `lib`, `llm-engines` and `lit-cache` symlinks **removed** and its
//! `sandbox-rootfs/` **intact**. That is exactly what a `remove_dir_all` that ran,
//! got partway, and hit `ENOSYS` leaves behind — not what a killed process leaves
//! behind, which is everything.
//!
//! ## The fix
//!
//! [`PerTestDataDir`] owns the directory the way `PerTestDb` owns its database:
//! the reclaim is bound to a value's lifetime, so the language runs it on the
//! normal return, the early `?`, and the panicking body alike. Its drop
//! **unmounts every mount point under the tree, deepest first, before** it
//! removes anything — and when the removal still fails it says so on stderr
//! instead of discarding the error, because a reclaim that fails and a reclaim
//! that never ran leave identical state, which is how this stayed invisible.
//!
//! What no destructor in this process can cover — `panic = "abort"`, SIGKILL, an
//! OOM kill, a test-binary timeout — is [`sweep_stale_test_data_dirs`]'s job at
//! the start of the NEXT test process, which needs no cooperation from the run
//! that died. That is the same two-halves structure the database leak needed.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Prefix of every per-test data dir this harness creates. The ONE literal — the
/// creator ([`PerTestDataDir::new`]) and the sweep ([`sweep_stale_test_data_dirs`])
/// both key on it, so they cannot drift into "creates one shape, collects another".
pub const TEST_DATA_DIR_PREFIX: &str = "ziee-test-data-";

/// Name of the liveness lock file inside each per-test data dir.
///
/// The owning process holds an exclusive `flock(2)` on this file for the whole life
/// of the directory. `flock` is the right primitive precisely because it is the
/// KERNEL that releases it: the lock dies with the file descriptor, and the
/// descriptor dies with the process — on a clean exit, on a panic, on `SIGKILL`, on
/// an OOM kill, on a hard timeout. There is no cooperation to forget and no record
/// to go stale. A sweeper that can take this lock therefore knows no live owner
/// exists, which is a fact about the present rather than a guess about the past.
pub const LIVE_LOCK_FILE: &str = ".harness-live";

/// Default age floor for [`sweep_stale_test_data_dirs`], in seconds.
///
/// Matches the database sweep's floor for the same reason: a per-test data dir lives
/// exactly one test's wall time, so two hours is two orders of magnitude of margin
/// over anything real. It is not a tuning knob — see the mixed-version note on
/// [`sweep_stale_test_data_dirs`] for the case where it, and not the lock, is the
/// only thing standing between the sweep and a live run.
pub const DEFAULT_SWEEP_MIN_AGE_SECS: u64 = 7200;

/// Upper bound on one sweep pass, so a badly-leaked `/tmp` costs a bounded startup
/// rather than minutes of unmount-and-delete. The sweep is self-healing across runs,
/// so a capped pass still converges.
const SWEEP_MAX_PER_PASS: usize = 400;

// ───────────────────────────── mount-table handling ─────────────────────────────

/// Un-escape the octal escapes the kernel writes into `/proc/self/mounts` for
/// characters that would otherwise break its space-separated format.
fn unescape_mount_field(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let oct = &raw[i + 1..i + 4];
            if let Ok(v) = u8::from_str_radix(oct, 8) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every mount point in `mounts_text` that lies at or under `root`, **deepest
/// first**.
///
/// Depth ordering is load-bearing, not tidiness: unmounting a parent before its
/// child fails with `EBUSY`, so a shallow-first pass would leave the deeper mounts —
/// and therefore the un-removable directory — exactly where it found them.
///
/// Split out as a pure function over the file's TEXT so the parsing (octal escapes,
/// the at-or-under test, the ordering) is testable without needing to create real
/// mounts, which needs privileges a unit test does not have.
pub fn mount_points_under(mounts_text: &str, root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = mounts_text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let _device = fields.next()?;
            let target = unescape_mount_field(fields.next()?);
            let target = PathBuf::from(target);
            target.starts_with(root).then_some(target)
        })
        .collect();
    // Deepest first. `components().count()` is the depth; the path itself is the
    // tie-break so the order is total and the function is deterministic.
    found.sort_by(|a, b| {
        b.components()
            .count()
            .cmp(&a.components().count())
            .then_with(|| a.cmp(b))
    });
    found.dedup();
    found
}

/// Read the live mount table. Linux-only; every other platform reports none, which
/// makes the unmount pass a no-op there rather than a compile error.
fn current_mount_points_under(root: &Path) -> Vec<PathBuf> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    match std::fs::read_to_string("/proc/self/mounts") {
        Ok(text) => mount_points_under(&text, root),
        Err(_) => Vec::new(),
    }
}

/// Unmount one mount point, trying the unprivileged FUSE path first.
///
/// `fusermount -u` is what a squashfuse mount needs and is what an ordinary user is
/// permitted to call; plain `umount(2)` on a FUSE mount from a non-root user is
/// `EPERM`. The lazy form is the last resort rather than the first: `-z` detaches the
/// name immediately but tears the mount down asynchronously, so reaching for it
/// eagerly would race the `remove_dir_all` that follows.
fn unmount_one(mount_point: &Path) -> bool {
    let attempts: [(&str, &[&str]); 3] = [
        ("fusermount", &["-u"]),
        ("fusermount3", &["-u"]),
        ("umount", &[]),
    ];
    for (bin, flags) in attempts {
        let ok = std::process::Command::new(bin)
            .args(flags)
            .arg(mount_point)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return true;
        }
    }
    // Last resort: detach lazily. Leaves no un-removable directory behind even when
    // something still holds the mount open, which is the property that matters here.
    std::process::Command::new("fusermount")
        .args(["-uz"])
        .arg(mount_point)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Unmount everything under `root`, deepest first. Returns how many mount points
/// were still listed after the pass (0 = the tree is now removable).
pub fn unmount_all_under(root: &Path) -> usize {
    for mp in current_mount_points_under(root) {
        unmount_one(&mp);
    }
    current_mount_points_under(root).len()
}

// ─────────────────────────────── the owned data dir ──────────────────────────────

/// Sole owner of one per-test `app.data_dir`, from `mkdir` until the tree is gone.
pub struct PerTestDataDir {
    path: PathBuf,
    /// The `flock`ed liveness file. Held open for the whole life of the directory —
    /// see [`LIVE_LOCK_FILE`]. `None` only if the lock could not be taken, which is
    /// never fatal: it costs the sweep its precise signal and falls back to the age
    /// floor, exactly as for a directory created by an older harness.
    _live_lock: Option<File>,
}

impl PerTestDataDir {
    /// Create a fresh per-test data dir under the system temp dir and take its
    /// liveness lock.
    pub fn new() -> io::Result<Self> {
        // `into_path` deliberately DISARMS `TempDir`'s own destructor. That
        // destructor — a `remove_dir_all` whose error is discarded — is the defect
        // this type exists to replace; leaving it armed would run a silent,
        // mount-blind removal immediately before ours.
        let path = tempfile::Builder::new()
            .prefix(TEST_DATA_DIR_PREFIX)
            .tempdir()?
            .keep();
        let live_lock = take_live_lock(&path);
        Ok(Self {
            path,
            _live_lock: live_lock,
        })
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Create and exclusively `flock` the liveness file inside `dir`.
fn take_live_lock(dir: &Path) -> Option<File> {
    let file = File::create(dir.join(LIVE_LOCK_FILE)).ok()?;
    lock_exclusive_nonblocking(&file).then_some(file)
}

/// `flock(fd, LOCK_EX | LOCK_NB)` — true when the lock was taken.
#[cfg(unix)]
fn lock_exclusive_nonblocking(file: &File) -> bool {
    use std::os::unix::io::AsRawFd;
    // SAFETY: `file` is a live, open descriptor for the duration of the call, and
    // `flock` has no other precondition.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

#[cfg(not(unix))]
fn lock_exclusive_nonblocking(_file: &File) -> bool {
    false
}

impl Drop for PerTestDataDir {
    /// Unmount, then remove, then REPORT — in that order, and never silently.
    fn drop(&mut self) {
        // DECISION — mirrors `ZIEE_TEST_KEEP_DB`: a deliberate, per-run opt-out for
        // someone debugging a test's on-disk state. It cannot re-open the leak,
        // because the startup sweep reclaims a kept directory on a later run.
        if std::env::var("ZIEE_TEST_KEEP_DATA_DIR").as_deref() == Ok("1") {
            eprintln!(
                "test harness: ZIEE_TEST_KEEP_DATA_DIR=1 — keeping {}",
                self.path.display()
            );
            return;
        }
        reclaim_data_dir(&self.path);
    }
}

/// Unmount everything under `dir`, remove the tree, and report a failure loudly.
///
/// Shared by [`PerTestDataDir::drop`] and [`sweep_stale_test_data_dirs`] so the
/// in-process reclaim and the next-process sweep cannot drift into removing
/// differently — the sweep would otherwise re-acquire the exact `ENOSYS` bug this
/// change removes.
fn reclaim_data_dir(dir: &Path) -> bool {
    let remaining = unmount_all_under(dir);
    match std::fs::remove_dir_all(dir) {
        Ok(()) => true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => true,
        Err(e) => {
            eprintln!(
                "test harness: LEAKED {} — remove failed after unmounting ({} mount point(s) \
                 still listed): {e}",
                dir.display(),
                remaining
            );
            false
        }
    }
}

// ──────────────────────────────────── the sweep ──────────────────────────────────

/// What one [`sweep_stale_test_data_dirs`] pass did.
#[derive(Debug, Default, Clone)]
pub struct DataDirSweepReport {
    /// Every directory the sweep judged abandoned and attempted to reclaim.
    ///
    /// This — not `removed` — is what the safety controls assert against. A sweep
    /// that has lost its liveness check would still usually FAIL to remove a live
    /// directory (its mounts are busy), so a control that only inspects `removed`
    /// goes green on exactly the mutation it exists to catch. `considered` is the
    /// set the sweep BELIEVED was dead, which is the thing under test.
    pub considered: Vec<PathBuf>,
    /// Successfully removed.
    pub removed: Vec<PathBuf>,
    /// Considered, but the removal did not fully succeed.
    pub refused: Vec<PathBuf>,
}

/// Exactly the shape [`PerTestDataDir::new`] generates: the prefix followed by
/// `tempfile`'s random alphanumeric run. Anything else is somebody else's directory
/// and is never a candidate — the sweep deletes trees, so the name test is the first
/// and cheapest containment on its blast radius.
fn is_engine_test_data_dir_name(name: &str) -> bool {
    match name.strip_prefix(TEST_DATA_DIR_PREFIX) {
        Some(rest) => !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()),
        None => false,
    }
}

/// True when no live process owns `dir`.
///
/// If the lock file is present, the answer is exact: taking the `flock` succeeds
/// only when every descriptor that held it is closed, and the kernel closes them
/// however the owner died. If the lock file is ABSENT the directory predates this
/// mechanism (or belongs to a concurrently-running OLDER harness build), and there
/// is no signal to read — the caller's age floor is the only guard, which is why it
/// is not optional.
fn appears_unowned(dir: &Path) -> bool {
    let lock_path = dir.join(LIVE_LOCK_FILE);
    match File::open(&lock_path) {
        Ok(f) => lock_exclusive_nonblocking(&f),
        Err(e) if e.kind() == io::ErrorKind::NotFound => true,
        // Unreadable for any other reason: treat as owned. Refusing to guess is the
        // safe direction; the cost is a directory that survives one more run.
        Err(_) => false,
    }
}

/// Reclaim per-test data dirs that no `Drop` will ever reach.
///
/// ## What it will and will not take
///
/// A directory is reclaimed only when ALL of:
///
///   1. it sits directly in the system temp dir and its name matches the anchored
///      [`TEST_DATA_DIR_PREFIX`] shape — never an arbitrary neighbour;
///   2. no live process holds its `flock` (see [`appears_unowned`]);
///   3. its modification time is older than `min_age_secs`.
///
/// **Neither (2) nor (3) is sufficient alone**, which is why both are required — the
/// same conclusion the database sweep reached, for a different reason on each side.
///
/// (2) alone is unsafe in the one case that matters on a SHARED box: a concurrently
/// running test process built from an OLDER harness creates no lock file at all, so
/// its very-much-live directory answers "unowned" trivially. The age floor is what
/// covers that mixed-version window, and it is the reason the floor is measured in
/// hours rather than minutes.
///
/// (3) alone is unsafe because mtime is a wall-clock guess about a stranger's
/// process — a long-running test that has simply not written for a while is
/// indistinguishable from a corpse. The lock is what makes the common case exact.
///
/// The removal itself goes through [`reclaim_data_dir`], so the sweep unmounts before
/// it deletes for the same reason the destructor does.
pub fn sweep_stale_test_data_dirs(min_age_secs: u64) -> DataDirSweepReport {
    sweep_dir_root(&std::env::temp_dir(), min_age_secs)
}

/// [`sweep_stale_test_data_dirs`] against an explicit root, so the harness's own
/// control tests can exercise the real predicate against a scratch directory instead
/// of the shared machine's `/tmp`.
pub fn sweep_dir_root(root: &Path, min_age_secs: u64) -> DataDirSweepReport {
    let mut report = DataDirSweepReport::default();
    let min_age = Duration::from_secs(min_age_secs);

    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("test harness: data-dir sweep could not read {root:?}: {e}");
            return report;
        }
    };

    for entry in entries.flatten() {
        if report.considered.len() >= SWEEP_MAX_PER_PASS {
            break;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // (1) the anchored name shape.
        if !is_engine_test_data_dir_name(name) {
            continue;
        }
        // `symlink_metadata`: a symlink named like a data dir is not a data dir, and
        // must never be followed into someone else's tree.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        // (3) the age floor.
        let old_enough = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age >= min_age);
        if !old_enough {
            continue;
        }
        // (2) no live owner. Checked LAST because it is the only predicate that
        // costs a syscall on a file, and the two cheap ones have already excluded
        // almost everything.
        if !appears_unowned(&path) {
            continue;
        }

        report.considered.push(path.clone());
        if reclaim_data_dir(&path) {
            report.removed.push(path);
        } else {
            report.refused.push(path);
        }
    }

    if !report.removed.is_empty() {
        eprintln!(
            "test harness: data-dir sweep reclaimed {} orphaned {TEST_DATA_DIR_PREFIX}* tree(s)",
            report.removed.len()
        );
    }
    report
}

static STARTUP_DATA_DIR_SWEEP_RAN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether the once-per-process data-dir sweep hook has run in THIS process.
///
/// Exists so a test can assert the sweep is INSTALLED, not merely callable. Every
/// other sweep test invokes [`sweep_dir_root`] directly and would stay green if the
/// call inside the harness's own startup were deleted — which is the half of this
/// fix that covers SIGKILL, OOM, `panic = "abort"` and binary timeouts.
pub fn startup_data_dir_sweep_ran() -> bool {
    STARTUP_DATA_DIR_SWEEP_RAN.load(std::sync::atomic::Ordering::SeqCst)
}

/// Run [`sweep_stale_test_data_dirs`] once per test process with the configured
/// floor. `ZIEE_TEST_DATA_DIR_SWEEP=0` disables it;
/// `ZIEE_TEST_DATA_DIR_SWEEP_MIN_AGE_SECS` overrides the floor.
pub fn sweep_stale_test_data_dirs_once() {
    // Recorded BEFORE the opt-out, because what this flag attests is that the
    // startup hook RAN — not what it decided. Deleting the call site must not leave
    // the direct-call tests green while the self-healing half is simply gone.
    STARTUP_DATA_DIR_SWEEP_RAN.store(true, std::sync::atomic::Ordering::SeqCst);
    if std::env::var("ZIEE_TEST_DATA_DIR_SWEEP").as_deref() == Ok("0") {
        return;
    }
    let min_age = std::env::var("ZIEE_TEST_DATA_DIR_SWEEP_MIN_AGE_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SWEEP_MIN_AGE_SECS);
    sweep_stale_test_data_dirs(min_age);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_engines_own_data_dir_names_are_admitted() {
        assert!(is_engine_test_data_dir_name("ziee-test-data-AbC123"));
        // The bare prefix is not a dir this harness made.
        assert!(!is_engine_test_data_dir_name("ziee-test-data-"));
        // Neighbours in /tmp that merely resemble one.
        assert!(!is_engine_test_data_dir_name("ziee-test-data-a/b"));
        assert!(!is_engine_test_data_dir_name("ziee-test-data-a.b"));
        assert!(!is_engine_test_data_dir_name("cyto-test-data-abc"));
        assert!(!is_engine_test_data_dir_name("tmp"));
        assert!(!is_engine_test_data_dir_name(""));
        // Not a PREFIX match on a longer unrelated name.
        assert!(!is_engine_test_data_dir_name("Xziee-test-data-abc"));
    }

    #[test]
    fn mount_points_are_selected_by_containment_and_ordered_deepest_first() {
        let root = Path::new("/tmp/ziee-test-data-AAA");
        let text = "\
squashfuse /tmp/ziee-test-data-AAA/sandbox-rootfs/dd/mnt fuse.squashfuse rw 0 0
tmpfs /tmp/ziee-test-data-AAA fuse rw 0 0
squashfuse /tmp/ziee-test-data-BBB/sandbox-rootfs/dd/mnt fuse.squashfuse rw 0 0
proc /proc proc rw 0 0
";
        let got = mount_points_under(text, root);
        assert_eq!(
            got,
            vec![
                PathBuf::from("/tmp/ziee-test-data-AAA/sandbox-rootfs/dd/mnt"),
                PathBuf::from("/tmp/ziee-test-data-AAA"),
            ],
            "only mounts under the root, deepest first"
        );
    }

    #[test]
    fn a_sibling_with_a_shared_name_prefix_is_not_under_the_root() {
        // `starts_with` on a Path is component-wise, so `-BBB` must not match `-AAA`
        // and, critically, `…-AAAX` must not match `…-AAA`. A textual prefix test
        // here would unmount a NEIGHBOURING live test's squashfuse.
        let root = Path::new("/tmp/ziee-test-data-AAA");
        let text = "squashfuse /tmp/ziee-test-data-AAAX/sandbox-rootfs/dd/mnt fuse rw 0 0\n";
        assert!(mount_points_under(text, root).is_empty());
    }

    #[test]
    fn octal_escapes_in_mount_points_are_decoded() {
        let root = Path::new("/tmp/ziee-test-data-AAA");
        let text = "squashfuse /tmp/ziee-test-data-AAA/a\\040b/mnt fuse rw 0 0\n";
        assert_eq!(
            mount_points_under(text, root),
            vec![PathBuf::from("/tmp/ziee-test-data-AAA/a b/mnt")]
        );
    }

    #[test]
    fn a_locked_dir_is_owned_and_an_unlocked_one_is_not() {
        let scratch = tempfile::tempdir().expect("scratch");
        let dir = scratch.path().join("ziee-test-data-Locked1");
        std::fs::create_dir(&dir).expect("mkdir");

        // No lock file yet: nothing to read, so the age floor is the only guard.
        assert!(appears_unowned(&dir), "absent lock file reads as unowned");

        let held = take_live_lock(&dir).expect("take lock");
        assert!(
            !appears_unowned(&dir),
            "a directory whose flock is held must read as OWNED"
        );

        drop(held);
        assert!(
            appears_unowned(&dir),
            "releasing the flock must make it collectable again"
        );
    }

    #[test]
    fn the_sweep_takes_an_old_unowned_dir_and_leaves_an_old_owned_one() {
        let scratch = tempfile::tempdir().expect("scratch");

        let dead = scratch.path().join("ziee-test-data-Dead001");
        std::fs::create_dir(&dead).expect("mkdir dead");
        // A lock file exists but nothing holds it — a process that died.
        drop(take_live_lock(&dead).expect("lock dead"));

        let live = scratch.path().join("ziee-test-data-Live001");
        std::fs::create_dir(&live).expect("mkdir live");
        let _held = take_live_lock(&live).expect("lock live");

        // Age floor 0 so BOTH are old enough: the only thing that can separate them
        // is the liveness lock. This is what makes the control kill a mutation that
        // deletes the `appears_unowned` check — with a non-zero floor the age
        // predicate would mask it.
        let report = sweep_dir_root(scratch.path(), 0);

        assert!(
            report.considered.contains(&dead),
            "an old, unlocked dir must be reclaimed; considered={:?}",
            report.considered
        );
        assert!(!dead.exists(), "and actually removed");
        assert!(
            !report.considered.contains(&live),
            "a LIVE run's dir must never even be considered; considered={:?}",
            report.considered
        );
        assert!(live.exists(), "and must still be there");
    }

    #[test]
    fn the_age_floor_alone_protects_a_young_dir_with_no_lock_file() {
        // The mixed-version case: a concurrently-running OLDER harness makes no lock
        // file, so `appears_unowned` says "unowned" for a directory that is very much
        // alive. Only the age floor stands between the sweep and that live run. This
        // control kills a mutation that drops the age predicate.
        let scratch = tempfile::tempdir().expect("scratch");
        let young = scratch.path().join("ziee-test-data-Young01");
        std::fs::create_dir(&young).expect("mkdir");
        assert!(appears_unowned(&young), "no lock file => reads as unowned");

        let report = sweep_dir_root(scratch.path(), DEFAULT_SWEEP_MIN_AGE_SECS);
        assert!(
            report.considered.is_empty(),
            "a freshly-created dir must be excluded by the age floor alone; considered={:?}",
            report.considered
        );
        assert!(young.exists());
    }

    #[test]
    fn the_sweep_never_considers_a_neighbour_that_is_not_ours() {
        let scratch = tempfile::tempdir().expect("scratch");
        for name in ["important-data", "ziee-test-data-", "cyto-test-data-abc"] {
            std::fs::create_dir(scratch.path().join(name)).expect("mkdir");
        }
        let report = sweep_dir_root(scratch.path(), 0);
        assert!(
            report.considered.is_empty(),
            "only the harness's own name shape is a candidate; considered={:?}",
            report.considered
        );
        for name in ["important-data", "ziee-test-data-", "cyto-test-data-abc"] {
            assert!(scratch.path().join(name).exists(), "{name} must survive");
        }
    }

    #[test]
    fn a_symlink_named_like_a_data_dir_is_never_followed() {
        // A sweep that used `metadata` instead of `symlink_metadata` would see a
        // directory here and `remove_dir_all` the TARGET's contents.
        let scratch = tempfile::tempdir().expect("scratch");
        let victim = scratch.path().join("victim");
        std::fs::create_dir(&victim).expect("mkdir victim");
        std::fs::write(victim.join("precious.txt"), b"keep me").expect("write");

        let link = scratch.path().join("ziee-test-data-Link001");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&victim, &link).expect("symlink");

        let report = sweep_dir_root(scratch.path(), 0);
        assert!(
            report.considered.is_empty(),
            "a symlink is not a data dir; considered={:?}",
            report.considered
        );
        assert!(victim.join("precious.txt").exists(), "target untouched");
        assert!(link.exists());
    }

    #[test]
    fn a_created_data_dir_is_locked_and_removed_on_drop() {
        let d = PerTestDataDir::new().expect("create");
        let path = d.path().to_path_buf();
        assert!(path.exists());
        assert!(
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_engine_test_data_dir_name),
            "the created name must be the shape the sweep collects"
        );
        assert!(
            !appears_unowned(&path),
            "a live PerTestDataDir must hold its liveness lock"
        );
        drop(d);
        assert!(!path.exists(), "drop must remove the tree");
    }
}
