//! Crate-scoped DB integration test for `ziee-auth`'s `AuthRepository`.
//!
//! Drives the real repository against a fresh throwaway DB migrated with the
//! crate's OWN `AUTH_MIGRATOR`, covering the transactional / dedup / lookup
//! logic the in-source `#[cfg(test)]` unit tests (which don't touch a DB)
//! can't reach: the local-user + default-group transaction, external-user
//! provisioning + auth-link lookup, the pending-account-link single-use
//! lifecycle, OAuth-session round-trip, case-insensitive FBL email lookup, and
//! the TTL cleanup sweep. The assembled-app login/OAuth HTTP flows stay in
//! ziee's `server/tests/auth`.

mod common;

use common::{DEFAULT_GROUP_NAME, SEEDED_PROVIDER_ID, drop_db, fresh_db};
use uuid::Uuid;
use ziee_auth::auth::AuthRepository;
use ziee_auth::auth::hash_password;
use ziee_auth::auth::providers::models::OAuthSession;

fn provider_id() -> Uuid {
    Uuid::parse_str(SEEDED_PROVIDER_ID).unwrap()
}

#[tokio::test]
async fn default_group_is_the_seeded_users_group() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool);

    let g = repo
        .get_default_group()
        .await
        .expect("query ok")
        .expect("the seed migration creates a default group");
    assert_eq!(g.name, DEFAULT_GROUP_NAME);
    assert!(g.is_default);
    assert!(g.is_system);

    drop_db(&db).await;
}

#[tokio::test]
async fn create_local_user_assigns_default_group_atomically() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool.clone());

    let hash = hash_password("s3cret-password").unwrap();
    let user = repo
        .create_local_user_with_default_group(
            "alice",
            "alice@corp.com",
            Some(hash),
            Some("Alice".to_string()),
        )
        .await
        .expect("create local user");

    assert_eq!(user.username, "alice");
    assert_eq!(user.email, "alice@corp.com");
    assert!(user.is_active);
    assert!(!user.is_admin);
    assert!(user.password_hash.is_some());

    // The default-group membership row was written in the SAME transaction.
    let default_group = repo.get_default_group().await.unwrap().unwrap();
    let member: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM user_groups WHERE user_id = $1 AND group_id = $2",
    )
    .bind(user.id)
    .bind(default_group.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(member.0, 1, "user must be assigned the default group");

    drop_db(&db).await;
}

#[tokio::test]
async fn duplicate_username_is_a_conflict_not_a_500() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool);

    repo.create_local_user_with_default_group("bob", "bob@corp.com", None, None)
        .await
        .expect("first insert ok");

    // Same username → unique violation surfaced as a Conflict AppError.
    let err = repo
        .create_local_user_with_default_group("bob", "different@corp.com", None, None)
        .await
        .expect_err("duplicate username must error");
    let msg = format!("{err:?}").to_lowercase();
    assert!(
        msg.contains("conflict") || msg.contains("already"),
        "expected a conflict, got: {msg}"
    );

    drop_db(&db).await;
}

#[tokio::test]
async fn provision_external_user_links_identity_and_is_findable() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool);
    let pid = provider_id();

    // No link exists yet.
    assert_eq!(
        repo.find_user_by_auth_link(pid, "ext-123").await.unwrap(),
        None
    );

    let uid = repo
        .provision_external_user_atomic(
            "carol",
            Some("carol@corp.com"),
            true,
            "Carol",
            pid,
            "ext-123",
            None,
        )
        .await
        .expect("provision external user");

    // The auth link now resolves to the freshly-provisioned user.
    assert_eq!(
        repo.find_user_by_auth_link(pid, "ext-123").await.unwrap(),
        Some(uid)
    );

    // touch_auth_link_and_get_user_id returns the same id (and bumps last_login).
    assert_eq!(
        repo.touch_auth_link_and_get_user_id(pid, "ext-123")
            .await
            .unwrap(),
        Some(uid)
    );
    // An unknown external id resolves to nothing.
    assert_eq!(
        repo.touch_auth_link_and_get_user_id(pid, "nope")
            .await
            .unwrap(),
        None
    );

    drop_db(&db).await;
}

/// Read `users.email_verified` for a user id.
async fn email_verified_of(pool: &sqlx::PgPool, uid: Uuid) -> bool {
    let row: (bool,) = sqlx::query_as("SELECT email_verified FROM users WHERE id = $1")
        .bind(uid)
        .fetch_one(pool)
        .await
        .unwrap();
    row.0
}

/// The OAuth/Gmail defect: provisioning used to omit `email_verified`
/// from its INSERT, so every social-login user landed on the column
/// default `false` even though the callback had already proven the
/// provider asserted the address verified.
#[tokio::test]
async fn provision_external_user_persists_email_verified() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool.clone());
    let pid = provider_id();

    let uid = repo
        .provision_external_user_atomic(
            "gina",
            Some("gina@gmail.com"),
            true,
            "Gina",
            pid,
            "ext-gina",
            None,
        )
        .await
        .expect("provision external user");

    assert!(
        email_verified_of(&pool, uid).await,
        "a provider-verified email must persist email_verified = true"
    );

    drop_db(&db).await;
}

/// The flag is THREADED from the caller, not hardcoded `true`: the same
/// call with `false` must leave the row unverified. Without this, the
/// fix would silently mark every future provisioning path verified.
#[tokio::test]
async fn provision_external_user_honors_unverified_email() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool.clone());
    let pid = provider_id();

    let uid = repo
        .provision_external_user_atomic(
            "hank",
            Some("hank@corp.com"),
            false,
            "Hank",
            pid,
            "ext-hank",
            None,
        )
        .await
        .expect("provision external user");

    assert!(
        !email_verified_of(&pool, uid).await,
        "an unverified email must NOT be recorded as verified"
    );

    drop_db(&db).await;
}

/// Regression guard for the password path: a local signup's email is
/// genuinely unverified, so it must stay `false`. This is the boundary
/// the OAuth fix must not cross.
#[tokio::test]
async fn local_signup_email_is_not_verified() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool.clone());

    let user = repo
        .create_local_user_with_default_group("ivy", "ivy@corp.com", None, None)
        .await
        .unwrap();

    assert!(!user.email_verified, "returned row must be unverified");
    assert!(
        !email_verified_of(&pool, user.id).await,
        "a password signup must not be email-verified"
    );

    drop_db(&db).await;
}

/// First-Broker-Login: binding a provider-verified identity to an
/// existing local account upgrades that account's `email_verified`,
/// atomically with the link row. Case-insensitive, because
/// `find_user_by_email_for_linking` matched case-insensitively.
#[tokio::test]
async fn linking_a_verified_identity_verifies_the_matching_email() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool.clone());
    let pid = provider_id();

    let user = repo
        .create_local_user_with_default_group("jane", "Jane@Corp.com", None, None)
        .await
        .unwrap();
    assert!(!user.email_verified, "starts unverified (local signup)");

    let flipped = repo
        .link_verified_external_identity(user.id, pid, "ext-jane", Some("jane@corp.com"), None)
        .await
        .expect("link verified identity");

    assert!(flipped, "the method reports the flip it made");
    assert!(
        email_verified_of(&pool, user.id).await,
        "a provider-verified matching email must verify the local account"
    );
    // The link row was written in the SAME transaction.
    assert_eq!(
        repo.find_user_by_auth_link(pid, "ext-jane").await.unwrap(),
        Some(user.id)
    );

    drop_db(&db).await;
}

/// The write-side guard: an external email that is NOT this user's
/// address (or is absent entirely) links the identity but must never
/// vouch for the row's email.
#[tokio::test]
async fn linking_does_not_verify_a_mismatched_email() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool.clone());
    let pid = provider_id();

    let mismatched = repo
        .create_local_user_with_default_group("kyle", "kyle@corp.com", None, None)
        .await
        .unwrap();
    let flipped = repo
        .link_verified_external_identity(
            mismatched.id,
            pid,
            "ext-kyle",
            Some("someone-else@corp.com"),
            None,
        )
        .await
        .expect("link identity");
    assert!(!flipped, "a mismatched address must not report a flip");
    assert!(
        !email_verified_of(&pool, mismatched.id).await,
        "a different address must not verify this user's email"
    );
    // The identity is still linked — only the verification is withheld.
    assert_eq!(
        repo.find_user_by_auth_link(pid, "ext-kyle").await.unwrap(),
        Some(mismatched.id)
    );

    let absent = repo
        .create_local_user_with_default_group("lena", "lena@corp.com", None, None)
        .await
        .unwrap();
    let flipped_absent = repo
        .link_verified_external_identity(absent.id, pid, "ext-lena", None, None)
        .await
        .expect("link identity without an external email");
    assert!(!flipped_absent, "no external email → nothing to verify");
    assert!(
        !email_verified_of(&pool, absent.id).await,
        "a missing external email must not verify the account"
    );

    drop_db(&db).await;
}

#[tokio::test]
async fn create_external_user_with_link_gets_default_group() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool.clone());
    let pid = provider_id();

    let uid = repo
        .create_external_user_with_link("dave", Some("dave@corp.com".into()), "Dave", pid, "ext-dave")
        .await
        .expect("create external user with link");

    let default_group = repo.get_default_group().await.unwrap().unwrap();
    let member: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM user_groups WHERE user_id = $1 AND group_id = $2",
    )
    .bind(uid)
    .bind(default_group.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(member.0, 1);
    assert_eq!(repo.find_user_by_auth_link(pid, "ext-dave").await.unwrap(), Some(uid));

    drop_db(&db).await;
}

#[tokio::test]
async fn email_lookup_for_linking_is_case_insensitive_and_active_only() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool.clone());

    let user = repo
        .create_local_user_with_default_group("erin", "Erin@Corp.com", None, None)
        .await
        .unwrap();

    // Canonical lowercase form matches the mixed-case stored email.
    assert_eq!(
        repo.find_user_by_email_for_linking("erin@corp.com")
            .await
            .unwrap(),
        Some(user.id)
    );

    // Deactivating the user hides them from the FBL lookup (security posture).
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.find_user_by_email_for_linking("erin@corp.com")
            .await
            .unwrap(),
        None,
        "a disabled user must not surface via FBL email lookup"
    );

    drop_db(&db).await;
}

#[tokio::test]
async fn pending_account_link_single_use_lifecycle() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool);
    let pid = provider_id();

    let target = repo
        .create_local_user_with_default_group("frank", "frank@corp.com", None, None)
        .await
        .unwrap();

    let token = repo
        .create_pending_link(pid, target.id, "ext-frank", Some("frank@ext.com"), None)
        .await
        .expect("create pending link");

    // Peek does NOT consume; the token still resolves.
    let peeked = repo.peek_pending_link(&token).await.unwrap().unwrap();
    assert_eq!(peeked.target_user_id, target.id);
    assert_eq!(peeked.external_id, "ext-frank");
    assert_eq!(peeked.attempts, 0);
    assert!(repo.peek_pending_link(&token).await.unwrap().is_some());

    // Bump increments the attempt counter monotonically.
    assert_eq!(repo.bump_pending_link_attempts(&token).await.unwrap(), Some(1));
    assert_eq!(repo.bump_pending_link_attempts(&token).await.unwrap(), Some(2));

    // Delete makes it single-use; subsequent peek/bump see nothing.
    repo.delete_pending_link(&token).await.unwrap();
    assert!(repo.peek_pending_link(&token).await.unwrap().is_none());
    assert_eq!(repo.bump_pending_link_attempts(&token).await.unwrap(), None);

    drop_db(&db).await;
}

#[tokio::test]
async fn oauth_session_round_trip_by_state() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool);
    let pid = provider_id();

    let session = OAuthSession {
        id: Uuid::new_v4(),
        state: "state-abc".to_string(),
        provider_id: pid,
        pkce_verifier: Some("verifier".to_string()),
        nonce: Some("nonce".to_string()),
        redirect_uri: "https://app/callback".to_string(),
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
        return_to: Some("/dashboard".to_string()),
    };
    repo.create_oauth_session(&session).await.expect("create oauth session");

    let got = repo
        .get_oauth_session_by_state("state-abc")
        .await
        .unwrap()
        .expect("session resolves by state while unexpired");
    assert_eq!(got.id, session.id);
    assert_eq!(got.pkce_verifier.as_deref(), Some("verifier"));
    assert_eq!(got.return_to.as_deref(), Some("/dashboard"));

    // Delete removes it.
    repo.delete_oauth_session("state-abc").await.unwrap();
    assert!(repo.get_oauth_session_by_state("state-abc").await.unwrap().is_none());

    drop_db(&db).await;
}

#[tokio::test]
async fn cleanup_prunes_only_expired_rows() {
    let (pool, db) = fresh_db().await;
    let repo = AuthRepository::new(pool.clone());
    let pid = provider_id();

    // A fresh (unexpired) oauth session — must survive cleanup.
    let live = OAuthSession {
        id: Uuid::new_v4(),
        state: "live-state".to_string(),
        provider_id: pid,
        pkce_verifier: None,
        nonce: None,
        redirect_uri: "https://app/cb".to_string(),
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        return_to: None,
    };
    repo.create_oauth_session(&live).await.unwrap();

    // An expired oauth session (write it directly with a past expiry).
    sqlx::query(
        "INSERT INTO oauth_sessions (id, state, provider_id, redirect_uri, expires_at)
         VALUES ($1, $2, $3, $4, NOW() - INTERVAL '1 hour')",
    )
    .bind(Uuid::new_v4())
    .bind("dead-state")
    .bind(pid)
    .bind("https://app/cb")
    .execute(&pool)
    .await
    .unwrap();

    let (sessions, _links, _tokens) = repo.cleanup_expired_auth_rows().await.unwrap();
    assert_eq!(sessions, 1, "exactly the one expired oauth session is pruned");

    // The live session is untouched.
    assert!(repo.get_oauth_session_by_state("live-state").await.unwrap().is_some());
    assert!(repo.get_oauth_session_by_state("dead-state").await.unwrap().is_none());

    drop_db(&db).await;
}
