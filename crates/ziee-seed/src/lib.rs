//! Domain-neutral declarative config-as-code seeding engine for the ziee SDK.
//!
//! Every app on the SDK (ziee, CytoAnalyst, …) needs config-as-code deployment seeding:
//! come up fully configured from a declarative YAML with no manual UI setup, idempotent on
//! every boot and after every wipe. This crate is the ENGINE — the [`SeedProvider`] seam,
//! the layered YAML loader + deep-merge, the `${ENV_VAR}` secret templating (routed through
//! [`ziee_framework::secret`]), the idempotent ownership [`SeedLedger`] (+ its module-owned
//! `seed_ledger` migration), the reconcile/wipe directives, a deterministic-UUID primitive,
//! and the `--dump-seed` serializer.
//!
//! It names NO app concept (decision N9): apps register their OWN [`SeedProvider`]s (into
//! the [`SEED_PROVIDERS`] distributed-slice, mirroring `MODULE_ENTRIES`) and supply their
//! own default-seed YAML + data. An app boots the engine by calling [`run`] in the
//! post-migration window (schema exists, storage key set, before it serves).
//!
//! ```ignore
//! // In the app's boot, after migrations + init_storage_key:
//! ziee_seed::run(&pool, &seed_config, MY_EMBEDDED_DEFAULT_YAML).await?;
//! ```

pub mod config;
pub mod dump;
pub mod ledger;
pub mod provider;
pub mod runner;
pub mod settings;
pub mod template;
pub mod uuid;

pub use config::SeedConfig;
pub use dump::dump_all;
pub use ledger::{LedgerRow, SeedLedger};
pub use provider::{
    SeedCtx, SeedEntry, SeedError, SeedMode, SeedOutcome, SeedProvider, SeedSection, SEED_PROVIDERS,
};
pub use runner::{merge_docs, parse_doc, run, run_from_yaml, Doc, SEED_FILE_ENV, SEED_RECONCILE_ENV};
pub use settings::{build_update_sql, GenericSingleton, Kind, SingletonSeedProvider, SingletonSettings};
pub use template::{resolve_secret_with, resolve_with, TemplateError};
pub use uuid::{seed_uuid, SEED_NAMESPACE};
