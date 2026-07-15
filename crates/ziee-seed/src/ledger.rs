//! The seed ownership ledger — the single, table-agnostic record of which rows the
//! declarative seed engine owns. Keyed by `(section, natural_key)`; `entity_id` is
//! the owned row's UUID for multi-row families (NULL for singleton settings tables).
//! Its presence is the "already-seeded" latch that makes seed-if-empty idempotent and
//! keeps an admin-deleted default from being resurrected.

use sqlx::PgPool;
use uuid::Uuid;

use ziee_core::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRow {
    pub section: String,
    pub natural_key: String,
    pub entity_id: Option<Uuid>,
}

#[derive(Clone)]
pub struct SeedLedger {
    pool: PgPool,
}

impl SeedLedger {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Record (idempotently) that the seed owns `(section, natural_key)`.
    pub async fn record(
        &self,
        section: &str,
        natural_key: &str,
        entity_id: Option<Uuid>,
    ) -> Result<(), AppError> {
        sqlx::query!(
            r#"INSERT INTO seed_ledger (section, natural_key, entity_id)
               VALUES ($1, $2, $3)
               ON CONFLICT (section, natural_key)
               DO UPDATE SET entity_id = COALESCE(EXCLUDED.entity_id, seed_ledger.entity_id)"#,
            section,
            natural_key,
            entity_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Take ownership of a PRE-EXISTING row (the optional "adopt-in-place" takeover): an
    /// app that has deployed DBs whose rows were created by an earlier migration can adopt
    /// them into the ledger (matched by natural key) so the seed owns them going forward
    /// WITHOUT a data rewrite. A fresh-DB app that seeds its data from YAML does not need
    /// this — the create path records ownership itself. Semantically identical to
    /// [`record`]; named separately to document intent at the call site.
    pub async fn adopt(
        &self,
        section: &str,
        natural_key: &str,
        entity_id: Option<Uuid>,
    ) -> Result<(), AppError> {
        self.record(section, natural_key, entity_id).await
    }

    pub async fn lookup(
        &self,
        section: &str,
        natural_key: &str,
    ) -> Result<Option<LedgerRow>, AppError> {
        let row = sqlx::query!(
            r#"SELECT section, natural_key, entity_id
               FROM seed_ledger WHERE section = $1 AND natural_key = $2"#,
            section,
            natural_key,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| LedgerRow {
            section: r.section,
            natural_key: r.natural_key,
            entity_id: r.entity_id,
        }))
    }

    pub async fn is_seeded(&self, section: &str, natural_key: &str) -> Result<bool, AppError> {
        Ok(self.lookup(section, natural_key).await?.is_some())
    }

    /// All rows the seed owns in `section` (for reconcile-delete + dump).
    pub async fn list_owned(&self, section: &str) -> Result<Vec<LedgerRow>, AppError> {
        let rows = sqlx::query!(
            r#"SELECT section, natural_key, entity_id
               FROM seed_ledger WHERE section = $1 ORDER BY natural_key"#,
            section,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| LedgerRow {
                section: r.section,
                natural_key: r.natural_key,
                entity_id: r.entity_id,
            })
            .collect())
    }

    pub async fn remove(&self, section: &str, natural_key: &str) -> Result<(), AppError> {
        sqlx::query!(
            r#"DELETE FROM seed_ledger WHERE section = $1 AND natural_key = $2"#,
            section,
            natural_key,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
