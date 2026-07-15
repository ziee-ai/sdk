//! Crate-scoped DB integration test for `ziee-auth`'s `SessionSettingsRepository`.
//!
//! The admin-configurable JWT-lifetime singleton is pure DB logic with no
//! in-source unit tests. Covers: reading the seeded singleton, partial PUT
//! (COALESCE keeps absent fields) with the `seeded_from_config` latch, and the
//! one-time boot seed (`seed_from_config_once`) — its run-once semantics AND
//! the CHECK-range clamping. The HTTP GET/PUT + permission gating + sync-emit
//! stay in ziee's `server/tests/auth/session_settings_test.rs`.

mod common;

use common::{drop_db, fresh_db};
use ziee_auth::auth::SessionSettingsRepository;

#[tokio::test]
async fn get_returns_the_seeded_defaults() {
    let (pool, db) = fresh_db().await;
    let repo = SessionSettingsRepository::new(pool);

    let s = repo.get().await.expect("seed migration wrote the singleton");
    assert_eq!(s.access_token_expiry_hours, 24);
    assert_eq!(s.refresh_token_expiry_days, 30);

    drop_db(&db).await;
}

#[tokio::test]
async fn partial_update_coalesces_and_latches_seeded_flag() {
    let (pool, db) = fresh_db().await;
    let repo = SessionSettingsRepository::new(pool.clone());

    // Update only the access hours; refresh days must be left untouched.
    let updated = repo.update(Some(48), None).await.expect("partial update");
    assert_eq!(updated.access_token_expiry_hours, 48);
    assert_eq!(updated.refresh_token_expiry_days, 30, "absent field is preserved");

    // An admin edit latches seeded_from_config = TRUE so a later boot seed no-ops.
    let seeded: (bool,) = sqlx::query_as("SELECT seeded_from_config FROM session_settings WHERE id = TRUE")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(seeded.0, "update() must latch seeded_from_config");

    // A subsequent one-time seed is now a no-op (the admin choice survives).
    repo.seed_from_config_once(1, 1).await.unwrap();
    let after = repo.get().await.unwrap();
    assert_eq!(after.access_token_expiry_hours, 48, "boot seed must not clobber an admin edit");
    assert_eq!(after.refresh_token_expiry_days, 30);

    drop_db(&db).await;
}

#[tokio::test]
async fn seed_from_config_once_writes_then_is_idempotent() {
    let (pool, db) = fresh_db().await;
    let repo = SessionSettingsRepository::new(pool);

    // Fresh DB → seeded_from_config is FALSE, so the first seed writes.
    repo.seed_from_config_once(100, 200).await.unwrap();
    let s = repo.get().await.unwrap();
    assert_eq!(s.access_token_expiry_hours, 100);
    assert_eq!(s.refresh_token_expiry_days, 200);

    // Second call is a no-op (seeded_from_config now TRUE).
    repo.seed_from_config_once(5, 5).await.unwrap();
    let s = repo.get().await.unwrap();
    assert_eq!(s.access_token_expiry_hours, 100, "seed runs exactly once");
    assert_eq!(s.refresh_token_expiry_days, 200);

    drop_db(&db).await;
}

#[tokio::test]
async fn seed_from_config_once_clamps_to_check_ranges() {
    let (pool, db) = fresh_db().await;
    let repo = SessionSettingsRepository::new(pool);

    // Out-of-range YAML values are clamped (not a boot failure): the DB CHECK
    // caps are 8760h / 3650d.
    repo.seed_from_config_once(999_999, 999_999).await.unwrap();
    let s = repo.get().await.unwrap();
    assert_eq!(s.access_token_expiry_hours, 8760);
    assert_eq!(s.refresh_token_expiry_days, 3650);

    drop_db(&db).await;
}
