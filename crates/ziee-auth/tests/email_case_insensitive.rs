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
// TEST-4 [acceptance, INV-1] (issue #251) — the DATABASE refuses the second principal.
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
        msg.contains("users_email_lower_unique_idx"),
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
         WHERE c.relname = 'users_email_lower_unique_idx'",
    )
    .fetch_one(&pool)
    .await
    .expect("users_email_lower_unique_idx exists");
    assert!(unique.0, "users_email_lower_unique_idx must be a UNIQUE index");

    drop_db(&db).await;
}

// =====================================================================================
// TEST-5 [acceptance, INV-3] — the trim agrees with the index, at the schema level
// =====================================================================================

#[tokio::test]
// TEST-5 [acceptance, INV-3] (issue #251) — the trim CHECK refuses every Unicode-whitespace pad.
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
// TEST-6 (issue #251) — every by-email resolver resolves the variants to one principal.
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

    // The username-or-email resolver (the login path) does NOT inherit it, deliberately —
    // see `login_resolver_keeps_its_pre_251_semantics_while_get_by_email_is_fixed` for why
    // two attempts to extend the fix there were each reproduced as a worse attack. Pinned
    // here so the boundary is visible from both sides.
    assert!(
        users
            .get_by_username_or_email("BoB@Corp.com")
            .await
            .expect("query ok")
            .is_none(),
        "login by a case variant is out of scope for #251 and stays as it was"
    );
    // ...and it still resolves by username, byte-exactly.
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
// TEST-6 (issue #251) — storage keeps the casing the user typed.
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
// TEST-7 (issue #251) — every users.email writer trims at the write.
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
// TEST-7 (issue #251) — a real writer cannot create a padded twin.
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

/// `UserRepository::update` is a `pub` writer on a library crate, and it was the ONE
/// `users.email` writer the first version of this fix missed — a blind audit reproduced the
/// consequence: an untrimmed address raised `23514 users_email_trimmed`, and the
/// unique-violation-only error mapper turned that into a 500.
#[tokio::test]
// TEST-17 (issue #251) — UserRepository::update trims and names the right field.
async fn update_trims_and_reports_an_email_collision_as_an_email_collision() {
    let (pool, db) = fresh_db().await;
    let auth = AuthRepository::new(pool.clone());
    let users = UserRepository::new(pool.clone());

    let bob = auth
        .create_local_user_with_default_group("bob", "bob@corp.com", None, None)
        .await
        .expect("bob");
    let carol = auth
        .create_local_user_with_default_group("carol", "carol@corp.com", None, None)
        .await
        .expect("carol");

    // (a) It TRIMS — this used to be a 500.
    let updated = users
        .update(
            carol.id,
            None,
            Some("\u{00A0}\u{2009} carol.new@corp.com \u{3000}".to_string()),
            None,
            None,
        )
        .await
        .expect("an untrimmed address must be normalized, not 500");
    assert_eq!(updated.email, "carol.new@corp.com");
    assert_eq!(stored_email(&pool, carol.id).await, "carol.new@corp.com");

    // (b) A CASE VARIANT of someone else's address is refused, and named as an EMAIL
    //     collision rather than mislabelled "Username already exists".
    let err = users
        .update(carol.id, None, Some("BOB@CORP.COM".to_string()), None, None)
        .await
        .expect_err("a case variant of another user's address must be refused");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("Email"),
        "the refusal must name the EMAIL collision — mislabelling it as a username \
         collision sends an operator to look at a username that is fine; got {msg}"
    );
    assert!(
        !msg.contains("Username"),
        "and must not simultaneously blame the username; got {msg}"
    );

    // POSITIVE CONTROL — a genuinely free address still updates.
    users
        .update(carol.id, None, Some("carol.other@corp.com".to_string()), None, None)
        .await
        .expect("an unused address is accepted");
    assert_eq!(bob.email, "bob@corp.com");

    drop_db(&db).await;
}

/// The login resolver is DELIBERATELY UNCHANGED by #251, and this pins that.
///
/// Two revisions tried to make it case-insensitive on the email half. Blind audits
/// reproduced each as a worse attack than the arbitrary resolution it replaced: an ordering
/// preference handed the attacker a deterministic win in the mirror direction, and failing
/// closed let two unauthenticated registrations lock a victim out of BOTH their identifiers
/// permanently (there is no self-service email change and no password reset). Every one of
/// those attacks needed the email half to be case-insensitive HERE.
///
/// #251 does not need it: the invitation binding and registration's collision pre-check both
/// resolve through `get_by_email`, not through this function. So this test asserts the
/// pre-existing behaviour is intact — and, importantly, that reverting it did NOT undo the
/// actual fix.
#[tokio::test]
// TEST-18 [acceptance, INV-4] (issue #251) — the login resolver is deliberately unchanged; get_by_email is not.
async fn login_resolver_keeps_its_pre_251_semantics_while_get_by_email_is_fixed() {
    let (pool, db) = fresh_db().await;
    let auth = AuthRepository::new(pool.clone());
    let users = UserRepository::new(pool.clone());

    let bob = auth
        .create_local_user_with_default_group(
            "bob",
            "Bob@Corp.com",
            Some(hash_password("userPassw0rd!").expect("hash")),
            None,
        )
        .await
        .expect("bob");

    // Byte-exact on BOTH halves, as before this branch.
    assert_eq!(
        users
            .get_by_username_or_email("bob")
            .await
            .expect("query ok")
            .expect("username resolves")
            .id,
        bob.id
    );
    assert_eq!(
        users
            .get_by_username_or_email("Bob@Corp.com")
            .await
            .expect("query ok")
            .expect("the exact stored address resolves")
            .id,
        bob.id
    );
    assert!(
        users
            .get_by_username_or_email("bob@corp.com")
            .await
            .expect("query ok")
            .is_none(),
        "login by a case VARIANT of the address is not supported, exactly as before #251 — \
         making it so is what bought two new attack classes, and the invitation binding \
         does not go through this function"
    );

    // THE FIX IS STILL IN PLACE where it is actually needed. Without this the test above
    // would be satisfied by a branch that reverted everything.
    assert_eq!(
        users
            .get_by_email("bob@corp.com")
            .await
            .expect("query ok")
            .expect("get_by_email IS case-insensitive")
            .id,
        bob.id,
        "reverting the login resolver must not have undone #251 itself"
    );
    assert!(
        auth.create_local_user_with_default_group("bob2", "BOB@CORP.COM", None, None)
            .await
            .is_err(),
        "and a second principal for that mailbox is still refused"
    );

    drop_db(&db).await;
}

// =====================================================================================
// TEST-12 — OAuth provisioning resolves to the existing user, not a duplicate
// =====================================================================================

#[tokio::test]
// TEST-12 (issue #251) — OAuth provisioning resolves to the existing user.
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

/// The First-Broker-Login write guard must agree with the lookup that reached it.
///
/// `find_user_by_email_for_linking` trims, so a provider address padded with Unicode
/// whitespace resolves to the local user and takes the FBL branch. The confirmation write
/// then re-states the invariant with `lower(email) = lower($2)` — and `users.email` is now
/// guaranteed TRIMMED by `users_email_trimmed`, so an untrimmed `external_email` would never
/// match: the identity would link while `email_verified` silently stayed false. Fail-closed,
/// but the two would disagree about the same string.
#[tokio::test]
// TEST-21 (issue #251) — the FBL write guard agrees with the lookup that reached it.
async fn first_broker_login_verifies_a_whitespace_padded_provider_address() {
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

    // The same padded address that `find_user_by_email_for_linking` resolves.
    let padded = "\u{00A0}BOB@corp.com\u{2009}";
    assert_eq!(
        auth.find_user_by_email_for_linking(padded)
            .await
            .expect("query ok"),
        Some(bob.id),
        "precondition: the lookup reaches FBL for this address"
    );

    let verified = auth
        .link_verified_external_identity(
            bob.id,
            provider_id(),
            "ext-padded",
            Some(padded),
            None,
        )
        .await
        .expect("link");
    assert!(
        verified,
        "the write guard must agree with the lookup that reached it — otherwise the identity \
         links and email_verified stays false forever"
    );

    let row: (bool,) = sqlx::query_as("SELECT email_verified FROM users WHERE id = $1")
        .bind(bob.id)
        .fetch_one(&pool)
        .await
        .expect("read back");
    assert!(row.0, "and the column is actually set");

    // NEGATIVE CONTROL — a MISMATCHED provider address must still verify nobody, so the
    // trim did not loosen the guard into accepting anything.
    let carol = auth
        .create_local_user_with_default_group("carol", "carol@corp.com", None, None)
        .await
        .expect("carol");
    assert!(
        !auth
            .link_verified_external_identity(
                carol.id,
                provider_id(),
                "ext-mismatch",
                Some("\u{00A0}someone.else@corp.com "),
                None,
            )
            .await
            .expect("link"),
        "a mismatched address must link the identity but verify nothing"
    );

    drop_db(&db).await;
}

// =====================================================================================
// TEST-8 — the migration REFUSES a pre-existing collision rather than guessing
// =====================================================================================

/// Rewind the schema to its PRE-#251 shape so the collision state the bug produced can
/// actually be built: drop the case-insensitive index and the trim CHECK, and restore the
/// case-SENSITIVE constraint that let the two principals coexist.
async fn rewind_to_pre_migration(pool: &PgPool) {
    pool.execute(
        "DROP INDEX IF EXISTS users_email_lower_unique_idx; \
         ALTER TABLE users DROP CONSTRAINT IF EXISTS users_email_trimmed; \
         ALTER TABLE users ADD CONSTRAINT users_email_key UNIQUE (email);",
    )
    .await
    .expect("rewind to the pre-migration schema");
}

async fn seed(pool: &PgPool, username: &str, email: &str, created_at: &str) {
    sqlx::query(
        "INSERT INTO users (username, email, created_at) VALUES ($1, $2, $3::timestamptz)",
    )
    .bind(username)
    .bind(email)
    .bind(created_at)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("seed {username}: {e}"));
}

async fn email_of(pool: &PgPool, username: &str) -> String {
    sqlx::query_as::<_, (String,)>("SELECT email FROM users WHERE username = $1")
        .bind(username)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("row {username}: {e}"))
        .0
}

/// The schema this migration leaves behind, asserted in one place.
async fn assert_fixed_schema(pool: &PgPool) {
    let unique: (bool,) = sqlx::query_as(
        "SELECT i.indisunique FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid \
         WHERE c.relname = 'users_email_lower_unique_idx'",
    )
    .fetch_one(pool)
    .await
    .expect("users_email_lower_unique_idx must exist");
    assert!(unique.0, "and must be UNIQUE");

    // The byte-identical non-unique duplicate created by 202607140050 must be GONE, or every
    // users write maintains two identical btrees.
    let redundant: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM pg_indexes WHERE indexname = 'idx_users_lower_email'",
    )
    .fetch_one(pool)
    .await
    .expect("count redundant index");
    assert_eq!(redundant.0, 0, "the redundant idx_users_lower_email must be dropped");

    let old: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM pg_constraint WHERE conname = 'users_email_key'",
    )
    .fetch_one(pool)
    .await
    .expect("count old constraint");
    assert_eq!(old.0, 0, "the case-sensitive users_email_key must be gone");
}

/// A pre-existing collision STOPS the migration, names the accounts, and changes NOTHING.
///
/// # Why refusing is the resolution, and why this test asserts the rollback so hard
///
/// Three earlier versions of this migration resolved collisions automatically, and a blind
/// audit reproduced each as an attack: ranking by `created_at` awarded the mailbox to the
/// squatter (who registers first) and deactivated the legitimate account, sometimes the root
/// admin; adding `is_admin`/`email_verified` still fell through to `created_at` in the normal
/// case, because registration is open and unverified; adding `is_active` transferred a
/// mailbox from a suspended legitimate holder to an active squatter; and parking every
/// address on a tie FREED the mailbox for the attacker who knew to poll for it, while letting
/// one unauthenticated registration force a chosen victim's address to be parked.
///
/// Which of two accounts owns a mailbox is not derivable from this schema — every available
/// signal is either attacker-controlled or unrelated to mailbox control — so the migration
/// stops and asks. The rollback assertions below are the important half: a diagnostic that
/// left the table half-modified would be worse than no diagnostic.
#[tokio::test]
// TEST-8 / TEST-14 [acceptance, INV-1] (issue #251) — the migration refuses and changes nothing.
async fn migration_refuses_a_preexisting_collision_and_changes_nothing() {
    let (pool, db) = fresh_db().await;
    rewind_to_pre_migration(&pool).await;

    seed(&pool, "bob", "bob@corp.com", "2026-01-01T00:00:00Z").await;
    seed(&pool, "squatter", "BOB@corp.com", "2026-02-01T00:00:00Z").await;
    seed(&pool, "carol", "carol@corp.com", "2026-01-15T00:00:00Z").await;
    // A padded twin, to prove step 2's normalization is INSIDE the transaction that rolls
    // back — it would otherwise be a silent rewrite of a user identifier that survives a
    // failed migration.
    seed(&pool, "bob_padded", "\u{00A0}dave@corp.com", "2026-03-01T00:00:00Z").await;

    let err = pool
        .execute(MIGRATION_SQL)
        .await
        .expect_err("the migration must REFUSE to proceed, not guess which account owns it");
    let msg = format!("{err}");
    assert!(
        msg.contains("MIGRATION 202609050010 STOPPED"),
        "the refusal must be the named diagnostic, not an incidental constraint error; \
         got: {msg}"
    );
    assert!(
        msg.contains('2'),
        "and must state HOW MANY accounts collide; got: {msg}"
    );

    // NOTHING CHANGED. The whole migration is one transaction, so step 2's trim rolls back
    // with the rest — asserted explicitly because a partially-applied normalization would be
    // exactly the silent identifier rewrite this design exists to avoid.
    assert_eq!(email_of(&pool, "bob").await, "bob@corp.com");
    assert_eq!(email_of(&pool, "squatter").await, "BOB@corp.com");
    assert_eq!(email_of(&pool, "carol").await, "carol@corp.com");
    assert_eq!(
        email_of(&pool, "bob_padded").await,
        "\u{00A0}dave@corp.com",
        "step 2's whitespace normalization must have rolled back too"
    );

    let total: (i64,) = sqlx::query_as("SELECT count(*) FROM users")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(total.0, 4, "no row deleted");
    let deactivated: (i64,) = sqlx::query_as("SELECT count(*) FROM users WHERE NOT is_active")
        .fetch_one(&pool)
        .await
        .expect("count deactivated");
    assert_eq!(deactivated.0, 0, "nobody deactivated");

    // The schema is still the OLD one — the fix did not half-apply.
    let old: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM pg_constraint WHERE conname = 'users_email_key'",
    )
    .fetch_one(&pool)
    .await
    .expect("count old constraint");
    assert_eq!(old.0, 1, "the pre-existing constraint survives the rollback");
    let new_idx: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM pg_indexes WHERE indexname = 'users_email_lower_unique_idx'",
    )
    .fetch_one(&pool)
    .await
    .expect("count new index");
    assert_eq!(new_idx.0, 0, "and the new index was not created");

    drop_db(&db).await;
}

/// The operator resolves the collision, re-runs, and the migration completes — so the refusal
/// is a PAUSE, not a dead end. Without this the test above would be satisfied by a migration
/// that can never apply to a database that ever had a collision.
#[tokio::test]
// TEST-15 (issue #251) — the refusal is a pause, not a dead end.
async fn migration_applies_once_the_operator_has_resolved_the_collision() {
    let (pool, db) = fresh_db().await;
    rewind_to_pre_migration(&pool).await;

    seed(&pool, "bob", "bob@corp.com", "2026-01-01T00:00:00Z").await;
    seed(&pool, "squatter", "BOB@corp.com", "2026-02-01T00:00:00Z").await;

    assert!(pool.execute(MIGRATION_SQL).await.is_err(), "refused first");

    // The operator adjudicates — exactly the action the HINT describes.
    sqlx::query("UPDATE users SET email = 'squatter@corp.com' WHERE username = 'squatter'")
        .execute(&pool)
        .await
        .expect("operator resolves the collision");

    pool.execute(MIGRATION_SQL)
        .await
        .expect("and the migration then applies");

    assert_eq!(email_of(&pool, "bob").await, "bob@corp.com");
    assert_eq!(email_of(&pool, "squatter").await, "squatter@corp.com");
    assert_fixed_schema(&pool).await;
    assert!(
        raw_insert(&pool, "bob_again", "BOB@CORP.COM").await.is_err(),
        "and the collision is now unrecreatable"
    );

    drop_db(&db).await;
}

/// A whitespace-padded twin IS a collision under the new rule, and is caught by the guard
/// rather than silently merged — step 2 normalizes before step 3 counts.
#[tokio::test]
// TEST-16 (issue #251) — a padded twin is a collision.
async fn migration_refuses_a_whitespace_padded_twin_as_a_collision() {
    let (pool, db) = fresh_db().await;
    rewind_to_pre_migration(&pool).await;

    seed(&pool, "bob", "bob@corp.com", "2026-01-01T00:00:00Z").await;
    seed(&pool, "bob_padded", "\u{00A0}bob@corp.com", "2026-02-01T00:00:00Z").await;

    let err = pool
        .execute(MIGRATION_SQL)
        .await
        .expect_err("a padded twin is the same mailbox and must be refused as a collision");
    assert!(format!("{err}").contains("MIGRATION 202609050010 STOPPED"));

    // Rolled back, including the trim.
    assert_eq!(email_of(&pool, "bob_padded").await, "\u{00A0}bob@corp.com");

    drop_db(&db).await;
}

/// The clean path: no collisions, so the migration applies and leaves the fixed schema.
/// This is the POSITIVE CONTROL for the three refusal tests above — without it they would
/// pass just as well against a migration that always refuses.
#[tokio::test]
// TEST-20 (issue #251) — POSITIVE CONTROL for the refusal tests.
async fn migration_applies_cleanly_when_there_are_no_collisions() {
    let (pool, db) = fresh_db().await;
    rewind_to_pre_migration(&pool).await;

    seed(&pool, "bob", "bob@corp.com", "2026-01-01T00:00:00Z").await;
    seed(&pool, "carol", "Carol@Corp.com", "2026-02-01T00:00:00Z").await;
    // Untrimmed but NOT colliding — step 2 normalizes it and the migration proceeds.
    seed(&pool, "dave", "\u{2009}dave@corp.com\u{3000}", "2026-03-01T00:00:00Z").await;

    pool.execute(MIGRATION_SQL)
        .await
        .expect("with no collisions the migration must apply");

    assert_eq!(email_of(&pool, "bob").await, "bob@corp.com");
    assert_eq!(
        email_of(&pool, "carol").await,
        "Carol@Corp.com",
        "casing is preserved — only whitespace is normalized"
    );
    assert_eq!(
        email_of(&pool, "dave").await,
        "dave@corp.com",
        "step 2 trims the padded address so it satisfies the new CHECK"
    );
    assert_fixed_schema(&pool).await;

    drop_db(&db).await;
}
