//! Module-owned migration composition for `build.rs` (decision N7 / G5).
//!
//! Composes the merged migration directory used by BOTH an app's build-DB
//! provisioner AND its runtime `sqlx::migrate!`. The merged dir is the UNION of
//! every **module-owned** `migrations/` dir found one level under each supplied
//! `module_root` (e.g. `<manifest>/src/modules/*/migrations/` and
//! `<repo>/sdk/crates/*/migrations/`). The squashed baselines are named
//! `<YYYYMMDDNNNN>_<module>_<desc>.sql`; the merged set version-sorts by
//! filename into a valid FK-topological order.
//!
//! Moved verbatim (behaviour-preserving) from ziee's `server/build.rs::
//! compose_merged_migrations()` in chunk sdk-batteries; ziee's `build.rs` now
//! calls this with its own `merged_dir` + `module_roots`, so the composed
//! output is byte-identical.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Compose `merged_dir` = the UNION of every `*/migrations/` dir found directly
/// under each `module_root`. Wipes + recreates `merged_dir`, guards against
/// basename collisions across modules (a duplicate migration filename would
/// silently drop one), and copies each `.sql` in. Emits `cargo:rerun-if-changed`
/// for every root, every source dir, and the merged dir so cargo re-runs the
/// build when any migration changes.
///
/// `merged_dir` is a generated artifact (gitignored). `build.rs` always runs
/// before the crate compiles, so the dir is populated before the runtime
/// `sqlx::migrate!(<merged_dir>)` macro embeds it.
pub fn compose_merged_migrations(merged_dir: &Path, module_roots: &[PathBuf]) {
    // Back-compat: the original glob-every-crate signature is exactly the
    // explicit-dirs variant with no explicit SDK-crate dirs supplied.
    compose_merged_migrations_from(merged_dir, module_roots, &[]);
}

/// Compose `merged_dir` from an app's OWN module roots (globbed, as
/// [`compose_merged_migrations`]) PLUS an EXPLICIT list of SDK-crate migration
/// dirs the app actually schema-binds (gap N-2).
///
/// [`compose_merged_migrations`] globs `<root>/*/migrations` under every root —
/// including `sdk/crates`, which pulls in the migrations of EVERY SDK crate
/// present on disk regardless of what the app links. A fresh app that links only
/// `ziee-auth` would then merge in `ziee-file` + `ziee-notification` migrations
/// it never uses (and, pre-N-1, break on their domain tables). This variant lets
/// the app scope to its real schema-bound deps:
///
/// - `app_module_roots` — the app's own module roots, GLOBBED for `*/migrations`
///   (e.g. `<manifest>/src/modules`). Same behaviour as the glob signature.
/// - `sdk_crate_migration_dirs` — an EXPLICIT list of `.../migrations` dirs, one
///   per SDK crate the app schema-binds (e.g.
///   `sdk/crates/ziee-auth/migrations`). NOT globbed — the app names exactly the
///   crates it links, so an unlinked crate's schema never enters the merged set.
///   A listed dir that doesn't exist is skipped (a crate may ship no migrations).
///
/// An app should list ONLY its linked schema-bound SDK crates here; the merged
/// set is otherwise identical to the glob path when the two produce the same set
/// of source dirs. Collision-guard, version-sort, and `cargo:rerun-if-changed`
/// emission are shared with the glob path.
pub fn compose_merged_migrations_from(
    merged_dir: &Path,
    app_module_roots: &[PathBuf],
    sdk_crate_migration_dirs: &[PathBuf],
) {
    let _ = std::fs::remove_dir_all(merged_dir);
    std::fs::create_dir_all(merged_dir).expect("build.rs: create migrations-merged failed");

    // Glob every `migrations` dir under each app module root. A root that has no
    // module with migrations yet is simply skipped.
    let mut sources: Vec<PathBuf> = Vec::new();
    for root in app_module_roots {
        println!("cargo:rerun-if-changed={}", root.display());
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let mig = entry.path().join("migrations");
            if mig.is_dir() {
                sources.push(mig);
            }
        }
    }
    // Add the EXPLICIT SDK-crate migration dirs (already `.../migrations`; not
    // globbed). A non-existent dir is skipped — a linked crate may ship none.
    for dir in sdk_crate_migration_dirs {
        // A crate root was passed by mistake? Only accept an actual dir named
        // `migrations` or any existing dir of `.sql` files. We keep it simple:
        // include it iff it is a directory.
        if dir.is_dir() {
            sources.push(dir.clone());
        }
    }
    // Deterministic module order (the per-file version prefix drives the final
    // sqlx apply order; sorting the source dirs only makes the copy stable).
    sources.sort();

    // Guard against basename collisions across modules (two modules must never
    // ship the same migration filename — it would silently drop one).
    let mut seen: HashMap<String, PathBuf> = HashMap::new();
    for src in &sources {
        println!("cargo:rerun-if-changed={}", src.display());
        let entries = std::fs::read_dir(src).unwrap_or_else(|e| {
            panic!(
                "build.rs: cannot read migration source {}: {}",
                src.display(),
                e
            )
        });
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("sql") {
                let name = path.file_name().expect("migration file name");
                let name_str = name.to_string_lossy().to_string();
                if let Some(prev) = seen.insert(name_str.clone(), path.clone()) {
                    panic!(
                        "build.rs: duplicate migration filename '{}' in {} and {} \
                         — module-owned migrations must have unique <YYYYMMDDNNNN> names",
                        name_str,
                        prev.display(),
                        path.display()
                    );
                }
                let dst = merged_dir.join(name);
                std::fs::copy(&path, &dst).unwrap_or_else(|e| {
                    panic!("build.rs: copy {} → merged failed: {}", path.display(), e)
                });
            }
        }
    }
    println!("cargo:rerun-if-changed={}", merged_dir.display());
}
