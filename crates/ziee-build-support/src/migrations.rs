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
/// under each `module_root`. CONTENT-STABLE: creates `merged_dir` if missing,
/// guards against basename collisions across modules (a duplicate migration
/// filename would silently drop one), writes each `.sql` only when its bytes
/// differ, and deletes only merged files no longer in the composed set — so a
/// no-op build leaves the dir + file mtimes untouched. Emits
/// `cargo:rerun-if-changed` for every root, every source dir, and the merged dir
/// so cargo re-runs the build when any migration ACTUALLY changes (but not
/// spuriously on every build).
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
    // CONTENT-STABLE compose (no wipe-and-recreate). The merged dir is declared
    // `cargo:rerun-if-changed` (below + in build.rs), and cargo watches a
    // DIRECTORY by its OWN mtime. The old `remove_dir_all` + unconditional
    // `std::fs::copy` gave the dir AND every file a fresh mtime on EVERY build,
    // so build.rs re-dirtied its own watched input AFTER cargo captured the
    // fingerprint → a spurious full recompile of the crate on every no-op build.
    // Instead we create the dir if missing, write each file only when its bytes
    // actually differ (unchanged files keep their mtime), and delete only files
    // that are no longer in the composed set. A true no-op build touches nothing,
    // so the dir mtime is stable and cargo stays Fresh.
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
    // ship the same migration filename — it would silently drop one). `seen`
    // doubles as the set of composed basenames for the delete-removed step.
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
                // Write-on-diff: only touch the destination when its bytes differ
                // (or it's missing). An unchanged migration is NOT rewritten, so
                // its mtime is preserved and the merged dir stays Fresh.
                let src_bytes = std::fs::read(&path).unwrap_or_else(|e| {
                    panic!("build.rs: read migration {} failed: {}", path.display(), e)
                });
                let needs_write = match std::fs::read(&dst) {
                    Ok(existing) => existing != src_bytes,
                    Err(_) => true, // missing (or unreadable) → write
                };
                if needs_write {
                    std::fs::write(&dst, &src_bytes).unwrap_or_else(|e| {
                        panic!("build.rs: write {} → merged failed: {}", dst.display(), e)
                    });
                }
            }
        }
    }

    // Mirror a REMOVED source migration: delete any `.sql` in the merged dir that
    // is no longer in the composed set. Done by name so unchanged files are never
    // touched (deleting a file that IS in the set would churn the dir mtime).
    // Non-`.sql` files are left alone (the compose only ever writes `.sql`).
    if let Ok(entries) = std::fs::read_dir(merged_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("sql") {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !seen.contains_key(&name) {
                std::fs::remove_file(&path).unwrap_or_else(|e| {
                    panic!(
                        "build.rs: remove stale merged migration {} failed: {}",
                        path.display(),
                        e
                    )
                });
            }
        }
    }

    println!("cargo:rerun-if-changed={}", merged_dir.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create `<root>/<module>/migrations/<file>` with `body` and return the
    /// `migrations` dir path.
    fn write_module_migration(root: &Path, module: &str, file: &str, body: &str) -> PathBuf {
        let dir = root.join(module).join("migrations");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), body).unwrap();
        dir
    }

    fn sorted_merged_names(merged: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(merged)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn unions_globbed_module_migrations_and_ignores_non_sql() {
        let tmp = tempfile::tempdir().unwrap();
        let app_root = tmp.path().join("modules");
        write_module_migration(&app_root, "foo", "00000000000001_foo_init.sql", "-- foo");
        write_module_migration(&app_root, "bar", "00000000000002_bar_init.sql", "-- bar");
        // A non-.sql file in a migrations dir must be ignored, not copied.
        fs::write(
            app_root.join("bar").join("migrations").join("README.md"),
            "notes",
        )
        .unwrap();
        // A module directory with no `migrations` subdir is simply skipped.
        fs::create_dir_all(app_root.join("baz")).unwrap();

        let merged = tmp.path().join("merged");
        compose_merged_migrations(&merged, &[app_root]);

        assert_eq!(
            sorted_merged_names(&merged),
            vec![
                "00000000000001_foo_init.sql".to_string(),
                "00000000000002_bar_init.sql".to_string(),
            ]
        );
        // Content is copied verbatim.
        assert_eq!(
            fs::read_to_string(merged.join("00000000000001_foo_init.sql")).unwrap(),
            "-- foo"
        );
    }

    #[test]
    fn stale_merged_migration_is_removed_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let app_root = tmp.path().join("modules");
        write_module_migration(&app_root, "foo", "00000000000001_foo_init.sql", "-- foo");
        let merged = tmp.path().join("merged");
        // Seed a stale file that a prior compose would have left behind (a source
        // migration that was since removed).
        fs::create_dir_all(&merged).unwrap();
        fs::write(merged.join("99999999999999_stale.sql"), "-- stale").unwrap();

        compose_merged_migrations(&merged, &[app_root]);

        assert_eq!(
            sorted_merged_names(&merged),
            vec!["00000000000001_foo_init.sql".to_string()],
            "a merged .sql no longer in the composed set must be deleted by name"
        );
    }

    /// TEST-1: a no-op recompose (unchanged sources) must NOT rewrite any merged
    /// file — the file mtimes are preserved. This is the mechanism that keeps
    /// `migrations-merged`'s directory mtime stable so cargo stops spuriously
    /// recompiling the crate on every build.
    #[test]
    fn recompose_over_unchanged_sources_preserves_file_mtimes() {
        let tmp = tempfile::tempdir().unwrap();
        let app_root = tmp.path().join("modules");
        write_module_migration(&app_root, "foo", "00000000000001_foo_init.sql", "-- foo");
        write_module_migration(&app_root, "bar", "00000000000002_bar_init.sql", "-- bar");
        let merged = tmp.path().join("merged");

        compose_merged_migrations(&merged, &[app_root.clone()]);
        let f1 = merged.join("00000000000001_foo_init.sql");
        let f2 = merged.join("00000000000002_bar_init.sql");
        let m1 = fs::metadata(&f1).unwrap().modified().unwrap();
        let m2 = fs::metadata(&f2).unwrap().modified().unwrap();
        // The DIRECTORY mtime is what cargo's `rerun-if-changed=<dir>` watch
        // actually keys on (a dir mtime advances only on entry add/remove/rename).
        let dir_m = fs::metadata(&merged).unwrap().modified().unwrap();

        // Sleep so a *rewrite* / dir mutation would produce a strictly different
        // mtime on any filesystem with >=1s granularity — then recompose unchanged.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        compose_merged_migrations(&merged, &[app_root]);

        assert_eq!(
            fs::metadata(&f1).unwrap().modified().unwrap(),
            m1,
            "unchanged migration must not be rewritten (mtime preserved)"
        );
        assert_eq!(
            fs::metadata(&f2).unwrap().modified().unwrap(),
            m2,
            "unchanged migration must not be rewritten (mtime preserved)"
        );
        // The load-bearing invariant: the merged DIR mtime is unchanged across a
        // no-op recompose (no entry added/removed) — this is precisely why cargo
        // stops spuriously recompiling the consuming crate.
        assert_eq!(
            fs::metadata(&merged).unwrap().modified().unwrap(),
            dir_m,
            "no-op recompose must not touch the merged dir mtime (cargo watches it)"
        );
    }

    /// TEST-3 (changed-content leg): a source whose BYTES changed IS rewritten,
    /// and the merged copy reflects the new content verbatim.
    #[test]
    fn recompose_rewrites_only_changed_content() {
        let tmp = tempfile::tempdir().unwrap();
        let app_root = tmp.path().join("modules");
        write_module_migration(&app_root, "foo", "00000000000001_foo_init.sql", "-- foo v1");
        let merged = tmp.path().join("merged");
        compose_merged_migrations(&merged, &[app_root.clone()]);
        let dst = merged.join("00000000000001_foo_init.sql");
        assert_eq!(fs::read_to_string(&dst).unwrap(), "-- foo v1");

        // Change the source bytes and recompose.
        write_module_migration(&app_root, "foo", "00000000000001_foo_init.sql", "-- foo v2");
        compose_merged_migrations(&merged, &[app_root]);
        assert_eq!(
            fs::read_to_string(&dst).unwrap(),
            "-- foo v2",
            "changed source content must be written through to the merged copy"
        );
    }

    #[test]
    fn includes_explicit_sdk_dirs_and_skips_missing_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let app_root = tmp.path().join("modules");
        write_module_migration(&app_root, "foo", "00000000000001_foo_init.sql", "-- foo");

        // An explicit SDK-crate migrations dir that exists → included.
        let sdk_auth = tmp.path().join("sdk").join("ziee-auth").join("migrations");
        fs::create_dir_all(&sdk_auth).unwrap();
        fs::write(sdk_auth.join("00000000000003_auth.sql"), "-- auth").unwrap();

        // An explicit SDK dir that does NOT exist → silently skipped (a linked
        // crate may ship no migrations).
        let sdk_missing = tmp.path().join("sdk").join("ziee-absent").join("migrations");

        let merged = tmp.path().join("merged");
        compose_merged_migrations_from(
            &merged,
            &[app_root],
            &[sdk_auth.clone(), sdk_missing],
        );

        assert_eq!(
            sorted_merged_names(&merged),
            vec![
                "00000000000001_foo_init.sql".to_string(),
                "00000000000003_auth.sql".to_string(),
            ]
        );
    }

    #[test]
    #[should_panic(expected = "duplicate migration filename")]
    fn panics_on_basename_collision_across_modules() {
        let tmp = tempfile::tempdir().unwrap();
        let app_root = tmp.path().join("modules");
        // Two different modules ship the SAME migration filename → must panic
        // (silently dropping one would corrupt the schema history).
        write_module_migration(&app_root, "foo", "00000000000001_dup.sql", "-- foo");
        write_module_migration(&app_root, "bar", "00000000000001_dup.sql", "-- bar");

        let merged = tmp.path().join("merged");
        compose_merged_migrations(&merged, &[app_root]);
    }

    #[test]
    fn empty_roots_produce_empty_merged_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let merged = tmp.path().join("merged");
        compose_merged_migrations(&merged, &[]);
        assert!(merged.is_dir(), "merged dir is created even with no sources");
        assert_eq!(sorted_merged_names(&merged), Vec::<String>::new());
    }
}
