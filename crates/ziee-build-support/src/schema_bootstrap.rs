//! Session-level advisory-lock serialization of the build-DB schema bootstrap.
//!
//! The SDK twin of the ziee app's build-db race fix
//! (`fix/build-rs-schema-advisory-lock`, app commit `894001f864`): a build
//! script wipes its build DB with `DROP SCHEMA public CASCADE` and re-applies
//! migrations on EVERY build, so two build scripts pointed at the SAME build
//! database race each other — the losing build's `CREATE SCHEMA` dies with
//! `schema "public" already exists` (SQLSTATE 42P06), and under lock-table
//! pressure the DROP itself can die with `out of shared memory` (SQLSTATE
//! 53200), leaving the pair in a half-wiped state.
//!
//! The fix is a SESSION-level, blocking `pg_advisory_lock` taken BEFORE the
//! first schema write and held through `DROP` + `CREATE` + the migrations, on
//! ONE raw connection (not a pool: a pooled connection can be silently
//! discarded/replaced after a connection-level error, which would DROP the
//! session lock and reopen the race). The second concurrent build blocks at
//! the lock until the first finishes, then rebuilds a consistent schema.
//!
//! Postgres advisory-lock keys are scoped PER DATABASE (the key space lives in
//! the database, not the cluster), so the lock key here is derived from the
//! DATABASE NAME being built: builds of different per-worktree build DBs never
//! contend, while every build script pointed at the SAME database — whatever
//! crate or worktree it comes from — derives the SAME key and serializes.

use std::path::Path;
use std::time::Duration;

use sqlx::postgres::PgConnection;

/// Bounded retry budget for the schema DROP on SQLSTATE 53200 ("out of shared
/// memory" — Postgres lock-table exhaustion).
///
/// On the shared build cluster (127.0.0.1:54321, max_locks_per_transaction=64)
/// a `DROP SCHEMA public CASCADE` of a fully-migrated schema needs thousands
/// of lock slots, so concurrent DROP/migrate activity from OTHER databases on
/// the cluster can transiently saturate the lock table and fail the DROP
/// (`ERROR: out of shared memory ... HINT: You might need to increase
/// "max_locks_per_transaction"`, immediately followed by
/// `ERROR: schema "public" already exists` on the CREATE). The advisory lock
/// guarantees no OTHER build script on THIS database is interfering, so a
/// bounded retry after the transient clears is safe. A persistent failure
/// still returns the error — real errors are never swallowed.
pub const SCHEMA_DROP_RETRIES: u32 = 8;
/// Base backoff for the retry above (doubles per attempt: 250ms, 500ms, ...).
pub const SCHEMA_DROP_RETRY_BASE_MS: u64 = 250;

/// FNV-1a 64-bit session advisory-lock key for one build database.
///
/// Hashes `sdk.build-schema-lock:<db_name>` — the DB name is the discriminator,
/// so two build scripts pointed at the same database (same derived per-worktree
/// name, or the same explicit `DATABASE_URL` override) derive the SAME key and
/// serialize, while different per-crate build DBs get different keys and never
/// contend. Any fixed i64 would do; the value only has to agree between the
/// build scripts of one database.
pub fn schema_lock_key(db_name: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("sdk.build-schema-lock:{db_name}").bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash as i64
}

/// The database component of a postgres URL (the path after the last `/`,
/// with any query/fragment stripped) — the discriminator [`schema_lock_key`]
/// hashes.
pub fn database_name_of(url: &str) -> &str {
    let name = url.rsplit('/').next().unwrap_or("").trim_end_matches('/');
    name.split(['?', '#']).next().unwrap_or(name)
}

/// Take the blocking, session-level advisory lock. Called BEFORE the first
/// schema write; the lock lives until [`unlock_schema`] or the session ends.
pub async fn lock_schema(
    conn: &mut PgConnection,
    lock_key: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(lock_key)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Release the session-level advisory lock (best-effort in practice: the
/// caller closes the connection right after, and process exit ends the
/// session, so the lock can never leak past a build script).
pub async fn unlock_schema(
    conn: &mut PgConnection,
    lock_key: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(lock_key)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Retry a transient SQLSTATE 53200 ("out of shared memory") up to
/// [`SCHEMA_DROP_RETRIES`] times with exponential backoff; any other error or
/// an exhausted budget returns the error to the caller.
pub async fn drop_schema_with_retry(
    conn: &mut PgConnection,
) -> Result<(), sqlx::Error> {
    let mut attempt = 0;
    loop {
        match sqlx::query("DROP SCHEMA IF EXISTS public CASCADE")
            .execute(&mut *conn)
            .await
        {
            Ok(_) => return Ok(()),
            Err(e) => {
                let transient = e.as_database_error().is_some_and(|d| {
                    d.code().is_some_and(|c| c.as_ref() == "53200")
                });
                if !transient || attempt >= SCHEMA_DROP_RETRIES {
                    return Err(e);
                }
                let wait = SCHEMA_DROP_RETRY_BASE_MS << attempt;
                tokio::time::sleep(Duration::from_millis(wait)).await;
                attempt += 1;
            }
        }
    }
}

/// Full schema bootstrap under the per-DB session advisory lock:
/// `lock → DROP (53200-retried) → CREATE → GRANT → migrate → unlock`, all on
/// the caller's raw connection. Returns errors verbatim so the build script
/// can panic with its own crate-specific message.
///
/// The caller MUST hold the connection exclusively for the whole call —
/// that is what makes the session lock effective (see the module docs).
pub async fn bootstrap_schema(
    conn: &mut PgConnection,
    lock_key: i64,
    migrations_dir: &Path,
) -> Result<(), sqlx::Error> {
    lock_schema(conn, lock_key).await?;
    drop_schema_with_retry(conn).await?;

    sqlx::query("CREATE SCHEMA public")
        .execute(&mut *conn)
        .await?;
    // Best-effort (matches the historical behavior): the build DB is a local
    // scratch database whose owner already holds the schema rights.
    sqlx::query("GRANT ALL ON SCHEMA public TO PUBLIC")
        .execute(&mut *conn)
        .await
        .ok();

    let migrator = sqlx::migrate::Migrator::new(migrations_dir)
        .await
        .map_err(|e| sqlx::Error::Migrate(Box::new(e)))?;
    migrator.run(&mut *conn).await?;

    unlock_schema(conn, lock_key).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_key_is_stable_and_distinct_per_db() {
        for name in [
            "ziee_file_build_00000000",
            "ziee_seed_build_00000000",
            "ziee_auth_build_00000000",
        ] {
            assert_eq!(schema_lock_key(name), schema_lock_key(name));
        }
        // Distinct per-crate build DBs → distinct keys (no cross-DB lock-key
        // collision, and the same DB always has the same key).
        let keys: std::collections::HashSet<i64> = [
            "ziee_file_build_00000000",
            "ziee_seed_build_00000000",
            "ziee_auth_build_00000000",
            "ziee_notification_build_00000000",
            "ziee_onboarding_build_00000000",
        ]
        .into_iter()
        .map(schema_lock_key)
        .collect();
        assert_eq!(keys.len(), 5);
    }

    #[test]
    fn database_name_of_strips_path_and_query() {
        assert_eq!(
            database_name_of("postgresql://postgres:password@127.0.0.1:54321/ziee_file_build_ab12cd34"),
            "ziee_file_build_ab12cd34"
        );
        assert_eq!(
            database_name_of("postgresql://u:p@db:5432/real?sslmode=disable"),
            "real"
        );
        assert_eq!(database_name_of("postgresql://u:p@db:5432/"), "");
        // The lock key is a function of the parsed name — same URL ⇒ same key.
        assert_eq!(
            schema_lock_key(database_name_of(
                "postgresql://u:p@db:5432/real?sslmode=disable"
            )),
            schema_lock_key("real")
        );
    }
}
