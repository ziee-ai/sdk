//! Crate-scoped DB integration tests for CASE-INSENSITIVE `users.email` uniqueness
//! (issue #251), against a fresh throwaway DB migrated with the crate's own
//! `AUTH_MIGRATOR`.
//!
//! The defect: `users_email_key` was a CASE-SENSITIVE UNIQUE constraint, so `bob@corp.com`
//! and `BOB@corp.com` were two distinct principals — and both satisfied an account
//! invitation issued to `bob@corp.com`, whose binding normalizes with `lower(trim(...))`.
//! Registration is open and unverified, so anyone holding a leaked invite link could
//! register the case variant and redeem it.
//!
//! These tests prove the refusal is in the DATABASE (not only in an application pre-check a
//! race can slip past), that it survives the Unicode-whitespace variant, that every writer
//! and every by-email resolver inherits it, and that the migration's collision branch is
//! correct rather than merely unreached.

mod common;

use common::{SEEDED_PROVIDER_ID, drop_db, fresh_db};
use sqlx::{Executor, PgPool};
use uuid::Uuid;
use ziee_auth::auth::AuthRepository;
use ziee_auth::auth::hash_password;
use ziee_auth::user::UserRepository;

/// The migration under test, embedded at COMPILE time so a rename is a build error rather
/// than a test that silently stops checking anything.
const MIGRATION_SQL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/migrations/202609050010_users_email_case_insensitive.sql"
));

fn provider_id() -> Uuid {
    Uuid::parse_str(SEEDED_PROVIDER_ID).unwrap()
}

/// A raw INSERT that bypasses every Rust writer — which is the point: these assertions must
/// be about what the SCHEMA permits, not about what the application happens to send.
async fn raw_insert(pool: &PgPool, username: &str, email: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO users (username, email) VALUES ($1, $2)")
        .bind(username)
        .bind(email)
        .execute(pool)
        .await
        .map(|_| ())
}

async fn stored_email(pool: &PgPool, id: Uuid) -> String {
    sqlx::query_as::<_, (String,)>("SELECT email FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read back the stored address")
        .0
}

// =====================================================================================
// TEST-4 [acceptance, INV-1] — the DATABASE refuses the second principal
// =====================================================================================

#[tokio::test]
async fn database_refuses_a_case_variant_of_an_existing_address() {
    let (pool, db) = fresh_db().await;

    raw_insert(&pool, "bob", "bob@corp.com")
        .await
        .expect("the first principal is accepted");

    let err = raw_insert(&pool, "bob_variant", "BOB@corp.com")
        .await
        .expect_err("#251: a case variant of an existing address must be REFUSED");
    let msg = err.to_string();
    assert!(
        msg.contains("users_email_lower_key"),
        "the refusal must come from the case-insensitive unique index, not from something \
         incidental; got: {msg}"
    );

    // A mixed-case variant of a mixed-case original, in both directions.
    raw_insert(&pool, "carol", "Carol.Smith@Corp.com")
        .await
        .expect("a genuinely different address is still accepted");
    assert!(
        raw_insert(&pool, "carol_variant", "carol.smith@corp.com")
            .await
            .is_err(),
        "case-insensitivity is symmetric: the all-lowercase variant of a mixed-case original \
         must be refused too"
    );

    // The case-SENSITIVE rule must be GONE, not merely shadowed by the new one: leaving it
    // in place would let the old, weaker constraint keep reading as if it were the rule.
    let old_constraint: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM pg_constraint WHERE conname = 'users_email_key'",
    )
    .fetch_one(&pool)
    .await
    .expect("query pg_constraint");
    assert_eq!(
        old_constraint.0, 0,
        "the case-sensitive users_email_key must be dropped by 202609050010"
    );

    // ...and the new index must actually be UNIQUE (a non-unique index would satisfy a naive
    // "the index exists" check while enforcing nothing).
    let unique: (bool,) = sqlx::query_as(
        "SELECT i.indisunique FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid \
         WHERE c.relname = 'users_email_lower_key'",
    )
    .fetch_one(&pool)
    .await
    .expect("users_email_lower_key exists");
    assert!(unique.0, "users_email_lower_key must be a UNIQUE index");

    drop_db(&db).await;
}

// =====================================================================================
// TEST-5 [acceptance, INV-3] — the trim agrees with the index, at the schema level
// =====================================================================================

#[tokio::test]
async fn check_constraint_rejects_every_unicode_whitespace_pad() {
    let (pool, db) = fresh_db().await;

    for (n, padded) in [
        "\u{00A0}bob@corp.com",
        "bob@corp.com\u{00A0}",
        "\u{2009}bob@corp.com",
        "bob@corp.com\u{2009}",
        "\u{3000}bob@corp.com",
        "\u{2028}bob@corp.com",
        "\u{202F}bob@corp.com",
        "\u{205F}bob@corp.com",
        "\u{1680}bob@corp.com",
        "\u{0085}bob@corp.com",
        " bob@corp.com",
        "bob@corp.com\t",
    ]
    .into_iter()
    .enumerate()
    {
        let err = raw_insert(&pool, &format!("padded_{n}"), padded)
            .await
            .expect_err(&format!(
                "an untrimmed address (variant #{n}, {padded:?}) must be refused by \
                 users_email_trimmed — otherwise it sits beside its trimmed twin as a \
                 second principal and #251 survives in a narrower form"
            ));
        assert!(
            err.to_string().contains("users_email_trimmed"),
            "variant #{n} must be refused by the trim CHECK specifically; got: {err}"
        );
    }

    // Control: the trimmed form of that same address IS accepted, so the CHECK is rejecting
    // the padding and not the address.
    raw_insert(&pool, "bob", "bob@corp.com")
        .await
        .expect("the trimmed form of the same address is accepted");

    drop_db(&db).await;
}

// =====================================================================================
// TEST-6 — every by-email resolver resolves the variants to the SAME principal
// =====================================================================================

#[tokio::test]
async fn get_by_email_resolves_case_and_whitespace_variants() {
    let (pool, db) = fresh_db().await;
    let auth = AuthRepository::new(pool.clone());
    let users = UserRepository::new(pool.clone());

    let created = auth
        .create_local_user_with_default_group(
            "bob",
            "bob@corp.com",
            Some(hash_password("userPassw0rd!").expect("hash")),
            None,
        )
        .await
        .expect("create the genuine principal");

    for probe in [
        "BOB@CORP.COM",
        "Bob@Corp.Com",
        "  bob@corp.com  ",
        "\u{00A0}BOB@corp.com\u{2009}",
        "bob@corp.com",
    ] {
        let found = users
            .get_by_email(probe)
            .await
            .expect("query ok")
            .unwrap_or_else(|| panic!("{probe:?} must resolve to the existing principal"));
        assert_eq!(
            found.id, created.id,
            "{probe:?} resolved to a DIFFERENT user — the binding would then admit two \
             principals for one mailbox, which is #251"
        );
    }

    // The username-or-email resolver (the login path) inherits it on the email half.
    for probe in ["BoB@Corp.com", " bob@corp.com "] {
        let found = users
            .get_by_username_or_email(probe)
            .await
            .expect("query ok")
            .unwrap_or_else(|| panic!("{probe:?} must resolve via the email half"));
        assert_eq!(found.id, created.id);
    }
    // ...and still resolves by username, byte-exactly.
    assert_eq!(
        users
            .get_by_username_or_email("bob")
            .await
            .expect("query ok")
            .expect("username still resolves")
            .id,
        created.id
    );

    // NEGATIVE CONTROL — without this the assertions above would pass just as well if the
    // lookup matched everything.
    assert!(
        users
            .get_by_email("bob@other.com")
            .await
            .expect("query ok")
            .is_none(),
        "an unrelated address must NOT resolve"
    );
    assert!(
        users
            .get_by_email("bobb@corp.com")
            .await
            .expect("query ok")
            .is_none(),
        "a near-miss address must NOT resolve — the comparison is case-insensitive, not fuzzy"
    );

    // The stored value keeps the casing the user typed (display / SMTP), per the documented
    // best practice: lowercase for COMPARISON, original for presentation.
    assert_eq!(
        stored_email(&pool, created.id).await,
        "bob@corp.com",
        "storage must not be lowercased"
    );

    drop_db(&db).await;
}

#[tokio::test]
async fn stored_casing_is_preserved_verbatim() {
    let (pool, db) = fresh_db().await;
    let auth = AuthRepository::new(pool.clone());
    let users = UserRepository::new(pool.clone());

    let created = auth
        .create_local_user_with_default_group("carol", "Carol.Smith@Corp.COM", None, None)
        .await
        .expect("create");

    assert_eq!(
        stored_email(&pool, created.id).await,
        "Carol.Smith@Corp.COM",
        "the address is stored exactly as typed; only comparisons lower() it"
    );
    assert_eq!(
        users
            .get_by_email("carol.smith@corp.com")
            .await
            .expect("query ok")
            .expect("resolves case-insensitively")
            .id,
        created.id
    );

    drop_db(&db).await;
}

// =====================================================================================
// TEST-7 — every `users` writer trims at the write
// =====================================================================================

#[tokio::test]
async fn every_users_writer_trims_at_the_write() {
    let (pool, db) = fresh_db().await;
    let auth = AuthRepository::new(pool.clone());
    let users = UserRepository::new(pool.clone());

    // ASCII space + U+00A0 + U+2009 + U+3000 on both ends — none of which a one-argument
    // `btrim` would remove, and all of which `str::trim` does.
    let pad = |local: &str| format!("\u{00A0}\u{2009} {local}@corp.com \u{3000}\u{2009}");

    // 1. UserRepository::create — the admin-create + first-run/setup path.
    let a = users
        .create("writer_a", &pad("a"), None, None, None)
        .await
        .expect("UserRepository::create");
    assert_eq!(stored_email(&pool, a.id).await, "a@corp.com");

    // 2. AuthRepository::create_local_user_with_default_group — POST /api/auth/register.
    let b = auth
        .create_local_user_with_default_group("writer_b", &pad("b"), None, None)
        .await
        .expect("create_local_user_with_default_group");
    assert_eq!(stored_email(&pool, b.id).await, "b@corp.com");

    // 3. AuthRepository::create_external_user_with_link — LDAP/OAuth link provisioning.
    let c = auth
        .create_external_user_with_link(
            "writer_c",
            Some(pad("c")),
            "Writer C",
            provider_id(),
            "ext-c",
        )
        .await
        .expect("create_external_user_with_link");
    assert_eq!(stored_email(&pool, c).await, "c@corp.com");

    // 4. AuthRepository::provision_external_user_atomic — the OAuth auto-provision branch.
    let d = auth
        .provision_external_user_atomic(
            "writer_d",
            Some(&pad("d")),
            true,
            "Writer D",
            provider_id(),
            "ext-d",
            None,
        )
        .await
        .expect("provision_external_user_atomic");
    assert_eq!(stored_email(&pool, d).await, "d@corp.com");

    // 5. AuthRepository::create_external_user — the bare external writer.
    let e = auth
        .create_external_user("writer_e", Some(pad("e")), "Writer E")
        .await
        .expect("create_external_user");
    assert_eq!(stored_email(&pool, e).await, "e@corp.com");

    // The whole point: not one of those writers can produce a row the trim CHECK would have
    // had to reject, so no untrimmed twin can exist for a padded variant to become.
    let untrimmed: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM users WHERE email <> btrim(email, \
         E'\\u0009\\u000A\\u000B\\u000C\\u000D\\u0020\\u0085\\u00A0\\u1680\\u2000\\u2001\
         \\u2002\\u2003\\u2004\\u2005\\u2006\\u2007\\u2008\\u2009\\u200A\\u2028\\u2029\
         \\u202F\\u205F\\u3000')",
    )
    .fetch_one(&pool)
    .await
    .expect("count untrimmed");
    assert_eq!(untrimmed.0, 0, "no writer may leave an untrimmed address");

    drop_db(&db).await;
}

#[tokio::test]
async fn a_writer_cannot_create_a_padded_twin_of_an_existing_principal() {
    let (pool, db) = fresh_db().await;
    let auth = AuthRepository::new(pool.clone());

    auth.create_local_user_with_default_group("bob", "bob@corp.com", None, None)
        .await
        .expect("the genuine principal");

    // This is #251's narrower form: a padded/cased variant through a REAL writer must
    // collide, not create a second principal.
    for (n, variant) in [
        "BOB@CORP.COM",
        "\u{00A0}bob@corp.com",
        "bob@corp.com\u{2009}",
        "  Bob@Corp.com  ",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            auth.create_local_user_with_default_group(
                &format!("attacker_{n}"),
                variant,
                None,
                None
            )
            .await
            .is_err(),
            "variant #{n} ({variant:?}) must be refused — it is the same mailbox"
        );
    }

    let count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM users WHERE lower(email) = 'bob@corp.com'",
    )
    .fetch_one(&pool)
    .await
    .expect("count principals");
    assert_eq!(
        count.0, 1,
        "exactly ONE principal may normalize to the invited address — this is the property \
         an invitation's email binding rests on"
    );

    drop_db(&db).await;
}

// =====================================================================================
// TEST-12 — OAuth provisioning resolves to the existing user, not a duplicate
// =====================================================================================

#[tokio::test]
async fn oauth_linking_lookup_resolves_case_and_whitespace_variants() {
    let (pool, db) = fresh_db().await;
    let auth = AuthRepository::new(pool.clone());

    let bob = auth
        .create_local_user_with_default_group(
            "bob",
            "bob@corp.com",
            Some(hash_password("userPassw0rd!").expect("hash")),
            None,
        )
        .await
        .expect("local principal");

    // The three shapes a provider can hand back. Each must take the First-Broker-Login
    // branch (link to the existing user) rather than falling through to auto-provision,
    // which is what would create the duplicate principal.
    for probe in [
        "BOB@CORP.COM",
        "  bob@corp.com  ",
        "\u{00A0}BOB@corp.com\u{2009}",
        "Bob@Corp.Com",
    ] {
        assert_eq!(
            auth.find_user_by_email_for_linking(probe)
                .await
                .expect("query ok"),
            Some(bob.id),
            "{probe:?} must resolve to the existing local user"
        );
    }

    // NEGATIVE CONTROL — an unrelated address must not resolve.
    assert_eq!(
        auth.find_user_by_email_for_linking("someone@else.com")
            .await
            .expect("query ok"),
        None
    );

    // NEGATIVE CONTROL — the pre-existing `is_active` filter is not regressed. A disabled
    // user's address must still NOT trigger the FBL flow, because the /auth/link-account
    // page rendering would tell an attacker the address is registered.
    sqlx::query("UPDATE users SET is_active = false WHERE id = $1")
        .bind(bob.id)
        .execute(&pool)
        .await
        .expect("deactivate");
    assert_eq!(
        auth.find_user_by_email_for_linking("BOB@CORP.COM")
            .await
            .expect("query ok"),
        None,
        "a deactivated user must not be discoverable through the linking lookup"
    );

    drop_db(&db).await;
}

// =====================================================================================
// TEST-8 — the migration's collision branch is CORRECT, not merely unreached
// =====================================================================================

#[tokio::test]
async fn migration_resolves_preexisting_collisions_without_failing() {
    let (pool, db) = fresh_db().await;

    // Rewind the schema to its PRE-#251 shape so the collision state the bug produced can
    // actually be built: drop the case-insensitive index and the trim CHECK, and restore the
    // case-SENSITIVE constraint that let the two principals coexist.
    pool.execute(
        "DROP INDEX users_email_lower_key; \
         ALTER TABLE users DROP CONSTRAINT users_email_trimmed; \
         ALTER TABLE users ADD CONSTRAINT users_email_key UNIQUE (email);",
    )
    .await
    .expect("rewind to the pre-migration schema");

    // The exact artifact of the defect: three rows, one mailbox.
    for (username, email, created_at) in [
        ("bob", "bob@corp.com", "2026-01-01T00:00:00Z"),
        ("bob_variant", "BOB@corp.com", "2026-02-01T00:00:00Z"),
        ("bob_padded", "\u{00A0}bob@corp.com", "2026-03-01T00:00:00Z"),
    ] {
        sqlx::query("INSERT INTO users (username, email, created_at) VALUES ($1, $2, $3::timestamptz)")
            .bind(username)
            .bind(email)
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("seed {username}: {e}"));
    }
    // A bystander who must be left completely alone.
    sqlx::query("INSERT INTO users (username, email) VALUES ('carol', 'carol@corp.com')")
        .execute(&pool)
        .await
        .expect("seed bystander");

    // Re-apply the migration over that state. It MUST succeed — a migration that fails at
    // deploy on data the bug itself created is not a resolution.
    pool.execute(MIGRATION_SQL)
        .await
        .expect("202609050010 must APPLY over pre-existing collisions, not fail at deploy");

    // The earliest-created row keeps the address and stays live.
    let winner: (String, bool) = sqlx::query_as(
        "SELECT email, is_active FROM users WHERE username = 'bob'",
    )
    .fetch_one(&pool)
    .await
    .expect("winner row");
    assert_eq!(winner.0, "bob@corp.com", "the earliest row keeps the address");
    assert!(winner.1, "the earliest row stays active");

    // Every later collider is deactivated and re-addressed to the reserved form.
    for username in ["bob_variant", "bob_padded"] {
        let loser: (String, bool) = sqlx::query_as(
            "SELECT email, is_active FROM users WHERE username = $1",
        )
        .bind(username)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| panic!("{username} row: {e}"));
        assert!(
            loser.0.starts_with("dup.") && loser.0.ends_with("@invalid"),
            "{username} must be re-addressed to the reserved dup.<id>@invalid form, got {:?}",
            loser.0
        );
        assert!(!loser.1, "{username} must be deactivated");
    }

    // NOTHING is deleted — the resolution must stay reversible by an operator.
    let total: (i64,) = sqlx::query_as("SELECT count(*) FROM users")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(total.0, 4, "no row may be deleted by the resolution");

    // The bystander is untouched.
    let bystander: (String, bool) = sqlx::query_as(
        "SELECT email, is_active FROM users WHERE username = 'carol'",
    )
    .fetch_one(&pool)
    .await
    .expect("bystander");
    assert_eq!(bystander.0, "carol@corp.com");
    assert!(bystander.1);

    // And the schema ends up in the fixed shape regardless.
    let unique: (bool,) = sqlx::query_as(
        "SELECT i.indisunique FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid \
         WHERE c.relname = 'users_email_lower_key'",
    )
    .fetch_one(&pool)
    .await
    .expect("the unique index exists after the resolution");
    assert!(unique.0);

    // ...and it really enforces: the collision cannot be recreated.
    assert!(
        raw_insert(&pool, "bob_again", "BOB@CORP.COM").await.is_err(),
        "after the migration the collision must be unrecreatable"
    );

    drop_db(&db).await;
}
