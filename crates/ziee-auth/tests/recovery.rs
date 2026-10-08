//! DB integration tests for the account-recovery repository and the nullable
//! email column: code-set replacement, the single-use compare-and-set, the
//! atomic attempt counter (against the pure `next_counter` reference), and
//! email-less users coexisting under the UNIQUE constraint.

mod common;

use common::{drop_db, fresh_db};
use ziee_auth::auth::recovery::{Counter, RecoveryRepository, next_counter};
use ziee_auth::user::UserRepository;

async fn user(pool: &sqlx::PgPool, name: &str, email: &str) -> uuid::Uuid {
    UserRepository::new(pool.clone())
        .create(name, email, Some("x".into()), None, None)
        .await
        .expect("create user")
        .id
}

#[tokio::test]
async fn many_users_without_an_email_coexist_but_a_real_email_stays_unique() {
    let (pool, db) = fresh_db().await;
    let a = user(&pool, "a", "").await;
    let b = user(&pool, "b", "").await;
    assert_ne!(a, b, "two email-less users must not collide on the UNIQUE email");
    let stored: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(a)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, None, "no email is NULL, never an empty string");
    let got = UserRepository::new(pool.clone()).get_by_id(a).await.unwrap().unwrap();
    assert_eq!(got.email, "", "the wire type maps NULL to an empty string");

    user(&pool, "c", "c@example.com").await;
    let dup = UserRepository::new(pool.clone())
        .create("d", "c@example.com", Some("x".into()), None, None)
        .await;
    assert!(dup.is_err(), "a real email is still unique");
    drop_db(&db).await;
}

#[tokio::test]
async fn replacing_codes_deletes_the_previous_set_and_status_counts_unused() {
    let (pool, db) = fresh_db().await;
    let u = user(&pool, "u", "").await;
    let repo = RecoveryRepository::new(pool.clone());
    repo.replace_codes(u, &(0..10).map(|i| format!("h{i}")).collect::<Vec<_>>()).await.unwrap();
    assert_eq!(repo.codes_status(u).await.unwrap().0, 10);
    let first = repo.unused_codes(u).await.unwrap();
    assert!(repo.complete_reset(u, Some(first[0].id), "newhash").await.unwrap());
    assert_eq!(repo.codes_status(u).await.unwrap().0, 9);

    repo.replace_codes(u, &(0..10).map(|i| format!("n{i}")).collect::<Vec<_>>()).await.unwrap();
    let after = repo.unused_codes(u).await.unwrap();
    assert_eq!(after.len(), 10);
    assert!(after.iter().all(|c| c.code_hash.starts_with('n')), "old set is gone");
    drop_db(&db).await;
}

#[tokio::test]
async fn a_code_can_be_consumed_exactly_once_even_concurrently() {
    let (pool, db) = fresh_db().await;
    let u = user(&pool, "u", "").await;
    let repo = RecoveryRepository::new(pool.clone());
    repo.replace_codes(u, &["h".to_string()]).await.unwrap();
    let id = repo.unused_codes(u).await.unwrap()[0].id;
    let (a, b) = tokio::join!(
        repo.complete_reset(u, Some(id), "p1"),
        repo.complete_reset(u, Some(id), "p2"),
    );
    let wins = [a.unwrap(), b.unwrap()].iter().filter(|w| **w).count();
    assert_eq!(wins, 1, "two concurrent resets with one code: exactly one wins");
    let hash: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(u)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(matches!(hash.as_deref(), Some("p1") | Some("p2")), "the loser wrote nothing");
    drop_db(&db).await;
}

#[tokio::test]
async fn questions_are_replaced_atomically_and_cascade_with_the_user() {
    let (pool, db) = fresh_db().await;
    let u = user(&pool, "u", "").await;
    let repo = RecoveryRepository::new(pool.clone());
    repo.replace_questions(u, &[("first_pet".into(), "h1".into()), ("first_job".into(), "h2".into())])
        .await
        .unwrap();
    assert_eq!(repo.questions(u).await.unwrap().len(), 2);
    repo.replace_questions(u, &[("first_car".into(), "h3".into()), ("first_job".into(), "h4".into()), ("first_pet".into(), "h5".into())])
        .await
        .unwrap();
    let q = repo.questions(u).await.unwrap();
    assert_eq!(q.iter().map(|r| r.question_key.as_str()).collect::<Vec<_>>(), ["first_car", "first_job", "first_pet"]);
    sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.unwrap();
    assert!(repo.questions(u).await.unwrap().is_empty(), "questions go with the user");
    drop_db(&db).await;
}

#[tokio::test]
async fn the_sql_counter_matches_the_pure_reference() {
    let (pool, db) = fresh_db().await;
    let repo = RecoveryRepository::new(pool.clone());
    let mut model: Option<Counter> = None;
    for n in 1..=8 {
        let now = chrono::Utc::now();
        let (next, model_allowed) = next_counter(model, now, 15, 4);
        model = Some(next);
        let allowed = repo.begin_attempt("name", "ghost", 15, 4).await.unwrap();
        assert_eq!(allowed, model_allowed, "SQL and pure reference agree at attempt {n}");
        assert_eq!(allowed, n <= 4, "attempts 1..=4 proceed, the rest do not");
    }
    assert!(
        repo.begin_attempt("name", "someone-else-is-fresh-ok", 15, 4).await.unwrap(),
        "other keys are unaffected"
    );
    assert!(repo.begin_attempt("name", "another", 15, 4).await.unwrap(), "a fresh key proceeds");
    repo.clear_attempts("name", "ghost").await.unwrap();
    assert!(repo.begin_attempt("name", "ghost", 15, 4).await.unwrap(), "cleared: proceeds again");
    drop_db(&db).await;
}

/// The race the audit found: with the count taken BEFORE verification, a burst of
/// concurrent attempts is admitted exactly `max` times, no matter how many arrive.
#[tokio::test]
async fn a_concurrent_burst_is_admitted_exactly_max_times() {
    let (pool, db) = fresh_db().await;
    let repo = RecoveryRepository::new(pool.clone());
    let mut tasks = Vec::new();
    for _ in 0..40 {
        let r = repo.clone();
        tasks.push(tokio::spawn(async move { r.begin_attempt("name", "victim", 15, 5).await.unwrap() }));
    }
    let mut admitted = 0;
    for t in tasks {
        if t.await.unwrap() {
            admitted += 1;
        }
    }
    assert_eq!(admitted, 5, "40 simultaneous attempts, 5 admitted");
    drop_db(&db).await;
}

#[tokio::test]
async fn an_expired_window_restarts_the_count() {
    let (pool, db) = fresh_db().await;
    let repo = RecoveryRepository::new(pool.clone());
    for _ in 0..2 {
        assert!(repo.begin_attempt("ip", "9.9.9.9", 15, 3).await.unwrap());
    }
    sqlx::query("UPDATE auth_recovery_attempts SET window_started_at = NOW() - INTERVAL '20 minutes'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo.begin_attempt("ip", "9.9.9.9", 15, 3).await.unwrap(), "window expired: count restarts at 1");
    let f: i32 = sqlx::query_scalar("SELECT failures FROM auth_recovery_attempts WHERE key = '9.9.9.9'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(f, 1);
    drop_db(&db).await;
}

#[tokio::test]
async fn a_reset_ends_every_session_in_the_same_transaction() {
    let (pool, db) = fresh_db().await;
    let u = user(&pool, "u", "").await;
    sqlx::query("INSERT INTO refresh_tokens (jti, user_id, expires_at) VALUES (gen_random_uuid(), $1, NOW() + INTERVAL '1 day')")
        .bind(u)
        .execute(&pool)
        .await
        .unwrap();
    let repo = RecoveryRepository::new(pool.clone());
    assert!(repo.complete_reset(u, None, "newhash").await.unwrap());
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM refresh_tokens WHERE user_id = $1 AND revoked_at IS NULL")
        .bind(u)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(live, 0, "no refresh token survives the reset");
    let v: i32 = sqlx::query_scalar("SELECT token_version FROM users WHERE id = $1").bind(u).fetch_one(&pool).await.unwrap();
    assert_eq!(v, 1, "the access-token epoch was bumped by the same transaction");
    drop_db(&db).await;
}

/// A key that is LOCKED keeps its lock even when its window start has aged out:
/// the window restarts only once the lock itself has ended.
#[tokio::test]
async fn a_live_lock_outlives_an_aged_window() {
    let (pool, db) = fresh_db().await;
    let repo = RecoveryRepository::new(pool.clone());
    for _ in 0..3 {
        repo.begin_attempt("name", "locked-one", 15, 3).await.unwrap();
    }
    assert!(!repo.begin_attempt("name", "locked-one", 15, 3).await.unwrap(), "over the limit");
    // The window start ages out, but the lock (set when the limit was reached) is still ahead.
    sqlx::query("UPDATE auth_recovery_attempts SET window_started_at = NOW() - INTERVAL '30 minutes' WHERE key = 'locked-one'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(!repo.begin_attempt("name", "locked-one", 15, 3).await.unwrap(), "a live lock is not escaped by an aged window");
    // Once the lock itself ends, the window restarts.
    sqlx::query("UPDATE auth_recovery_attempts SET locked_until = NOW() - INTERVAL '1 minute' WHERE key = 'locked-one'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo.begin_attempt("name", "locked-one", 15, 3).await.unwrap(), "lock over, window restarted");
    drop_db(&db).await;
}
