//! The seed runner: load + deep-merge the layered YAML (an app-supplied embedded default
//! + an operator overlay), take a Postgres advisory lock, and dispatch each section to the
//! [`SeedProvider`](crate::SeedProvider) the app registered into
//! [`SEED_PROVIDERS`](crate::SEED_PROVIDERS).
//!
//! Domain-neutral: the engine embeds NO default data — the app passes its embedded default
//! YAML as a parameter (decision N9). Modes: `seed-if-empty` (default, safe every boot +
//! after every wipe — never clobbers) vs `reconcile` (opt-in, authoritative — re-syncs +
//! deletes seed-owned rows). Secrets are `${ENV_VAR}` placeholders only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sqlx::PgPool;

use crate::config::SeedConfig;
use crate::provider::{SeedCtx, SeedMode, SeedOutcome, SeedSection, SEED_PROVIDERS};

/// A parsed seed document: `section -> section body`.
pub type Doc = BTreeMap<String, SeedSection>;

/// Env var naming the operator overlay file (or dir). A `SeedConfig.overlay_path` takes
/// precedence. Neutral (no app prefix) — an app may map its own env var onto
/// `SeedConfig.overlay_path` in its config layer.
pub const SEED_FILE_ENV: &str = "SEED_FILE";
/// Env var forcing authoritative reconcile mode globally (truthy).
pub const SEED_RECONCILE_ENV: &str = "SEED_RECONCILE";

/// Largest overlay file/dir-entry we read (a manifest is a few KiB).
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Advisory lock key serializing the seed across concurrently-booting containers.
const SEED_LOCK_KEY: i64 = 0x5EED_1ED6;
const LOCK_WAIT_ATTEMPTS: usize = 60;
const LOCK_WAIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

fn truthy(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// Parse a YAML string into a [`Doc`]. A section body may be a mapping (with
/// `mode`/`reset`/`remove`/`items`) or a bare sequence (treated as `items`).
pub fn parse_doc(raw: &str) -> Result<Doc, String> {
    let root: serde_norway::Value =
        serde_norway::from_str(raw).map_err(|e| format!("invalid seed YAML: {e}"))?;
    let mut doc = Doc::new();
    match root {
        serde_norway::Value::Null => {}
        serde_norway::Value::Mapping(map) => {
            for (k, v) in map {
                let section = match k {
                    serde_norway::Value::String(s) => s,
                    other => return Err(format!("seed section key must be a string, got {other:?}")),
                };
                let body: SeedSection = match v {
                    serde_norway::Value::Sequence(items) => SeedSection {
                        items,
                        ..Default::default()
                    },
                    serde_norway::Value::Null => SeedSection::default(),
                    mapping @ serde_norway::Value::Mapping(_) => serde_norway::from_value(mapping)
                        .map_err(|e| format!("invalid `{section}` seed section: {e}"))?,
                    other => {
                        return Err(format!(
                            "seed section `{section}` must be a mapping or list, got {other:?}"
                        ))
                    }
                };
                doc.insert(section, body);
            }
        }
        other => return Err(format!("seed document root must be a mapping, got {other:?}")),
    }
    Ok(doc)
}

/// An item's natural key = its `name` field (if any). Used for merge-by-key.
fn item_key(v: &serde_norway::Value) -> Option<String> {
    v.get("name").and_then(|n| n.as_str()).map(str::to_string)
}

/// Deep-merge `overlay` onto `base` by `(section, item natural-key)`. Named overlay
/// items override the base item with the same `name`; new named items append; unnamed
/// overlay items REPLACE the base's unnamed items (singleton settings). A section's
/// `mode`/`reset` overlay value wins; `remove` lists concatenate.
pub fn merge_docs(base: Doc, overlay: Doc) -> Doc {
    let mut merged = base;
    for (section, ov) in overlay {
        let entry = merged.entry(section).or_default();

        if ov.mode.is_some() {
            entry.mode = ov.mode;
        }
        if ov.reset.is_some() {
            entry.reset = ov.reset;
        }
        entry.remove.extend(ov.remove);

        let ov_has_unnamed = ov.items.iter().any(|i| item_key(i).is_none());
        if ov_has_unnamed {
            entry.items.retain(|i| item_key(i).is_some());
        }
        for ov_item in ov.items {
            match item_key(&ov_item) {
                Some(key) => {
                    if let Some(slot) = entry
                        .items
                        .iter_mut()
                        .find(|b| item_key(b).as_deref() == Some(key.as_str()))
                    {
                        *slot = ov_item;
                    } else {
                        entry.items.push(ov_item);
                    }
                }
                None => entry.items.push(ov_item),
            }
        }
    }
    merged
}

/// Resolve the overlay path: `SeedConfig.overlay_path`, else the `SEED_FILE` env var.
fn overlay_path(config: &SeedConfig) -> Option<PathBuf> {
    if let Some(p) = &config.overlay_path {
        if !p.trim().is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    std::env::var_os(SEED_FILE_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn read_capped(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "{} is {} bytes, over the {MAX_FILE_BYTES}-byte cap",
            path.display(),
            meta.len()
        ));
    }
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Load + merge the overlay (a file OR a directory of `*.yaml`, lexically sorted).
/// `None` path ⇒ `Ok(None)`. A REQUESTED-but-unusable path is a fatal `Err`.
fn load_overlay(config: &SeedConfig) -> Result<Option<Doc>, String> {
    let Some(path) = overlay_path(config) else {
        return Ok(None);
    };
    if !path.exists() {
        return Err(format!(
            "seed overlay {} was requested but does not exist",
            path.display()
        ));
    }
    let meta = std::fs::metadata(&path).map_err(|e| format!("cannot stat {}: {e}", path.display()))?;

    let mut acc = Doc::new();
    if meta.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
            .map_err(|e| format!("cannot read seed dir {}: {e}", path.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.is_file()
                    && matches!(
                        p.extension().and_then(|s| s.to_str()),
                        Some("yaml") | Some("yml")
                    )
            })
            .collect();
        files.sort();
        for f in files {
            let raw = read_capped(&f)?;
            let doc = parse_doc(&raw).map_err(|e| format!("{}: {e}", f.display()))?;
            acc = merge_docs(acc, doc);
        }
    } else if meta.is_file() {
        let raw = read_capped(&path)?;
        acc = parse_doc(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
    } else {
        return Err(format!(
            "{} is neither a file nor a directory (a bind-mount of a missing host path creates a directory)",
            path.display()
        ));
    }
    Ok(Some(acc))
}

/// Build the merged desired-state Doc from the app's embedded default + optional overlay.
fn build_doc(config: &SeedConfig, default_yaml: &str) -> Result<Doc, String> {
    let base =
        parse_doc(default_yaml).map_err(|e| format!("the app's embedded default seed is invalid: {e}"))?;
    match load_overlay(config)? {
        Some(overlay) => Ok(merge_docs(base, overlay)),
        None => Ok(base),
    }
}

/// Reconcile the declarative seed into the DB.
///
/// `default_yaml` is the APP's embedded Layer-0 default (the engine embeds none — N9);
/// pass `"{}"` for an app with no baked defaults. `Ok(())` also covers "nothing to do"
/// (disabled / empty). `Err` means the seed configuration is unusable (a requested overlay
/// file that cannot be read/parsed) — the caller FAILS BOOT rather than serve a silently-
/// misconfigured deployment. A single failing PROVIDER is logged and skipped, never fatal.
pub async fn run(pool: &PgPool, config: &SeedConfig, default_yaml: &str) -> Result<(), String> {
    if !config.enabled {
        tracing::info!("seed: disabled (seed.enabled = false) — no seeding");
        return Ok(());
    }

    let global_reconcile = config.reconcile
        || std::env::var(SEED_RECONCILE_ENV).map(|v| truthy(&v)).unwrap_or(false);

    // Build the merged doc first — a bad requested overlay must fail before we take the
    // lock or touch the DB.
    let doc = build_doc(config, default_yaml)?;

    tracing::info!(
        reconcile = global_reconcile,
        sections = doc.len(),
        "seed: applying declarative seed"
    );

    apply_doc_locked(pool, &doc, global_reconcile).await
}

/// Programmatic/test entrypoint: apply a seed doc built from an optional base (default)
/// YAML plus an overlay YAML string, under the advisory lock. `run()` is the config/env-
/// driven wrapper around the same machinery. A bad YAML is an `Err` (mirrors the
/// fatal-bad-file contract); a failing provider is logged + skipped.
pub async fn run_from_yaml(
    pool: &PgPool,
    overlay_yaml: Option<&str>,
    reconcile: bool,
    base_yaml: &str,
) -> Result<(), String> {
    let base = parse_doc(base_yaml).map_err(|e| format!("base seed YAML is invalid: {e}"))?;
    let doc = match overlay_yaml {
        Some(raw) => merge_docs(base, parse_doc(raw)?),
        None => base,
    };
    apply_doc_locked(pool, &doc, reconcile).await
}

/// Take the advisory lock, dispatch every provider, release the lock.
///
/// The lock serializes across concurrently-booting containers (rolling redeploy / scale):
/// a section may upsert into a table without a unique index, so a naive check-then-insert
/// would race into duplicates.
async fn apply_doc_locked(pool: &PgPool, doc: &Doc, reconcile: bool) -> Result<(), String> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| format!("cannot acquire a connection for the seed lock: {e}"))?;

    let mut locked = false;
    for _ in 0..LOCK_WAIT_ATTEMPTS {
        match sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
            .bind(SEED_LOCK_KEY)
            .fetch_one(&mut *conn)
            .await
        {
            Ok(true) => {
                locked = true;
                break;
            }
            Ok(false) => {
                tracing::info!("seed: another instance is seeding; waiting for the lock");
                tokio::time::sleep(LOCK_WAIT_INTERVAL).await;
            }
            Err(e) => return Err(format!("cannot take the seed advisory lock: {e}")),
        }
    }
    if !locked {
        return Err(
            "another instance has held the seed lock for too long; giving up (the container will \
             retry on restart)"
                .to_string(),
        );
    }

    // Dispatch isolates each provider's panic internally (see `dispatch`), so it can never
    // unwind past the unlock below and leak the session-level advisory lock (which survives
    // the connection's return to the pool).
    dispatch(pool, doc, reconcile).await;

    // Release the lock. A PoolConnection returns the LIVE session to the pool, so a
    // session-level advisory lock survives `drop()` — on unlock failure, CLOSE the
    // connection so Postgres drops the lock with the session.
    match sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(SEED_LOCK_KEY)
        .execute(&mut *conn)
        .await
    {
        Ok(_) => drop(conn),
        Err(e) => {
            tracing::warn!(error = %e, "seed: could not release the lock; closing the connection so Postgres drops it");
            let _ = conn.close().await;
        }
    }

    Ok(())
}

/// Dispatch each provider (sorted by order) with its merged section. A provider error OR
/// PANIC is logged and skipped — a single buggy provider never aborts the boot nor skips
/// the providers ordered after it (each `apply` is panic-isolated), and it can never
/// unwind past the advisory-lock release in the caller.
async fn dispatch(pool: &PgPool, doc: &Doc, global_reconcile: bool) {
    use futures::FutureExt;

    let mut entries: Vec<_> = SEED_PROVIDERS.iter().collect();
    entries.sort_by_key(|e| e.order);

    let ctx = SeedCtx::new(pool.clone());
    let mut total = SeedOutcome::default();

    for entry in entries {
        let provider = (entry.factory)();
        let section = doc.get(entry.section);
        let mode = match section {
            Some(s) => s.effective_mode(global_reconcile),
            None if global_reconcile => SeedMode::Reconcile,
            None => SeedMode::SeedIfEmpty,
        };

        let applied = std::panic::AssertUnwindSafe(provider.apply(section, mode, &ctx))
            .catch_unwind()
            .await;
        let result = match applied {
            Ok(r) => r,
            Err(_) => {
                tracing::error!(section = entry.section, "seed: section PANICKED; skipping");
                continue;
            }
        };

        match result {
            Ok(outcome) => {
                if !outcome.is_noop() {
                    tracing::info!(
                        section = entry.section,
                        created = outcome.created,
                        adopted = outcome.adopted,
                        updated = outcome.updated,
                        deleted = outcome.deleted,
                        "seed: section applied"
                    );
                }
                total.merge(outcome);
            }
            Err(e) => {
                tracing::error!(section = entry.section, error = %e, "seed: section failed; skipping");
            }
        }
    }

    tracing::info!(
        created = total.created,
        adopted = total.adopted,
        updated = total.updated,
        deleted = total.deleted,
        "seed: complete"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_list_and_section_body() {
        let doc = parse_doc(
            "providers:\n  - name: a\n  - name: b\nservers:\n  mode: reconcile\n  items:\n    - name: fetch\n",
        )
        .unwrap();
        assert_eq!(doc["providers"].items.len(), 2);
        assert_eq!(doc["servers"].mode, Some(SeedMode::Reconcile));
        assert_eq!(doc["servers"].items.len(), 1);
    }

    #[test]
    fn empty_doc_is_a_legal_noop() {
        assert!(parse_doc("{}").unwrap().is_empty());
        assert!(parse_doc("").unwrap().is_empty());
    }

    #[test]
    fn parses_directives() {
        let doc = parse_doc(
            "providers:\n  mode: seed-if-empty\n  reset: true\n  remove: [old]\n  items: []\n",
        )
        .unwrap();
        let sect = &doc["providers"];
        assert_eq!(sect.mode, Some(SeedMode::SeedIfEmpty));
        assert_eq!(sect.reset, Some(true));
        assert_eq!(sect.remove, vec!["old".to_string()]);
        assert_eq!(sect.effective_mode(false), SeedMode::Reconcile, "reset forces reconcile");
    }

    #[test]
    fn merge_overrides_named_items_and_appends_new() {
        let base = parse_doc("providers:\n  - name: a\n    url: old\n  - name: b\n").unwrap();
        let overlay = parse_doc("providers:\n  - name: a\n    url: new\n  - name: c\n").unwrap();
        let merged = merge_docs(base, overlay);
        let items = &merged["providers"].items;
        assert_eq!(items.len(), 3, "a overridden, b kept, c appended");
        let a = items.iter().find(|i| item_key(i).as_deref() == Some("a")).unwrap();
        assert_eq!(a.get("url").unwrap().as_str(), Some("new"));
    }

    #[test]
    fn merge_directives_and_unnamed_replace() {
        let base = parse_doc("settings:\n  items:\n    - max: 10\n").unwrap();
        let overlay = parse_doc("settings:\n  mode: reconcile\n  items:\n    - max: 50\n").unwrap();
        let merged = merge_docs(base, overlay);
        let sect = &merged["settings"];
        assert_eq!(sect.mode, Some(SeedMode::Reconcile));
        assert_eq!(sect.items.len(), 1);
        assert_eq!(sect.items[0].get("max").unwrap().as_i64(), Some(50));
    }

    #[test]
    fn truthy_matches_usual_values() {
        for on in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(truthy(on));
        }
        for off in ["", "0", "false", "no", "off", "maybe"] {
            assert!(!truthy(off));
        }
    }

    // The SEED_PROVIDERS distributed-slice collects registered entries and the runner
    // sorts them by `order` (a referenced section seeds before a referencing one). Two
    // test-only providers prove the linkme seam works in the engine crate itself.
    use crate::provider::{SeedCtx as Ctx, SeedEntry, SeedError, SeedProvider, SeedSection as Sect};
    use std::sync::Arc;

    struct TestSeeder(&'static str);
    #[async_trait::async_trait]
    impl SeedProvider for TestSeeder {
        fn section(&self) -> &'static str {
            self.0
        }
        async fn apply(
            &self,
            _s: Option<&Sect>,
            _m: SeedMode,
            _c: &Ctx,
        ) -> Result<SeedOutcome, SeedError> {
            Ok(SeedOutcome::default())
        }
        async fn dump(&self, _c: &Ctx) -> Result<Option<serde_norway::Value>, SeedError> {
            Ok(None)
        }
    }

    #[linkme::distributed_slice(SEED_PROVIDERS)]
    static T_LOW: SeedEntry = SeedEntry {
        section: "zz_referenced",
        order: 10,
        factory: || Arc::new(TestSeeder("zz_referenced")),
    };
    #[linkme::distributed_slice(SEED_PROVIDERS)]
    static T_HIGH: SeedEntry = SeedEntry {
        section: "zz_referencing",
        order: 30,
        factory: || Arc::new(TestSeeder("zz_referencing")),
    };

    #[test]
    fn seed_providers_collect_and_sort_by_order() {
        let mut entries: Vec<_> = SEED_PROVIDERS.iter().collect();
        entries.sort_by_key(|e| e.order);
        let pos = |sec: &str| entries.iter().position(|e| e.section == sec);
        let low = pos("zz_referenced").expect("test provider registered");
        let high = pos("zz_referencing").expect("test provider registered");
        assert!(low < high, "lower order sorts first (referenced before referencing)");
    }

    #[test]
    fn seed_config_defaults_and_explicit_values() {
        let default: SeedConfig = serde_norway::from_str("{}").unwrap();
        assert!(default.enabled, "seed defaults to enabled");
        assert!(!default.reconcile, "reconcile defaults to off");
        assert!(default.overlay_path.is_none());
        let explicit: SeedConfig =
            serde_norway::from_str("enabled: false\nreconcile: true\noverlay_path: /etc/app/seed.d\n")
                .unwrap();
        assert!(!explicit.enabled);
        assert!(explicit.reconcile);
        assert_eq!(explicit.overlay_path.as_deref(), Some("/etc/app/seed.d"));
    }
}
