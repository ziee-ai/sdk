//! The `SeedProvider` seam — module-owned declarative seeding.
//!
//! Mirrors the `CHAT_EXTENSIONS` registry (the SDK `MODULE_ENTRIES` registry): each
//! data module self-registers a `SeedEntry` into the `SEED_PROVIDERS` linkme
//! distributed-slice at link time; the runner collects them sorted by `order` and
//! dispatches each section's desired-state to its provider. Zero central list.

use async_trait::async_trait;
use linkme::distributed_slice;
use serde::Deserialize;
use std::sync::Arc;

use crate::ledger::SeedLedger;
use crate::template::{env_lookup, resolve_secret_with, resolve_with, TemplateError};
use ziee_core::AppError;

/// Registration entry for distributed collection (mirrors `ModuleEntry`).
#[derive(Clone, Copy)]
pub struct SeedEntry {
    /// Top-level YAML section key this provider owns, e.g. `"providers"`.
    pub section: &'static str,
    /// Lower runs first — a referenced section (one another cross-references) seeds before a
    /// referencing one.
    pub order: i32,
    pub factory: fn() -> Arc<dyn SeedProvider>,
}

/// Distributed slice collecting all seed providers.
#[distributed_slice]
pub static SEED_PROVIDERS: [SeedEntry] = [..];

/// Reconcile mode for a section.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum SeedMode {
    /// Create/adopt when absent; leave existing (ledger-owned) rows untouched — never
    /// clobbers an admin edit and never resurrects an admin-deleted default.
    #[default]
    #[serde(rename = "seed-if-empty")]
    SeedIfEmpty,
    /// Authoritative: re-sync declared fields on owned rows AND delete ledger-owned
    /// rows absent from the declared list.
    #[serde(rename = "reconcile")]
    Reconcile,
}

/// One YAML section: the per-section directives plus the raw items (each provider
/// deserializes its own item shape). `deny_unknown_fields` is deliberately OFF here
/// because item shapes vary per provider.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SeedSection {
    /// Overrides the global reconcile flag for this section only.
    #[serde(default)]
    pub mode: Option<SeedMode>,
    /// `reset: true` ⇒ reconcile this section (make the seed-owned set exactly the
    /// declared list), regardless of the global mode.
    #[serde(default)]
    pub reset: Option<bool>,
    /// Natural keys of seed-owned rows to delete (only ever affects ledger-owned rows).
    #[serde(default)]
    pub remove: Vec<String>,
    /// The declared entities; each provider deserializes these into its own type.
    #[serde(default)]
    pub items: Vec<serde_norway::Value>,
}

impl SeedSection {
    /// Effective mode: `reset` or a per-section `mode` override wins over the global default.
    pub fn effective_mode(&self, global_reconcile: bool) -> SeedMode {
        if self.reset == Some(true) {
            return SeedMode::Reconcile;
        }
        match self.mode {
            Some(m) => m,
            None => {
                if global_reconcile {
                    SeedMode::Reconcile
                } else {
                    SeedMode::SeedIfEmpty
                }
            }
        }
    }
}

/// What a provider's `apply` did (aggregated + logged by the runner).
#[derive(Debug, Clone, Copy, Default)]
pub struct SeedOutcome {
    pub created: u32,
    pub adopted: u32,
    pub updated: u32,
    pub deleted: u32,
    pub skipped: u32,
}

impl SeedOutcome {
    pub fn merge(&mut self, other: SeedOutcome) {
        self.created += other.created;
        self.adopted += other.adopted;
        self.updated += other.updated;
        self.deleted += other.deleted;
        self.skipped += other.skipped;
    }

    pub fn is_noop(&self) -> bool {
        self.created == 0 && self.updated == 0 && self.deleted == 0 && self.adopted == 0
    }
}

/// A seed apply/dump failure. A provider error is soft (logged + skipped) at the
/// runner level; it never aborts boot.
#[derive(Debug)]
pub enum SeedError {
    /// A declared item did not match the provider's expected shape.
    Parse(String),
    /// A `${VAR}` was unset/empty or a secret was inlined (the entry is skipped).
    Template(TemplateError),
    /// A repository / DB error.
    Db(AppError),
    /// Anything else.
    Other(String),
}

impl std::fmt::Display for SeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SeedError::Parse(m) => write!(f, "seed item parse error: {m}"),
            SeedError::Template(e) => write!(f, "{e}"),
            SeedError::Db(e) => write!(f, "seed db error: {e:?}"),
            SeedError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for SeedError {}

impl From<AppError> for SeedError {
    fn from(e: AppError) -> Self {
        SeedError::Db(e)
    }
}
impl From<TemplateError> for SeedError {
    fn from(e: TemplateError) -> Self {
        SeedError::Template(e)
    }
}
impl From<sqlx::Error> for SeedError {
    fn from(e: sqlx::Error) -> Self {
        SeedError::Db(AppError::from(e))
    }
}

/// Context handed to each provider's `apply`/`dump`.
pub struct SeedCtx {
    pub pool: sqlx::PgPool,
    /// Section-agnostic seed-ownership ledger (idempotency + provenance + delete-safety).
    pub ledger: SeedLedger,
    /// Injectable env lookup: process env in production, a map in unit tests.
    lookup: Box<dyn Fn(&str) -> Option<String> + Send + Sync>,
}

impl SeedCtx {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            ledger: SeedLedger::new(pool.clone()),
            pool,
            lookup: Box::new(env_lookup),
        }
    }

    /// Test constructor with an injected env lookup.
    pub fn with_lookup(
        pool: sqlx::PgPool,
        lookup: Box<dyn Fn(&str) -> Option<String> + Send + Sync>,
    ) -> Self {
        Self {
            ledger: SeedLedger::new(pool.clone()),
            pool,
            lookup,
        }
    }

    /// Resolve `${VAR}` placeholders in a NON-secret field.
    pub fn resolve(&self, raw: &str) -> Result<String, TemplateError> {
        resolve_with(raw, &|n| (self.lookup)(n))
    }

    /// Resolve a SECRET field (must be exactly one `${VAR}` placeholder).
    pub fn resolve_secret(&self, raw: &str) -> Result<String, TemplateError> {
        resolve_secret_with(raw, &|n| (self.lookup)(n))
    }
}

/// A module-owned declarative seeder for one YAML section.
#[async_trait]
pub trait SeedProvider: Send + Sync {
    /// The top-level YAML section key this provider owns.
    fn section(&self) -> &'static str;

    /// Apply the (merged) desired-state for this section. `None` = the section is
    /// absent from every layer (still runs so a provider with an embedded default
    /// can act; most providers no-op on `None`).
    async fn apply(
        &self,
        section: Option<&SeedSection>,
        mode: SeedMode,
        ctx: &SeedCtx,
    ) -> Result<SeedOutcome, SeedError>;

    /// Serialize current seed-owned DB state back to a YAML section (for `--dump-seed`).
    /// Secrets are emitted as `${…}` placeholders, never values. `None` = nothing owned.
    async fn dump(&self, ctx: &SeedCtx) -> Result<Option<serde_norway::Value>, SeedError>;
}
