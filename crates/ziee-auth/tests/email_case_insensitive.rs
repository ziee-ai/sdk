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

/// `UserRepository::update` is a `pub` writer on a library crate, and it was the ONE
/// `users.email` writer the first version of this fix missed — a blind audit reproduced the
/// consequence: an untrimmed address raised `23514 users_email_trimmed`, and the
/// unique-violation-only error mapper turned that into a 500.
#[tokio::test]
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

/// The login resolver must REFUSE an ambiguous identifier — in BOTH directions.
///
/// # Why this test is shaped the way it is
///
/// Its predecessor was proven VACUOUS by two independent blind auditors: it seeded the
/// victim FIRST, so an unordered sequential scan returned the victim anyway, and it stayed
/// green with the entire ordering clause deleted — 5/5 and 4/4 runs. It observed seed order,
/// not the resolver. So here the ATTACKER is seeded first in every arm; if the resolver
/// stopped refusing and fell back to heap order, the attacker's row would come back and each
/// assertion would fail.
///
/// And it covers BOTH directions, because the previous FIX covered only one. Ordering by
/// `(username = $1) DESC` fixed "attacker's email shadows victim's username" and made the
/// mirror — "attacker's username shadows victim's email" — resolve to the attacker
/// deterministically, which is worse than the coin-flip it replaced: an invite-based product
/// hands out email addresses, so the email is the more guessable identifier of the two.
#[tokio::test]
async fn login_resolver_refuses_an_ambiguous_identifier_in_both_directions() {
    let (pool, db) = fresh_db().await;
    let auth = AuthRepository::new(pool.clone());
    let users = UserRepository::new(pool.clone());

    // ── Direction 1: the attacker's EMAIL is the victim's USERNAME ───────────────────
    // Attacker FIRST, so heap order favours them if the refusal is removed.
    auth.create_local_user_with_default_group("atk001", "Admin", None, None)
        .await
        .expect("attacker");
    auth.create_local_user_with_default_group("Admin", "realadmin@corp.com", None, None)
        .await
        .expect("victim");

    let matches: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM users WHERE username = $1 OR lower(email) = lower($1)",
    )
    .bind("Admin")
    .fetch_one(&pool)
    .await
    .expect("count matches");
    assert_eq!(matches.0, 2, "the seeded state must actually be ambiguous");

    assert!(
        users
            .get_by_username_or_email("Admin")
            .await
            .expect("query ok")
            .is_none(),
        "an AMBIGUOUS identifier must resolve to NOBODY. Any total order over the two rows \
         hands one of the two attacks a reliable win — the caller bcrypt-verifies the \
         submitted password against whichever row comes back, so a victim's correct \
         password would be checked against an attacker-chosen hash."
    );

    // ── Direction 2: the attacker's USERNAME is the victim's EMAIL ───────────────────
    // This is the direction the previous fix made WORSE. Attacker seeded first again.
    auth.create_local_user_with_default_group("bob@corp.com", "atk@evil.test", None, None)
        .await
        .expect("attacker 2");
    let victim2 = auth
        .create_local_user_with_default_group("victim2", "bob@corp.com", None, None)
        .await
        .expect("victim 2");

    assert!(
        users
            .get_by_username_or_email("bob@corp.com")
            .await
            .expect("query ok")
            .is_none(),
        "the MIRROR direction must be refused too — an ordering that prefers the exact \
         username match resolves this to the attacker deterministically"
    );
    // A CASE VARIANT of that same identifier is NOT ambiguous, and resolves to the victim —
    // because the username half stays byte-exact while the email half folds case, so
    // `BOB@CORP.COM` matches only the email. Worth pinning: it means the attacker's squatted
    // username cannot be reached through a case variant, and the genuine email holder can
    // still log in with any casing of their own address.
    assert_eq!(
        users
            .get_by_username_or_email("BOB@CORP.COM")
            .await
            .expect("query ok")
            .expect("a case variant matches only the email, so it is unambiguous")
            .id,
        victim2.id
    );

    // ── POSITIVE CONTROLS — the refusal is scoped to the ambiguity, not to logins ────
    // Without these, `is_none()` above would pass just as well against a resolver that
    // returns None for everything.
    assert_eq!(
        users
            .get_by_username_or_email("victim2")
            .await
            .expect("query ok")
            .expect("an UNAMBIGUOUS username still resolves")
            .id,
        victim2.id,
        "each party keeps the identifier the other did not take — the victim can always log \
         in with their username, so the squat is a nuisance, not a lockout"
    );
    assert_eq!(
        users
            .get_by_username_or_email("realadmin@corp.com")
            .await
            .expect("query ok")
            .expect("an UNAMBIGUOUS email still resolves")
            .username,
        "Admin"
    );
    assert_eq!(
        users
            .get_by_username_or_email("REALADMIN@CORP.COM")
            .await
            .expect("query ok")
            .expect("and still case-insensitively — #251 must not be undone by the refusal")
            .username,
        "Admin"
    );

    drop_db(&db).await;
}

/// The ledger is APPEND-ONLY: a user parked twice gets two records.
///
/// A blind audit reproduced the loss: with `user_id` as the primary key and
/// `ON CONFLICT (user_id) DO NOTHING`, the second parking was silently dropped, the ledger
/// kept asserting a stale original, and following the table comment's reinstatement recipe
/// restored a DIFFERENT PERSON'S address. The sequence is reachable because an operator
/// reinstates a parked user under a corrected address, which can collide again later.
#[tokio::test]
async fn collision_ledger_is_append_only_across_repeated_parkings() {
    let (pool, db) = fresh_db().await;
    rewind_to_pre_migration(&pool).await;

    seed(&pool, "vera", "VERA@corp.com", "2026-01-01T00:00:00Z", false, false).await;
    seed(&pool, "other", "vera@corp.com", "2026-02-01T00:00:00Z", false, true).await;

    pool.execute(MIGRATION_SQL).await.expect("first apply");
    let first: (String,) = sqlx::query_as(
        "SELECT l.original_email FROM users_email_collision_log l \
           JOIN users u ON u.id = l.user_id WHERE u.username = 'vera'",
    )
    .fetch_one(&pool)
    .await
    .expect("first ledger row");
    assert_eq!(first.0, "VERA@corp.com");

    // The operator reinstates vera under a CORRECTED address...
    sqlx::query("UPDATE users SET email = 'Vera.Real@corp.com' WHERE username = 'vera'")
        .execute(&pool)
        .await
        .expect("reinstate");
    // ...which a later account then collides with.
    rewind_to_pre_migration(&pool).await;
    seed(&pool, "later", "vera.real@corp.com", "2026-03-01T00:00:00Z", false, true).await;
    pool.execute(MIGRATION_SQL).await.expect("second apply");

    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT l.original_email FROM users_email_collision_log l \
           JOIN users u ON u.id = l.user_id WHERE u.username = 'vera' \
          ORDER BY l.detected_at",
    )
    .fetch_all(&pool)
    .await
    .expect("ledger rows");
    let rows: Vec<String> = rows.into_iter().map(|r| r.0).collect();
    assert_eq!(
        rows.len(),
        2,
        "BOTH parkings must be recorded — a ledger that drops the second one destroys \
         `Vera.Real@corp.com` entirely while still claiming vera's original was \
         `VERA@corp.com`, which is a different person's mailbox"
    );
    assert_eq!(rows, vec!["VERA@corp.com".to_string(), "Vera.Real@corp.com".to_string()]);

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

/// Rewind the schema to its PRE-#251 shape so the collision state the bug produced can
/// actually be built: drop the case-insensitive index, the trim CHECK and the ledger, and
/// restore the case-SENSITIVE constraint that let the two principals coexist.
async fn rewind_to_pre_migration(pool: &PgPool) {
    // Deliberately does NOT drop `users_email_collision_log`: it is an APPEND-ONLY ledger
    // that must outlive a re-apply, and dropping it here would hide exactly the record-loss
    // that `collision_ledger_is_append_only_across_repeated_parkings` exists to catch.
    pool.execute(
        "DROP INDEX IF EXISTS users_email_lower_unique_idx; \
         ALTER TABLE users DROP CONSTRAINT IF EXISTS users_email_trimmed; \
         ALTER TABLE users ADD CONSTRAINT users_email_key UNIQUE (email);",
    )
    .await
    .expect("rewind to the pre-migration schema");
}

/// `(email, is_active, is_admin)` for a seeded username.
async fn row_of(pool: &PgPool, username: &str) -> (String, bool, bool) {
    sqlx::query_as::<_, (String, bool, bool)>(
        "SELECT email, is_active, is_admin FROM users WHERE username = $1",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("row {username}: {e}"))
}

async fn seed(
    pool: &PgPool,
    username: &str,
    email: &str,
    created_at: &str,
    is_admin: bool,
    email_verified: bool,
) {
    sqlx::query(
        "INSERT INTO users (username, email, created_at, is_admin, email_verified) \
         VALUES ($1, $2, $3::timestamptz, $4, $5)",
    )
    .bind(username)
    .bind(email)
    .bind(created_at)
    .bind(is_admin)
    .bind(email_verified)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("seed {username}: {e}"));
}

/// Nothing is deactivated, nothing is deleted, and every parked address is recoverable.
///
/// Strengthened after a blind audit proved the first version vacuous: it JOINed FROM the
/// ledger, so it was trivially true on an EMPTY ledger, and three of the four migration
/// tests stayed GREEN with the ledger writes deleted entirely. The count equality below is
/// what closes that — it fails if a parked row has no ledger record, which is exactly the
/// direction the old form declined to check.
async fn assert_non_destructive(pool: &PgPool, expected_rows: i64, expected_parked: i64) {
    let total: (i64,) = sqlx::query_as("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await
        .expect("count users");
    assert_eq!(total.0, expected_rows, "no row may be deleted by the resolution");

    let deactivated: (i64,) = sqlx::query_as("SELECT count(*) FROM users WHERE NOT is_active")
        .fetch_one(pool)
        .await
        .expect("count deactivated");
    assert_eq!(
        deactivated.0, 0,
        "the resolution must DEACTIVATE NOBODY — disabling a collider can brick the \
         deployment (the root admin), orphan an organization account's only owner, or \
         disable an innocent bystander"
    );

    // EVERY parked row has a ledger record, and every ledger record describes the row it
    // names. Both directions, by count — an empty ledger fails this.
    let parked: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM users WHERE email ~ '^dup\\.[0-9a-f-]{36}\\.[0-9a-f-]{36}@invalid$'",
    )
    .fetch_one(pool)
    .await
    .expect("count parked");
    assert_eq!(
        parked.0, expected_parked,
        "exactly {expected_parked} address(es) should have been parked"
    );

    let logged: (i64,) = sqlx::query_as("SELECT count(*) FROM users_email_collision_log")
        .fetch_one(pool)
        .await
        .expect("count ledger");
    assert_eq!(
        logged.0, expected_parked,
        "the ledger must hold exactly one record per parking — a RAISE NOTICE is not a \
         record (Postgres does not write NOTICE to the server log at the default \
         log_min_messages), so if this is short the original address is simply gone"
    );

    let stale: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM users_email_collision_log l \
           JOIN users u ON u.id = l.user_id \
          WHERE u.email <> l.parked_email OR l.original_email = l.parked_email",
    )
    .fetch_one(pool)
    .await
    .expect("count stale ledger rows");
    assert_eq!(
        stale.0, 0,
        "every ledger row must name the address the user now holds and a DIFFERENT \
         original — otherwise the ledger cannot reinstate anything"
    );
}

/// The schema this migration is supposed to leave behind, asserted in one place.
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

/// `decided_by_evidence` for a seeded username's most recent parking.
async fn parked_verdict(pool: &PgPool, username: &str) -> bool {
    sqlx::query_as::<_, (bool,)>(
        "SELECT l.decided_by_evidence FROM users_email_collision_log l \
           JOIN users u ON u.id = l.user_id WHERE u.username = $1 \
          ORDER BY l.detected_at DESC LIMIT 1",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("ledger verdict for {username}: {e}"))
    .0
}

/// NO EVIDENCE ⇒ NOBODY WINS. This is the case the two earlier versions got wrong.
///
/// Registration is open and unverified, so in the common collision nothing distinguishes the
/// two accounts: neither is an admin, neither has a verified address, both are active. v1 and
/// v2 of this migration both fell through to `created_at` there — and since the squatter in
/// the #251 attack registers FIRST, that awarded the contested mailbox to the ATTACKER, which
/// the new unique index then made permanent (the victim could not take it back through any
/// route). Registration order is not evidence, so it no longer decides: every address in the
/// group is parked and the ledger flags it for review.
#[tokio::test]
async fn migration_awards_nothing_when_no_evidence_separates_the_colliders() {
    let (pool, db) = fresh_db().await;
    rewind_to_pre_migration(&pool).await;

    // The squatter registers FIRST, off a leaked invite link. The genuine invitee follows.
    seed(&pool, "squatter", "BOB@corp.com", "2026-01-01T00:00:00Z", false, false).await;
    seed(&pool, "bob", "bob@corp.com", "2026-02-01T00:00:00Z", false, false).await;
    seed(&pool, "carol", "carol@corp.com", "2026-01-15T00:00:00Z", false, false).await;

    pool.execute(MIGRATION_SQL)
        .await
        .expect("202609050010 must APPLY, not fail at deploy");

    // NEITHER holds the mailbox.
    let held: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM users WHERE lower(email) = 'bob@corp.com'",
    )
    .fetch_one(&pool)
    .await
    .expect("count holders");
    assert_eq!(
        held.0, 0,
        "with nothing to tell the two accounts apart, the migration must award the mailbox \
         to NEITHER — awarding it on registration order hands the squatter a binding the \
         new unique index then protects, and the victim cannot reclaim it"
    );

    for username in ["squatter", "bob"] {
        let row = row_of(&pool, username).await;
        assert!(row.0.ends_with("@invalid"), "{username} is parked");
        assert!(row.1, "{username} keeps their account");
        assert!(
            !parked_verdict(&pool, username).await,
            "{username}'s ledger row must be flagged decided_by_evidence = false — it is \
             the flag telling an operator a human must adjudicate this group"
        );
    }

    // The bystander is untouched, so the resolution is scoped to the collision.
    let bystander = row_of(&pool, "carol").await;
    assert_eq!(bystander.0, "carol@corp.com");
    assert!(bystander.1);

    assert_non_destructive(&pool, 3, 2).await;
    assert_fixed_schema(&pool).await;

    // Both originals are recoverable, verbatim. Ordered by the C collation, not the
    // cluster's: en_US sorts case-insensitively, which would make this depend on the locale.
    let originals: Vec<(String,)> = sqlx::query_as(
        "SELECT original_email FROM users_email_collision_log \
          ORDER BY original_email COLLATE \"C\"",
    )
    .fetch_all(&pool)
    .await
    .expect("read the ledger");
    assert_eq!(
        originals.into_iter().map(|r| r.0).collect::<Vec<_>>(),
        vec!["BOB@corp.com".to_string(), "bob@corp.com".to_string()]
    );

    // Parking BOTH frees the mailbox — which is the intended outcome: nobody was awarded it,
    // so it is available for whoever the operator decides owns it. What must NOT be possible
    // is the collision itself recurring, so claim it once and assert the variant is then
    // refused. (Asserting the first claim fails would be asserting the old, wrong semantics.)
    raw_insert(&pool, "bob_reinstated", "bob@corp.com")
        .await
        .expect("the freed mailbox can be claimed once");
    assert!(
        raw_insert(&pool, "bob_again", "BOB@CORP.COM").await.is_err(),
        "and the collision must then be unrecreatable — the index is doing its job"
    );

    drop_db(&db).await;
}

/// Whitespace normalization (step 2) feeds the collision pass (step 4): a padded twin must
/// collide with its unpadded original rather than survive as a second principal.
#[tokio::test]
async fn migration_resolves_a_whitespace_padded_twin_as_a_collision() {
    let (pool, db) = fresh_db().await;
    rewind_to_pre_migration(&pool).await;

    // The verified account is the evidenced winner, so this also exercises the decided branch.
    seed(&pool, "bob", "bob@corp.com", "2026-01-01T00:00:00Z", false, true).await;
    seed(&pool, "bob_padded", "\u{00A0}bob@corp.com", "2026-03-01T00:00:00Z", false, false).await;

    pool.execute(MIGRATION_SQL).await.expect("must apply");

    let winner = row_of(&pool, "bob").await;
    assert_eq!(winner.0, "bob@corp.com", "the verified account keeps the address");
    let loser = row_of(&pool, "bob_padded").await;
    assert!(loser.0.ends_with("@invalid"), "the padded twin is parked");
    assert!(
        parked_verdict(&pool, "bob_padded").await,
        "this parking WAS decided by evidence (email_verified), so the ledger must not flag \
         it for adjudication"
    );

    // The ledger holds the WHITESPACE-NORMALIZED original, because step 2 ran first.
    let original: (String,) = sqlx::query_as(
        "SELECT l.original_email FROM users_email_collision_log l \
           JOIN users u ON u.id = l.user_id WHERE u.username = 'bob_padded'",
    )
    .fetch_one(&pool)
    .await
    .expect("ledger row");
    assert_eq!(original.0, "bob@corp.com");

    assert_non_destructive(&pool, 2, 1).await;
    assert_fixed_schema(&pool).await;
    drop_db(&db).await;
}

/// THE ORDERING THAT MATTERS, and the one an earlier version of this test could not fail on.
///
/// In the attack #251 describes, the SQUATTER registers the case variant — so the squatter
/// may well be the EARLIER row. A `created_at`-only ranking then awards the mailbox to the
/// attacker and (in the first version of this migration) deactivated the legitimate account.
/// A blind audit reproduced exactly that, with the victim being the root admin.
///
/// The ranking now consults evidence first: `is_admin`, then `email_verified` — which is set
/// only by OAuth provisioning after a provider asserted the address, i.e. the one real proof
/// of mailbox control this schema holds — and only then registration order.
#[tokio::test]
async fn migration_never_parks_the_admin_even_when_the_squatter_registered_first() {
    let (pool, db) = fresh_db().await;
    rewind_to_pre_migration(&pool).await;

    // The squatter is FIRST. The admin is second.
    seed(&pool, "squatter", "BOB@corp.com", "2026-01-01T00:00:00Z", false, false).await;
    seed(&pool, "root", "bob@corp.com", "2026-02-01T00:00:00Z", true, false).await;

    pool.execute(MIGRATION_SQL)
        .await
        .expect("the migration must apply");

    let admin = row_of(&pool, "root").await;
    assert_eq!(
        admin.0, "bob@corp.com",
        "the ADMIN keeps the address even though the squatter registered first — otherwise \
         a single-admin deployment is bricked: has_admin() has no is_active filter, so \
         first-run setup still refuses, and unique_root_admin blocks promoting anyone else"
    );
    assert!(admin.1, "the admin stays active");
    assert!(admin.2, "and stays an admin");

    let squatter = row_of(&pool, "squatter").await;
    assert!(squatter.0.ends_with("@invalid"), "the squatter is parked");
    assert!(squatter.1, "but keeps their account");
    assert_non_destructive(&pool, 2, 1).await;
    assert_fixed_schema(&pool).await;
    assert!(
        parked_verdict(&pool, "squatter").await,
        "this parking was decided by EVIDENCE (is_admin), so it must not be flagged for \\
         adjudication"
    );

    drop_db(&db).await;
}

/// `email_verified` outranks registration order, because it is the only evidence of mailbox
/// control in this schema (OAuth provisioning sets it after a provider asserted the address).
#[tokio::test]
async fn migration_prefers_the_verified_address_over_the_earlier_one() {
    let (pool, db) = fresh_db().await;
    rewind_to_pre_migration(&pool).await;

    seed(&pool, "squatter", "BOB@corp.com", "2026-01-01T00:00:00Z", false, false).await;
    seed(&pool, "genuine", "bob@corp.com", "2026-02-01T00:00:00Z", false, true).await;

    pool.execute(MIGRATION_SQL)
        .await
        .expect("the migration must apply");

    let genuine = row_of(&pool, "genuine").await;
    assert_eq!(
        genuine.0, "bob@corp.com",
        "a provider-VERIFIED address beats an earlier unverified one — where exactly one \
         collider is verified this is not a guess at all"
    );
    let squatter = row_of(&pool, "squatter").await;
    assert!(squatter.0.ends_with("@invalid"));
    assert!(parked_verdict(&pool, "squatter").await, "decided by evidence");
    assert_non_destructive(&pool, 2, 1).await;
    assert_fixed_schema(&pool).await;

    drop_db(&db).await;
}

/// `is_active` is part of the evidence, so a DEACTIVATED account cannot outrank a live one.
///
/// An earlier ranking omitted it, and an audit reproduced the consequence: a BANNED squatter
/// that registered first kept the mailbox while the active legitimate account was parked.
/// Being disabled is at least as strong an anti-signal as registration order is a signal.
#[tokio::test]
async fn migration_does_not_let_a_deactivated_account_outrank_a_live_one() {
    let (pool, db) = fresh_db().await;
    rewind_to_pre_migration(&pool).await;

    seed(&pool, "banned", "BOB@corp.com", "2026-01-01T00:00:00Z", false, false).await;
    sqlx::query("UPDATE users SET is_active = false WHERE username = 'banned'")
        .execute(&pool)
        .await
        .expect("ban the squatter");
    seed(&pool, "legit", "bob@corp.com", "2026-02-01T00:00:00Z", false, false).await;

    pool.execute(MIGRATION_SQL).await.expect("must apply");

    let legit = row_of(&pool, "legit").await;
    assert_eq!(
        legit.0, "bob@corp.com",
        "the ACTIVE account keeps the address even though the banned one registered first"
    );
    assert!(legit.1);
    let banned = row_of(&pool, "banned").await;
    assert!(banned.0.ends_with("@invalid"), "the banned squatter is parked");
    assert!(
        parked_verdict(&pool, "banned").await,
        "is_active uniquely separated them, so this parking WAS decided by evidence"
    );
    assert!(!banned.1, "and the migration did not reactivate it either");

    // `assert_non_destructive` deliberately not used here: it asserts nobody is deactivated,
    // and this fixture starts with a legitimately deactivated user. The properties that
    // matter are asserted directly instead.
    let total: (i64,) = sqlx::query_as("SELECT count(*) FROM users")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(total.0, 2, "nothing deleted");
    assert_fixed_schema(&pool).await;

    drop_db(&db).await;
}

/// The non-convergence the audit reproduced: a pre-existing row holding the literal parking
/// target used to make the loop re-park forever and abort the migration permanently (every
/// retry rolls the transaction back and fails identically). The target is now salted with a
/// per-run uuid, so no pre-existing value can equal it and one pass suffices.
#[tokio::test]
async fn migration_converges_even_when_a_row_already_holds_a_parking_shaped_address() {
    let (pool, db) = fresh_db().await;
    rewind_to_pre_migration(&pool).await;

    let victim_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, created_at) \
         VALUES ('victim', 'BOB@corp.com', '2026-03-01T00:00:00Z'::timestamptz) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("seed the collider");

    // A row that already holds exactly what a naive parking scheme would write, created
    // EARLIER than the collider — the shape that made the old loop diverge.
    seed(
        &pool,
        "landmine",
        &format!("dup.{victim_id}@invalid"),
        "2026-01-01T00:00:00Z",
        false,
        false,
    )
    .await;
    seed(&pool, "bob", "bob@corp.com", "2026-02-01T00:00:00Z", false, false).await;

    pool.execute(MIGRATION_SQL)
        .await
        .expect("the migration must CONVERGE and apply — the old loop aborted here forever");

    // The landmine never collided with anything, so it is untouched.
    let landmine = row_of(&pool, "landmine").await;
    assert_eq!(
        landmine.0,
        format!("dup.{victim_id}@invalid"),
        "a bystander whose address merely LOOKS like a parking target must be untouched"
    );
    assert!(landmine.1);
    // No evidence separates bob from victim, so BOTH are parked: 2 parkings, 3 rows.
    assert_non_destructive(&pool, 3, 2).await;
    assert_fixed_schema(&pool).await;

    drop_db(&db).await;
}
