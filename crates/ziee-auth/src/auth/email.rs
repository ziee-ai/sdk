//! The single normalisation rule for `users.email`, and the character-set gate
//! that makes it safe.
//!
//! ── why this exists (#251) ──────────────────────────────────────────────────
//!
//! `users_email_key` is a plain `UNIQUE (email)`, i.e. byte-exact. So
//! `bob@corp.com` and `BOB@CORP.COM` were two DISTINCT principals, and — since
//! `POST /api/auth/register` is open self-registration — anyone holding a
//! leaked invitation link for `bob@corp.com` could register the case variant
//! and satisfy an accept path that binds on `lower(trim(users.email))`.
//!
//! The chosen fix is NOT a functional unique index. A `UNIQUE (lower(email))`
//! index has to have a NAME, and reserving a name is what made #283 (a
//! violation attributed by constraint name is mis-attributed when a same-shape
//! index exists under another name) and #284 (an under-qualified `DROP INDEX`
//! destroying an unrelated table's index) P1 security bugs. Instead:
//!
//!   * every write path normalises here, and
//!   * migration `202609090010` adds `CHECK (email = lower(email))`, so the
//!     normalisation is an INVARIANT the database holds rather than a
//!     convention the writers are trusted to observe, and
//!   * the pre-existing plain `UNIQUE (email)` then IS case-insensitive
//!     uniqueness, because there is no un-normalised data for it to miss.
//!
//! With no reserved name there is nothing to attribute by and nothing to drop.
//!
//! ── the character-set gate is LOAD-BEARING, not hygiene (#260) ──────────────
//!
//! Rust's `str::to_lowercase` and Postgres's `lower()` disagree on 56 code
//! points — `lower('İ')` (U+0130) is one character in Postgres and two in Rust
//! — and `lower()` is additionally collation-driven, so a C-locale cluster does
//! not fold non-ASCII at all. A value lowered by Rust could therefore FAIL the
//! Postgres `CHECK`, turning a signup into a `500` on a shipped path.
//!
//! Over printable ASCII the two folds agree exactly and are
//! collation-invariant. So the gate below refuses anything outside printable
//! ASCII **at the write boundary** rather than sanitising it: a refusal is
//! visible and reversible, a silent transformation is neither. That keeps the
//! CHECK satisfiable by construction. #260 tracks admitting non-ASCII
//! addresses properly (punycode / a stored normalised column); until it lands,
//! DO NOT widen this set — the CHECK is what would break, on the register path.
//!
//! Note the fold used is [`str::to_ascii_lowercase`], not `to_lowercase`: once
//! the gate has proven the value is ASCII the two are identical, and the ASCII
//! one cannot drift into the Unicode special-casing rules that create the
//! divergence in the first place.

use ziee_core::AppError;

/// Fold an address to its canonical stored form: outer whitespace removed,
/// ASCII letters lowercased.
///
/// Deliberately total (no `Result`) and deliberately the ONE fold in the crate:
/// [`normalize_email`] is defined in terms of it, so a lookup and a write can
/// never disagree about what "the same address" means. On input the gate would
/// reject, the fold is a partial no-op — which is correct for a lookup (no
/// stored row can contain such a value once the CHECK is in place, so it
/// simply matches nothing) and irrelevant for a write (the gate rejects first).
pub fn fold_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

/// The write-path normaliser: fold, then refuse anything the Postgres CHECK
/// could disagree with.
///
/// Returns a `400 INVALID_EMAIL`, never a sanitised value. Every path that
/// writes `users.email` calls this — including the repository layer, so a
/// consumer of this SDK that reaches a repository directly inherits the rule
/// rather than having to remember it.
pub fn normalize_email(email: &str) -> Result<String, AppError> {
    let folded = fold_email(email);

    if folded.is_empty() {
        return Err(AppError::bad_request(
            "INVALID_EMAIL",
            "Email cannot be empty",
        ));
    }

    // The `character varying(255)` column bound, counted in characters as
    // Postgres counts it. An over-long address previously reached Postgres and
    // surfaced as a generic 500 `22001 value too long`.
    if folded.chars().count() > EMAIL_MAX_CHARS {
        return Err(AppError::bad_request(
            "INVALID_EMAIL",
            format!("Email must be at most {EMAIL_MAX_CHARS} characters"),
        ));
    }

    // Printable ASCII, excluding the space (an address with an interior space
    // is not an address, and the outer ones are already gone). This is the
    // guarantee the `CHECK (email = lower(email))` invariant rests on.
    if let Some(bad) = folded.chars().find(|c| !is_admissible_email_char(*c)) {
        return Err(AppError::bad_request(
            "INVALID_EMAIL",
            format!(
                "Email must contain only printable ASCII characters (found {bad:?}). \
                 Non-ASCII addresses are not accepted yet — see issue #260."
            ),
        ));
    }

    Ok(folded)
}

/// [`normalize_email`] over an optional address — the external/OAuth
/// provisioning shape, where "no email" is a legitimate input and `None` must
/// stay `None` rather than becoming `Some("")`.
pub fn normalize_optional_email(email: Option<&str>) -> Result<Option<String>, AppError> {
    match email {
        None => Ok(None),
        Some(e) => normalize_email(e).map(Some),
    }
}

/// Upper bound, in characters. Matches `users.email character varying(255)`
/// exactly (see `migrations/202607140050_auth_schema.sql`).
pub const EMAIL_MAX_CHARS: usize = 255;

/// Printable ASCII excluding space: `!`..`~`.
fn is_admissible_email_char(c: char) -> bool {
    matches!(c, '\u{21}'..='\u{7e}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_and_outer_whitespace() {
        assert_eq!(fold_email("  BOB@CORP.COM  "), "bob@corp.com");
        assert_eq!(fold_email("Bob@Corp.Com"), "bob@corp.com");
        assert_eq!(fold_email("bob@corp.com"), "bob@corp.com");
    }

    #[test]
    fn the_fold_is_idempotent() {
        // The CHECK re-tests the stored value on every write; a non-idempotent
        // fold would make an update of an untouched row fail.
        for s in ["  BOB@CORP.COM ", "b@c", "X+tag@Y.Co"] {
            let once = fold_email(s);
            assert_eq!(fold_email(&once), once, "fold must be idempotent for {s:?}");
        }
    }

    #[test]
    fn normalize_is_the_fold_plus_a_gate() {
        // The write normaliser must never produce something the plain fold
        // would not — otherwise a lookup and a write disagree.
        assert_eq!(normalize_email(" Bob@Corp.com ").unwrap(), fold_email(" Bob@Corp.com "));
    }

    #[test]
    fn refuses_the_empty_address() {
        assert_eq!(normalize_email("   ").unwrap_err().status_code(), 400);
        assert_eq!(normalize_email("").unwrap_err().error_code(), "INVALID_EMAIL");
    }

    #[test]
    fn refuses_an_over_long_address() {
        let long = format!("{}@corp.com", "a".repeat(250));
        assert_eq!(normalize_email(&long).unwrap_err().status_code(), 400);
    }

    /// The #260 seam. Each of these is a code point on which the Rust fold and
    /// the Postgres fold are known to disagree, or a control/space character
    /// that has no business in a stored address.
    #[test]
    fn refuses_everything_outside_printable_ascii() {
        for bad in [
            "İgor@corp.com",   // U+0130 — 1 char in PG lower, 2 in Rust
            "i\u{307}gor@corp.com", // the Rust fold's own output for the above
            "ÄNNA@corp.com",   // folds differently under a C-locale cluster
            "bob\u{a0}@corp.com", // NBSP: Rust `trim` strips it, PG `btrim` does not
            "bob@corp.com\u{200e}", // bidi mark
            "bob b@corp.com",  // interior space
            "bob\n@corp.com",
            "bob\0@corp.com",
        ] {
            let err = normalize_email(bad).unwrap_err();
            assert_eq!(err.status_code(), 400, "must refuse {bad:?}");
            assert_eq!(err.error_code(), "INVALID_EMAIL", "must refuse {bad:?}");
        }
    }

    /// Every character the gate admits is unchanged by an ASCII fold applied
    /// twice, and is a character Postgres's `lower()` treats identically. The
    /// Postgres half of that claim is asserted against a live server in
    /// `tests/email_case_invariant.rs`; this is the Rust half.
    #[test]
    fn every_admissible_character_survives_the_fold() {
        for b in 0x21u8..=0x7eu8 {
            let c = b as char;
            let addr = format!("a{c}b@corp.com");
            // Only ASCII uppercase may change, and only into its ASCII lowercase.
            let want = addr.to_ascii_lowercase();
            assert_eq!(fold_email(&addr), want);
            assert_eq!(
                addr.to_lowercase(),
                want,
                "Rust's Unicode fold and its ASCII fold must agree on {c:?}"
            );
        }
    }
}
