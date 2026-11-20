//! `ziee-auth` build script — provisions the **auth-only** build DB for sqlx
//! compile-time `query!` verification.
//!
//! ziee-auth is the ONE schema-bound SDK crate (framework/core/identity are
//! build-DB-free). Its `query!` macros only touch the auth tables, so it needs
//! an auth-only database — a sibling DB `ziee_auth_build_<key>` on the same
//! `:54321` cluster the app's `build.rs` uses. The `<key>` is derived (FNV-1a)
//! from this crate's manifest dir so concurrent worktree builds don't clobber
//! each other's schema, mirroring `server/build_helper/worktree_db.rs`.
//!
//! Migrations applied here are EXACTLY ziee-auth's own `migrations/` dir (the
//! structural auth-table migrations, byte-identical to the app's originals).
//! The app composes these ∪ its remaining migrations into a merged set for its
//! own (full-schema) build DB; here we verify against the auth subset alone.
//!
//! A genuine external `DATABASE_URL` override (a host:port other than the
//! `:54321` sentinel cluster, as CI/production sets) is honored unchanged.
//!
//! The DROP/CREATE/migrate critical section is serialized with a session-level
//! `pg_advisory_lock` keyed on the build DATABASE NAME (shared
//! `ziee-build-support::schema_bootstrap`), so two concurrent build scripts
//! pointed at the same build DB wait for each other instead of racing
//! `DROP SCHEMA public CASCADE` / `CREATE SCHEMA public`; the DROP itself is
//! retried on transient lock-table exhaustion (SQLSTATE 53200).

use std::path::PathBuf;

use sqlx::Connection;
use ziee_build_support::schema_bootstrap;

/// The committed build cluster (matches docker-compose.yaml + the app's
/// `worktree_db::DEFAULT_BUILD_DB_URL`).
const DEFAULT_CLUSTER_URL: &str = "postgresql://postgres:password@127.0.0.1:54321/postgres";

/// FNV-1a 64-bit → first 8 hex chars. Stable across processes; identical
/// derivation to `server/build_helper/worktree_db.rs::worktree_key`.
fn fnv1a_key(s: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", (hash & 0xffff_ffff) as u32)
}

/// Replace the path (db name) of a postgres URL.
fn with_database(url: &str, db_name: &str) -> String {
    match url.rfind('/') {
        Some(idx) => format!("{}/{}", &url[..idx], db_name),
        None => url.to_string(),
    }
}

/// True when we should auto-provision our own auth-only DB: no `DATABASE_URL`,
/// or one pointing at the committed `:54321` sentinel cluster. A real external
/// override (different host:port) is honored unchanged.
fn should_auto_isolate(explicit: &Option<String>) -> bool {
    match explicit {
        None => true,
        Some(u) => u.contains("127.0.0.1:54321") || u.contains("localhost:54321"),
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 1)]
async fn main() {
    println!("cargo:rerun-if-env-changed=DATABASE_URL");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let migrations_path = PathBuf::from(&manifest_dir).join("migrations");
    println!("cargo:rerun-if-changed={}", migrations_path.display());

    let explicit = std::env::var("DATABASE_URL").ok();

    let database_url = if should_auto_isolate(&explicit) {
        let base = explicit
            .clone()
            .unwrap_or_else(|| DEFAULT_CLUSTER_URL.to_string());
        let db_name = format!("ziee_auth_build_{}", fnv1a_key(&manifest_dir));
        let admin_url = with_database(&base, "postgres");
        match sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
        {
            Ok(admin) => {
                let exists: Option<(i32,)> =
                    sqlx::query_as("SELECT 1 FROM pg_database WHERE datname = $1")
                        .bind(&db_name)
                        .fetch_optional(&admin)
                        .await
                        .ok()
                        .flatten();
                if exists.is_none() {
                    // CREATE DATABASE can't run in a tx; swallow the
                    // duplicate_database error if another build raced us.
                    let _ = sqlx::query(&format!("CREATE DATABASE {db_name}"))
                        .execute(&admin)
                        .await;
                }
                admin.close().await;
                println!("cargo:warning=ziee-auth: auth-only build DB → {db_name}");
                with_database(&base, &db_name)
            }
            Err(e) => {
                eprintln!("ziee-auth build.rs: auth-only DB provisioning skipped: {e}");
                base
            }
        }
    } else {
        explicit.unwrap()
    };

    // Deliberately a single RAW connection — NOT a pool: the session-level
    // advisory lock below must live on the very session that performs every
    // schema write, and a pooled connection could be silently
    // discarded/replaced after a connection-level error, which would DROP the
    // session lock and reopen the build race.
    let mut conn = match sqlx::postgres::PgConnection::connect(&database_url).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("\nziee-auth build.rs: failed to connect to build DB: {e}");
            panic!("ziee-auth build DB connection failed");
        }
    };

    // Wipe + reapply the auth migration subset under the per-DB session
    // advisory lock (idempotent, isolated schema). The lock key derives from
    // the build DATABASE NAME, so concurrent build scripts pointed at the same
    // database serialize here — the second waits, then rebuilds a consistent
    // schema — instead of racing DROP/CREATE. The DROP carries a bounded
    // SQLSTATE 53200 retry for transient lock-table exhaustion.
    let lock_key =
        schema_bootstrap::schema_lock_key(schema_bootstrap::database_name_of(&database_url));
    schema_bootstrap::bootstrap_schema(&mut conn, lock_key, &migrations_path)
        .await
        .unwrap_or_else(|e| {
            eprintln!("\nziee-auth build.rs: schema bootstrap failed: {e}");
            panic!("ziee-auth schema bootstrap failed");
        });

    // Best-effort close: the advisory lock is already released inside the
    // bootstrap above; on a close failure the process exit ends the session
    // anyway (the lock can never leak past this build script).
    if let Err(e) = conn.close().await {
        eprintln!("Warning: failed to close build-DB connection: {e}");
    }

    println!("cargo:rustc-env=DATABASE_URL={database_url}");
}
