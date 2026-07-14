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
    let _ = std::fs::remove_dir_all(merged_dir);
    std::fs::create_dir_all(merged_dir).expect("build.rs: create migrations-merged failed");

    // Glob every `migrations` dir under each module root. A root that has no
    // module with migrations yet is simply skipped.
    let mut sources: Vec<PathBuf> = Vec::new();
    for root in module_roots {
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
