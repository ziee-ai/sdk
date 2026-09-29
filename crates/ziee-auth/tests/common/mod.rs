//! Shared throwaway-DB harness for `ziee-auth`'s crate-scoped integration tests.
//!
//! Each test provisions a fresh, uniquely-named database on the shared
//! `:54321` cluster, applies the crate's OWN migrator (`AUTH_MIGRATOR` — the
//! schema + fkeys + seed set, in version order), hands back a pool, and drops
//! the DB with `(FORCE)` on teardown. Mirrors the committed patterns in
//! `ziee-onboarding/tests/repository.rs` and
//! `ziee-notification/tests/schema_and_models.rs`, except it runs the real
//! `Migrator` so the seed rows (Administrators/Users groups, the 3 auth
//! providers, the `session_settings` singleton) are present — the auth
//! repositories depend on them.

// Each integration-test binary includes this module and uses only a subset of
// its helpers, so silence the per-binary dead-code pass.
#![allow(dead_code)]

use sqlx::{Connection, Executor, PgConnection, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

/// One of the seeded `auth_providers` rows (google) — a valid FK target for
/// `oauth_sessions` / `pending_account_links` / `user_auth_links`.
pub const SEEDED_PROVIDER_ID: &str = "92e74a99-ffa4-4c49-a05f-4a2b3e2b2efe";

/// The seeded default group (`Users`, `is_default = true`).
pub const DEFAULT_GROUP_NAME: &str = "Users";

pub fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:password@127.0.0.1:54321/postgres".to_string())
}

/// Provision a fresh throwaway DB, run the crate's `AUTH_MIGRATOR`, return
/// `(pool, dbname)`. The migrator applying at all IS the standalone-apply proof.
pub async fn fresh_db() -> (PgPool, String) {
    let admin = admin_url();
    let base_full = admin.split('?').next().unwrap();
    let (base, _) = base_full.rsplit_once('/').expect("admin url has a /db suffix");
    let dbname = format!("ziee_auth_test_{}", Uuid::new_v4().simple());

    let mut conn = PgConnection::connect(&admin).await.expect("connect admin db");
    conn.execute(format!("CREATE DATABASE \"{dbname}\"").as_str())
        .await
        .expect("create test db");
    conn.close().await.ok();

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&format!("{base}/{dbname}"))
        .await
        .expect("connect test db");

    ziee_auth::AUTH_MIGRATOR
        .run(&pool)
        .await
        .expect("AUTH_MIGRATOR applies (schema + fkeys + seed) on a bare DB");

    (pool, dbname)
}

pub async fn drop_db(dbname: &str) {
    if let Ok(mut conn) = PgConnection::connect(&admin_url()).await {
        let _ = conn
            .execute(format!("DROP DATABASE IF EXISTS \"{dbname}\" WITH (FORCE)").as_str())
            .await;
        conn.close().await.ok();
    }
}
