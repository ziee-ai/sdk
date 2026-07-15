//! Shared helper for seeding the singleton settings tables.
//!
//! A settings table is one CHECK-enforced row; there is no create/delete, only
//! UPDATE-the-declared-columns. Each owning module implements [`SingletonSettings`]
//! (which knows its table + columns + validators) and registers
//! `SingletonSeedProvider(TheirImpl)` into `SEED_PROVIDERS`; the blanket
//! `SeedProvider` impl here supplies the uniform seed-if-empty / reconcile / ledger
//! semantics so no module re-implements them.
//!
//! [`GenericSingleton`] is the concrete impl used by the settings registry: a table +
//! primary-key predicate + a whitelist of `(column, kind)`. `apply_row` builds a runtime
//! `UPDATE <table> SET <declared cols> WHERE <pk>` binding only whitelisted columns the
//! item actually declares (so an admin's other tuned columns are never touched); `dump_row`
//! projects the whitelist back out. An empty whitelist ⇒ adopt + dump-only (the row's
//! migration default is authoritative, e.g. tables with no operator-tunable columns yet).

use async_trait::async_trait;
use sqlx::{Postgres, Row};

use crate::provider::{SeedCtx, SeedError, SeedMode, SeedOutcome, SeedProvider, SeedSection};

/// A module's declaration of how to seed its singleton settings row.
#[async_trait]
pub trait SingletonSettings: Send + Sync {
    /// The section key (also the ledger natural_key — a singleton).
    fn section(&self) -> &'static str;

    /// Apply the declared columns onto the singleton row. `declared` is the section's
    /// first (and only) item — the settings object — or `None` when the section is
    /// absent. Returns `true` if a write happened. Bounds validation is the module's
    /// responsibility (reuse its existing validators; never duplicate CHECK logic).
    async fn apply_row(
        &self,
        declared: Option<&serde_norway::Value>,
        ctx: &SeedCtx,
    ) -> Result<bool, SeedError>;

    /// Serialize the singleton row to a YAML value for `--dump-seed`.
    async fn dump_row(&self, ctx: &SeedCtx) -> Result<Option<serde_norway::Value>, SeedError>;
}

/// Wraps a [`SingletonSettings`] as a full [`SeedProvider`] with the uniform semantics.
pub struct SingletonSeedProvider<T: SingletonSettings>(pub T);

#[async_trait]
impl<T: SingletonSettings + 'static> SeedProvider for SingletonSeedProvider<T> {
    fn section(&self) -> &'static str {
        self.0.section()
    }

    async fn apply(
        &self,
        section: Option<&SeedSection>,
        mode: SeedMode,
        ctx: &SeedCtx,
    ) -> Result<SeedOutcome, SeedError> {
        let sect = self.0.section();
        // The ledger natural_key of a singleton IS its section (= table) name.
        let already = ctx.ledger.is_seeded(sect, sect).await?;

        // seed-if-empty + already owned ⇒ never clobber an admin's tuned setting.
        if already && mode != SeedMode::Reconcile {
            return Ok(SeedOutcome { skipped: 1, ..Default::default() });
        }

        let declared = section.and_then(|s| s.items.first());
        let wrote = self.0.apply_row(declared, ctx).await?;
        ctx.ledger.record(sect, sect, None).await?;

        Ok(if wrote {
            SeedOutcome { updated: 1, ..Default::default() }
        } else {
            SeedOutcome { skipped: 1, ..Default::default() }
        })
    }

    async fn dump(&self, ctx: &SeedCtx) -> Result<Option<serde_norway::Value>, SeedError> {
        self.0.dump_row(ctx).await
    }
}

// ─────────────────────────── generic singleton applier ───────────────────────────

/// Column type for runtime bind (sqlx `query()` is not compile-checked, so the seed keeps
/// a per-column type here). Binding an i64 to an `int4` column errors, so INT vs BIGINT
/// are distinct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    BigInt,
    Text,
    TextArray,
}

/// A declarative singleton: table + pk predicate + whitelisted columns.
pub struct GenericSingleton {
    pub section: &'static str,
    pub table: &'static str,
    /// e.g. `"id = TRUE"` (BOOLEAN pk) or `"id = 1"` (SMALLINT/INT pk).
    pub pk_where: &'static str,
    pub columns: &'static [(&'static str, Kind)],
}

enum Bound {
    Bool(bool),
    Int(i32),
    BigInt(i64),
    Text(String),
    TextArray(Vec<String>),
}

/// Parse a declared value into its column's typed bind, or `None` (skip the column).
fn coerce(value: &serde_norway::Value, kind: Kind) -> Option<Bound> {
    match kind {
        Kind::Bool => value.as_bool().map(Bound::Bool),
        Kind::Int => value.as_i64().and_then(|v| i32::try_from(v).ok()).map(Bound::Int),
        Kind::BigInt => value.as_i64().map(Bound::BigInt),
        Kind::Text => value.as_str().map(|s| Bound::Text(s.to_string())),
        Kind::TextArray => value.as_sequence().map(|seq| {
            Bound::TextArray(seq.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        }),
    }
}

/// Build `UPDATE <table> SET c1 = $1, c2 = $2 WHERE <pk>` for the given present columns.
/// Pure — unit-tested (TEST-10). Returns `None` when no whitelisted column is present.
///
/// `whitelist` is the table's compile-time column allow-list; a `debug_assert!` guards that
/// every emitted column came from it, so a future code path that lets a YAML-sourced column
/// name reach raw SQL trips in debug/test instead of interpolating an attacker string.
pub fn build_update_sql(table: &str, pk_where: &str, present: &[&str], whitelist: &[&str]) -> Option<String> {
    if present.is_empty() {
        return None;
    }
    debug_assert!(
        present.iter().all(|c| whitelist.contains(c)),
        "seed: build_update_sql refuses to emit a non-whitelisted column"
    );
    let sets: Vec<String> = present
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{c} = ${}", i + 1))
        .collect();
    Some(format!("UPDATE {table} SET {} WHERE {pk_where}", sets.join(", ")))
}

#[async_trait]
impl SingletonSettings for GenericSingleton {
    fn section(&self) -> &'static str {
        self.section
    }

    async fn apply_row(
        &self,
        declared: Option<&serde_norway::Value>,
        ctx: &SeedCtx,
    ) -> Result<bool, SeedError> {
        let Some(obj) = declared.and_then(|v| v.as_mapping()) else {
            return Ok(false);
        };

        let mut present: Vec<&str> = Vec::new();
        let mut binds: Vec<Bound> = Vec::new();
        for (col, kind) in self.columns {
            let Some(val) = obj.get(serde_norway::Value::String((*col).to_string())) else {
                continue;
            };
            match coerce(val, *kind) {
                Some(b) => {
                    present.push(col);
                    binds.push(b);
                }
                None => tracing::warn!(table = self.table, column = col, "seed: settings value has the wrong type; column skipped"),
            }
        }

        let whitelist: Vec<&str> = self.columns.iter().map(|(c, _)| *c).collect();
        let Some(sql) = build_update_sql(self.table, self.pk_where, &present, &whitelist) else {
            return Ok(false);
        };
        let mut q = sqlx::query::<Postgres>(&sql);
        for b in &binds {
            q = match b {
                Bound::Bool(v) => q.bind(*v),
                Bound::Int(v) => q.bind(*v),
                Bound::BigInt(v) => q.bind(*v),
                Bound::Text(v) => q.bind(v),
                Bound::TextArray(v) => q.bind(v),
            };
        }
        q.execute(&ctx.pool).await?;
        tracing::info!(table = self.table, columns = ?present, "seed: settings row reconciled");
        Ok(true)
    }

    async fn dump_row(&self, ctx: &SeedCtx) -> Result<Option<serde_norway::Value>, SeedError> {
        if self.columns.is_empty() {
            return Ok(None);
        }
        let cols: Vec<String> = self.columns.iter().map(|(c, _)| (*c).to_string()).collect();
        let sql = format!(
            "SELECT to_jsonb(t) AS row FROM {} t WHERE {}",
            self.table, self.pk_where
        );
        let row = sqlx::query(&sql).fetch_optional(&ctx.pool).await?;
        let Some(row) = row else { return Ok(None) };
        let full: serde_json::Value = row.try_get("row").map_err(|e| SeedError::Other(e.to_string()))?;
        let mut out = serde_json::Map::new();
        if let serde_json::Value::Object(map) = full {
            for c in &cols {
                if let Some(v) = map.get(c) {
                    out.insert(c.clone(), v.clone());
                }
            }
        }
        Ok(Some(serde_norway::to_value(serde_json::Value::Object(out))
            .map_err(|e| SeedError::Other(e.to_string()))?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TEST-10: the helper builds the correct declared-column UPDATE, and the singleton's
    // ledger key equals its section/table name.
    #[test]
    fn builds_declared_column_update_and_keys_by_table() {
        let g = GenericSingleton {
            section: "demo_settings",
            table: "demo_settings",
            pk_where: "id = TRUE",
            columns: &[("enabled", Kind::Bool), ("max_results", Kind::Int)],
        };
        // The ledger natural_key of a singleton is its section (= table) name.
        assert_eq!(g.section(), "demo_settings");

        // Only the present, whitelisted columns are in the UPDATE, positionally bound.
        let whitelist: Vec<&str> = g.columns.iter().map(|(c, _)| *c).collect();
        let sql = build_update_sql(g.table, g.pk_where, &["enabled", "max_results"], &whitelist).unwrap();
        assert_eq!(
            sql,
            "UPDATE demo_settings SET enabled = $1, max_results = $2 WHERE id = TRUE"
        );
        // No declared columns ⇒ no UPDATE (never an empty SET).
        assert!(build_update_sql(g.table, g.pk_where, &[], &whitelist).is_none());
    }
}
