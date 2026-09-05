//! Email normalization for `users.email`.
//!
//! # The one rule this module exists to enforce
//!
//! `users.email` uniqueness is CASE-INSENSITIVE (`UNIQUE (lower(email))`, migration
//! `202609050010`). Before that migration `users_email_key` was case-SENSITIVE, so
//! `bob@corp.com` and `BOB@corp.com` were two distinct principals and both satisfied an
//! account invitation issued to `bob@corp.com` -- a total bypass of the recipient control
//! (issue #251).
//!
//! Closing it needs a normalization that CANNOT disagree with the database, and the only
//! reliable way to get that is to make sure no expression is computed on both sides:
//!
//! | responsibility | sole authority | never done by |
//! |---|---|---|
//! | case-fold | Postgres `lower()` -- in the index AND on both sides of every lookup | Rust |
//! | trim | Rust `str::trim` -- at every writer | Postgres, in the hot path |
//!
//! That split is not stylistic. Postgres `lower('\u{0130}')` yields ONE character (`U+0069`)
//! while Rust's `"\u{0130}".to_lowercase()` yields TWO (`U+0069 U+0307`) -- measured, not
//! assumed. So a Rust-lowercased value compared against a Postgres-lowercased one is wrong
//! for at least that input, and a `to_lowercase()` in this module would have introduced a new
//! divergence class while closing the old one. **Nothing here lowercases.**
//!
//! # The limit of that claim
//!
//! An earlier version of this comment concluded that the crossing therefore "does not exist".
//! That is true INSIDE this crate and false across a table boundary, and a blind audit
//! demonstrated the difference. A consumer that stores a RUST-lowercased address in its own
//! table and matches it against `users.email` is performing exactly the comparison this
//! design avoids internally. A full `0..0x10FFFF` sweep finds 56 divergent code points: the
//! spellings `U+0130` and `U+0069 U+0307` are DIFFERENT under Postgres `lower()`, so both may
//! be stored here as separate principals -- yet IDENTICAL under Rust `to_lowercase()`, so
//! both would satisfy one such binding. For those inputs #251 survives in the consumer.
//!
//! Over printable ASCII the two folds agree exactly, so a consumer whose addresses are
//! ASCII-only is unaffected. That is a constraint on the CONSUMER, not a property of this
//! module, and it must be asserted where the charset rule lives rather than assumed here.
//! (cytoanalyst does assert it: `validate_invitation_email` admits only printable ASCII, and
//! `tests/auth/email_case_insensitive.rs` proves both halves -- the folds agree on everything
//! that validator admits, and it refuses everything they disagree on.) The remedy for a
//! consumer that widens its charset is the same discipline this module follows: do not
//! case-fold in Rust; compare with `lower()` on both sides.
//!
//! `lower()` is also collation-driven. ASCII folding is collation-INVARIANT, so the same
//! ASCII-only domain is unaffected; for non-ASCII addresses the fold -- and therefore the
//! uniqueness rule -- depends on the cluster locale and can shift across a glibc/ICU upgrade
//! or a restore into a differently-collated cluster. Same residual, same tracking.
//!
//! # The single crossing, and how it is kept honest
//!
//! Migration `202609050010` has to `btrim` once -- to normalize legacy rows and for its
//! defence-in-depth CHECK. A ONE-ARGUMENT `btrim` strips ASCII SPACE ONLY, while `str::trim`
//! strips all Unicode `White_Space`; that gap is the bypass in a narrower form (a
//! `U+00A0`-padded address sitting beside its unpadded twin). The migration therefore spells
//! its character set out, and [`UNICODE_WHITESPACE`] is that same set stated in Rust.
//!
//! [`tests::migration_charset_equals_rust_trim_set`] READS THE MIGRATION FILE, extracts every
//! `\uXXXX` escape from its `btrim` character-set literals, checks all such literals in the
//! file are identical, and asserts the set equals `{c : char::is_whitespace(c)}` scanned over
//! all of `0..=0x10FFFF` -- in BOTH directions. A set smaller than Rust's would leave the
//! bypass open; a set larger would make the CHECK reject a value a writer legitimately
//! produced, turning a legal registration into a 500. Neither can pass silently.
//!
//! # What this deliberately does NOT do
//!
//! No provider-specific canonicalization (`a.b@gmail` vs `ab@gmail`, `+tag` suffixes). Case
//! folding is the only normalization considered safe across providers -- RFC 5321 SS2.4 says
//! the local-part "MUST BE treated as case sensitive" and, in the same document, that relying
//! on it "impedes interoperability and is discouraged". It also does not VALIDATE: the reader
//! must accept whatever registration already stored, so only an empty-after-trim value (which
//! is not an address on any reading) is refused.

use ziee_core::AppError;

/// Every code point Rust's [`str::trim`] removes -- i.e. exactly the code points for which
/// [`char::is_whitespace`] is true, which is the Unicode `White_Space` property.
///
/// This is the Rust statement of the character set migration `202609050010` passes to
/// `btrim`. The two are asserted EQUAL by test, in both directions; see the module docs.
/// It is a fact about Unicode, not a tunable -- an operator who could shrink it could
/// reopen #251, so it is a constant and not a settings row.
pub const UNICODE_WHITESPACE: &[char] = &[
    '\u{0009}',
    '\u{000A}',
    '\u{000B}',
    '\u{000C}',
    '\u{000D}',
    '\u{0020}',
    '\u{0085}',
    '\u{00A0}',
    '\u{1680}',
    '\u{2000}',
    '\u{2001}',
    '\u{2002}',
    '\u{2003}',
    '\u{2004}',
    '\u{2005}',
    '\u{2006}',
    '\u{2007}',
    '\u{2008}',
    '\u{2009}',
    '\u{200A}',
    '\u{2028}',
    '\u{2029}',
    '\u{202F}',
    '\u{205F}',
    '\u{3000}'
];

/// Normalize an address for STORAGE: trim only. Case is preserved verbatim.
///
/// Trimming here (and at every other writer) is what makes the `lower(email)` unique index
/// sufficient: the index does not trim, so if an untrimmed row could be written it would sit
/// beside its trimmed twin as a second principal and #251 would survive in a narrower form.
///
/// The original casing is KEPT because the display and SMTP forms want the address the user
/// typed; lowercasing happens only inside comparisons, and only in Postgres.
///
/// Errors with `INVALID_EMAIL` only when the value is empty after trimming, which is not an
/// address on any reading. It does not otherwise validate -- see the module docs.
pub fn normalize_email_for_storage(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::bad_request(
            "INVALID_EMAIL",
            "Email cannot be empty",
        ));
    }
    Ok(trimmed.to_string())
}

/// Normalize an address for LOOKUP: trim only, and never fail.
///
/// The case-fold is supplied by the SQL (`lower(email) = lower($1)`), not here. A reader must
/// not be fallible on shapes the writer already accepted, so an empty result is returned as
/// an empty string and simply matches nothing.
pub fn trim_email_for_lookup(raw: &str) -> String {
    raw.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The migration this module is paired with, read at COMPILE time so a rename is a build
    /// error rather than a test that silently stops checking anything.
    const MIGRATION_SQL: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/migrations/202609050010_users_email_case_insensitive.sql"
    ));

    fn rust_trim_set() -> BTreeSet<char> {
        (0u32..=0x10FFFF)
            .filter_map(char::from_u32)
            .filter(|c| c.is_whitespace())
            .collect()
    }

    /// Extract every `E'...'` escape-string literal from the migration.
    fn escape_string_literals(sql: &str) -> Vec<String> {
        let bytes: Vec<char> = sql.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == 'E' && bytes[i + 1] == '\'' {
                let start = i + 2;
                let mut j = start;
                while j < bytes.len() && bytes[j] != '\'' {
                    j += 1;
                }
                out.push(bytes[start..j].iter().collect());
                i = j + 1;
            } else {
                i += 1;
            }
        }
        out
    }

    fn parse_unicode_escapes(literal: &str) -> BTreeSet<char> {
        let chars: Vec<char> = literal.chars().collect();
        let mut out = BTreeSet::new();
        let mut i = 0;
        while i < chars.len() {
            assert_eq!(
                chars[i], '\\',
                "the btrim charset literal must contain ONLY \\uXXXX escapes, \
                 so a literal (invisible) whitespace character cannot hide in it; found {:?}",
                chars[i]
            );
            assert_eq!(chars[i + 1], 'u', "expected \\u escape at offset {i}");
            let hex: String = chars[i + 2..i + 6].iter().collect();
            let cp = u32::from_str_radix(&hex, 16).expect("4 hex digits after \\u");
            out.insert(char::from_u32(cp).expect("valid scalar value"));
            i += 6;
        }
        out
    }

    /// TEST-2 -- the Rust constant cannot drift from the language's own definition.
    #[test]
    fn unicode_whitespace_const_equals_char_is_whitespace() {
        let from_const: BTreeSet<char> = UNICODE_WHITESPACE.iter().copied().collect();
        assert_eq!(
            from_const.len(),
            UNICODE_WHITESPACE.len(),
            "UNICODE_WHITESPACE must not contain duplicates"
        );
        assert_eq!(
            from_const,
            rust_trim_set(),
            "UNICODE_WHITESPACE must be exactly the set char::is_whitespace accepts"
        );
    }

    /// TEST-3 (acceptance, INV-3) -- THE TRIM AGREES WITH THE INDEX.
    ///
    /// This is the test the owner ruling names as the most likely way a "fixed" version stays
    /// broken. It fails if migration `202609050010` omits `U+00A0`/`U+2009` (bypass survives)
    /// OR includes a code point Rust does not trim (the CHECK spuriously rejects a legal
    /// address), and it fails vacuously for neither: the count is pinned and every literal in
    /// the file must agree.
    #[test]
    fn migration_charset_equals_rust_trim_set() {
        let literals = escape_string_literals(MIGRATION_SQL);
        assert!(
            literals.len() >= 3,
            "expected the btrim charset literal at least 3 times (2 in the legacy-row UPDATE, \
             1 in the CHECK); found {}",
            literals.len()
        );
        let first = &literals[0];
        for (n, lit) in literals.iter().enumerate() {
            assert_eq!(
                lit, first,
                "every E'...' literal in the migration must be the SAME charset -- literal #{n} \
                 differs, so one btrim would trim a different set from another"
            );
        }

        let from_sql = parse_unicode_escapes(first);
        assert_eq!(
            from_sql.len(),
            25,
            "the Unicode White_Space set is 25 code points; a literal that parsed to {} is \
             mangled or emptied, and a subset would pass every other assertion vacuously",
            from_sql.len()
        );
        assert_eq!(
            from_sql,
            rust_trim_set(),
            "migration 202609050010's btrim charset must equal the set str::trim removes, \
             EXACTLY and in both directions -- otherwise #251 survives via a leading/trailing \
             Unicode-whitespace variant, or a legal address is rejected at insert"
        );
    }

    /// TEST-1 -- storage normalization trims, preserves case, and is idempotent.
    #[test]
    fn normalize_for_storage_trims_but_never_case_folds() {
        assert_eq!(
            normalize_email_for_storage("  Bob@Corp.COM  ").unwrap(),
            "Bob@Corp.COM",
            "casing is preserved for display/SMTP; only comparisons lower()"
        );
        // Every Unicode-whitespace code point, not just ASCII space.
        let padded = format!("\u{00A0}\u{2009}\u{3000}Bob@Corp.COM\u{2028}\u{205F}");
        assert_eq!(normalize_email_for_storage(&padded).unwrap(), "Bob@Corp.COM");
        // Idempotent.
        let once = normalize_email_for_storage(&padded).unwrap();
        assert_eq!(normalize_email_for_storage(&once).unwrap(), once);
        // Interior whitespace is NOT touched -- this trims, it does not strip.
        assert_eq!(
            normalize_email_for_storage(" a b@c.com ").unwrap(),
            "a b@c.com"
        );
    }

    /// TEST-1 -- empty / whitespace-only is refused, including whitespace no ASCII-only
    /// trim would have removed.
    #[test]
    fn normalize_for_storage_refuses_empty_and_whitespace_only() {
        assert!(normalize_email_for_storage("").is_err());
        assert!(normalize_email_for_storage("   ").is_err());
        let all_ws: String = UNICODE_WHITESPACE.iter().collect();
        assert!(
            normalize_email_for_storage(&all_ws).is_err(),
            "a string of all 25 White_Space code points is not an address"
        );
    }

    /// TEST-1 -- the lookup form trims identically and is total.
    #[test]
    fn trim_for_lookup_matches_storage_normalization() {
        for raw in [
            "  bob@corp.com  ",
            "\u{00A0}BOB@CORP.COM\u{2009}",
            "bob@corp.com",
        ] {
            assert_eq!(
                trim_email_for_lookup(raw),
                normalize_email_for_storage(raw).unwrap(),
                "lookup and storage must trim the same, or a stored row becomes unfindable"
            );
        }
        assert_eq!(trim_email_for_lookup("   "), "", "lookup never fails");
    }
}
