//! Crate-scoped DB integration test for `ziee-notification`.
//!
//! Applies the crate's OWN migration to a fresh throwaway database and
//! round-trips the `Notification` `FromRow` model against the resulting schema.
//! Covers two things the in-source unit tests can't, and the ziee HTTP inbox
//! tests only reach indirectly:
//!   1. STANDALONE MIGRATION APPLY (gap N-1) — the migration must apply on a bare
//!      DB with none of chat/scheduler/workflow/auth's domain tables present
//!      (it keeps `*_id` columns as plain nullable UUIDs, no FKs). A negative
//!      control asserts the FK-free posture directly.
//!   2. MIGRATION ↔ MODEL mapping — every `Notification` field selects back from
//!      the migrated schema, so a column rename / type drift fails the suite.
//!
//! The full inbox flow (scheduler → notification, auth gating, owner-scope)
//! stays in ziee's `tests/notification` — that's assembled-app behavior.

use sqlx::{Connection, Executor, PgConnection, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;
use ziee_notification::models::{NewNotification, Notification};

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:password@127.0.0.1:54321/postgres".to_string())
}

/// Provision a fresh throwaway DB, apply the crate's migration, return
/// (pool, dbname). The migration applying at all IS the standalone-apply proof.
async fn fresh_db() -> (PgPool, String) {
    let admin = admin_url();
    let (base, _) = admin.rsplit_once('/').expect("admin url has a /db suffix");
    let dbname = format!("ziee_notif_test_{}", Uuid::new_v4().simple());

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

    // Apply the crate's own migration — no domain tables exist here, so a clean
    // apply proves the N-1 domain-agnostic posture.
    pool.execute(include_str!(
        "../migrations/202607140190_notification_schema.sql"
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
async fn migration_applies_standalone_and_model_round_trips() {
    let (pool, dbname) = fresh_db().await;

    let new = NewNotification::new(Uuid::new_v4(), "task_done", "Task finished")
        .body("your scheduled task completed")
        .conversation(Uuid::new_v4());

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO notifications (user_id, kind, title, body, interrupt, conversation_id)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(new.user_id)
    .bind(&new.kind)
    .bind(&new.title)
    .bind(&new.body)
    .bind(new.interrupt)
    .bind(new.conversation_id)
    .fetch_one(&pool)
    .await
    .expect("insert notification");

    // Round-trip every field through the FromRow model.
    let row: Notification = sqlx::query_as("SELECT * FROM notifications WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("select notification back as the FromRow model");

    assert_eq!(row.id, id);
    assert_eq!(row.user_id, new.user_id);
    assert_eq!(row.kind, "task_done");
    assert_eq!(row.title, "Task finished");
    assert_eq!(row.body, "your scheduled task completed");
    assert!(row.interrupt);
    assert_eq!(row.conversation_id, new.conversation_id);
    assert_eq!(row.scheduled_task_id, None);
    assert_eq!(row.workflow_run_id, None);
    assert!(
        row.is_unread(),
        "a freshly-inserted notification has NULL read_at → unread"
    );
    assert!(row.created_at.timestamp() > 0, "created_at defaulted to now()");

    drop_db(&dbname).await;
}

#[tokio::test]
async fn silent_builder_yields_a_durable_only_row() {
    let (pool, dbname) = fresh_db().await;

    let new = NewNotification::new(Uuid::new_v4(), "digest", "Daily digest").silent();
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO notifications (user_id, kind, title, interrupt)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(new.user_id)
    .bind(&new.kind)
    .bind(&new.title)
    .bind(new.interrupt)
    .fetch_one(&pool)
    .await
    .expect("insert silent notification");

    let row: Notification = sqlx::query_as("SELECT * FROM notifications WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        !row.interrupt,
        "`.silent()` → interrupt=false: a durable inbox row with no live toast"
    );

    drop_db(&dbname).await;
}

/// Negative control for N-1: the migrated schema must carry NO foreign-key
/// constraints (the domain FKs live in a ziee-side migration), otherwise it
/// couldn't apply on a bare DB.
#[tokio::test]
async fn schema_has_no_foreign_keys() {
    let (pool, dbname) = fresh_db().await;

    let fk_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.table_constraints
         WHERE table_name = 'notifications' AND constraint_type = 'FOREIGN KEY'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        fk_count, 0,
        "the crate migration must be domain-agnostic (no FKs) so it applies standalone"
    );

    drop_db(&dbname).await;
}
