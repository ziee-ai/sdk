//! The injectable, in-transaction user-created hook (gap G-AUTHEVT) fires on
//! EVERY user-creation path, runs INSIDE the creation transaction, and rolls the
//! whole creation back when it errors.
//!
//! ── why this is its own process ─────────────────────────────────────────────
//!
//! The hook is a process-wide `OnceLock` (install-once, first wins), so "nothing
//! installed yet" and "exactly this hook installed" each exist once per process.
//! A `#[cfg(test)]` sibling that installed would make the negative half
//! unassertable and turn the positive half order-dependent. Integration test
//! files each get their own binary, so this file owns the whole sequence and
//! runs it in order inside ONE `#[tokio::test]`, mirroring
//! `auth_sync_must_be_declared.rs`.
//!
//! It drives the REAL repository creation functions against a fresh migrated DB —
//! those are the funnel every SDK creation path (local register, LDAP/OAuth
//! first-login, admin-create, first-run setup) flows through, so covering them
//! proves the hook reaches all of them without an HTTP harness per path.

mod common;

use std::sync::{Arc, Mutex};

use sqlx::{PgConnection, Row};
use uuid::Uuid;

use common::{SEEDED_PROVIDER_ID, drop_db, fresh_db};
use ziee_auth::auth::{AuthRepository, hash_password};
use ziee_auth::user::UserRepository;
use ziee_auth::{UserCreatedHook, install_user_created_hook, user_created_hook};
use ziee_core::AppError;

/// A hook that (1) records every user id it is handed, (2) proves it runs INSIDE
/// the creation transaction by WRITING through the passed connection — it stamps
/// the just-created user's `avatar_url` with a sentinel, which can only survive
/// if that write commits atomically with the user row — and (3) errors for any
/// username containing `rollback`, so the abort/rollback contract is exercisable.
#[derive(Default)]
struct RecordingHook {
    seen: Arc<Mutex<Vec<Uuid>>>,
}

fn sentinel(id: Uuid) -> String {
    format!("hooked:{id}")
}

#[async_trait::async_trait]
impl UserCreatedHook for RecordingHook {
    async fn on_user_created(
        &self,
        user: &ziee_auth::User,
        tx: &mut PgConnection,
    ) -> Result<(), AppError> {
        self.seen.lock().unwrap().push(user.id);

        if user.username.contains("rollback") {
            // Abort: the caller's `?` must roll the whole creation back.
            return Err(AppError::internal_error("hook rejected this user"));
        }

        // Write through the CREATION transaction's own connection. If the hook
        // did not run in-transaction, this write would target a different
        // connection and either fail (row not yet visible) or not commit with
        // the user row — the post-commit assertion on this sentinel is the proof.
        sqlx::query("UPDATE users SET avatar_url = $1 WHERE id = $2")
            .bind(sentinel(user.id))
            .bind(user.id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::database_error)?;
        Ok(())
    }
}

/// A second hook whose installation must be IGNORED (first-wins). It records into
/// its OWN vec; if the second install won, creations would land here instead.
#[derive(Default)]
struct SecondHook {
    seen: Arc<Mutex<Vec<Uuid>>>,
}

#[async_trait::async_trait]
impl UserCreatedHook for SecondHook {
    async fn on_user_created(
        &self,
        user: &ziee_auth::User,
        _tx: &mut PgConnection,
    ) -> Result<(), AppError> {
        self.seen.lock().unwrap().push(user.id);
        Ok(())
    }
}

async fn user_exists(pool: &sqlx::PgPool, username: &str) -> Option<(Uuid, Option<String>)> {
    sqlx::query("SELECT id, avatar_url FROM users WHERE username = $1")
        .bind(username)
        .fetch_optional(pool)
        .await
        .expect("query users")
        .map(|r| {
            (
                r.get::<Uuid, _>("id"),
                r.get::<Option<String>, _>("avatar_url"),
            )
        })
}

#[tokio::test]
async fn user_created_hook_fires_on_every_path_in_tx_and_rolls_back_on_error() {
    let (pool, db) = fresh_db().await;
    let provider_id = Uuid::parse_str(SEEDED_PROVIDER_ID).unwrap();
    let auth = AuthRepository::new(pool.clone());
    let users = UserRepository::new(pool.clone());

    // ── 1. NO hook installed → the default is inert (backward compatible) ──────
    assert!(
        user_created_hook().is_none(),
        "this test owns the process; nothing may have installed a hook before it"
    );
    let pre = auth
        .create_local_user_with_default_group("nohook", "nohook@example.com", None, None)
        .await
        .expect("creation works with no hook installed (unchanged behaviour)");
    let (_, avatar) = user_exists(&pool, "nohook")
        .await
        .expect("nohook user exists");
    assert_eq!(
        avatar, None,
        "with no hook installed, nothing stamps the sentinel — today's behaviour, unchanged"
    );
    let _ = pre;

    // ── 2. install the recording hook ─────────────────────────────────────────
    let rec = Arc::new(RecordingHook::default());
    let seen = rec.seen.clone();
    install_user_created_hook(rec);
    assert!(user_created_hook().is_some(), "the hook is now installed");

    // ── 3. a SECOND install is ignored (first wins) ───────────────────────────
    let second = Arc::new(SecondHook::default());
    let second_seen = second.seen.clone();
    install_user_created_hook(second);

    // ── 4. fire on EVERY creation path; each stamps the in-tx sentinel ─────────
    // 4a. local register.
    let hash = hash_password("pw-123456").unwrap();
    let u1 = auth
        .create_local_user_with_default_group("local1", "local1@example.com", Some(hash), None)
        .await
        .expect("local register");
    // 4b. LDAP first-login.
    let u2 = auth
        .create_external_user_with_link(
            "ldap1",
            Some("ldap1@example.com".into()),
            "LDAP One",
            provider_id,
            "ext-ldap-1",
        )
        .await
        .expect("ldap create");
    // 4c. OAuth first-login.
    let u3 = auth
        .provision_external_user_atomic(
            "oauth1",
            Some("oauth1@example.com"),
            true,
            "OAuth One",
            provider_id,
            "ext-oauth-1",
            None,
        )
        .await
        .expect("oauth provision");
    // 4d. bare external create.
    let u4 = auth
        .create_external_user("ext1", Some("ext1@example.com".into()), "Ext One")
        .await
        .expect("external create");
    // 4e. admin-create / first-run setup (the app's user handlers call this).
    let u5 = users
        .create("admin1", "admin1@example.com", None, None, None)
        .await
        .expect("admin/first-run create")
        .id;

    // Every path was seen by the FIRST hook exactly once (and the u1..u5 ids).
    let recorded = seen.lock().unwrap().clone();
    for (label, id) in [
        ("local", u1.id),
        ("ldap", u2),
        ("oauth", u3),
        ("external", u4),
        ("admin", u5),
    ] {
        assert_eq!(
            recorded.iter().filter(|&&x| x == id).count(),
            1,
            "the {label} creation path must fire the hook exactly once"
        );
    }

    // The in-transaction sentinel survived the commit on every path — proof the
    // hook ran on the creation transaction's own connection, atomically.
    for username in ["local1", "ldap1", "oauth1", "ext1", "admin1"] {
        let (id, avatar) = user_exists(&pool, username)
            .await
            .unwrap_or_else(|| panic!("{username} must exist"));
        assert_eq!(
            avatar.as_deref(),
            Some(sentinel(id).as_str()),
            "the hook's in-tx write for {username} must have committed with the user row"
        );
    }

    // The second (ignored) hook never received anything — first-wins holds.
    assert!(
        second_seen.lock().unwrap().is_empty(),
        "a second install must be ignored; the FIRST hook stays the one that fires"
    );

    // ── 5. rollback contract: a hook error aborts the whole creation ──────────
    // Every creation function must roll back when the hook errors.
    let e1 = auth
        .create_local_user_with_default_group("rollback_local", "rb1@example.com", None, None)
        .await;
    assert!(e1.is_err(), "local: hook error must fail the creation");
    let e5 = users
        .create("rollback_admin", "rb5@example.com", None, None, None)
        .await;
    assert!(e5.is_err(), "admin: hook error must fail the creation");
    let e2 = auth
        .create_external_user_with_link(
            "rollback_ldap",
            Some("rb2@example.com".into()),
            "RB",
            provider_id,
            "ext-rb-2",
        )
        .await;
    assert!(e2.is_err(), "ldap: hook error must fail the creation");

    // …and NONE of the rolled-back users exist — the user INSERT was undone in
    // the same transaction the hook aborted.
    for username in ["rollback_local", "rollback_admin", "rollback_ldap"] {
        assert!(
            user_exists(&pool, username).await.is_none(),
            "{username} must NOT exist: a hook error rolls the user creation back"
        );
    }

    drop_db(&db).await;
}
