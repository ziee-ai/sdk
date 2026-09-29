//! Crate-scoped DB integration tests for the session record (`auth_sessions`)
//! and the session-scoped claims it anchors (queue fr3-417sess).
//!
//! What only a real database + the real refresh handler can prove:
//!   * the session ROW is the epoch source for a `sid` token (memo
//!     identity-and-access-339 §1 point 5), per session, without touching
//!     `users.token_version` — TEST-5;
//!   * logout (`end_session_atomically`) ends every session of the user in the
//!     same transaction as the epoch bump + refresh revoke — TEST-15;
//!   * `/auth/refresh` re-sources the epoch from the session row, refuses an
//!     ended session, and adopts a session for a legacy `sid`-less family —
//!     TEST-16;
//!   * the per-sign-in values stay FIXED across refreshes and the app's claim
//!     source is consulted exactly once per sign-in (RFC 9068 §2.2.1) — TEST-4.

#![cfg(feature = "routes")]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use common::{drop_db, fresh_db};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt; // `oneshot`
use uuid::Uuid;

use ziee_auth::auth::http::jwt_extractor::assert_session_epoch_current;
use ziee_auth::auth::jwt::JwtSettings;
use ziee_auth::auth::refresh_tokens as rt;
use ziee_auth::auth::{
    AccessTokenClaimValues, AuthContext, AuthMountOptions, AuthRepository, DefaultIdentityResolver,
    JwtService, MintContext, NoopAuthEventSink, NoopAuthSyncSink, TokenClaimsSource, mount_auth,
    sessions,
};

fn settings() -> JwtSettings {
    JwtSettings {
        secret: "integration-test-jwt-secret-at-least-32-bytes!!".to_string(),
        issuer: "ziee".to_string(),
        audience: "ziee-api".to_string(),
        access_token_expiry_hours: 24,
        refresh_token_expiry_days: 30,
        access_token_expiry_seconds: None,
    }
}

async fn make_user(pool: &PgPool, username: &str) -> Uuid {
    AuthRepository::new(pool.clone())
        .create_local_user_with_default_group(username, &format!("{username}@corp.com"), None, None)
        .await
        .unwrap()
        .id
}

async fn users_token_version(pool: &PgPool, user: Uuid) -> i32 {
    sqlx::query_scalar("SELECT token_version FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The full auth surface over a real pool — the same `mount_auth` an app uses.
fn app(pool: &PgPool, jwt: Arc<JwtService>) -> axum::Router {
    let ctx = AuthContext::new(
        Arc::new(pool.clone()),
        None,
        Arc::new(NoopAuthEventSink),
        Arc::new(NoopAuthSyncSink),
    );
    let resolver = Arc::new(DefaultIdentityResolver::new(pool.clone(), jwt.clone()));
    let router = mount_auth(
        aide::axum::ApiRouter::new(),
        resolver,
        jwt,
        ctx,
        AuthMountOptions {
            trust_forwarded_headers: false,
            session_expiry_seed: None,
        },
    );
    let mut api = aide::openapi::OpenApi::default();
    router.finish_api(&mut api)
}

/// POST /auth/refresh with a body token → (status, json body).
async fn refresh(app: &axum::Router, refresh_token: &str) -> (StatusCode, serde_json::Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/refresh")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "refresh_token": refresh_token }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// GET /auth/me (a `JwtAuth` route) → status.
async fn me(app: &axum::Router, access_token: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                .header("authorization", format!("Bearer {access_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Returns `auth_time = T0` on its FIRST call and `T1` on every later one, and
/// counts calls — so a refresh that re-consulted the source is visible twice.
struct DriftingSource {
    calls: AtomicUsize,
}

const T0: i64 = 1_700_000_000;
const T1: i64 = 1_800_000_000;

#[async_trait::async_trait]
impl TokenClaimsSource for DriftingSource {
    async fn claims_for(&self, ctx: &MintContext) -> AccessTokenClaimValues {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        AccessTokenClaimValues {
            aud: None,
            client_id: Some(ctx.user_id.to_string()),
            amr: Some(vec!["pwd".to_string()]),
            auth_time: Some(if n == 0 { T0 } else { T1 }),
            cnf: None,
        }
    }
}

/// TEST-4 (queue fr3-417sess, INV-5 — RFC 9068 §2.2.1 fixedness): the
/// authentication-event values of a refreshed access token are the ORIGINAL
/// sign-in's, copied from the refresh token — the app's source is consulted
/// once per sign-in and never on refresh.
#[tokio::test]
async fn refresh_reissue_keeps_the_original_amr_auth_time() {
    let (pool, db) = fresh_db().await;
    let src = Arc::new(DriftingSource {
        calls: AtomicUsize::new(0),
    });
    let jwt = Arc::new(
        JwtService::try_new(settings())
            .unwrap()
            .with_token_claims_source(src.clone()),
    );
    let user = make_user(&pool, "fixed").await;

    let minted = rt::mint_session_tokens(&pool, &jwt, user, "fixed", "fixed@corp.com", false)
        .await
        .unwrap();
    assert_eq!(src.calls.load(Ordering::SeqCst), 1);
    let first = jwt
        .validate_access_token(&minted.pair.access_token)
        .unwrap();
    assert_eq!(first.auth_time, Some(T0));

    let app = app(&pool, jwt.clone());
    let (status, body) = refresh(&app, &minted.pair.refresh_token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let re = jwt
        .validate_access_token(body["access_token"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        re.auth_time,
        Some(T0),
        "auth_time must stay the sign-in time"
    );
    assert_eq!(re.amr, Some(vec!["pwd".to_string()]));
    assert_eq!(re.client_id, first.client_id);
    assert_eq!(re.sid, first.sid);
    assert_ne!(re.jti, first.jti);
    assert_eq!(
        src.calls.load(Ordering::SeqCst),
        1,
        "the claim source must NOT be re-consulted on refresh"
    );

    // A second hop (the rotated refresh token) still carries T0.
    let (status, body2) = refresh(&app, body["refresh_token"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{body2}");
    let re2 = jwt
        .validate_access_token(body2["access_token"].as_str().unwrap())
        .unwrap();
    assert_eq!(re2.auth_time, Some(T0));

    drop_db(&db).await;
}

/// TEST-5 (queue fr3-417sess, INV-7 + INV-8 — memo 339 §1 point 5): for a
/// `sid` token the SESSION ROW is the epoch source. Bumping one session's
/// `ver` kills that session's tokens (401 SESSION_REVOKED) while
/// `users.token_version` is untouched and the user's other session lives;
/// ending a session kills its tokens the same way.
#[tokio::test]
async fn session_row_is_the_epoch_source_for_sid_tokens() {
    let (pool, db) = fresh_db().await;
    let jwt = Arc::new(JwtService::try_new(settings()).unwrap());
    let user = make_user(&pool, "epoch").await;

    let a = rt::mint_session_tokens(&pool, &jwt, user, "epoch", "e@corp.com", false)
        .await
        .unwrap();
    let b = rt::mint_session_tokens(&pool, &jwt, user, "epoch", "e@corp.com", false)
        .await
        .unwrap();
    let ca = jwt.validate_access_token(&a.pair.access_token).unwrap();
    let cb = jwt.validate_access_token(&b.pair.access_token).unwrap();
    let (sa, sb) = (ca.sid.unwrap(), cb.sid.unwrap());
    assert_ne!(sa, sb, "two sign-ins are two sessions");
    assert_eq!(a.session_id, Some(sa));

    assert!(
        assert_session_epoch_current(&pool, sa, ca.ver)
            .await
            .is_ok()
    );
    assert!(
        assert_session_epoch_current(&pool, sb, cb.ver)
            .await
            .is_ok()
    );

    let before = users_token_version(&pool, user).await;
    assert_eq!(
        sessions::bump_session_version(&pool, sa).await.unwrap(),
        Some(ca.ver.unwrap() + 1)
    );
    assert_eq!(
        users_token_version(&pool, user).await,
        before,
        "users.token_version untouched"
    );

    let (status, err) = assert_session_epoch_current(&pool, sa, ca.ver)
        .await
        .expect_err("a bumped session's token must die");
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(format!("{err:?}").contains("SESSION_REVOKED"), "{err:?}");
    assert!(
        assert_session_epoch_current(&pool, sb, cb.ver)
            .await
            .is_ok(),
        "the user's OTHER session is not collateral"
    );

    // The same, through a real `JwtAuth` route.
    let app = app(&pool, jwt.clone());
    assert_eq!(
        me(&app, &a.pair.access_token).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(me(&app, &b.pair.access_token).await, StatusCode::OK);

    assert!(sessions::end_session(&pool, sb).await.unwrap());
    let (status, err) = assert_session_epoch_current(&pool, sb, cb.ver)
        .await
        .expect_err("an ended session's token must die");
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(format!("{err:?}").contains("SESSION_REVOKED"), "{err:?}");
    assert_eq!(
        me(&app, &b.pair.access_token).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        !rt::is_active(&pool, b.refresh_jti).await.unwrap(),
        "ending a session revokes its refresh family too"
    );
    assert!(
        sessions::get_session_version(&pool, Uuid::new_v4())
            .await
            .unwrap()
            .is_none()
    );

    drop_db(&db).await;
}

/// TEST-15 (queue fr3-417sess, memo 339 point 4c): logout — the user-level
/// kill — ends EVERY live session of the user in the same transaction that
/// bumps the master epoch and revokes the refresh tokens; another user's
/// session is untouched.
#[tokio::test]
async fn logout_ends_every_session_of_the_user_in_one_transaction() {
    let (pool, db) = fresh_db().await;
    let jwt = JwtService::try_new(settings()).unwrap();
    let user = make_user(&pool, "leaver").await;
    let other = make_user(&pool, "stayer").await;

    let a = rt::mint_session_tokens(&pool, &jwt, user, "leaver", "l@corp.com", false)
        .await
        .unwrap();
    let b = rt::mint_session_tokens(&pool, &jwt, user, "leaver", "l@corp.com", false)
        .await
        .unwrap();
    let o = rt::mint_session_tokens(&pool, &jwt, other, "stayer", "s@corp.com", false)
        .await
        .unwrap();

    let before = users_token_version(&pool, user).await;
    let after = rt::end_session_atomically(&pool, user).await.unwrap();
    assert_eq!(after, before + 1);

    for s in [a.session_id.unwrap(), b.session_id.unwrap()] {
        assert!(
            sessions::get_session_version(&pool, s)
                .await
                .unwrap()
                .is_none()
        );
        let ended: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT ended_at FROM auth_sessions WHERE id = $1")
                .bind(s)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(ended.is_some(), "logout must END the session row");
    }
    assert!(!rt::is_active(&pool, a.refresh_jti).await.unwrap());
    assert!(!rt::is_active(&pool, b.refresh_jti).await.unwrap());
    assert!(
        sessions::get_session_version(&pool, o.session_id.unwrap())
            .await
            .unwrap()
            .is_some()
    );
    assert!(rt::is_active(&pool, o.refresh_jti).await.unwrap());

    drop_db(&db).await;
}

/// TEST-16 (queue fr3-417sess, ITEM-12f): the refresh handler reads the epoch
/// from the SESSION row, refuses an ended session, and moves a legacy
/// `sid`-less family onto a freshly adopted session.
#[tokio::test]
async fn refresh_uses_the_session_row_and_refuses_an_ended_session() {
    let (pool, db) = fresh_db().await;
    let jwt = Arc::new(JwtService::try_new(settings()).unwrap());
    let app = app(&pool, jwt.clone());
    let user = make_user(&pool, "rotor").await;

    // (a) the refreshed access token carries the SESSION's epoch.
    let m = rt::mint_session_tokens(&pool, &jwt, user, "rotor", "r@corp.com", false)
        .await
        .unwrap();
    let sid = m.session_id.unwrap();
    let bumped = sessions::bump_session_version(&pool, sid)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE users SET token_version = token_version + 5 WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert_ne!(users_token_version(&pool, user).await, bumped);
    let (status, body) = refresh(&app, &m.pair.refresh_token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let access = body["access_token"].as_str().unwrap().to_string();
    let c = jwt.validate_access_token(&access).unwrap();
    assert_eq!(
        c.ver,
        Some(bumped),
        "refresh must stamp the session row's ver"
    );
    assert_eq!(c.sid, Some(sid));
    assert_eq!(me(&app, &access).await, StatusCode::OK);

    // (b) an ended session refuses its (unexpired) refresh token.
    let next_refresh = body["refresh_token"].as_str().unwrap().to_string();
    // End the session ROW ONLY — its refresh token stays whitelisted, so the
    // refusal below can come only from the handler's session-row check (not
    // from the whitelist, which `sessions::end_session` would also clear).
    sqlx::query("UPDATE auth_sessions SET ended_at = now() WHERE id = $1")
        .bind(sid)
        .execute(&pool)
        .await
        .unwrap();
    let next_jti = Uuid::parse_str(
        jwt.validate_refresh_token(&next_refresh)
            .unwrap()
            .jti
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert!(
        rt::is_active(&pool, next_jti).await.unwrap(),
        "precondition: the refresh token is still whitelisted"
    );
    let (status, body) = refresh(&app, &next_refresh).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(body.to_string().contains("REFRESH_TOKEN_REVOKED"), "{body}");

    // (c) a legacy refresh token (whitelisted row, no session, no `sid`)
    // adopts a fresh session on refresh.
    let legacy_jti = Uuid::new_v4();
    let exp = Utc::now() + Duration::days(30);
    rt::register(&pool, legacy_jti, user, exp).await.unwrap();
    let legacy = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &serde_json::json!({
            "sub": user.to_string(),
            "exp": exp.timestamp(),
            "iat": Utc::now().timestamp(),
            "iss": "ziee",
            "aud": "ziee-api-refresh",
            "username": "", "email": "", "is_admin": false,
            "jti": legacy_jti.to_string(),
        }),
        &jsonwebtoken::EncodingKey::from_secret(settings().secret.as_bytes()),
    )
    .unwrap();
    let (status, body) = refresh(&app, &legacy).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let c = jwt
        .validate_access_token(body["access_token"].as_str().unwrap())
        .unwrap();
    let adopted = c.sid.expect("a legacy family adopts a session");
    assert_eq!(
        sessions::get_session_version(&pool, adopted).await.unwrap(),
        Some(users_token_version(&pool, user).await),
        "the adopted session's epoch is the user's master epoch"
    );
    assert_eq!(c.ver, Some(users_token_version(&pool, user).await));
    let succ = jwt
        .validate_refresh_token(body["refresh_token"].as_str().unwrap())
        .unwrap();
    let succ_jti = Uuid::parse_str(succ.jti.as_deref().unwrap()).unwrap();
    assert_eq!(
        sessions::session_of_refresh_token(&pool, succ_jti)
            .await
            .unwrap(),
        Some(adopted),
        "the successor refresh row carries the adopted session"
    );

    drop_db(&db).await;
}

/// TEST-10 (queue fr3-417sess, INV-4 — one session id per rotation family):
/// the successor row inherits the presented row's session through the
/// UNCHANGED `claim_rotation_and_register`, and through the real handler both
/// the rotated pair and a grace re-issue (the same refresh token presented
/// again within the grace window) carry the family's `sid`.
#[tokio::test]
async fn rotation_preserves_the_session_id_across_the_family() {
    let (pool, db) = fresh_db().await;
    let jwt = Arc::new(JwtService::try_new(settings()).unwrap());
    let user = make_user(&pool, "fam").await;

    // DB level: the unchanged signature inherits.
    let mut conn = pool.acquire().await.unwrap();
    let sid = sessions::create_session(&mut conn, user, 0).await.unwrap();
    drop(conn);
    let presented = Uuid::new_v4();
    let successor = Uuid::new_v4();
    rt::register_with_session(&pool, presented, user, sid, Utc::now() + Duration::days(30))
        .await
        .unwrap();
    assert!(
        rt::claim_rotation_and_register(
            &pool,
            presented,
            successor,
            user,
            Utc::now() + Duration::days(30)
        )
        .await
        .unwrap()
    );
    assert_eq!(
        sessions::session_of_refresh_token(&pool, successor)
            .await
            .unwrap(),
        Some(sid)
    );

    // Handler level: rotation, then a grace re-issue of the same token.
    let app = app(&pool, jwt.clone());
    let m = rt::mint_session_tokens(&pool, &jwt, user, "fam", "f@corp.com", false)
        .await
        .unwrap();
    let family = m.session_id.unwrap();
    let (status, rotated) = refresh(&app, &m.pair.refresh_token).await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    let (status, grace) = refresh(&app, &m.pair.refresh_token).await;
    assert_eq!(status, StatusCode::OK, "within grace: {grace}");
    for body in [&rotated, &grace] {
        let a = jwt
            .validate_access_token(body["access_token"].as_str().unwrap())
            .unwrap();
        assert_eq!(a.sid, Some(family));
        let r = jwt
            .validate_refresh_token(body["refresh_token"].as_str().unwrap())
            .unwrap();
        assert_eq!(r.sid, Some(family));
        let jti = Uuid::parse_str(r.jti.as_deref().unwrap()).unwrap();
        assert_eq!(
            sessions::session_of_refresh_token(&pool, jti)
                .await
                .unwrap(),
            Some(family)
        );
    }
    let jti_of = |body: &serde_json::Value| {
        jwt.validate_refresh_token(body["refresh_token"].as_str().unwrap())
            .unwrap()
            .jti
    };
    assert_eq!(
        jti_of(&rotated),
        jti_of(&grace),
        "the grace re-issue converges on the successor family"
    );

    drop_db(&db).await;
}

/// TEST-17 (queue fr3-417sess, F1(a)): a sign-in racing a logout is strictly
/// ordered by the `users` row lock `mint_session_tokens` takes inside its
/// transaction — it waits for the in-flight logout and starts the session at
/// the POST-logout epoch, so the session row and the master never diverge.
#[tokio::test]
async fn sign_in_racing_logout_starts_at_the_post_logout_epoch() {
    let (pool, db) = fresh_db().await;
    let jwt = Arc::new(JwtService::try_new(settings()).unwrap());
    let user = make_user(&pool, "racer").await;
    let before = users_token_version(&pool, user).await;

    // A logout, mid-flight: epoch bumped, not yet committed.
    let mut logout = pool.begin().await.unwrap();
    sqlx::query("UPDATE users SET token_version = token_version + 1 WHERE id = $1")
        .bind(user)
        .execute(&mut *logout)
        .await
        .unwrap();

    let (p2, j2) = (pool.clone(), jwt.clone());
    let mint = tokio::spawn(async move {
        rt::mint_session_tokens(&p2, &j2, user, "racer", "r@corp.com", false)
            .await
            .unwrap()
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !mint.is_finished(),
        "the sign-in must wait for the in-flight logout"
    );
    sqlx::query(
        "UPDATE auth_sessions SET ended_at = now() WHERE user_id = $1 AND ended_at IS NULL",
    )
    .bind(user)
    .execute(&mut *logout)
    .await
    .unwrap();
    logout.commit().await.unwrap();

    let minted = mint.await.unwrap();
    let sid = minted.session_id.unwrap();
    assert_eq!(
        sessions::get_session_version(&pool, sid).await.unwrap(),
        Some(before + 1),
        "the new session is live at the post-logout epoch"
    );
    assert_eq!(users_token_version(&pool, user).await, before + 1);

    drop_db(&db).await;
}

/// TEST-18 (queue fr3-417sess, F9): a legacy (sid-less) refresh token that is
/// no longer on the whitelist is refused WITHOUT adopting a session — a replay
/// cannot mint session rows.
#[tokio::test]
async fn revoked_legacy_refresh_token_adopts_no_session() {
    let (pool, db) = fresh_db().await;
    let jwt = Arc::new(JwtService::try_new(settings()).unwrap());
    let app = app(&pool, jwt.clone());
    let user = make_user(&pool, "replay").await;

    let jti = Uuid::new_v4();
    let exp = Utc::now() + Duration::days(30);
    rt::register(&pool, jti, user, exp).await.unwrap();
    rt::revoke(&pool, jti).await.unwrap();
    let legacy = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &serde_json::json!({
            "sub": user.to_string(),
            "exp": exp.timestamp(),
            "iat": Utc::now().timestamp(),
            "iss": "ziee",
            "aud": "ziee-api-refresh",
            "username": "", "email": "", "is_admin": false,
            "jti": jti.to_string(),
        }),
        &jsonwebtoken::EncodingKey::from_secret(settings().secret.as_bytes()),
    )
    .unwrap();

    for _ in 0..3 {
        let (status, body) = refresh(&app, &legacy).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert!(body.to_string().contains("REFRESH_TOKEN_REVOKED"), "{body}");
    }
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_sessions WHERE user_id = $1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        rows, 0,
        "a replayed revoked legacy token must not create sessions"
    );

    drop_db(&db).await;
}
