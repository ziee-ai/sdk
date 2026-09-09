//! `users.email` is stored lowercased, and that is a DATABASE INVARIANT.
//!
//! ── the hole this file exists to close (#251) ───────────────────────────────
//!
//! `users_email_key` is `UNIQUE (email)` — byte-exact, so `bob@corp.com` and
//! `BOB@CORP.COM` were two DISTINCT principals. `POST /api/auth/register` is
//! open self-registration, so anyone holding a leaked invitation link for
//! `bob@corp.com` could register the case variant and satisfy the accept
//! path's `lower(trim(users.email)) == invitation.email` binding — a total
//! bypass of the recipient control, not a weakening of it.
//!
//! The fix is deliberately NOT a functional unique index: the whole point is
//! that no index NAME is reserved, so there is nothing for an error-attribution
//! mapper to mis-match on (#283) and nothing for an under-qualified
//! `DROP INDEX` to destroy on a foreign table (#284). Instead:
//!
//!   1. every write path lowercases in Rust (`auth::email::normalize_email`),
//!   2. `202609090010` makes that an invariant with `CHECK (email = lower(email))`,
//!   3. the pre-existing plain `UNIQUE (email)` does the rest — on normalised
//!      data it IS case-insensitive uniqueness.
//!
//! ── the Unicode seam (#260) ─────────────────────────────────────────────────
//!
//! Rust's `to_lowercase` and Postgres's `lower` disagree on 56 code points, so
//! a Rust-lowered value could fail a Postgres CHECK (→ a 500 on a shipped
//! path). Over printable ASCII the two folds agree EXACTLY, so the ASCII
//! restriction is load-bearing rather than incidental — and this file asserts
//! it at every write path.

mod common;

use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use common::{drop_db, fresh_db};
use sqlx::{PgPool, Row};
use ziee_auth::auth::http::handlers::register;
use ziee_auth::auth::jwt::{JwtService, JwtSettings};
use ziee_auth::auth::types::RegisterRequest;
use ziee_auth::auth::{AuthContext, NoopAuthEventSink, NoopAuthSyncSink};

fn jwt() -> Arc<JwtService> {
    Arc::new(
        JwtService::try_new(JwtSettings {
            secret: "0123456789abcdef0123456789abcdef-strong".to_string(),
            issuer: "test".to_string(),
            audience: "test".to_string(),
            access_token_expiry_hours: 24,
            refresh_token_expiry_days: 30,
            access_token_expiry_seconds: None,
        })
        .expect("test jwt secret is strong enough"),
    )
}

fn ctx(pool: &PgPool) -> AuthContext {
    AuthContext::new(
        Arc::new(pool.clone()),
        None,
        Arc::new(NoopAuthEventSink),
        Arc::new(NoopAuthSyncSink),
    )
}

const PASSWORD: &str = "S3cret-password!42";

/// Drive the real `POST /api/auth/register` handler and return the status the
/// client would see. `ApiResult`'s error arm carries the status the handler
/// chose, which is what `IntoResponse` turns into the wire status.
async fn post_register(pool: &PgPool, username: &str, email: &str) -> (StatusCode, Option<String>) {
    let req = RegisterRequest {
        username: username.to_string(),
        email: email.to_string(),
        password: PASSWORD.to_string(),
        display_name: None,
    };
    match register(Extension(jwt()), Extension(ctx(pool)), HeaderMap::new(), Json(req)).await {
        Ok((status, _resp)) => (status, None),
        Err((status, err)) => (status, Some(err.error_code().to_string())),
    }
}

/// Every stored address, sorted in RUST (byte order) — the database's own
/// `ORDER BY` is collation-dependent, and this file is about case, so the
/// ordering must not be.
async fn emails_in(pool: &PgPool) -> Vec<String> {
    let mut v: Vec<String> = sqlx::query("SELECT email FROM users")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.get::<String, _>("email"))
        .collect();
    v.sort();
    v
}

// ───────────────────────────────────────────────────────────────────────────
// PHASE A — reproduce the #251 defect on the shipped code. This asserts the
// BUG (201 Created for a case variant of a held address) so the "before" state
// is recorded as a passing test, not a claim. The next commit flips it to the
// 409 the fix must produce.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn registering_a_case_variant_of_a_held_address_is_a_conflict_not_a_second_principal() {
    let (pool, db) = fresh_db().await;

    let (first, _) = post_register(&pool, "bob", "bob@corp.com").await;
    assert_eq!(first, StatusCode::CREATED, "the legitimate signup must work");

    let (second, _code) = post_register(&pool, "mallory", "BOB@CORP.COM").await;
    assert_eq!(
        second,
        StatusCode::CREATED,
        "DEFECT #251: the shipped code accepts a case variant as a SECOND PRINCIPAL"
    );
    assert_eq!(
        emails_in(&pool).await,
        vec!["BOB@CORP.COM".to_string(), "bob@corp.com".to_string()],
        "DEFECT #251: two principals now hold the same mailbox, and either \
         satisfies an invitation binding issued to that address"
    );

    drop_db(&db).await;
}
