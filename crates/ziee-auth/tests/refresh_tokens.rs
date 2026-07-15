//! Crate-scoped DB integration test for `ziee-auth`'s refresh-token whitelist
//! + single-use rotation subsystem (`auth::refresh_tokens`).
//!
//! These functions are the security-critical primitives behind logout (F-02)
//! and refresh rotation (F-03) and are pure DB logic with NO in-source unit
//! tests. Covers: whitelist register/is_active/expiry, single-jti + per-user
//! revocation, the ATOMIC claim-rotation-and-register race guard (winner
//! writes the successor, loser gets `false`), the 30s rotation-grace successor
//! lookup and its hard-fail clauses, and the full `mint_session_tokens` path
//! (session_settings lifetimes → mint → whitelist → validate).

mod common;

use chrono::{Duration, Utc};
use common::{drop_db, fresh_db};
use sqlx::PgPool;
use uuid::Uuid;
use ziee_auth::auth::AuthRepository;
use ziee_auth::auth::JwtService;
use ziee_auth::auth::jwt::JwtSettings;
use ziee_auth::auth::refresh_tokens as rt;

fn jwt_service() -> JwtService {
    JwtService::try_new(JwtSettings {
        secret: "integration-test-jwt-secret-at-least-32-bytes!!".to_string(),
        issuer: "ziee".to_string(),
        audience: "ziee-api".to_string(),
        access_token_expiry_hours: 24,
        refresh_token_expiry_days: 30,
        access_token_expiry_seconds: None,
    })
    .unwrap()
}

/// Create a real user row (refresh_tokens.user_id has an FK → users).
async fn make_user(pool: &PgPool, username: &str) -> Uuid {
    AuthRepository::new(pool.clone())
        .create_local_user_with_default_group(username, &format!("{username}@corp.com"), None, None)
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn register_then_is_active_then_revoke() {
    let (pool, db) = fresh_db().await;
    let user = make_user(&pool, "ruth").await;
    let jti = Uuid::new_v4();

    // Not whitelisted yet.
    assert!(!rt::is_active(&pool, jti).await.unwrap());

    rt::register(&pool, jti, user, Utc::now() + Duration::days(30))
        .await
        .unwrap();
    assert!(rt::is_active(&pool, jti).await.unwrap());

    rt::revoke(&pool, jti).await.unwrap();
    assert!(
        !rt::is_active(&pool, jti).await.unwrap(),
        "a revoked token is no longer active"
    );

    drop_db(&db).await;
}

#[tokio::test]
async fn expired_token_is_not_active() {
    let (pool, db) = fresh_db().await;
    let user = make_user(&pool, "sam").await;
    let jti = Uuid::new_v4();

    rt::register(&pool, jti, user, Utc::now() - Duration::seconds(1))
        .await
        .unwrap();
    assert!(
        !rt::is_active(&pool, jti).await.unwrap(),
        "a past expires_at means the token is inactive despite being unrevoked"
    );

    drop_db(&db).await;
}

#[tokio::test]
async fn claim_rotation_is_single_use_and_registers_successor() {
    let (pool, db) = fresh_db().await;
    let user = make_user(&pool, "tom").await;

    let presented = Uuid::new_v4();
    rt::register(&pool, presented, user, Utc::now() + Duration::days(30))
        .await
        .unwrap();

    let successor = Uuid::new_v4();
    let succ_exp = Utc::now() + Duration::days(30);

    // First claim wins and writes the successor.
    let won = rt::claim_rotation_and_register(&pool, presented, successor, user, succ_exp)
        .await
        .unwrap();
    assert!(won, "the first claim of an active token wins");
    assert!(rt::is_active(&pool, successor).await.unwrap(), "successor is whitelisted");
    assert!(!rt::is_active(&pool, presented).await.unwrap(), "presented is now revoked");

    // A second claim of the same (now-revoked) token loses; no new successor.
    let successor2 = Uuid::new_v4();
    let won2 = rt::claim_rotation_and_register(&pool, presented, successor2, user, succ_exp)
        .await
        .unwrap();
    assert!(!won2, "re-claiming an already-rotated token must return false");
    assert!(
        !rt::is_active(&pool, successor2).await.unwrap(),
        "the loser's successor is never registered"
    );

    drop_db(&db).await;
}

#[tokio::test]
async fn rotation_grace_returns_active_successor_then_hard_fails() {
    let (pool, db) = fresh_db().await;
    let user = make_user(&pool, "uma").await;

    let presented = Uuid::new_v4();
    rt::register(&pool, presented, user, Utc::now() + Duration::days(30))
        .await
        .unwrap();
    let successor = Uuid::new_v4();
    let succ_exp = Utc::now() + Duration::days(30);
    rt::claim_rotation_and_register(&pool, presented, successor, user, succ_exp)
        .await
        .unwrap();

    // Within grace, the just-rotated token resolves to its live successor.
    let grace = rt::rotation_grace_successor(&pool, presented)
        .await
        .unwrap()
        .expect("a within-grace rotation resolves to the successor");
    assert_eq!(grace.0, successor);

    // Revoking the successor family (e.g. a logout) hard-fails the grace path.
    rt::revoke_all_for_user(&pool, user).await.unwrap();
    assert!(
        rt::rotation_grace_successor(&pool, presented).await.unwrap().is_none(),
        "logout revoking the successor must hard-fail even a just-rotated token"
    );

    drop_db(&db).await;
}

#[tokio::test]
async fn rotation_grace_none_for_plain_revoked_token() {
    let (pool, db) = fresh_db().await;
    let user = make_user(&pool, "vic").await;

    let jti = Uuid::new_v4();
    rt::register(&pool, jti, user, Utc::now() + Duration::days(30))
        .await
        .unwrap();
    // A logout-style revoke sets revoked_at but leaves rotated_to NULL.
    rt::revoke(&pool, jti).await.unwrap();

    assert!(
        rt::rotation_grace_successor(&pool, jti).await.unwrap().is_none(),
        "a revoked-but-never-rotated token has no grace successor"
    );

    drop_db(&db).await;
}

#[tokio::test]
async fn revoke_all_for_user_kills_every_active_token() {
    let (pool, db) = fresh_db().await;
    let user = make_user(&pool, "wes").await;
    let other = make_user(&pool, "xena").await;

    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let others = Uuid::new_v4();
    rt::register(&pool, a, user, Utc::now() + Duration::days(30)).await.unwrap();
    rt::register(&pool, b, user, Utc::now() + Duration::days(30)).await.unwrap();
    rt::register(&pool, others, other, Utc::now() + Duration::days(30)).await.unwrap();

    rt::revoke_all_for_user(&pool, user).await.unwrap();

    assert!(!rt::is_active(&pool, a).await.unwrap());
    assert!(!rt::is_active(&pool, b).await.unwrap());
    assert!(
        rt::is_active(&pool, others).await.unwrap(),
        "another user's token is untouched"
    );

    drop_db(&db).await;
}

#[tokio::test]
async fn session_expiries_reads_the_seeded_singleton() {
    let (pool, db) = fresh_db().await;
    let svc = jwt_service();

    // The seed migration writes 24h / 30d.
    let (access_hours, refresh_days) = rt::session_expiries(&pool, &svc).await;
    assert_eq!(access_hours, 24);
    assert_eq!(refresh_days, 30);

    drop_db(&db).await;
}

#[tokio::test]
async fn mint_session_tokens_whitelists_and_validates() {
    let (pool, db) = fresh_db().await;
    let svc = jwt_service();
    let user = make_user(&pool, "yan").await;

    let minted = rt::mint_session_tokens(&pool, &svc, user, "yan", "yan@corp.com", false)
        .await
        .expect("mint a full session token pair");

    // Access + refresh tokens both validate under the same service.
    let access = svc.validate_access_token(&minted.pair.access_token).unwrap();
    assert_eq!(access.sub, user.to_string());
    assert_eq!(access.username, "yan");
    let refresh = svc.validate_refresh_token(&minted.pair.refresh_token).unwrap();
    assert_eq!(refresh.jti.as_deref(), Some(minted.refresh_jti.to_string().as_str()));

    // The refresh jti was registered in the whitelist (fail-closed mint).
    assert!(
        rt::is_active(&pool, minted.refresh_jti).await.unwrap(),
        "mint must register the refresh token before returning it"
    );

    drop_db(&db).await;
}
