//! Crate-scoped DB integration test for `ziee-onboarding`.
//!
//! Drives the real `OnboardingRepository` against a fresh throwaway DB migrated
//! with the crate's OWN migration. Covers the DB-level logic the in-source unit
//! tests can't reach: lazy row creation, idempotent completion, and the ATOMIC
//! cardinality-cap guard (the TOCTOU fix). Also proves standalone migration
//! apply (gap N-1 — `user_id` is a plain FK-free column). The HTTP guide/step
//! flow + auth gating stays in ziee's `tests/onboarding` (assembled-app behavior).

use sqlx::{Connection, Executor, PgConnection, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;
use ziee_onboarding::OnboardingRepository;

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:password@127.0.0.1:54321/postgres".to_string())
}

async fn fresh_db() -> (PgPool, String) {
    let admin = admin_url();
    // Strip any query string, then split the /db suffix.
    let base_full = admin.split('?').next().unwrap();
    let (base, _) = base_full.rsplit_once('/').expect("admin url has a /db suffix");
    let dbname = format!("ziee_onboard_test_{}", Uuid::new_v4().simple());

    let mut conn = PgConnection::connect(&admin).await.expect("connect admin db");
    conn.execute(format!("CREATE DATABASE \"{dbname}\"").as_str())
        .await
        .expect("create test db");
    conn.close().await.ok();

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&format!("{base}/{dbname}"))
        .await
        .expect("connect test db");

    pool.execute(include_str!(
        "../migrations/202607140195_onboarding_schema.sql"
    ))
    .await
    .expect("crate migration applies standalone on a bare DB");

    (pool, dbname)
}

async fn drop_db(dbname: &str) {
    if let Ok(mut conn) = PgConnection::connect(&admin_url()).await {
        let _ = conn
            .execute(format!("DROP DATABASE IF EXISTS \"{dbname}\" WITH (FORCE)").as_str())
            .await;
        conn.close().await.ok();
    }
}

#[tokio::test]
async fn unknown_user_has_empty_progress() {
    let (pool, db) = fresh_db().await;
    let repo = OnboardingRepository::new(pool);

    let p = repo
        .get_progress(Uuid::new_v4())
        .await
        .expect("no row → empty progress, not an error");
    assert!(p.completed_guide_ids.is_empty());
    assert!(p.completed_step_ids.is_empty());

    drop_db(&db).await;
}

#[tokio::test]
async fn complete_guide_creates_row_and_is_idempotent() {
    let (pool, db) = fresh_db().await;
    let repo = OnboardingRepository::new(pool);
    let user = Uuid::new_v4();

    // Lazy row creation on first completion.
    let p = repo.complete_guide(user, "getting-started", 10).await.unwrap();
    assert_eq!(p.completed_guide_ids, vec!["getting-started".to_string()]);

    // Repeat completion is a no-op (idempotent), not a duplicate.
    let p = repo.complete_guide(user, "getting-started", 10).await.unwrap();
    assert_eq!(
        p.completed_guide_ids,
        vec!["getting-started".to_string()],
        "completing the same guide twice must not append a duplicate"
    );

    // A distinct guide appends.
    let p = repo.complete_guide(user, "memory-setup", 10).await.unwrap();
    assert_eq!(p.completed_guide_ids.len(), 2);
    assert!(p.completed_guide_ids.contains(&"memory-setup".to_string()));

    drop_db(&db).await;
}

#[tokio::test]
async fn complete_guide_enforces_cardinality_cap() {
    let (pool, db) = fresh_db().await;
    let repo = OnboardingRepository::new(pool);
    let user = Uuid::new_v4();

    repo.complete_guide(user, "g1", 2).await.unwrap();
    repo.complete_guide(user, "g2", 2).await.unwrap();
    // Third distinct completion exceeds the cap → atomically rejected (no append).
    let p = repo.complete_guide(user, "g3", 2).await.unwrap();

    assert_eq!(
        p.completed_guide_ids.len(),
        2,
        "the cardinality(<$cap) guard must reject the over-cap append"
    );
    assert!(!p.completed_guide_ids.contains(&"g3".to_string()));

    drop_db(&db).await;
}

#[tokio::test]
async fn complete_step_tracks_composite_key() {
    let (pool, db) = fresh_db().await;
    let repo = OnboardingRepository::new(pool);
    let user = Uuid::new_v4();

    let p = repo
        .complete_guide_step(user, "getting-started/memory-setup", 50)
        .await
        .unwrap();
    assert_eq!(
        p.completed_step_ids,
        vec!["getting-started/memory-setup".to_string()]
    );
    // Guides + steps are independent columns.
    assert!(p.completed_guide_ids.is_empty());

    drop_db(&db).await;
}
