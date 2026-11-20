//! Per-worktree build-DB provisioning for `build.rs` (G4).
//!
//! sqlx compile-time `query!` verification needs a migrated build database.
//! These helpers create + migrate a per-worktree-isolated database on the
//! shared build cluster so concurrent worktree builds don't clobber each
//! other's schema, and hand back the URL to feed sqlx's `DATABASE_URL`.
//!
//! Two entry points:
//!   - [`ensure_build_db`] — async, CREATE-if-missing only. Used by ziee's own
//!     `#[tokio::main]` `build.rs`, which owns the schema wipe + multi-source
//!     migrate itself (equivalence-preserving refactor of the former inline
//!     block).
//!   - [`provision_build_db`] — sync, ONE-CALL (derive isolated URL → create →
//!     wipe → migrate → return URL) for a plain app's `build.rs`. Spins its own
//!     current-thread runtime, so it must NOT be called from inside an existing
//!     tokio runtime.
//!
//! The wipe + migrate of BOTH paths is serialized with a per-DB session-level
//! pg_advisory_lock (see [`crate::schema_bootstrap`]) so two build scripts
//! pointed at the same build database never race DROP/CREATE.

use std::path::Path;

use sqlx::Connection;

use crate::schema_bootstrap;
use crate::worktree_db;

/// Ensure `db_name` exists on the cluster of `base_url`: connect to the
/// maintenance `postgres` database, `CREATE DATABASE` if missing (idempotent +
/// race tolerant — a concurrent creator just makes our CREATE a swallowed
/// duplicate-database error), and return the URL pointing at `db_name`. On a
/// cluster-unreachable error, fall back to `base_url` unchanged so the caller's
/// connect surfaces the real error. Async — the caller must already be on a
/// tokio runtime.
pub async fn ensure_build_db(base_url: &str, db_name: &str) -> String {
    let admin_url = worktree_db::with_database(base_url, "postgres");
    match sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
    {
        Ok(admin) => {
            let exists: Option<(i32,)> =
                sqlx::query_as("SELECT 1 FROM pg_database WHERE datname = $1")
                    .bind(db_name)
                    .fetch_optional(&admin)
                    .await
                    .ok()
                    .flatten();
            if exists.is_none() {
                // CREATE DATABASE can't run in a tx; ignore the
                // duplicate_database error if another worktree's build raced us.
                let _ = sqlx::query(&format!("CREATE DATABASE {db_name}"))
                    .execute(&admin)
                    .await;
            }
            admin.close().await;
            println!(
                "build.rs: per-worktree build DB → {db_name} \
                 (set ZIEE_BUILD_DB_PERWORKTREE=0 to disable)"
            );
            worktree_db::with_database(base_url, db_name)
        }
        Err(e) => {
            eprintln!("build.rs: per-worktree DB provisioning skipped: {e}");
            base_url.to_string()
        }
    }
}

/// ONE-CALL build-DB provisioning for a plain app's `build.rs` — the ergonomic
/// entry point CytoAnalyst (G4) needed so an app module can use compile-time
/// `query!` verification without copying ziee's ~60 lines of glue.
///
/// Derives a per-worktree-isolated database name (`<CARGO_PKG_NAME>_build_<key>`)
/// on the shared local build cluster when `DATABASE_URL` is unset / the
/// committed sentinel, honors a genuine external `DATABASE_URL` override
/// unchanged, creates the database, wipes + re-applies `migrations_dir`, and
/// returns the URL to hand to sqlx. Emits `cargo:rustc-env=DATABASE_URL=<url>`.
///
/// Reads `CARGO_MANIFEST_DIR`, `CARGO_PKG_NAME`, and `DATABASE_URL` from the
/// build environment. Creates its own current-thread runtime, so it must be
/// called from a SYNC `fn main()` (NOT a `#[tokio::main]` one).
pub fn provision_build_db(migrations_dir: &Path) -> String {
    let pkg = std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "app".to_string());
    // Sanitize the package name to a valid unquoted postgres identifier.
    let pkg_ident: String = pkg
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    provision_build_db_named(&format!("{pkg_ident}_build_"), migrations_dir)
}

/// [`provision_build_db`] with an explicit database-name prefix (the per-worktree
/// key is appended). Use when the default `<CARGO_PKG_NAME>_build_` prefix isn't
/// what you want.
pub fn provision_build_db_named(db_prefix: &str, migrations_dir: &Path) -> String {
    println!("cargo:rerun-if-env-changed=DATABASE_URL");
    println!("cargo:rerun-if-env-changed=ZIEE_BUILD_DB_PERWORKTREE");
    println!("cargo:rerun-if-changed={}", migrations_dir.display());

    let explicit = std::env::var("DATABASE_URL").ok();
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build.rs: failed to build current-thread runtime");

    let database_url = rt.block_on(async move {
        let database_url = if worktree_db::should_auto_isolate(&explicit) {
            let base = explicit
                .clone()
                .unwrap_or_else(|| worktree_db::DEFAULT_BUILD_DB_URL.to_string());
            let db_name = format!("{db_prefix}{}", worktree_db::worktree_key(&manifest_dir));
            ensure_build_db(&base, &db_name).await
        } else {
            explicit.unwrap_or_else(|| worktree_db::DEFAULT_BUILD_DB_URL.to_string())
        };

        // Deliberately a single RAW connection — NOT a pool: the session-level
        // advisory lock in the bootstrap below must live on the very session
        // that performs every schema write, and a pooled connection could be
        // silently discarded/replaced after a connection-level error, which
        // would DROP the session lock and reopen the build race.
        let mut conn = sqlx::postgres::PgConnection::connect(&database_url)
            .await
            .unwrap_or_else(|e| panic!("build.rs: failed to connect to build DB: {e}"));

        // Wipe + reapply the migration set under the per-DB session advisory
        // lock (idempotent, isolated schema); the DROP carries a bounded
        // SQLSTATE 53200 retry for transient lock-table exhaustion.
        let lock_key =
            schema_bootstrap::schema_lock_key(schema_bootstrap::database_name_of(&database_url));
        if let Err(e) =
            schema_bootstrap::bootstrap_schema(&mut conn, lock_key, migrations_dir).await
        {
            panic!("build.rs: schema bootstrap failed: {e}");
        }

        // Best-effort close: the advisory lock is already released inside the
        // bootstrap above; on a close failure the process exit ends the
        // session anyway.
        if let Err(e) = conn.close().await {
            eprintln!("Warning: failed to close build-DB connection: {e}");
        }
        database_url
    });

    println!("cargo:rustc-env=DATABASE_URL={database_url}");
    database_url
}
