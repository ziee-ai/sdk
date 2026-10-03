// Filesystem storage implementation

use super::{FileStorage, StorageResult};
use ziee_core::AppError;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

/// Reject reads that would follow a symlink. If the storage tree
/// somehow contains a symlink (planted by a co-located process, a
/// privilege escalation, or a future bug), refuse to read it rather
/// than silently following to arbitrary host paths. Closes
/// 05-file F-15 (Medium). NotFound is the same shape callers already
/// expect, so no surface-level change.
async fn reject_if_symlink(path: &Path) -> StorageResult<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if meta.file_type().is_symlink() => {
            tracing::error!(
                path = %path.display(),
                "Refusing to read storage path that is a symlink"
            );
            Err(AppError::not_found("File"))
        }
        // ENOENT → propagate as not_found later in the read call.
        // Other errors (permission denied, etc.) → propagate the same.
        _ => Ok(()),
    }
}

/// The outcome of [`open_regular_nofollow`]. Deliberately INFALLIBLE (no
/// `Err` arm anywhere this type is produced) — see that function's doc for
/// why: a type that cannot express "propagate" is what makes "every open
/// failure other than NotFound is this object's problem, never the whole
/// run's" true by construction instead of by convention.
#[derive(Debug)]
enum RegularFile {
    /// Opened, and confirmed FROM THE HANDLE (never a separate `stat`) to be
    /// a regular file.
    Open(tokio::fs::File),
    /// Nothing at the path (`ENOENT`) — the caller proceeds as if it were
    /// free to create.
    NotFound,
    /// Something is there but it is not safe to read as a plain regular
    /// file, OR it could not be opened/stat'd at all, for any reason OTHER
    /// than not existing — a symlink (`ELOOP`), a non-regular special file
    /// (FIFO, device, socket — `ENXIO` on Linux for a socket node), a
    /// permission error (`EACCES`), or anything else. The caller must treat
    /// this as a conflict for THIS ONE OBJECT — never read through it, never
    /// act on its say-so, and never let it abort the whole run — and should
    /// log the carried reason (the real `io::Error`/explanation text, not a
    /// generic claim).
    Refused(String),
}

/// Open `path` read-only with `O_NOFOLLOW` (unix only) — the raw, UNMAPPED
/// operation `open_regular_nofollow` builds on. Exists as its own function
/// (rather than inlined) so a test can assert the exact OS-level error
/// (`ELOOP` on a symlink leaf) that only a REAL `O_NOFOLLOW` open can
/// produce — a mechanism a check-then-open implementation could never be
/// mistaken for, unlike asserting on `open_regular_nofollow`'s mapped
/// `RegularFile::Refused` outcome, which a check-then-open implementation
/// reaches too (via its `symlink_metadata` check), just by a different route.
/// `O_NONBLOCK` keeps a planted FIFO from hanging the open.
#[cfg(unix)]
async fn open_nofollow(path: &Path) -> std::io::Result<tokio::fs::File> {
    tokio::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .await
}

/// Open `path` read-only and confirm it is a REGULAR file, atomically with
/// respect to a symlink swap landing between "check what's there" and "read
/// it" — and INFALLIBLY: every way this can fail other than "nothing there"
/// comes back as `RegularFile::Refused(reason)`, never a propagated `Err`.
///
/// `shard_flat_originals`'s dedup compare used to `symlink_metadata` the
/// shard target, then — only if that said "regular file" — `fs::read` it: two
/// syscalls with a gap an attacker able to write into the shard leaf could
/// land a symlink swap into. On unix this collapses to ONE syscall-level
/// operation via [`open_nofollow`]: `O_NOFOLLOW` makes `open` itself fail
/// with `ELOOP` if the leaf name is a symlink, so a symlink is refused before
/// a single byte is ever read through it. The `metadata()` check on the
/// returned HANDLE (not a fresh path lookup) is defense in depth against a
/// non-symlink special file that `O_NOFOLLOW` does not filter (a device
/// node, another FIFO opened non-blocking still reports as a FIFO).
///
/// Round 3's MEDIUM finding: an earlier version of this function mapped only
/// `NotFound` and `ELOOP`, letting any OTHER open error (`ENXIO` from a UNIX
/// socket node, `EACCES`, …) propagate as `Err` — the mover's `bail!` then
/// aborted the WHOLE migration over one object it could not open, where the
/// pre-`open_regular_nofollow` check-then-read code (a plain
/// `symlink_metadata`) would have skipped exactly such a target as a
/// conflict. This function is now infallible so that regression class cannot
/// recur: the return type has no `Err` variant to propagate through.
#[cfg(unix)]
async fn open_regular_nofollow(path: &Path) -> RegularFile {
    match open_nofollow(path).await {
        Ok(file) => match file.metadata().await {
            Ok(meta) if meta.file_type().is_file() => RegularFile::Open(file),
            Ok(meta) => {
                RegularFile::Refused(format!("not a regular file ({:?})", meta.file_type()))
            }
            Err(e) => RegularFile::Refused(format!("metadata after open failed: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => RegularFile::NotFound,
        // Any other open error — ELOOP (a symlink leaf), ENXIO (a UNIX
        // socket node; Linux refuses to open those at all), EACCES, or
        // anything else — means THIS ONE OBJECT cannot be safely opened as a
        // regular file. That is a conflict for this object, never a reason
        // to abort the whole migration.
        Err(e) => RegularFile::Refused(format!("open failed: {e}")),
    }
}

/// Non-unix fallback: no portable `O_NOFOLLOW` in `std`/`tokio`, so this stays
/// check-then-read (`symlink_metadata` then `fs::File::open`) — the same gap
/// as before `open_regular_nofollow` existed. Accepted because this storage
/// backend's shipped targets are unix servers; revisit if that changes.
/// Infallible for the same reason as the unix version above: any failure
/// other than "nothing there" is this object's conflict, never the run's.
#[cfg(not(unix))]
async fn open_regular_nofollow(path: &Path) -> RegularFile {
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if !meta.file_type().is_file() => {
            RegularFile::Refused(format!("not a regular file ({:?})", meta.file_type()))
        }
        Ok(_) => match tokio::fs::File::open(path).await {
            Ok(file) => RegularFile::Open(file),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RegularFile::NotFound,
            Err(e) => RegularFile::Refused(format!("open failed: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => RegularFile::NotFound,
        Err(e) => RegularFile::Refused(format!("stat failed: {e}")),
    }
}

/// The outcome of comparing ONE flat object (`from`) against its shard
/// target (`to`) — the single decision procedure both
/// [`FilesystemStorage::shard_flat_originals`]'s mover and
/// [`FilesystemStorage::census`] resolve a flat entry through (via
/// [`classify_against_target`]), so the two can never classify the same
/// object differently. FIX_ROUND-6.md: this is what replaced a per-pass
/// descriptive snapshot that drifted from reality on every early-return path
/// — there is now exactly one place that decides refused/conflict/identical
/// for a given pair, called by both the actor (the mover) and the observer
/// (the census).
#[derive(Debug)]
enum Classification {
    /// No shard target exists yet.
    TargetAbsent,
    /// The shard target exists but could not be opened and confirmed as a
    /// regular file (symlink, directory, FIFO/device/socket, permission
    /// error, ...). No bytes were ever compared.
    TargetRefused(String),
    /// The target opened fine, but the flat source itself vanished (a racing
    /// delete) between being listed and this check. Nothing to compare,
    /// nothing to move — not a refusal, not a conflict.
    SourceVanished,
    /// The target opened fine, but the flat source could not be opened and
    /// confirmed as a regular file. No bytes were ever compared.
    SourceRefused(String),
    /// Both sides opened fine as regular files and were byte-compared.
    Compared { identical: bool },
}

/// Resolve [`Classification`] for flat object `from` against its shard
/// target `to`, using [`open_regular_nofollow`] on BOTH sides — unix:
/// `O_NOFOLLOW`, atomic with the symlink check, never a `symlink_metadata`
/// probe followed by a separate `fs::read` (the gap a symlink swapped in
/// between those two steps could use to make an attacker-controlled target
/// read as "identical" and get the REAL flat object unlinked out from under
/// it). Never mutates anything — opens for read only.
async fn classify_against_target(to: &Path, from: &Path) -> Classification {
    match open_regular_nofollow(to).await {
        RegularFile::NotFound => Classification::TargetAbsent,
        RegularFile::Refused(reason) => Classification::TargetRefused(reason),
        RegularFile::Open(mut to_file) => match open_regular_nofollow(from).await {
            RegularFile::NotFound => Classification::SourceVanished,
            RegularFile::Refused(reason) => Classification::SourceRefused(reason),
            RegularFile::Open(mut from_file) => {
                let mut a = Vec::new();
                let mut b = Vec::new();
                let identical = matches!(
                    (
                        from_file.read_to_end(&mut a).await,
                        to_file.read_to_end(&mut b).await,
                    ),
                    (Ok(_), Ok(_))
                ) && a == b;
                Classification::Compared { identical }
            }
        },
    }
}

/// Where NEW originals are written under `originals/<user>/`.
///
/// Reads and deletes are layout-agnostic: they look in BOTH places
/// ([`FileStorage::resolve_original_path`], [`FileStorage::delete_original`]),
/// so a store can be migrated from `Flat` to `Sharded` while it is serving.
///
/// - `Flat` — `originals/<user>/<id>.<ext>`: one directory per user. Fine for
///   a per-user upload directory; one enormous directory for a store that
///   keeps millions of objects under a single namespace id (every `readdir`
///   tool is O(n) there, and ext4 without `large_dir` caps one directory at
///   roughly 10–15M entries).
/// - `Sharded` — `originals/<user>/<id[0..2]>/<id[2..4]>/<id>.<ext>`, where
///   `id` is the hyphenated UUID string, so the shard key is its first four
///   hex digits: 65 536 leaf directories that fill evenly for v4 ids. **If ids
///   ever become UUIDv7** (time-ordered prefix) the key must come from the
///   random tail instead, or a whole day lands in one leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OriginalsLayout {
    #[default]
    Flat,
    Sharded,
}

/// Knobs for [`FilesystemStorage::shard_flat_originals`].
#[derive(Debug, Clone)]
pub struct ShardOptions {
    /// Moves between progress callbacks and pauses.
    pub batch: usize,
    /// Sleep after each batch, so a backlog of millions of renames does not
    /// starve serving I/O.
    pub pause: std::time::Duration,
    /// Stop after this many objects were moved (a partial run); `None` ⇒
    /// until nothing flat is left.
    pub limit: Option<u64>,
    /// Bound on the number of `read_dir` passes a single call makes. The loop
    /// normally stops when a pass moves nothing, but a sustained concurrent
    /// flat writer (a not-yet-restarted server still writing flat) can keep
    /// adding entries forever, so without a ceiling the call would never
    /// return. [`ShardCensus::remaining_flat`] reports what is still flat
    /// when the bound is hit; a follow-up call continues from there.
    pub max_passes: usize,
}

impl Default for ShardOptions {
    fn default() -> Self {
        Self {
            batch: 2000,
            pause: std::time::Duration::from_millis(50),
            limit: None,
            max_passes: 3,
        }
    }
}

/// A point-in-time snapshot of what [`FilesystemStorage::census`] found still
/// sitting in the flat directory, produced by walking it ONCE, read-only,
/// classifying every entry with the EXACT SAME predicates
/// [`FilesystemStorage::shard_flat_originals`]'s mover uses for its own
/// open/compare decisions (so "what the mover would do with this entry" and
/// "what the census says about it" can never drift apart by construction —
/// there is only one classification function, not two kept in sync by hand).
///
/// **This is the ONLY place these fields are ever produced.** The mover's
/// pass loop never constructs, mutates, or carries one of these between
/// passes — see [`ShardReport`]'s doc comment for why that used to be a
/// recurring bug class (FIX_ROUND-6.md).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShardCensus {
    /// Flat object files present right now. Zero ⇒ the directory is fully
    /// migrated. `conflicts + refused` is a LOWER bound on this (a flat
    /// object whose shard target is simply absent — not yet visited by a
    /// mover, or left by a `limit`ed/bounded run — counts here but is
    /// neither a conflict nor a refusal).
    pub remaining_flat: u64,
    /// Flat objects whose shard target and flat source BOTH opened fine as
    /// regular files, but hold DIFFERENT bytes — both left in place, for a
    /// human to decide. An object whose target or source could not even be
    /// OPENED as a regular file at all (a symlink, a directory, a socket, a
    /// permission error, ...) is never counted here — see `refused`.
    pub conflicts: u64,
    /// Flat objects whose shard target or flat source exists but could not
    /// be opened and confirmed as a regular file — a symlink, a directory, a
    /// FIFO/device/socket node, a permission error, or any other `open`
    /// failure other than "nothing there". No bytes were ever compared for
    /// these — distinct from `conflicts`, which is only a byte-different pair
    /// that both opened fine. Both sides are left on disk either way.
    pub refused: u64,
    /// A bounded, FIRST-SEEN sample of `refused`'s reasons, capped at 5 —
    /// enough for a human to tell at a glance WHAT is being refused (every
    /// sample reading "Too many levels of symbolic links" is a planted
    /// symlink; every one reading "Permission denied" is a permission/fd
    /// problem) without this growing unbounded over a large store.
    pub refused_samples: Vec<String>,
    /// Directory entries that are not a regular `<uuid>.<ext>` object file
    /// (symlinks, foreign names; sub-directories are the shard levels and are
    /// not counted) — never touched.
    pub skipped: u64,
}

/// What one [`FilesystemStorage::shard_flat_originals`] call did, plus what
/// the directory looked like when it finished.
///
/// **Re-scoped in FIX_ROUND-6.md (phase-7 ABORT — the fix loop stopped
/// converging across rounds 2–5, every finding landing in this struct's OWN
/// bookkeeping, never in what happens to the files).** The root cause was a
/// CLASS, not a field: a per-pass descriptive snapshot (`conflicts`,
/// `refused`, `refused_samples`, `skipped`, `remaining_flat`) held as locals
/// and copied into the report at chosen moments — "the last completed pass",
/// "the instant a refusal is seen" — drifts from both reality and each other
/// on every early-return path, and each round's fix just relocated the copy
/// point to a different early-return.
///
/// The fix removes the class by construction. Exactly two fields are
/// EVENT counters: `moved` and `deduplicated`, bumped the instant the
/// action happens and NEVER reset, reassigned, or copied — the pass loop
/// touches nothing else. Everything descriptive — "what does the flat
/// directory look like" — lives in [`ShardCensus`], produced by exactly ONE
/// function, [`FilesystemStorage::census`], which walks the directory
/// ONCE, independently of the pass loop, and never mid-run: `census` is
/// `None` for every progress callback during the run (there is no
/// "in-progress" descriptive state to read — the type cannot express a
/// stale one) and is filled exactly once, at the very end — on a normal
/// finish AND on an aborting I/O error alike. If the end-of-run census
/// itself cannot run (e.g. the same abort also broke directory access),
/// `census` stays `None` — UNKNOWN, which a caller must not confuse with
/// "zero found" / "fully converged".
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShardReport {
    /// Flat objects renamed into their shard leaf. Cumulative across every
    /// pass this call made; bumped the instant a rename succeeds.
    pub moved: u64,
    /// Flat objects whose shard target already held IDENTICAL bytes — the
    /// flat copy was removed. Cumulative across every pass this call made;
    /// bumped the instant the removal succeeds.
    pub deduplicated: u64,
    /// What [`FilesystemStorage::census`] found in the flat directory at the
    /// END of this call — `None` only when that end-of-run census itself
    /// could not complete (UNKNOWN, never to be read as zero/converged).
    pub census: Option<ShardCensus>,
}

/// Filesystem-based file storage
#[derive(Debug, Clone)]
pub struct FilesystemStorage {
    base_path: PathBuf,
    layout: OriginalsLayout,
}

impl FilesystemStorage {
    /// Create new filesystem storage (the `Flat` originals layout).
    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self::with_layout(base_path, OriginalsLayout::Flat)
    }

    /// Create filesystem storage that writes NEW originals in `layout`.
    pub fn with_layout(base_path: impl AsRef<Path>, layout: OriginalsLayout) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            layout,
        }
    }

    /// The layout NEW originals are written in.
    pub fn layout(&self) -> OriginalsLayout {
        self.layout
    }

    /// The extension sanitiser every original path goes through, whatever the
    /// layout.
    ///
    /// SECURITY: the extension flows from user input on upload. Without
    /// sanitization, a value like 'x/../../<victim_uuid>/y.pdf' lets
    /// create_dir_all + fs::write escape the per-user originals
    /// directory and overwrite (or shadow) another user's file. Same
    /// primitive on the read path. Closes 05-file F-03 (High).
    ///
    /// Allow only ASCII alphanumeric extensions (matches every legit
    /// mime-type-derived extension). Anything else is replaced with
    /// 'bin', which keeps the file storable but isolated.
    fn safe_extension(extension: &str) -> String {
        if extension.chars().all(|c| c.is_ascii_alphanumeric())
            && !extension.is_empty()
            && extension.len() <= 16
        {
            extension.to_ascii_lowercase()
        } else {
            "bin".to_string()
        }
    }

    /// `originals/<user>/<id>.<ext>` — the `Flat` location.
    pub fn flat_original_path(&self, user_id: Uuid, file_id: Uuid, extension: &str) -> PathBuf {
        self.get_user_path(user_id, "originals")
            .join(format!("{}.{}", file_id, Self::safe_extension(extension)))
    }

    /// `originals/<user>/<id[0..2]>/<id[2..4]>/` — an object's shard leaf.
    fn shard_dir(&self, user_id: Uuid, file_id: Uuid) -> PathBuf {
        let id = file_id.to_string();
        self.get_user_path(user_id, "originals")
            .join(&id[0..2])
            .join(&id[2..4])
    }

    /// `originals/<user>/<id[0..2]>/<id[2..4]>/<id>.<ext>` — the `Sharded`
    /// location.
    pub fn sharded_original_path(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> PathBuf {
        self.shard_dir(user_id, file_id)
            .join(format!("{}.{}", file_id, Self::safe_extension(extension)))
    }

    /// Move every FLAT original of `user_id` into its shard leaf — the one-time
    /// migration of a store written before `Sharded` existed.
    ///
    /// Streams `read_dir` of `originals/<user>/` (the last O(n) pass over the
    /// flat directory) and, for each REGULAR file named `<uuid>.<ext>`,
    /// `rename`s it into its leaf. Anything else — a symlink, a directory (the
    /// shard levels themselves), a foreign name — is never touched. Rename is
    /// atomic within one filesystem, so a crash at any point leaves each object
    /// at exactly one of its two paths, where the dual reader finds it; a
    /// re-run continues with whatever is still flat. A target that already
    /// exists is compared byte for byte: identical ⇒ the flat copy is removed;
    /// different ⇒ both are left and logged.
    ///
    /// A delete racing the move cannot resurrect its object:
    /// [`FileStorage::delete_original`] unlinks the flat name, then the sharded
    /// one, and `rename`/`unlink` of one name are atomic with respect to each
    /// other — either the unlink wins (the rename then fails `ENOENT` and is
    /// skipped) or the rename wins (the sharded unlink, which runs after, hits).
    ///
    /// Both sides of the byte-compare go through [`classify_against_target`],
    /// which opens each side with [`open_regular_nofollow`] (unix: `O_NOFOLLOW`,
    /// atomic with the symlink check) rather than a `symlink_metadata` probe
    /// followed by a separate `fs::read`: if either side exists and is NOT a
    /// regular file — a symlink, a directory, anything else — it is never
    /// opened and the flat copy is never unlinked on its say-so. A
    /// check-then-read pair would leave a gap for a symlink swapped in between
    /// the two steps; `open_regular_nofollow` has none. Without this, a
    /// symlink planted at the shard target pointing at bytes identical to the
    /// flat copy would make the byte-compare report "same" and the mover would
    /// unlink the REAL flat object, leaving only the attacker-controlled
    /// symlink behind.
    ///
    /// Bounded by `opts.max_passes`: a sustained concurrent flat writer can
    /// keep adding entries to the directory forever, so without a ceiling
    /// this would never return. [`ShardCensus::remaining_flat`] (in
    /// [`ShardReport::census`]) reports what is left when the bound is hit
    /// (or a `limit`/conflict stopped it early); a follow-up call continues
    /// from there.
    ///
    /// **Reporting model (FIX_ROUND-6.md — re-scoped after the fix loop
    /// stopped converging: rounds 2–5 each found issues ONLY in this
    /// function's OWN bookkeeping, never in what happens to the files,
    /// because the root cause was a per-pass descriptive snapshot held as
    /// locals and copied into the report at a chosen moment — a CLASS of
    /// bug, fixed here by removing the class rather than patching another
    /// copy point).** This function's pass loop now touches exactly two
    /// fields on `report`: `moved` and `deduplicated`, bumped the instant the
    /// action happens and never reset or reassigned. It carries NO
    /// descriptive per-pass locals at all — no `conflicts`, `refused`,
    /// `refused_samples`, `skipped`, or `remaining_flat` tracking of any
    /// kind. Every `progress` callback made DURING the pass loop therefore
    /// reports `report.census == None` — there is no in-progress descriptive
    /// state to read, so there is no stale copy of it to leak.
    ///
    /// At the END of the run — whether the loop finished normally or an I/O
    /// error aborted it — [`Self::census`] walks the flat directory ONCE,
    /// fresh, independent of anything the pass loop tracked, and its result
    /// becomes `report.census`. On an aborting error, the census is still
    /// attempted (best-effort): if it succeeds, the report describes the
    /// directory exactly as it stands after the abort; if it ALSO fails
    /// (e.g. the same permission/I-O problem that aborted the mover blocks
    /// the census's own `read_dir` too), `report.census` stays `None` —
    /// UNKNOWN, never a zero that a caller could misread as "converged" —
    /// and the MOVER's own error (never the census's) is what propagates. If
    /// the run finishes normally but the end-of-run census itself fails,
    /// `report.census` stays `None` for the same reason and the census's
    /// error (the only one there is) propagates instead.
    pub async fn shard_flat_originals(
        &self,
        user_id: Uuid,
        opts: &ShardOptions,
        progress: &mut (dyn FnMut(&ShardReport) + Send),
    ) -> std::io::Result<ShardReport> {
        let dir = self.get_user_path(user_id, "originals");
        let mut report = ShardReport::default();
        let mut leaves: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let batch = opts.batch.max(1) as u64;
        let mut since_pause = 0u64;
        let max_passes = opts.max_passes.max(1);
        let mut passes = 0usize;

        // On ANY abort — a real I/O error anywhere below — attempt the SAME
        // end-of-run census a normal finish gets (best-effort: `.ok()`
        // leaves `report.census` at `None`, i.e. UNKNOWN, if the census
        // itself can't run either), hand `progress` the result, and
        // propagate the MOVER's error — never the census's. There is no
        // per-pass descriptive local left to get out of sync with reality:
        // the loop below never builds one.
        macro_rules! abort {
            ($e:expr) => {{
                report.census = self.census(user_id).await.ok();
                progress(&report);
                return Err($e);
            }};
        }

        // Passes until one moves nothing (or the bound is hit): a directory
        // being renamed out of while it is read may skip entries, and a
        // concurrent writer may add flat ones — the next pass picks both up.
        loop {
            passes += 1;
            let mut moved_this_pass = 0u64;
            let mut entries = match fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Nothing ever existed for this user — an empty,
                    // converged census, same shape `census` itself would
                    // report against an absent directory.
                    report.census = Some(ShardCensus::default());
                    return Ok(report);
                }
                Err(e) => abort!(e),
            };
            loop {
                let entry = match entries.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(e) => abort!(e),
                };
                if opts.limit.is_some_and(|l| report.moved >= l) {
                    continue;
                }
                let Some((file_id, ext)) = Self::flat_object_name(&entry).await else {
                    continue;
                };
                let from = entry.path();
                let to = self.sharded_original_path(user_id, file_id, &ext);
                let leaf = self.shard_dir(user_id, file_id);
                if !leaves.contains(&leaf) {
                    if let Err(e) = fs::create_dir_all(&leaf).await {
                        abort!(e);
                    }
                    leaves.insert(leaf);
                }
                match classify_against_target(&to, &from).await {
                    Classification::TargetAbsent => {}
                    Classification::TargetRefused(reason) => {
                        // INV-6: never read through it, never unlink the flat
                        // copy on its say-so. Never fatal to the run — a
                        // planted special file must not be able to halt
                        // every future re-run. The end-of-run census counts
                        // and samples this; the pass loop tracks nothing
                        // about it beyond this log line.
                        tracing::warn!(
                            flat = %from.display(),
                            sharded = %to.display(),
                            reason = %reason,
                            "shard move: target exists but could not be safely opened as \
                             a regular file — left the flat copy in place; the end-of-run \
                             census will count it"
                        );
                        continue;
                    }
                    Classification::SourceVanished => {
                        // A racing delete got there first between the
                        // directory listing and here — nothing to compare,
                        // nothing to move, matches the rename-`NotFound` path
                        // below.
                        continue;
                    }
                    Classification::SourceRefused(reason) => {
                        tracing::warn!(
                            flat = %from.display(),
                            sharded = %to.display(),
                            reason = %reason,
                            "shard move: flat source exists but could not be safely opened \
                             as a regular file — left both in place; the end-of-run census \
                             will count it"
                        );
                        continue;
                    }
                    Classification::Compared { identical: true } => {
                        match fs::remove_file(&from).await {
                            Ok(()) => report.deduplicated += 1,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                            Err(e) => abort!(e),
                        }
                        continue;
                    }
                    Classification::Compared { identical: false } => {
                        tracing::warn!(
                            flat = %from.display(),
                            sharded = %to.display(),
                            "shard move: both locations hold DIFFERENT bytes for one object \
                             — left both in place; the end-of-run census will count it as a \
                             conflict"
                        );
                        continue;
                    }
                }
                match fs::rename(&from, &to).await {
                    Ok(()) => {
                        report.moved += 1;
                        moved_this_pass += 1;
                    }
                    // Deleted (or moved) under us — nothing left to move.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => abort!(e),
                }
                since_pause += 1;
                if since_pause >= batch {
                    since_pause = 0;
                    progress(&report);
                    tokio::time::sleep(opts.pause).await;
                }
            }
            if moved_this_pass == 0
                || opts.limit.is_some_and(|l| report.moved >= l)
                || passes >= max_passes
            {
                break;
            }
        }
        match self.census(user_id).await {
            Ok(c) => {
                report.census = Some(c);
                progress(&report);
                Ok(report)
            }
            Err(e) => {
                // The run itself finished; only the END-OF-RUN census
                // failed. `report.census` stays `None` (UNKNOWN, never a
                // zero that reads as "converged") and the census's own error
                // — the only one there is here — propagates.
                progress(&report);
                Err(e)
            }
        }
    }

    /// Walk `originals/<user>/` ONCE, read-only, and report what is still
    /// flat — the ONLY function that ever produces a [`ShardCensus`]. Uses
    /// the EXACT SAME [`classify_against_target`] decision procedure
    /// [`Self::shard_flat_originals`]'s mover uses for its own open/compare
    /// step, so "what the mover would do with this entry" and "what the
    /// census says about it" are the same computation, never two
    /// hand-synchronized ones. Never renames, removes, or creates anything.
    ///
    /// Safe to call at any time, including while a mover is running
    /// concurrently (it is read-only), though a report taken mid-run is a
    /// snapshot of that moment, not a prediction of the run's eventual
    /// outcome.
    pub async fn census(&self, user_id: Uuid) -> std::io::Result<ShardCensus> {
        let dir = self.get_user_path(user_id, "originals");
        let mut c = ShardCensus::default();
        let mut entries = match fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(c),
            Err(e) => return Err(e),
        };
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(e) => return Err(e),
            };
            let Some((file_id, ext)) = Self::flat_object_name(&entry).await else {
                match entry.file_type().await {
                    Ok(t) if t.is_dir() => {}
                    _ => c.skipped += 1,
                }
                continue;
            };
            let from = entry.path();
            let to = self.sharded_original_path(user_id, file_id, &ext);
            match classify_against_target(&to, &from).await {
                Classification::TargetAbsent => c.remaining_flat += 1,
                Classification::TargetRefused(reason) => {
                    c.remaining_flat += 1;
                    c.refused += 1;
                    if c.refused_samples.len() < 5 {
                        c.refused_samples.push(format!("target: {reason}"));
                    }
                }
                // The flat entry this census just listed vanished (a racing
                // delete) between the listing and here — it is no longer
                // there to count.
                Classification::SourceVanished => {}
                Classification::SourceRefused(reason) => {
                    c.remaining_flat += 1;
                    c.refused += 1;
                    if c.refused_samples.len() < 5 {
                        c.refused_samples.push(format!("source: {reason}"));
                    }
                }
                // Identical ⇒ a dedup opportunity not yet collected by a
                // mover pass; different ⇒ a genuine conflict. Either way the
                // object is still flat.
                Classification::Compared { identical } => {
                    c.remaining_flat += 1;
                    if !identical {
                        c.conflicts += 1;
                    }
                }
            }
        }
        Ok(c)
    }

    /// `Some((id, ext))` when `entry` is a REGULAR file named `<uuid>.<ext>`
    /// with an extension the sanitiser keeps as-is — i.e. an object this
    /// storage could have written flat. `file_type` does not follow symlinks.
    async fn flat_object_name(entry: &fs::DirEntry) -> Option<(Uuid, String)> {
        if !entry.file_type().await.ok()?.is_file() {
            return None;
        }
        let name = entry.file_name();
        let name = name.to_str()?;
        let (stem, ext) = name.split_once('.')?;
        let id = Uuid::parse_str(stem).ok()?;
        if id.to_string() != stem || Self::safe_extension(ext) != ext {
            return None;
        }
        Some((id, ext.to_string()))
    }

    /// Ensure directory exists
    async fn ensure_dir(&self, path: &Path) -> StorageResult<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| {
                tracing::error!(error = %e, "ensure_dir failed");
                AppError::internal_error("Storage error")
            })?;
        }
        Ok(())
    }

    /// Get base path for user
    fn get_user_path(&self, user_id: Uuid, subdir: &str) -> PathBuf {
        self.base_path.join(subdir).join(user_id.to_string())
    }
}

#[async_trait]
impl FileStorage for FilesystemStorage {
    async fn save_original(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
        data: &[u8],
    ) -> StorageResult<PathBuf> {
        let path = self.get_original_path(user_id, file_id, extension);
        self.ensure_dir(&path).await?;

        fs::write(&path, data)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "save_original write failed");
                AppError::internal_error("Storage error")
            })?;

        Ok(path)
    }

    async fn save_text_page(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        text: &str,
    ) -> StorageResult<PathBuf> {
        let path = self.get_text_path(user_id, file_id, page_num);
        self.ensure_dir(&path).await?;

        fs::write(&path, text)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "save_text_page write failed");
                AppError::internal_error("Storage error")
            })?;

        Ok(path)
    }

    async fn save_geometry_page(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        geometry_json: &str,
    ) -> StorageResult<PathBuf> {
        let path = self
            .get_user_path(user_id, "geometry")
            .join(file_id.to_string())
            .join(format!("page_{}.json", page_num));
        self.ensure_dir(&path).await?;
        fs::write(&path, geometry_json).await.map_err(|e| {
            tracing::error!(error = %e, "save_geometry_page write failed");
            AppError::internal_error("Storage error")
        })?;
        Ok(path)
    }

    async fn load_geometry_page(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
    ) -> StorageResult<String> {
        let path = self
            .get_user_path(user_id, "geometry")
            .join(file_id.to_string())
            .join(format!("page_{}.json", page_num));
        reject_if_symlink(&path).await?;
        fs::read_to_string(&path).await.map_err(|e| {
            tracing::warn!(error = %e, page = page_num, "load_geometry_page failed");
            AppError::not_found("Geometry page")
        })
    }

    async fn save_image(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        is_thumbnail: bool,
        data: &[u8],
    ) -> StorageResult<PathBuf> {
        let path = self.get_image_path(user_id, file_id, page_num, is_thumbnail);
        self.ensure_dir(&path).await?;

        fs::write(&path, data)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "save_image write failed");
                AppError::internal_error("Storage error")
            })?;

        Ok(path)
    }

    fn get_original_path(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> PathBuf {
        // Both layouts go through `safe_extension` (the F-03 sanitiser).
        match self.layout {
            OriginalsLayout::Flat => self.flat_original_path(user_id, file_id, extension),
            OriginalsLayout::Sharded => self.sharded_original_path(user_id, file_id, extension),
        }
    }

    fn get_text_path(&self, user_id: Uuid, file_id: Uuid, page_num: u32) -> PathBuf {
        self.get_user_path(user_id, "text")
            .join(file_id.to_string())
            .join(format!("page_{}.txt", page_num))
    }

    fn get_image_path(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
        is_thumbnail: bool,
    ) -> PathBuf {
        if is_thumbnail {
            // Single thumbnail: thumbnails/{user_id}/{file_id}.jpg
            self.get_user_path(user_id, "thumbnails")
                .join(format!("{}.jpg", file_id))
        } else {
            // Multiple images: images/{user_id}/{file_id}/page_N.jpg
            self.get_user_path(user_id, "images")
                .join(file_id.to_string())
                .join(format!("page_{}.jpg", page_num))
        }
    }

    /// Sharded, else flat, else sharded once more — the last probe closes the
    /// race with a concurrent mover that renamed the object between the first
    /// two. Whatever the write layout, so a store being migrated reads both.
    /// A symlink at a probed location is REFUSED (never followed, never
    /// skipped past): it means the tree was tampered with.
    async fn resolve_original_path(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> Option<PathBuf> {
        let sharded = self.sharded_original_path(user_id, file_id, extension);
        let flat = self.flat_original_path(user_id, file_id, extension);
        for path in [&sharded, &flat, &sharded] {
            match fs::symlink_metadata(path).await {
                Ok(meta) if meta.file_type().is_symlink() => {
                    tracing::error!(
                        path = %path.display(),
                        "Refusing to resolve storage path that is a symlink"
                    );
                    return None;
                }
                Ok(meta) if meta.file_type().is_file() => return Some(path.clone()),
                _ => {}
            }
        }
        None
    }

    /// Flat first, then sharded — the order that makes a racing mover unable
    /// to resurrect the object (see `shard_flat_originals`).
    async fn delete_original(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> std::io::Result<bool> {
        let mut removed = false;
        for path in [
            self.flat_original_path(user_id, file_id, extension),
            self.sharded_original_path(user_id, file_id, extension),
        ] {
            match fs::remove_file(&path).await {
                Ok(()) => removed = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(std::io::Error::new(
                        e.kind(),
                        format!("remove {}: {e}", path.display()),
                    ))
                }
            }
        }
        Ok(removed)
    }

    async fn load_original(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        extension: &str,
    ) -> StorageResult<Vec<u8>> {
        let path = self
            .resolve_original_path(user_id, file_id, extension)
            .await
            .unwrap_or_else(|| self.get_original_path(user_id, file_id, extension));
        reject_if_symlink(&path).await?;
        fs::read(&path)
            .await
            .map_err(|e| {
                tracing::warn!(
                    error = %e,
                    base = %self.base_path.display(),
                    path = %path.display(),
                    %user_id,
                    %file_id,
                    extension,
                    "load_original failed"
                );
                AppError::not_found("File")
            })
    }

    async fn load_text_page(&self, user_id: Uuid, file_id: Uuid, page_num: u32) -> StorageResult<String> {
        let path = self.get_text_path(user_id, file_id, page_num);
        reject_if_symlink(&path).await?;
        fs::read_to_string(&path)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, page = page_num, "load_text_page failed");
                AppError::not_found("Text page")
            })
    }

    async fn load_preview(
        &self,
        user_id: Uuid,
        file_id: Uuid,
        page_num: u32,
    ) -> StorageResult<Vec<u8>> {
        let path = self.get_image_path(user_id, file_id, page_num, false);
        reject_if_symlink(&path).await?;
        fs::read(&path)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "load_preview failed");
                AppError::not_found("Preview")
            })
    }

    async fn load_thumbnail(&self, user_id: Uuid, file_id: Uuid) -> StorageResult<Vec<u8>> {
        let path = self.get_image_path(user_id, file_id, 1, true);
        reject_if_symlink(&path).await?;
        fs::read(&path)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "load_thumbnail failed");
                AppError::not_found("Thumbnail")
            })
    }

    async fn delete_all(&self, user_id: Uuid, file_id: Uuid) -> StorageResult<()> {
        // Delete from all possible locations
        let locations = vec![
            ("originals", None),
            ("text", Some(file_id.to_string())),   // Directory with text pages
            ("images", Some(file_id.to_string())), // Directory with image pages
        ];

        for (subdir, file_subdir) in locations {
            let mut path = self.get_user_path(user_id, subdir);
            if let Some(ref subdir_name) = file_subdir {
                path = path.join(subdir_name);
                // Delete entire directory
                if path.exists() {
                    let _ = fs::remove_dir_all(&path).await;
                }
            } else {
                // Delete files matching pattern — in the flat directory and in
                // the id's shard leaf (a sharded or migrated object).
                for dir in [path.clone(), self.shard_dir(user_id, file_id)] {
                    if let Ok(mut entries) = fs::read_dir(&dir).await {
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            if let Some(name) = entry.file_name().to_str()
                                && name.starts_with(&file_id.to_string()) {
                                    let _ = fs::remove_file(entry.path()).await;
                                }
                        }
                    }
                }
            }
        }

        // Delete single thumbnail file: thumbnails/{user_id}/{file_id}.jpg
        let thumbnail_path = self.get_user_path(user_id, "thumbnails")
            .join(format!("{}.jpg", file_id));
        if thumbnail_path.exists() {
            let _ = fs::remove_file(&thumbnail_path).await;
        }

        Ok(())
    }

    async fn delete_user_dirs(&self, user_id: Uuid) -> StorageResult<()> {
        // Remove the per-user directory under every storage subdir so deleting
        // a user leaves no orphaned (even empty) dirs behind.
        for subdir in ["originals", "text", "images", "thumbnails"] {
            let path = self.get_user_path(user_id, subdir);
            if path.exists() {
                let _ = fs::remove_dir_all(&path).await;
            }
        }
        Ok(())
    }

    fn calculate_checksum(&self, data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        hex::encode(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> (tempfile::TempDir, FilesystemStorage) {
        let dir = tempfile::tempdir().unwrap();
        let s = FilesystemStorage::new(dir.path());
        (dir, s)
    }

    #[tokio::test]
    async fn save_then_load_original_roundtrips_bytes() {
        let (_dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();
        let data = b"hello core file bytes";

        let path = s.save_original(user, file, "txt", data).await.unwrap();
        assert!(path.exists(), "saved file must exist on disk");

        let loaded = s.load_original(user, file, "txt").await.unwrap();
        assert_eq!(loaded, data, "load must return exactly the saved bytes");
    }

    #[tokio::test]
    async fn load_missing_original_is_not_found() {
        let (_dir, s) = storage();
        let res = s
            .load_original(Uuid::new_v4(), Uuid::new_v4(), "txt")
            .await;
        assert!(res.is_err(), "loading a nonexistent file must error");
    }

    #[tokio::test]
    async fn delete_all_removes_the_original() {
        let (_dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();
        s.save_original(user, file, "txt", b"x").await.unwrap();

        s.delete_all(user, file).await.unwrap();

        assert!(
            s.load_original(user, file, "txt").await.is_err(),
            "the original must be gone after delete_all"
        );
    }

    #[test]
    fn calculate_checksum_is_sha256_hex() {
        let (_dir, s) = storage();
        // Known vector: sha256("hello").
        assert_eq!(
            s.calculate_checksum(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    /// Security (F-03): the upload-supplied `extension` feeds
    /// `get_original_path`; without sanitization a value containing path
    /// separators / `..` would let create_dir_all + write escape the per-user
    /// `originals/` dir and clobber another user's file. `safe_ext` must
    /// collapse any non-ASCII-alnum / empty / >16-char extension to `bin`,
    /// confining the result under the user's originals dir.
    #[test]
    fn get_original_path_sanitizes_traversal_extensions() {
        let (_dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();

        let originals_dir = s.get_user_path(user, "originals");
        let expected_bin = originals_dir.join(format!("{}.bin", file));

        // Every hostile / malformed extension collapses to `.bin` under the
        // user's own originals dir — no escape.
        for evil in [
            "x/../../victim/y",     // path-traversal via separators + ..
            "e/../../../etc",       // deeper traversal
            "png/../../../../root", // still contains separators
            "a".repeat(17).as_str(),// >16 chars → rejected
            "",                      // empty → rejected
            "pn g",                  // space (non-alnum)
            "p.g",                   // dot (non-alnum)
            "évil",                  // non-ASCII
        ] {
            let path = s.get_original_path(user, file, evil);
            assert_eq!(
                path, expected_bin,
                "extension {evil:?} must collapse to '.bin' under the user dir"
            );
            assert!(
                path.starts_with(&originals_dir),
                "sanitized path for {evil:?} escaped the user originals dir: {}",
                path.display()
            );
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(
                name.ends_with(".bin"),
                "expected a .bin file name for {evil:?}, got {name}"
            );
        }

        // A benign extension is preserved (lowercased).
        let ok = s.get_original_path(user, file, "PNG");
        assert_eq!(ok, originals_dir.join(format!("{}.png", file)));
        assert!(ok.starts_with(&originals_dir));
    }

    /// `delete_user_dirs` removes the per-user directory under every storage
    /// subdir it manages (originals / text / images / thumbnails) so deleting a
    /// user leaves no orphaned dirs. Save one of every artifact kind, delete the
    /// user's dirs, and assert each per-user subdir is gone.
    #[tokio::test]
    async fn delete_user_dirs_removes_every_per_user_subdir() {
        let (dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();

        // One of every artifact kind so each managed subdir exists on disk.
        s.save_original(user, file, "txt", b"orig").await.unwrap();
        s.save_text_page(user, file, 1, "page one").await.unwrap();
        s.save_image(user, file, 1, false, b"img").await.unwrap(); // images/
        s.save_image(user, file, 1, true, b"thumb").await.unwrap(); // thumbnails/

        // Sanity: the per-user subdirs exist before the delete.
        for subdir in ["originals", "text", "images", "thumbnails"] {
            assert!(
                s.get_user_path(user, subdir).exists(),
                "{subdir} per-user dir must exist before delete"
            );
        }

        s.delete_user_dirs(user).await.unwrap();

        // Every managed per-user subdir is gone after the delete.
        for subdir in ["originals", "text", "images", "thumbnails"] {
            assert!(
                !s.get_user_path(user, subdir).exists(),
                "{subdir} per-user dir must be gone after delete_user_dirs"
            );
        }
        // The storage root itself survives (only the user's dirs were removed).
        assert!(dir.path().exists());
    }

    /// Per-page citation geometry round-trips through
    /// `save_geometry_page`/`load_geometry_page` (the citation-highlight
    /// derivative). A load of a page that was never saved is a not-found.
    #[tokio::test]
    async fn save_then_load_geometry_page_roundtrips() {
        let (_dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();
        let geom = r#"{"text":"hello","boxes":[[0.1,0.1,0.05,0.02]]}"#;

        let path = s.save_geometry_page(user, file, 3, geom).await.unwrap();
        assert!(path.exists(), "saved geometry page must exist on disk");

        let loaded = s.load_geometry_page(user, file, 3).await.unwrap();
        assert_eq!(loaded, geom, "geometry must round-trip byte-for-byte");

        // A page that was never written is a not-found.
        assert!(
            s.load_geometry_page(user, file, 99).await.is_err(),
            "an unsaved geometry page must be not-found"
        );
    }

    /// Security (F-15), geometry path: a symlink planted at the geometry page's
    /// location must NOT be followed on load — the read is refused (mirrors the
    /// original-blob symlink guard).
    #[cfg(unix)]
    #[tokio::test]
    async fn load_geometry_page_refuses_to_follow_a_symlink() {
        let (dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();

        // A secret outside the storage tree the symlink would point at.
        let secret = dir.path().join("secret.json");
        tokio::fs::write(&secret, b"{\"text\":\"TOP SECRET\"}").await.unwrap();

        // Plant a symlink AT the path load_geometry_page will compute.
        let target = s
            .get_user_path(user, "geometry")
            .join(file.to_string())
            .join("page_1.json");
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        std::os::unix::fs::symlink(&secret, &target).unwrap();

        let res = s.load_geometry_page(user, file, 1).await;
        assert!(
            res.is_err(),
            "a symlinked geometry page must be refused, not followed"
        );
    }

    /// Security (F-15): a symlink planted in the storage tree must NOT be
    /// followed on load — the read is refused.
    #[cfg(unix)]
    #[tokio::test]
    async fn load_refuses_to_follow_a_symlink() {
        let (dir, s) = storage();
        let user = Uuid::new_v4();
        let file = Uuid::new_v4();

        // A secret outside the storage tree the symlink would point at.
        let secret = dir.path().join("secret.txt");
        tokio::fs::write(&secret, b"TOP SECRET").await.unwrap();

        // Plant a symlink AT the path load_original will compute.
        let target = s.get_original_path(user, file, "txt");
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        std::os::unix::fs::symlink(&secret, &target).unwrap();

        let res = s.load_original(user, file, "txt").await;
        assert!(res.is_err(), "a symlinked original must be refused, not followed");
    }

    fn sharded() -> (tempfile::TempDir, FilesystemStorage) {
        let dir = tempfile::tempdir().unwrap();
        let s = FilesystemStorage::sharded_for_test(dir.path());
        (dir, s)
    }

    impl FilesystemStorage {
        fn sharded_for_test(base: &Path) -> Self {
            Self::with_layout(base, OriginalsLayout::Sharded)
        }
    }

    async fn write_at(path: &Path, bytes: &[u8]) {
        tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        tokio::fs::write(path, bytes).await.unwrap();
    }

    /// `Sharded` writes `originals/<user>/<id[0..2]>/<id[2..4]>/<id>.<ext>`;
    /// `Flat` (the default) is byte-for-byte the old location.
    #[tokio::test]
    async fn sharded_layout_writes_under_the_first_four_hex_digits() {
        let (dir, s) = sharded();
        let user = Uuid::new_v4();
        let file = Uuid::parse_str("1c0a7e11-2233-4455-8899-aabbccddeeff").unwrap();
        let path = s.save_original(user, file, "webp", b"x").await.unwrap();
        assert_eq!(
            path,
            dir.path()
                .join("originals")
                .join(user.to_string())
                .join("1c")
                .join("0a")
                .join(format!("{file}.webp"))
        );
        assert_eq!(s.get_original_path(user, file, "webp"), path);
        let flat = FilesystemStorage::new(dir.path());
        assert_eq!(flat.layout(), OriginalsLayout::Flat);
        assert_eq!(
            flat.get_original_path(user, file, "webp"),
            dir.path().join("originals").join(user.to_string()).join(format!("{file}.webp"))
        );
        // The sanitiser still applies inside the leaf.
        let evil = s.get_original_path(user, file, "x/../../y");
        assert_eq!(evil, path.with_extension("bin"));
    }

    /// Resolve and load read BOTH layouts whatever the write layout: sharded
    /// wins when both exist.
    #[tokio::test]
    async fn resolve_reads_both_layouts_sharded_first() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let only_flat = Uuid::new_v4();
        let both = Uuid::new_v4();
        write_at(&s.flat_original_path(user, only_flat, "webp"), b"flat").await;
        write_at(&s.flat_original_path(user, both, "webp"), b"old").await;
        write_at(&s.sharded_original_path(user, both, "webp"), b"new").await;
        assert_eq!(s.load_original(user, only_flat, "webp").await.unwrap(), b"flat");
        assert_eq!(s.load_original(user, both, "webp").await.unwrap(), b"new");
        assert_eq!(
            s.resolve_original_path(user, both, "webp").await,
            Some(s.sharded_original_path(user, both, "webp"))
        );
        assert_eq!(s.resolve_original_path(user, Uuid::new_v4(), "webp").await, None);
        let flat_mode = FilesystemStorage::new(s.base_path.clone());
        let sharded_only = Uuid::new_v4();
        write_at(&s.sharded_original_path(user, sharded_only, "webp"), b"s").await;
        assert_eq!(flat_mode.load_original(user, sharded_only, "webp").await.unwrap(), b"s");
    }

    /// Delete removes the object from every location; absent is Ok(false).
    #[tokio::test]
    async fn delete_original_removes_flat_and_sharded_copies() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        write_at(&s.flat_original_path(user, id, "webp"), b"a").await;
        write_at(&s.sharded_original_path(user, id, "webp"), b"b").await;
        assert!(s.delete_original(user, id, "webp").await.unwrap());
        assert!(!s.flat_original_path(user, id, "webp").exists());
        assert!(!s.sharded_original_path(user, id, "webp").exists());
        assert!(!s.delete_original(user, id, "webp").await.unwrap());
    }

    /// `delete_all` (no extension) also sweeps the id's shard leaf.
    #[tokio::test]
    async fn delete_all_sweeps_the_shard_leaf() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let sibling = Uuid::new_v4();
        s.save_original(user, id, "webp", b"x").await.unwrap();
        write_at(&s.flat_original_path(user, id, "png"), b"y").await;
        s.save_original(user, sibling, "webp", b"z").await.unwrap();
        s.delete_all(user, id).await.unwrap();
        assert!(s.resolve_original_path(user, id, "webp").await.is_none());
        assert!(s.resolve_original_path(user, id, "png").await.is_none());
        assert!(s.resolve_original_path(user, sibling, "webp").await.is_some());
    }

    /// The mover: moves objects, leaves junk, dedups identical copies, keeps
    /// conflicting ones, honours `limit`, and a re-run finishes the job.
    #[cfg(unix)]
    #[tokio::test]
    async fn shard_flat_originals_moves_resumably_and_touches_nothing_else() {
        let (dir, s) = sharded();
        let user = Uuid::new_v4();
        let ids: Vec<Uuid> = (0..10).map(|_| Uuid::new_v4()).collect();
        for (i, id) in ids.iter().enumerate() {
            write_at(&s.flat_original_path(user, *id, "webp"), format!("o{i}").as_bytes()).await;
        }
        let flat_dir = dir.path().join("originals").join(user.to_string());
        tokio::fs::write(flat_dir.join("notes.txt"), b"keep").await.unwrap();
        let outside = dir.path().join("outside.webp");
        tokio::fs::write(&outside, b"outside").await.unwrap();
        let link_id = Uuid::new_v4();
        std::os::unix::fs::symlink(&outside, flat_dir.join(format!("{link_id}.webp"))).unwrap();
        let dup = Uuid::new_v4();
        write_at(&s.flat_original_path(user, dup, "webp"), b"same").await;
        write_at(&s.sharded_original_path(user, dup, "webp"), b"same").await;
        let conflict = Uuid::new_v4();
        write_at(&s.flat_original_path(user, conflict, "webp"), b"one").await;
        write_at(&s.sharded_original_path(user, conflict, "webp"), b"two").await;

        let opts = ShardOptions {
            batch: 3,
            pause: std::time::Duration::ZERO,
            limit: Some(4),
            ..Default::default()
        };
        let mut calls = 0;
        let first = s.shard_flat_originals(user, &opts, &mut |_| calls += 1).await.unwrap();
        assert_eq!(first.moved, 4);
        assert!(calls >= 1);
        for id in &ids {
            assert!(s.resolve_original_path(user, *id, "webp").await.is_some(), "readable mid-migration");
        }
        let opts = ShardOptions { limit: None, ..opts };
        let second = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        assert_eq!(second.moved, 6);
        let second_census = second.census.as_ref().expect("a normal finish always carries a census");
        assert_eq!(second_census.remaining_flat, 1, "only the conflict stays flat: {second:?}");
        assert_eq!(second_census.conflicts, 1);
        assert_eq!(first.deduplicated + second.deduplicated, 1);
        for (i, id) in ids.iter().enumerate() {
            let p = s.sharded_original_path(user, *id, "webp");
            assert_eq!(tokio::fs::read(&p).await.unwrap(), format!("o{i}").as_bytes());
            assert!(!s.flat_original_path(user, *id, "webp").exists());
        }
        assert!(!s.flat_original_path(user, dup, "webp").exists());
        assert!(s.flat_original_path(user, conflict, "webp").exists());
        assert_eq!(tokio::fs::read(s.sharded_original_path(user, conflict, "webp")).await.unwrap(), b"two");
        assert!(flat_dir.join("notes.txt").exists());
        assert!(tokio::fs::symlink_metadata(flat_dir.join(format!("{link_id}.webp"))).await.unwrap().file_type().is_symlink());
        assert!(!s.sharded_original_path(user, link_id, "webp").exists());
        let third = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        assert_eq!(third.moved, 0);
    }

    /// INV-6, HIGH finding: a symlink planted AT the shard target must never
    /// be read through for the dedup byte-compare, and must never make the
    /// mover unlink the real flat copy. Before the fix, `symlink_metadata`
    /// was checked only for existence (`Ok(_) =>`), so a symlink at `to`
    /// pointing at bytes identical to the flat copy made `fs::read(&to)`
    /// follow it, the compare report "same", and the flat (REAL) object get
    /// unlinked — leaving only the attacker-controlled symlink behind.
    #[cfg(unix)]
    #[tokio::test]
    async fn shard_flat_originals_refuses_to_read_through_a_symlinked_target() {
        let (dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let flat = s.flat_original_path(user, id, "webp");
        write_at(&flat, b"identical bytes").await;

        // A file OUTSIDE the store whose bytes are byte-for-byte identical to
        // the flat copy.
        let victim = dir.path().join("victim.webp");
        tokio::fs::write(&victim, b"identical bytes").await.unwrap();
        // Plant a symlink AT the shard target, pointing at the victim.
        let to = s.sharded_original_path(user, id, "webp");
        tokio::fs::create_dir_all(to.parent().unwrap()).await.unwrap();
        std::os::unix::fs::symlink(&victim, &to).unwrap();

        let mut report = ShardReport::default();
        let opts = ShardOptions::default();
        let r = s
            .shard_flat_originals(user, &opts, &mut |p| report = p.clone())
            .await
            .unwrap();

        assert!(flat.exists(), "the REAL flat object must SURVIVE a symlinked target");
        assert_eq!(
            tokio::fs::read(&flat).await.unwrap(),
            b"identical bytes",
            "the surviving flat object must be untouched"
        );
        assert!(
            tokio::fs::symlink_metadata(&to).await.unwrap().file_type().is_symlink(),
            "the symlink at the target must be left exactly as planted, never followed or replaced"
        );
        // Round-4 LOW finding: a non-regular-file target never had its bytes
        // compared, so it is counted as `refused`, NOT `conflicts` (which is
        // reserved for a pair that both opened fine but differ byte-for-byte).
        // FIX_ROUND-6: these numbers now come from the end-of-run `census`,
        // not a per-pass local.
        let c = r.census.as_ref().expect("a normal finish always carries a census");
        assert_eq!(
            c.refused, 1,
            "a non-regular-file target must be counted as refused, not conflicts: {r:?}"
        );
        assert_eq!(
            c.conflicts, 0,
            "no bytes were ever compared for a symlinked target: {r:?}"
        );
        assert_eq!(r.moved, 0);
        assert_eq!(
            r.deduplicated, 0,
            "must NOT be reported as a dedup — that would imply the flat copy was removed"
        );
        assert_eq!(c.refused_samples.len(), 1, "{r:?}");
        assert!(
            c.refused_samples[0].contains("target:"),
            "the sample must say which side (target/source) was refused: {r:?}"
        );
    }

    /// A DIRECTORY at the shard target (pathological, but the same class of
    /// bug as a symlink) must be refused the same way: never read through,
    /// never a reason to unlink the flat copy.
    #[tokio::test]
    async fn shard_flat_originals_refuses_a_non_file_target_that_is_a_directory() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let flat = s.flat_original_path(user, id, "webp");
        write_at(&flat, b"x").await;
        let to = s.sharded_original_path(user, id, "webp");
        tokio::fs::create_dir_all(&to).await.unwrap(); // a directory, not a file, at the target name

        let opts = ShardOptions::default();
        let r = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        assert!(flat.exists(), "the flat object must survive a directory at the target");
        // Round-4 LOW finding: never-opened, so `refused`, not `conflicts`.
        let c = r.census.as_ref().expect("a normal finish always carries a census");
        assert_eq!(c.refused, 1, "{r:?}");
        assert_eq!(c.conflicts, 0, "{r:?}");
        assert_eq!(r.moved, 0);
    }

    /// Round-4 LOW finding (both auditors): `ShardReport.conflicts` used to
    /// lump byte-different duplicates together with every kind of open
    /// refusal (symlink, socket, directory, permission error), so the CLI
    /// summary's single `conflicts` number made a systemic open failure read
    /// like N benign duplicate conflicts. This test proves the split with
    /// BOTH classes present in the SAME run: seven objects refused via a
    /// symlinked shard target (more than `refused_samples`'s 5-entry cap, to
    /// prove the cap too) plus one genuine byte-different pair (both sides
    /// open fine as regular files, differing bytes) — `refused` must count
    /// only the seven, `conflicts` must count only the one, and
    /// `refused_samples` must stay at exactly 5 despite seven refusals.
    #[cfg(unix)]
    #[tokio::test]
    async fn shard_flat_originals_separates_refused_opens_from_byte_different_conflicts_and_caps_samples()
     {
        let (dir, s) = sharded();
        let user = Uuid::new_v4();

        // Seven objects, each refused via a symlink planted at its shard
        // target (pointing at bytes identical to the flat copy, so a
        // pre-round-3 implementation would have reported these as dedups —
        // the point here is only that they must never land in `conflicts`).
        let refused_ids: Vec<Uuid> = (0..7).map(|_| Uuid::new_v4()).collect();
        for (i, id) in refused_ids.iter().enumerate() {
            let flat = s.flat_original_path(user, *id, "webp");
            write_at(&flat, format!("bytes-{i}").as_bytes()).await;
            let victim = dir.path().join(format!("victim-{i}.webp"));
            tokio::fs::write(&victim, format!("bytes-{i}").as_bytes())
                .await
                .unwrap();
            let to = s.sharded_original_path(user, *id, "webp");
            tokio::fs::create_dir_all(to.parent().unwrap())
                .await
                .unwrap();
            std::os::unix::fs::symlink(&victim, &to).unwrap();
        }

        // One genuine conflict: both sides open fine as regular files, but
        // hold DIFFERENT bytes.
        let conflict_id = Uuid::new_v4();
        write_at(
            &s.flat_original_path(user, conflict_id, "webp"),
            b"flat-bytes",
        )
        .await;
        write_at(
            &s.sharded_original_path(user, conflict_id, "webp"),
            b"sharded-bytes",
        )
        .await;

        let opts = ShardOptions::default();
        let r = s
            .shard_flat_originals(user, &opts, &mut |_| {})
            .await
            .unwrap();

        let c = r.census.as_ref().expect("a normal finish always carries a census");
        assert_eq!(
            c.refused, 7,
            "every symlinked target must count as refused, not conflicts: {r:?}"
        );
        assert_eq!(
            c.conflicts, 1,
            "only the byte-different pair may count as a conflict: {r:?}"
        );
        assert_eq!(
            c.refused_samples.len(),
            5,
            "refused_samples must cap at 5 even though 7 objects were refused: {r:?}"
        );
        assert_eq!(r.moved, 0);
        assert_eq!(r.deduplicated, 0);

        // Every refused flat object survives untouched.
        for (i, id) in refused_ids.iter().enumerate() {
            assert_eq!(
                tokio::fs::read(s.flat_original_path(user, *id, "webp"))
                    .await
                    .unwrap(),
                format!("bytes-{i}").as_bytes()
            );
        }
        // The conflicting pair survives on both sides, unchanged.
        assert_eq!(
            tokio::fs::read(s.flat_original_path(user, conflict_id, "webp"))
                .await
                .unwrap(),
            b"flat-bytes"
        );
        assert_eq!(
            tokio::fs::read(s.sharded_original_path(user, conflict_id, "webp"))
                .await
                .unwrap(),
            b"sharded-bytes"
        );
    }

    /// MEDIUM finding: `max_passes` bounds the mover against a sustained
    /// concurrent flat writer that keeps adding entries — without the bound
    /// this would loop until the writer stops, which under a live write load
    /// may be never. The writer here adds one new flat object after every
    /// pass boundary (paced by `pause`), forever — `max_passes` must still
    /// make the call return, reporting what is left in `remaining_flat`.
    #[tokio::test]
    async fn shard_flat_originals_is_bounded_by_max_passes_under_a_sustained_writer() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        write_at(&s.flat_original_path(user, Uuid::new_v4(), "webp"), b"x").await;

        let keep_writing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let writer_storage = s.clone();
        let writer_flag = keep_writing.clone();
        let writer = tokio::spawn(async move {
            while writer_flag.load(std::sync::atomic::Ordering::SeqCst) {
                write_at(&writer_storage.flat_original_path(user, Uuid::new_v4(), "webp"), b"y").await;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        });

        let opts = ShardOptions {
            batch: 1,
            pause: std::time::Duration::from_millis(5),
            limit: None,
            max_passes: 3,
        };
        let started = std::time::Instant::now();
        let report = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        let elapsed = started.elapsed();
        keep_writing.store(false, std::sync::atomic::Ordering::SeqCst);
        writer.await.unwrap();

        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "max_passes must make the call return promptly even under a sustained writer: {elapsed:?}"
        );
        // Under a sustained writer the bound is very likely to still find
        // something flat; this is the whole point of the census (a
        // follow-up run continues from here). Not asserting > 0 would make
        // this test pass for the wrong reason if the writer happened to be
        // slow, so just assert the call returned with a well-formed report.
        let c = report.census.as_ref().expect("a normal finish always carries a census");
        assert!(report.moved + c.remaining_flat >= 1, "{report:?}");
    }

    /// LOW finding: an I/O error must not drop the run's partial progress —
    /// `progress` must see a `ShardReport` reflecting what happened before
    /// the failure, not be skipped in favour of just propagating `Err`.
    ///
    /// Deterministic by construction (no dependence on `read_dir`'s
    /// unspecified entry order, which is why this is a SEPARATE test from the
    /// symlink/directory conflict ones above rather than reusing their
    /// multi-entry setup): exactly ONE object exists, so pass 1 moves it
    /// cleanly and, because it moved something, the mover's loop always goes
    /// on to a pass 2. The `progress` callback itself — called synchronously
    /// right after that one move, strictly BETWEEN pass 1 and pass 2 — is
    /// used to revoke read access to the namespace directory, so pass 2's own
    /// `read_dir` is what fails. The report `progress` is called with right
    /// before the error propagates must therefore already carry `moved: 1`
    /// from pass 1, not a blank default.
    #[cfg(unix)]
    #[tokio::test]
    async fn shard_flat_originals_reports_partial_progress_before_an_io_error() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, s) = sharded();
        let user = Uuid::new_v4();
        let ok_id = Uuid::new_v4();
        write_at(&s.flat_original_path(user, ok_id, "webp"), b"ok").await;
        let flat_dir = dir.path().join("originals").join(user.to_string());

        let opts = ShardOptions { batch: 1, pause: std::time::Duration::ZERO, ..Default::default() };
        let mut last_seen = ShardReport::default();
        let mut calls = 0u32;
        let flat_dir_for_cb = flat_dir.clone();
        let res = s
            .shard_flat_originals(user, &opts, &mut |r| {
                calls += 1;
                last_seen = r.clone();
                if calls == 1 {
                    // Fires once, right after the one object moved in pass 1
                    // (batch: 1 ⇒ a progress call after every move) and
                    // before pass 2's `read_dir` runs — revoke read+execute
                    // on the namespace dir so that NEXT read_dir fails with a
                    // real I/O error instead of NotFound.
                    let _ = std::fs::set_permissions(
                        &flat_dir_for_cb,
                        std::fs::Permissions::from_mode(0o000),
                    );
                }
            })
            .await;

        // Restore so the TempDir's own cleanup can recurse into it.
        let _ = std::fs::set_permissions(&flat_dir, std::fs::Permissions::from_mode(0o755));

        match res {
            Err(_) => {
                assert_eq!(
                    last_seen.moved, 1,
                    "the LAST progress call before the error must already carry \
                     pass 1's moved count, not a blank report: {last_seen:?}"
                );
                assert!(calls >= 2, "progress must be called again (with the partial report) at the failure site, not just after pass 1: {calls}");
                // The SAME revoke that aborted the mover's own `read_dir`
                // also blocks `census`'s `read_dir` on the identical
                // directory — FIX_ROUND-6: this is the UNKNOWN case, and
                // `report.census` must stay `None`, never a stale or zeroed
                // value that could be misread as "0 conflicts found".
                assert!(
                    last_seen.census.is_none(),
                    "a census that cannot run must report UNKNOWN (None), never a zero \
                     that reads as converged: {last_seen:?}"
                );
            }
            Ok(r) => {
                // Root ignores the directory's permission bits, so pass 2's
                // read_dir succeeds anyway and the run simply completes; the
                // property under test does not apply, but the one object must
                // still be correctly accounted for.
                assert_eq!(r.moved, 1);
                assert!(r.census.is_some(), "a normal finish always carries a census: {r:?}");
            }
        }
    }

    // FIX_ROUND-6.md: `shard_flat_originals_keeps_last_completed_pass_numbers_on_a_later_io_error`
    // (round-2 regression test) was DELETED here. It pinned the exact
    // mechanism this round removes — "the LAST COMPLETED pass's numbers
    // survive a later pass's abort" — which presupposes a per-pass
    // descriptive snapshot that no longer exists. Its scenario (an
    // EXECUTE-only revoke that blocks the mover's `read_dir` but leaves
    // path-based opens working) is now covered more directly by
    // `shard_flat_originals_never_reports_refused_samples_with_a_stale_refused_count_on_mid_pass_abort`
    // below, rewritten to assert the NEW behavior: an abort whose end-of-run
    // census CAN still run reports the directory's true, fresh state, not a
    // frozen "last completed pass" snapshot.

    /// Round-5 fix-round regression for the MEDIUM finding (ledger: before
    /// this round, `refused_samples` was pushed onto `report` the INSTANT a
    /// refusal was seen, while `refused` (like `conflicts`/`skipped`/
    /// `remaining_flat`) was a per-pass local committed only once a pass's
    /// `read_dir` listing finished. A pass that recorded a refusal and then
    /// `bail!`ed on a LATER entry in the SAME, still-incomplete pass
    /// therefore reported `refused_samples` non-empty alongside
    /// `refused == 0` (the last COMPLETED pass's committed value, which never
    /// saw this refusal) — a report with samples but no matching count, which
    /// also meant the CLI's `if r.refused > 0 { print samples }` gate never
    /// fired on exactly the run that needed it.
    ///
    /// **Rewritten for FIX_ROUND-6.md.** The per-pass "last completed pass"
    /// snapshot this test originally pinned no longer exists: there is no
    /// pass-scoped `refused`/`refused_samples` to go stale. Same setup (the
    /// scenario is still a real, valuable one — an abort whose `read_dir`
    /// keeps working because only WRITE was revoked), but the invariant under
    /// test is now the opposite of "stays frozen at the last safe value": the
    /// end-of-run census, attempted even on this abort, can still walk the
    /// (read+execute-only) directory fine, so it must report the directory's
    /// TRUE, FRESH state — not a zero, not a stale snapshot, not "unknown".
    ///
    /// Pass 1 moves one object (`moved_id`); its `progress` callback (batch:
    /// 1, fires right after that rename) then plants MANY new objects into
    /// the SAME namespace directory — all absent until now, so pass 1's
    /// already-open `read_dir` iterator never sees them and they land in
    /// pass 2 — and revokes WRITE only (keeps read+execute) on that
    /// directory: `read_dir` and every path-based `open` still work, but
    /// `remove_file`/`rename` of an entry directly inside it do not. Of the
    /// planted objects, 20 are REFUSED (a symlink at each shard target): a
    /// refusal's path never touches the filesystem beyond opens, so it is
    /// UNAFFECTED by the write revocation regardless of when in pass 2 it is
    /// visited. The last is a DEDUP pair (identical bytes both sides): it
    /// opens and compares equal, then its flat copy's `remove_file` hits the
    /// revoked write bit and aborts the run. Directory enumeration order is
    /// unspecified, so 20 refused entries (vs. the single aborting one) make
    /// it overwhelmingly likely at least one refusal is visited before the
    /// abort on every run, without requiring control over the real order —
    /// this is what makes the end-of-run census's answer deterministic
    /// regardless of visit order: `census` walks the WHOLE directory
    /// independent of where the mover happened to abort.
    #[cfg(unix)]
    #[tokio::test]
    async fn shard_flat_originals_never_reports_refused_samples_with_a_stale_refused_count_on_mid_pass_abort()
     {
        use std::os::unix::fs::PermissionsExt;
        let (dir, s) = sharded();
        let user = Uuid::new_v4();

        let moved_id = Uuid::new_v4();
        write_at(&s.flat_original_path(user, moved_id, "webp"), b"ok").await;

        let ns_dir = dir.path().join("originals").join(user.to_string());
        let opts = ShardOptions {
            batch: 1,
            pause: std::time::Duration::ZERO,
            ..Default::default()
        };
        let mut last_seen = ShardReport::default();
        let mut calls = 0u32;
        let ns_dir_for_cb = ns_dir.clone();
        // RAII-restored: even if an assertion below panics, this guard's
        // Drop still restores the namespace dir's permissions, so the
        // TempDir's own cleanup can recurse into it.
        struct RestorePerms {
            path: PathBuf,
            mode: u32,
        }
        impl Drop for RestorePerms {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(
                    &self.path,
                    std::fs::Permissions::from_mode(self.mode),
                );
            }
        }
        let _restore = RestorePerms {
            path: ns_dir.clone(),
            mode: 0o755,
        };

        let res = s
            .shard_flat_originals(user, &opts, &mut |r| {
                calls += 1;
                last_seen = r.clone();
                if calls == 1 {
                    // Fires once, right after `moved_id` is renamed. Plant
                    // all new objects now — AFTER pass 1's `read_dir` was
                    // already opened, so they land in pass 2, never pass 1.
                    let victim = ns_dir_for_cb
                        .parent()
                        .unwrap()
                        .join("victim-for-mid-pass-abort-test.webp");
                    std::fs::write(&victim, b"victim-bytes").unwrap();
                    // 20 refused objects (order-independence margin — see the
                    // doc comment above).
                    for i in 0..20u32 {
                        let refused_id = Uuid::new_v4();
                        std::fs::write(
                            ns_dir_for_cb.join(format!("{refused_id}.webp")),
                            format!("refused-flat-bytes-{i}").as_bytes(),
                        )
                        .unwrap();
                        let refused_target = s.sharded_original_path(user, refused_id, "webp");
                        std::fs::create_dir_all(refused_target.parent().unwrap()).unwrap();
                        std::os::unix::fs::symlink(&victim, &refused_target).unwrap();
                    }

                    let dedup_id = Uuid::new_v4();
                    std::fs::write(
                        ns_dir_for_cb.join(format!("{dedup_id}.webp")),
                        b"identical-bytes",
                    )
                    .unwrap();
                    let dedup_target = s.sharded_original_path(user, dedup_id, "webp");
                    std::fs::create_dir_all(dedup_target.parent().unwrap()).unwrap();
                    std::fs::write(&dedup_target, b"identical-bytes").unwrap();

                    // Revoke WRITE only (keep read+execute): `read_dir` and
                    // every path-based open still succeed; `remove_file` of
                    // an entry directly inside this dir does not.
                    let _ = std::fs::set_permissions(
                        &ns_dir_for_cb,
                        std::fs::Permissions::from_mode(0o500),
                    );
                }
            })
            .await;

        match res {
            Err(_) => {
                assert_eq!(
                    last_seen.moved, 1,
                    "moved_id's rename in pass 1 must survive into the abort report: \
                     {last_seen:?}"
                );
                assert_eq!(
                    last_seen.deduplicated, 0,
                    "the dedup's remove_file is exactly what failed — it must not be \
                     counted as a successful dedup: {last_seen:?}"
                );
                // FIX_ROUND-6: the end-of-run census CAN still run here (only
                // WRITE was revoked) and must report the TRUE, FRESH state —
                // never a stale snapshot, never "unknown".
                let c = last_seen.census.as_ref().expect(
                    "read+execute still works, so the end-of-run census must succeed \
                     on this abort, not report UNKNOWN",
                );
                assert_eq!(
                    c.refused, 20,
                    "all 20 planted symlinks are refused, regardless of which one the \
                     mover happened to abort on: {last_seen:?}"
                );
                assert_eq!(
                    c.refused_samples.len(),
                    5,
                    "refused_samples caps at 5 even though 20 are refused: {last_seen:?}"
                );
                assert_eq!(
                    c.conflicts, 0,
                    "the dedup pair holds IDENTICAL bytes — never a conflict: {last_seen:?}"
                );
                assert_eq!(
                    c.remaining_flat, 21,
                    "20 refused + 1 not-yet-deduped pair, independent of visit order: \
                     {last_seen:?}"
                );
            }
            Ok(r) => {
                // Root ignores permission bits (see the sibling tests'
                // identical caveat) — the run simply completes (including the
                // dedup's `remove_file`); all 20 plants are refused, and
                // samples cap at 5.
                let c = r.census.as_ref().expect("a normal finish always carries a census");
                assert_eq!(c.refused, 20, "{r:?}");
                assert_eq!(c.refused_samples.len(), 5, "{r:?}");
            }
        }
    }

    /// Round-2 fix-round regression for the LOW finding (ledger: "the
    /// non-regular-file guard is check-then-read ... an attacker with write
    /// access to the shard leaf could swap the regular file for a symlink
    /// between `symlink_metadata` and `fs::read`"). Exercises
    /// `open_regular_nofollow` directly, isolated from the mover: a symlink
    /// leaf must come back `Refused` — refused at the `open` syscall itself
    /// (unix `O_NOFOLLOW` → `ELOOP`), never a successful `Open` that a caller
    /// could then read through.
    ///
    /// Round-3 note (test-gap LOW finding): this test asserts the OUTCOME
    /// (`Refused`) only, which a check-then-open implementation reaches too
    /// (via its own `symlink_metadata` call) — it does NOT, by itself, pin
    /// that the refusal comes from a real `O_NOFOLLOW` open. That mechanism
    /// is pinned separately by `open_nofollow_fails_with_eloop_on_a_symlink_leaf`
    /// below, which calls the raw, unmapped `open_nofollow` directly. Since
    /// `open_regular_nofollow` is now infallible (round-3 MEDIUM fix), it
    /// returns `RegularFile` directly rather than `io::Result<RegularFile>`.
    #[cfg(unix)]
    #[tokio::test]
    async fn open_regular_nofollow_refuses_a_symlink_with_eloop() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.bin");
        tokio::fs::write(&target, b"real bytes").await.unwrap();
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        match open_regular_nofollow(&link).await {
            RegularFile::Refused(_) => {}
            other => panic!("a symlink leaf must be Refused, not {other:?}"),
        }

        let missing = dir.path().join("missing.bin");
        match open_regular_nofollow(&missing).await {
            RegularFile::NotFound => {}
            other => panic!("a missing path must be NotFound, not {other:?}"),
        }

        match open_regular_nofollow(&target).await {
            RegularFile::Open(mut f) => {
                let mut buf = Vec::new();
                f.read_to_end(&mut buf).await.unwrap();
                assert_eq!(
                    buf, b"real bytes",
                    "a genuine regular file must still open and read"
                );
            }
            other => panic!("a regular file must Open, not {other:?}"),
        }
    }

    /// Round-4 MEDIUM finding (test gap): the test above and
    /// `shard_flat_originals_refuses_to_read_through_a_symlinked_target`
    /// both assert only the MAPPED OUTCOME (`RegularFile::Refused`) of
    /// `open_regular_nofollow` ITSELF — which a check-then-open
    /// implementation of `open_regular_nofollow` (reverting its body to a
    /// `symlink_metadata` check, the exact shape the `#[cfg(not(unix))]`
    /// fallback below already is) reaches too, via its own `stat`, WITHOUT
    /// ever calling `open_nofollow`/attempting a real `O_NOFOLLOW` open.
    /// `open_nofollow_fails_with_eloop_on_a_symlink_leaf` (below) pins the
    /// MECHANISM of the raw `open_nofollow` helper, called directly — but
    /// nothing pinned that `open_regular_nofollow`, the mover's actual call
    /// path, still ROUTES THROUGH that helper rather than bypassing it.
    /// Mutation-confirmed (see FIX_ROUND-4.md): reverting `open_regular_nofollow`'s
    /// unix body to check-then-open keeps every other test in this file
    /// green, `open_nofollow_fails_with_eloop_on_a_symlink_leaf` included (it
    /// calls the untouched `open_nofollow` directly, never through
    /// `open_regular_nofollow`).
    ///
    /// This test closes that gap by asserting the `Refused` reason STRING
    /// carries the real OS-level `ELOOP` message
    /// (`io::Error::from_raw_os_error(ELOOP).to_string()`, e.g. "Too many
    /// levels of symbolic links (os error 40)") — a message that can only
    /// come from a REAL `open()` syscall failing with that errno. A
    /// check-then-open implementation's refusal reads `"not a regular file
    /// (<FileType Debug>)"` instead (see the `#[cfg(not(unix))]` fallback's
    /// identical message), which never contains the OS's ELOOP text, because
    /// it never performs the open that could produce it.
    #[cfg(unix)]
    #[tokio::test]
    async fn open_regular_nofollow_refusal_carries_the_real_eloop_os_error() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.bin");
        tokio::fs::write(&target, b"real bytes").await.unwrap();
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let expected_os_message = std::io::Error::from_raw_os_error(libc::ELOOP).to_string();
        match open_regular_nofollow(&link).await {
            RegularFile::Refused(reason) => {
                assert!(
                    reason.contains(&expected_os_message),
                    "a real O_NOFOLLOW open is the only thing on open_regular_nofollow's \
                     call path that can produce the OS's own ELOOP message ({expected_os_message:?}); \
                     a check-then-open implementation's refusal comes from `symlink_metadata` \
                     instead and can never contain it: {reason:?}"
                );
            }
            other => panic!("a symlink leaf must be Refused, not {other:?}"),
        }
    }

    /// Round-3 LOW finding (test gap): pins the `O_NOFOLLOW` MECHANISM
    /// itself, independent of `open_regular_nofollow`'s error-mapping layer.
    /// Before this test, reverting `open_nofollow`'s body to a plain
    /// `tokio::fs::File::open(path).await` (dropping the `O_NOFOLLOW` custom
    /// flag) kept every other test in this file green: they only assert the
    /// mapped OUTCOME ("Refused"/a conflict), which a check-then-open
    /// implementation reaches too, just via a `symlink_metadata` call rather
    /// than a real `O_NOFOLLOW` open. This test calls `open_nofollow`
    /// directly and asserts the SPECIFIC OS error (`ELOOP`) that only a
    /// genuine `O_NOFOLLOW` open against a symlink leaf can produce — a
    /// check-then-open implementation never attempts this open at all on
    /// that path, so it cannot produce this error (manually mutation-checked:
    /// reverting `open_nofollow` to drop `O_NOFOLLOW` makes this test fail —
    /// the open SUCCEEDS instead of returning `ELOOP`, since it silently
    /// follows the symlink; see FIX_ROUND-3.md).
    #[cfg(unix)]
    #[tokio::test]
    async fn open_nofollow_fails_with_eloop_on_a_symlink_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.bin");
        tokio::fs::write(&target, b"real bytes").await.unwrap();
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = open_nofollow(&link)
            .await
            .expect_err("O_NOFOLLOW must refuse to open a symlink leaf");
        assert_eq!(
            err.raw_os_error(),
            Some(libc::ELOOP),
            "a check-then-open implementation cannot produce this specific OS error: {err:?}"
        );

        // A genuine regular file still opens fine through the same raw helper.
        let mut f = open_nofollow(&target)
            .await
            .expect("a regular file must open");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"real bytes");
    }

    /// `FileStorage::open_original`'s wrapper: a [`FilesystemStorage`] whose
    /// FIRST `resolve_original_path` answer is a pre-recorded STALE path,
    /// regardless of where the object really is on disk — a deterministic
    /// stand-in for "the mover renamed the object between resolve and open"
    /// without racing a real background task. Every other call delegates
    /// straight through to the real storage.
    struct FlakyResolve<'a> {
        inner: &'a FilesystemStorage,
        stale_path: PathBuf,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl FileStorage for FlakyResolve<'_> {
        async fn save_original(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            extension: &str,
            data: &[u8],
        ) -> StorageResult<PathBuf> {
            self.inner.save_original(user_id, file_id, extension, data).await
        }
        async fn save_text_page(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
            text: &str,
        ) -> StorageResult<PathBuf> {
            self.inner.save_text_page(user_id, file_id, page_num, text).await
        }
        async fn save_geometry_page(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
            geometry_json: &str,
        ) -> StorageResult<PathBuf> {
            self.inner
                .save_geometry_page(user_id, file_id, page_num, geometry_json)
                .await
        }
        async fn load_geometry_page(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
        ) -> StorageResult<String> {
            self.inner.load_geometry_page(user_id, file_id, page_num).await
        }
        async fn save_image(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
            is_thumbnail: bool,
            data: &[u8],
        ) -> StorageResult<PathBuf> {
            self.inner
                .save_image(user_id, file_id, page_num, is_thumbnail, data)
                .await
        }
        fn get_original_path(&self, user_id: Uuid, file_id: Uuid, extension: &str) -> PathBuf {
            self.inner.get_original_path(user_id, file_id, extension)
        }
        fn get_text_path(&self, user_id: Uuid, file_id: Uuid, page_num: u32) -> PathBuf {
            self.inner.get_text_path(user_id, file_id, page_num)
        }
        fn get_image_path(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
            is_thumbnail: bool,
        ) -> PathBuf {
            self.inner.get_image_path(user_id, file_id, page_num, is_thumbnail)
        }
        async fn load_original(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            extension: &str,
        ) -> StorageResult<Vec<u8>> {
            self.inner.load_original(user_id, file_id, extension).await
        }
        async fn load_text_page(&self, user_id: Uuid, file_id: Uuid, page_num: u32) -> StorageResult<String> {
            self.inner.load_text_page(user_id, file_id, page_num).await
        }
        async fn load_preview(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            page_num: u32,
        ) -> StorageResult<Vec<u8>> {
            self.inner.load_preview(user_id, file_id, page_num).await
        }
        async fn load_thumbnail(&self, user_id: Uuid, file_id: Uuid) -> StorageResult<Vec<u8>> {
            self.inner.load_thumbnail(user_id, file_id).await
        }
        async fn resolve_original_path(
            &self,
            user_id: Uuid,
            file_id: Uuid,
            extension: &str,
        ) -> Option<PathBuf> {
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Some(self.stale_path.clone())
            } else {
                self.inner.resolve_original_path(user_id, file_id, extension).await
            }
        }
        async fn delete_all(&self, user_id: Uuid, file_id: Uuid) -> StorageResult<()> {
            self.inner.delete_all(user_id, file_id).await
        }
        async fn delete_user_dirs(&self, user_id: Uuid) -> StorageResult<()> {
            self.inner.delete_user_dirs(user_id).await
        }
        fn calculate_checksum(&self, data: &[u8]) -> String {
            self.inner.calculate_checksum(data)
        }
    }

    /// HIGH finding (serve TOCTOU): `open_original` must retry the resolve
    /// once when the first open answers `NotFound`. The object sits at the
    /// FLAT path when the (stale, pre-recorded) resolve answer is handed
    /// out, but by the time `open_original`'s first open runs it has
    /// genuinely been renamed to its SHARD LEAF (simulating a mover winning
    /// the race) — the first open must 404 internally, the retry must
    /// re-resolve for real, and the real resolver finds the object at its
    /// new home.
    #[tokio::test]
    async fn open_original_retries_the_resolve_after_a_toctou_open_failure() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let flat_path = s.flat_original_path(user, id, "webp");
        let sharded_path = s.sharded_original_path(user, id, "webp");
        write_at(&flat_path, b"moved-mid-flight").await;
        // The mover wins the race: gone from flat, now sharded — by the time
        // `open_original`'s first open runs against the stale answer below.
        tokio::fs::create_dir_all(sharded_path.parent().unwrap()).await.unwrap();
        tokio::fs::rename(&flat_path, &sharded_path).await.unwrap();

        let flaky = FlakyResolve {
            inner: &s,
            stale_path: flat_path.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let (path, mut file) = flaky
            .open_original(user, id, "webp")
            .await
            .expect("the retry must find the object at its new sharded path");
        assert_eq!(
            path, sharded_path,
            "the SECOND resolve must return the real, current path, not the stale one"
        );
        let mut bytes = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut file, &mut bytes).await.unwrap();
        assert_eq!(bytes, b"moved-mid-flight");
    }

    /// The retry does not paper over a genuine absence: an object that never
    /// existed is still `None` after both attempts.
    #[tokio::test]
    async fn open_original_is_none_when_the_object_never_existed() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        assert!(s.open_original(user, id, "webp").await.is_none());
    }

    /// MEDIUM finding: a concurrent writer creating new flat objects and a
    /// concurrent deleter removing existing ones, both racing the mover on
    /// ONE directory — every surviving object keeps its exact bytes and
    /// resolves, every deleted object stays gone (never resurrected by the
    /// mover winning a race against the delete), and a follow-up run
    /// converges to `remaining_flat == 0`.
    #[tokio::test]
    async fn shard_flat_originals_is_safe_against_a_concurrent_writer_and_deleter() {
        let (_dir, s) = sharded();
        let user = Uuid::new_v4();

        // Pre-existing objects: half will be deleted mid-run, half survive.
        let mut expected: std::collections::HashMap<Uuid, Vec<u8>> = std::collections::HashMap::new();
        let pre: Vec<Uuid> = (0..12).map(|_| Uuid::new_v4()).collect();
        for (i, id) in pre.iter().enumerate() {
            let bytes = format!("pre{i}").into_bytes();
            write_at(&s.flat_original_path(user, *id, "webp"), &bytes).await;
            expected.insert(*id, bytes);
        }
        let to_delete: Vec<Uuid> = pre.iter().step_by(2).cloned().collect();
        let to_survive: Vec<Uuid> = pre.iter().skip(1).step_by(2).cloned().collect();
        for id in &to_delete {
            expected.remove(id);
        }

        // New objects the "writer" adds WHILE the mover is running.
        let written: Vec<Uuid> = (0..12).map(|_| Uuid::new_v4()).collect();
        let mut written_bytes: std::collections::HashMap<Uuid, Vec<u8>> = std::collections::HashMap::new();
        for (i, id) in written.iter().enumerate() {
            written_bytes.insert(*id, format!("new{i}").into_bytes());
        }

        let writer_storage = s.clone();
        let writer_bytes = written_bytes.clone();
        let writer = tokio::spawn(async move {
            for (id, bytes) in writer_bytes {
                write_at(&writer_storage.flat_original_path(user, id, "webp"), &bytes).await;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        });
        let deleter_storage = s.clone();
        let deleter_ids = to_delete.clone();
        let deleter = tokio::spawn(async move {
            for id in deleter_ids {
                // May race the mover either way — both outcomes are
                // idempotent/NotFound-tolerant, so the result is ignored.
                let _ = deleter_storage.delete_original(user, id, "webp").await;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        });

        let opts = ShardOptions {
            batch: 2,
            pause: std::time::Duration::from_millis(1),
            ..Default::default()
        };
        let _first = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        writer.await.unwrap();
        deleter.await.unwrap();
        // A follow-up run picks up anything the race left flat (a write that
        // arrived after the mover's last pass, or a rename that lost a race
        // to a delete).
        let follow_up = s.shard_flat_originals(user, &opts, &mut |_| {}).await.unwrap();
        let follow_up_census = follow_up
            .census
            .as_ref()
            .expect("a normal finish always carries a census");
        assert_eq!(
            follow_up_census.remaining_flat, 0,
            "a follow-up run must converge to zero flat objects: {follow_up:?}"
        );

        for id in &to_survive {
            let bytes = s
                .load_original(user, *id, "webp")
                .await
                .unwrap_or_else(|e| panic!("surviving object {id} must still load: {e}"));
            assert_eq!(&bytes, &expected[id], "surviving object {id} must keep its exact bytes");
        }
        for id in &to_delete {
            assert!(
                s.resolve_original_path(user, *id, "webp").await.is_none(),
                "deleted object {id} must never be resurrected"
            );
        }
        for id in &written {
            let bytes = s.load_original(user, *id, "webp").await.unwrap();
            assert_eq!(
                &bytes, &written_bytes[id],
                "written object {id} must keep its exact bytes"
            );
        }
    }

    /// Round-3 regression, MEDIUM finding: `open_regular_nofollow` used to map
    /// only `NotFound` and `ELOOP`; ANY other open error (ENXIO from a UNIX
    /// socket planted at the shard target, EACCES, ENOTDIR, ...) propagated
    /// and the mover's `bail!` aborted the WHOLE migration. A single planted
    /// socket halted every future re-run. Binds a real `UnixListener`
    /// somewhere short (so it fits `sun_path`'s length limit) and hard-links
    /// the resulting socket special file into the shard target — a hard link
    /// to a socket is a normal directory-entry operation on the same
    /// filesystem, and `open()`'s behavior is driven by the inode's type
    /// regardless of which link reached it, so this is a genuine socket at
    /// the target, not a synthesized stand-in.
    #[cfg(unix)]
    #[tokio::test]
    async fn shard_flat_originals_treats_a_socket_at_the_target_as_a_conflict_not_a_fatal_error() {
        let (dir, s) = sharded();
        let user = Uuid::new_v4();

        let blocked = Uuid::new_v4();
        let flat_blocked = s.flat_original_path(user, blocked, "webp");
        write_at(&flat_blocked, b"should survive").await;
        let to = s.sharded_original_path(user, blocked, "webp");
        tokio::fs::create_dir_all(to.parent().unwrap())
            .await
            .unwrap();

        let short_dir = tempfile::Builder::new().prefix("sk").tempdir().unwrap();
        let short_sock = short_dir.path().join("s");
        let _listener = std::os::unix::net::UnixListener::bind(&short_sock)
            .expect("bind a real unix socket at a short path");
        std::fs::hard_link(&short_sock, &to)
            .expect("hard-link the socket special file into the shard target (same fs as /tmp)");

        // A SECOND, unrelated object in the same user namespace must still
        // move normally in the SAME run — the socket must not stall it.
        let other = Uuid::new_v4();
        write_at(&s.flat_original_path(user, other, "webp"), b"moves fine").await;

        let opts = ShardOptions::default();
        let r = s
            .shard_flat_originals(user, &opts, &mut |_| {})
            .await
            .expect("a planted socket must not make the whole run return Err");

        // Round-4 LOW finding: a socket was never opened, so it is counted
        // as `refused` (this test's name predates that split — see
        // FIX_ROUND-3.md, left as written per the "don't rewrite history"
        // convention — `conflicts` is now reserved for a byte-different pair
        // that both opened fine). FIX_ROUND-6: these come from the
        // end-of-run census now.
        let c = r.census.as_ref().expect("a normal finish always carries a census");
        assert_eq!(
            c.refused, 1,
            "the socket target must be counted as refused, not conflicts: {r:?}"
        );
        assert_eq!(c.conflicts, 0, "no bytes were ever compared: {r:?}");
        assert!(
            flat_blocked.exists(),
            "the flat object behind a refused target must survive"
        );
        assert_eq!(
            tokio::fs::read(&flat_blocked).await.unwrap(),
            b"should survive",
            "the surviving flat object must be untouched"
        );
        assert!(
            s.resolve_original_path(user, other, "webp").await.is_some(),
            "an unrelated object must still have moved despite the socket conflict"
        );
        assert!(!s.flat_original_path(user, other, "webp").exists());
        drop(dir);
    }

    /// A small writer that forwards every `write` into a shared buffer, used
    /// to capture `tracing` output for the next test (asserting on the LOG
    /// TEXT, not just the counters — the bug this regresses only changes the
    /// message, not the count).
    #[derive(Clone, Default)]
    struct CaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Round-3 regression, LOW finding: the FLAT-source side of the
    /// dedup-compare used to collapse `NotFound`/`Refused`/any real `Err`
    /// into a bare `_ => false`, so an unreadable (not vanished) flat source
    /// got logged with the same "both locations hold DIFFERENT bytes" text as
    /// a genuine byte mismatch — false, since no bytes were ever compared.
    /// Captures the actual `tracing::warn!` text to prove the log now states
    /// the REAL reason and no longer makes that false claim.
    #[cfg(unix)]
    #[tokio::test]
    async fn shard_flat_originals_counts_an_unreadable_flat_source_as_a_conflict() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipping: running as root — chmod 000 does not deny root's own reads");
            return;
        }
        let (dir, s) = sharded();
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let flat = s.flat_original_path(user, id, "webp");
        write_at(&flat, b"unreadable").await;
        // A pre-existing sharded target (any regular file) so the mover
        // reaches the dedup-compare's flat-source open at all — with no
        // target there it would simply rename, never opening `from` through
        // this path.
        write_at(&s.sharded_original_path(user, id, "webp"), b"target").await;
        std::fs::set_permissions(&flat, std::fs::Permissions::from_mode(0o000)).unwrap();

        let capture = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_ansi(false)
            .with_level(false)
            .with_target(false)
            .finish();
        let opts = ShardOptions::default();
        let r = {
            let _guard = tracing::subscriber::set_default(subscriber);
            s.shard_flat_originals(user, &opts, &mut |_| {})
                .await
                .unwrap()
        };

        // Restore so TempDir cleanup can remove it.
        std::fs::set_permissions(&flat, std::fs::Permissions::from_mode(0o644)).unwrap();

        // Round-4 LOW finding: never opened, so `refused`, not `conflicts`
        // (this test's name predates that split — see FIX_ROUND-3.md, left
        // as written per the "don't rewrite history" convention).
        // FIX_ROUND-6: these come from the end-of-run census now.
        let c = r.census.as_ref().expect("a normal finish always carries a census");
        assert_eq!(
            c.refused, 1,
            "an unreadable flat source must be counted as refused, not silently resolved: {r:?}"
        );
        assert_eq!(c.conflicts, 0, "no bytes were ever compared: {r:?}");
        assert!(
            flat.exists(),
            "the flat object must survive being left as refused"
        );

        let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert!(
            !logs.contains("DIFFERENT bytes"),
            "the log must not claim a byte mismatch when no bytes were ever compared: {logs}"
        );
        assert!(
            logs.to_ascii_lowercase().contains("permission") || logs.contains("reason"),
            "the log must state the REAL reason (a permission error), not a generic claim: {logs}"
        );
        drop(dir);
    }

    /// FIX_ROUND-6.md property test: for a randomized mix of flat-directory
    /// entries, a `shard_flat_originals` call's own `census` must equal an
    /// INDEPENDENT, freshly-taken `census()` of the same directory, and
    /// `moved + deduplicated + remaining_flat` must account for every flat
    /// object this test created — regardless of the random mix, batch size,
    /// or stopping point. This is the invariant the FIX_ROUND-6 re-scope
    /// exists to make true BY CONSTRUCTION (one classification function,
    /// one census function, called fresh at the end — never a per-pass
    /// snapshot that could drift).
    ///
    /// "Random abort point": this crate has no portable way to land a REAL
    /// I/O error at a specific, unspecified-`read_dir`-order point
    /// deterministically (that's exactly the kind of non-determinism the
    /// dedicated abort tests above work hard to avoid). This property test
    /// instead randomizes `limit` — a bounded/interrupted run is the
    /// realistic shape an operator's own re-run resumes from, and it
    /// exercises the SAME code path (the loop breaking before every flat
    /// object is visited) that a real I/O abort also leaves behind. Fixed
    /// seeds keep every run deterministic.
    #[cfg(unix)]
    #[tokio::test]
    async fn shard_flat_originals_census_matches_an_independent_recount_for_randomized_mixes() {
        for seed in [1u64, 7, 42, 1_000, 99_999] {
            run_one_randomized_census_check(seed).await;
        }
    }

    /// A tiny, dependency-free xorshift64* PRNG — deterministic per seed,
    /// which is the whole point of the property test above (no external
    /// `rand` crate dependency pulled in for one test).
    struct Lcg(u64);
    impl Lcg {
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        /// Uniform-enough in `0..n` for test fixture generation (not
        /// cryptographic, not perfectly unbiased — fine for picking a
        /// category out of 6 or a batch size out of 5).
        fn below(&mut self, n: u64) -> u64 {
            self.next_u64() % n.max(1)
        }
    }

    #[cfg(unix)]
    async fn run_one_randomized_census_check(seed: u64) {
        let mut rng = Lcg(seed.wrapping_mul(2) | 1); // nonzero, xorshift needs it
        let (dir, s) = sharded();
        let user = Uuid::new_v4();
        let ns_dir = dir.path().join("originals").join(user.to_string());

        // A shared socket special file, hard-linked into every
        // "refused-socket" target — same trick as
        // `shard_flat_originals_treats_a_socket_at_the_target_as_a_conflict_not_a_fatal_error`.
        let short_dir = tempfile::Builder::new().prefix("sk").tempdir().unwrap();
        let short_sock = short_dir.path().join("s");
        let _listener = std::os::unix::net::UnixListener::bind(&short_sock)
            .expect("bind a real unix socket at a short path");

        let n = 15 + rng.below(15); // 15..30 entries
        let mut initial_flat_objects = 0u64;
        for i in 0..n {
            match rng.below(6) {
                0 => {
                    // plain: no pre-existing shard target.
                    let id = Uuid::new_v4();
                    write_at(
                        &s.flat_original_path(user, id, "webp"),
                        format!("plain-{seed}-{i}").as_bytes(),
                    )
                    .await;
                    initial_flat_objects += 1;
                }
                1 => {
                    // dedup: an IDENTICAL pre-existing shard target.
                    let id = Uuid::new_v4();
                    let bytes = format!("dedup-{seed}-{i}");
                    write_at(&s.flat_original_path(user, id, "webp"), bytes.as_bytes()).await;
                    write_at(&s.sharded_original_path(user, id, "webp"), bytes.as_bytes()).await;
                    initial_flat_objects += 1;
                }
                2 => {
                    // conflict: a DIFFERENT pre-existing shard target.
                    let id = Uuid::new_v4();
                    write_at(
                        &s.flat_original_path(user, id, "webp"),
                        format!("flat-{seed}-{i}").as_bytes(),
                    )
                    .await;
                    write_at(
                        &s.sharded_original_path(user, id, "webp"),
                        format!("sharded-{seed}-{i}").as_bytes(),
                    )
                    .await;
                    initial_flat_objects += 1;
                }
                3 => {
                    // refused: a symlink planted at the shard target.
                    let id = Uuid::new_v4();
                    write_at(
                        &s.flat_original_path(user, id, "webp"),
                        format!("refused-sym-{seed}-{i}").as_bytes(),
                    )
                    .await;
                    let victim = dir.path().join(format!("victim-{seed}-{i}.webp"));
                    tokio::fs::write(&victim, format!("victim-{seed}-{i}").as_bytes())
                        .await
                        .unwrap();
                    let to = s.sharded_original_path(user, id, "webp");
                    tokio::fs::create_dir_all(to.parent().unwrap()).await.unwrap();
                    std::os::unix::fs::symlink(&victim, &to).unwrap();
                    initial_flat_objects += 1;
                }
                4 => {
                    // refused: a socket special file hard-linked at the
                    // shard target.
                    let id = Uuid::new_v4();
                    write_at(
                        &s.flat_original_path(user, id, "webp"),
                        format!("refused-sock-{seed}-{i}").as_bytes(),
                    )
                    .await;
                    let to = s.sharded_original_path(user, id, "webp");
                    tokio::fs::create_dir_all(to.parent().unwrap()).await.unwrap();
                    std::fs::hard_link(&short_sock, &to).unwrap();
                    initial_flat_objects += 1;
                }
                _ => {
                    // junk: never a flat OBJECT, so never counted in
                    // `initial_flat_objects` — a non-uuid regular file, or a
                    // symlink sitting directly at the flat level (its NAME
                    // may look like `<uuid>.<ext>`, but `flat_object_name`
                    // requires `is_file()`, which a symlink never is).
                    if rng.below(2) == 0 {
                        write_at(
                            &ns_dir.join(format!("junk-{seed}-{i}.txt")),
                            b"not an object",
                        )
                        .await;
                    } else {
                        tokio::fs::create_dir_all(&ns_dir).await.unwrap();
                        let victim = ns_dir.join(format!("victim-junk-{seed}-{i}.webp"));
                        tokio::fs::write(&victim, b"x").await.unwrap();
                        std::os::unix::fs::symlink(
                            &victim,
                            ns_dir.join(format!("{}.webp", Uuid::new_v4())),
                        )
                        .unwrap();
                    }
                }
            }
        }

        let batch = 1 + rng.below(5); // 1..=5
        // Random "stopping point": either an unbounded run (converges fully
        // in this single-threaded, no-concurrent-writer test) or a limited
        // one (the shape of a real interrupted/resumed migration).
        let limit = if rng.below(2) == 0 {
            None
        } else {
            Some(rng.below(initial_flat_objects + 1))
        };
        let opts = ShardOptions {
            batch: batch as usize,
            pause: std::time::Duration::ZERO,
            limit,
            max_passes: 50,
        };
        let report = s
            .shard_flat_originals(user, &opts, &mut |_| {})
            .await
            .unwrap_or_else(|e| panic!("seed {seed}: shard_flat_originals failed: {e}"));
        let fresh = s
            .census(user)
            .await
            .unwrap_or_else(|e| panic!("seed {seed}: independent recount failed: {e}"));

        let c = report.census.as_ref().unwrap_or_else(|| {
            panic!("seed {seed}: a normal finish always carries a census: {report:?}")
        });
        assert_eq!(
            c, &fresh,
            "seed {seed}: the report's own census must equal an independent recount taken \
             right after the run: {report:?} vs {fresh:?}"
        );
        assert_eq!(
            report.moved + report.deduplicated + c.remaining_flat,
            initial_flat_objects,
            "seed {seed}: moved + deduplicated + remaining_flat must account for every flat \
             object this test created (no concurrent deletions here): {report:?}"
        );
    }
}
