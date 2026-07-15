//! `--dump-seed` — serialize the current seed-owned DB state back to a YAML doc.
//!
//! Collects every registered `SeedProvider`, calls its `dump()`, and assembles a
//! `section -> value` mapping. Secrets are emitted as `${…}` placeholders by each
//! provider, never as values.

use sqlx::PgPool;

use crate::provider::{SeedCtx, SEED_PROVIDERS};

/// Produce the YAML string that, re-applied, reconstructs the current seed-owned set.
pub async fn dump_all(pool: &PgPool) -> Result<String, String> {
    let ctx = SeedCtx::new(pool.clone());

    let mut entries: Vec<_> = SEED_PROVIDERS.iter().collect();
    entries.sort_by_key(|e| e.order);

    let mut root = serde_norway::Mapping::new();
    for entry in entries {
        let provider = (entry.factory)();
        match provider.dump(&ctx).await {
            Ok(Some(value)) => {
                root.insert(
                    serde_norway::Value::String(provider.section().to_string()),
                    value,
                );
            }
            Ok(None) => {}
            Err(e) => {
                return Err(format!("dump failed for section {}: {e}", provider.section()));
            }
        }
    }

    serde_norway::to_string(&serde_norway::Value::Mapping(root))
        .map_err(|e| format!("serializing dumped seed to YAML failed: {e}"))
}
