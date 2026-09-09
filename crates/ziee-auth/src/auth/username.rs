//! Shared username + display-name validation.
//!
//! `users.username` is `character varying(100) NOT NULL` (see
//! `migrations/202607140050_auth_schema.sql`). Before this module the bound was
//! enforced on exactly ONE of the six write paths (first-run `setup/admin`), so
//! every other path let an over-long or structurally-junk username reach
//! Postgres, where a `22001 value too long` surfaced to the client as a generic
//! 500 `SYSTEM_DATABASE_ERROR` — and a username like
//! `admin' OR '1'='1; DROP TABLE users;--` was accepted and persisted outright,
//! locking the account out of login.
//!
//! This is the single source of truth every user-settable username path calls,
//! mirroring `auth::password::validate_password_strength`.
//!
//! NOTE the rules are deliberately *write-side only*: login and the external
//! IdP provisioning paths do NOT run them, so pre-existing rows (and
//! SSO-derived usernames) keep working — a user simply cannot *set* a
//! non-conforming username going forward.

use ziee_core::AppError;

/// Lower bound, in characters. Matches the long-standing `setup/admin` rule.
pub const USERNAME_MIN_CHARS: usize = 3;

/// Upper bound, in characters. Matches the `character varying(100)` column
/// exactly — Postgres bounds varchar by CHARACTERS, so this is counted in
/// `chars()`, not bytes (a byte bound would spuriously reject a legitimate
/// 40-character CJK username).
pub const USERNAME_MAX_CHARS: usize = 100;

/// Upper bound for `display_name`, in characters. Matches the
/// `users.display_name character varying(255)` column exactly, and is counted
/// in `chars()` for the same reason as [`USERNAME_MAX_CHARS`].
pub const DISPLAY_NAME_MAX_CHARS: usize = 255;

/// True for Unicode bidirectional-control / zero-width / format characters that
/// `char::is_control()` (category Cc only) misses. These enable Trojan-source /
/// homoglyph display spoofing (e.g. U+202E RIGHT-TO-LEFT OVERRIDE reordering an
/// admin's visible username), so they are rejected in user/display names.
pub fn is_bidi_or_zero_width(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}'   // zero-width space/joiner/non-joiner + LRM/RLM
        | '\u{202A}'..='\u{202E}' // bidi embeddings + LRO/RLO override
        | '\u{2060}'..='\u{2064}' // word joiner + invisible operators
        | '\u{2066}'..='\u{2069}' // bidi isolates
        | '\u{061C}'              // arabic letter mark
        | '\u{FEFF}'              // zero-width no-break space / BOM
    )
}

/// True for the punctuation a username may contain in addition to
/// alphanumerics. `.`/`_`/`-` are the conventional separators; `@`/`+` are
/// allowed so deployments that use an email address as the login identifier
/// keep working.
fn is_allowed_username_punctuation(c: char) -> bool {
    matches!(c, '.' | '_' | '-' | '@' | '+')
}

/// Validate a user-supplied username.
///
/// The caller is expected to have trimmed the value already (whitespace is
/// rejected outright, so an untrimmed value fails rather than being silently
/// accepted).
///
/// Charset is a positive allowlist — Unicode alphanumerics plus
/// `. _ - @ +` — so international usernames work while quotes, semicolons,
/// angle brackets, backslashes and comment markers do not. That is what makes
/// `admin' OR '1'='1; DROP TABLE users;--` a 400 rather than a stored row.
/// (Parameterised queries already prevent the injection from *executing*; this
/// rule is about not persisting garbage that breaks the account.)
pub fn validate_username(username: &str) -> Result<(), AppError> {
    let char_count = username.chars().count();

    if char_count < USERNAME_MIN_CHARS || char_count > USERNAME_MAX_CHARS {
        return Err(AppError::bad_request(
            "INVALID_USERNAME",
            format!("Username must be {USERNAME_MIN_CHARS}-{USERNAME_MAX_CHARS} characters"),
        ));
    }

    if username
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || is_bidi_or_zero_width(c))
    {
        return Err(AppError::bad_request(
            "INVALID_USERNAME",
            "Username cannot contain whitespace or control characters",
        ));
    }

    if let Some(bad) = username
        .chars()
        .find(|c| !c.is_alphanumeric() && !is_allowed_username_punctuation(*c))
    {
        return Err(AppError::bad_request(
            "INVALID_USERNAME",
            format!(
                "Username cannot contain '{bad}' — only letters, digits and . _ - @ + are allowed"
            ),
        ));
    }

    Ok(())
}

/// Validate a user-supplied display name.
///
/// Unlike a username this is free-form prose (spaces, punctuation and any
/// script are fine), so the rules are only the two the storage layer and the
/// renderer actually require:
///
/// * **length** — `users.display_name` is `character varying(255)`, and every
///   write path before this function left the bound entirely to Postgres. A
///   256-character name therefore raised `22001 value too long` which
///   `AppError::database_error` flattened into a generic 500
///   `SYSTEM_DATABASE_ERROR` on `POST /api/auth/profile` and `POST /api/users`.
/// * **control / bidi / zero-width characters** — U+0000 in particular cannot
///   be stored in a Postgres text column at all (`22021 invalid byte sequence`,
///   another 500), and the bidi overrides are the same Trojan-source display
///   spoof [`validate_username`] rejects.
///
/// The caller is expected to have trimmed the value; a blank name is the
/// caller's "clear it" signal, not an error, so emptiness is NOT rejected here.
pub fn validate_display_name(display_name: &str) -> Result<(), AppError> {
    if display_name.chars().count() > DISPLAY_NAME_MAX_CHARS {
        return Err(AppError::bad_request(
            "INVALID_DISPLAY_NAME",
            format!("Display name must be at most {DISPLAY_NAME_MAX_CHARS} characters"),
        ));
    }

    if display_name
        .chars()
        .any(|c| c.is_control() || is_bidi_or_zero_width(c))
    {
        return Err(AppError::bad_request(
            "INVALID_DISPLAY_NAME",
            "Display name cannot contain control or text-direction characters",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_of(u: &str) -> AppError {
        validate_username(u).expect_err("expected rejection")
    }

    fn display_err_of(d: &str) -> AppError {
        validate_display_name(d).expect_err("expected rejection")
    }

    // ─── display name ───────────────────────────────────────────

    #[test]
    fn display_name_accepts_free_form_prose() {
        for d in [
            "",
            "Admin User",
            "Ada Lovelace-Byron, Jr.",
            "山田 太郎",
            "😀 emoji name",
        ] {
            assert!(validate_display_name(d).is_ok(), "input {d:?}");
        }
    }

    #[test]
    fn display_name_accepts_exactly_the_column_bound() {
        assert!(validate_display_name(&"d".repeat(DISPLAY_NAME_MAX_CHARS)).is_ok());
    }

    #[test]
    fn display_name_rejects_over_the_column_bound() {
        // The reproduced 500: 256 chars overflowed varchar(255) as a raw 22001.
        for d in ["d".repeat(DISPLAY_NAME_MAX_CHARS + 1), "d".repeat(5000)] {
            let e = display_err_of(&d);
            assert_eq!(e.error_code(), "INVALID_DISPLAY_NAME");
            assert_eq!(e.status_code(), 400);
        }
    }

    #[test]
    fn display_name_bound_is_counted_in_characters_not_bytes() {
        // 255 astral chars = 1020 bytes. A byte bound would wrongly reject this
        // even though varchar(255) stores it fine.
        let astral = "😀".repeat(DISPLAY_NAME_MAX_CHARS);
        assert_eq!(astral.chars().count(), DISPLAY_NAME_MAX_CHARS);
        assert!(astral.len() > DISPLAY_NAME_MAX_CHARS);
        assert!(validate_display_name(&astral).is_ok());
    }

    #[test]
    fn display_name_rejects_nul_and_other_control_characters() {
        // U+0000 cannot be stored in a Postgres text column at all — it was a
        // raw 22021 -> 500 on every display-name write path.
        for d in ["abc\u{0}def", "line\u{7}bell", "a\nb"] {
            let e = display_err_of(d);
            assert_eq!(e.error_code(), "INVALID_DISPLAY_NAME", "input {d:?}");
            assert_eq!(e.status_code(), 400, "input {d:?}");
        }
    }

    #[test]
    fn display_name_rejects_bidi_and_zero_width_spoofs() {
        for d in ["spoof\u{202E}ed", "zero\u{200B}width"] {
            assert_eq!(display_err_of(d).error_code(), "INVALID_DISPLAY_NAME");
        }
    }

    // ─── length bound ───────────────────────────────────────────

    #[test]
    fn rejects_empty_and_under_minimum() {
        for u in ["", "a", "ab"] {
            let e = err_of(u);
            assert_eq!(e.error_code(), "INVALID_USERNAME", "input {u:?}");
            assert_eq!(e.status_code(), 400, "input {u:?}");
        }
    }

    #[test]
    fn accepts_minimum_and_maximum_lengths() {
        assert!(validate_username(&"a".repeat(USERNAME_MIN_CHARS)).is_ok());
        assert!(
            validate_username(&"a".repeat(USERNAME_MAX_CHARS)).is_ok(),
            "exactly-at-cap username must be accepted"
        );
    }

    #[test]
    fn rejects_over_maximum() {
        // The D2/D4 500-producer: >100 chars overflowed varchar(100).
        let e = err_of(&"a".repeat(USERNAME_MAX_CHARS + 1));
        assert_eq!(e.error_code(), "INVALID_USERNAME");
        assert_eq!(e.status_code(), 400);
        assert!(err_of(&"z".repeat(500)).status_code() == 400);
    }

    #[test]
    fn bounds_are_counted_in_characters_not_bytes() {
        // 100 CJK chars = 300 bytes. A byte bound would wrongly reject this;
        // varchar(100) counts characters, so it must be accepted.
        let cjk = "\u{4e2d}".repeat(USERNAME_MAX_CHARS);
        assert!(cjk.len() > USERNAME_MAX_CHARS, "precondition: multibyte");
        assert!(validate_username(&cjk).is_ok());

        // ...and 101 of them must still be rejected.
        assert_eq!(
            err_of(&"\u{4e2d}".repeat(USERNAME_MAX_CHARS + 1)).status_code(),
            400
        );
    }

    // ─── charset ────────────────────────────────────────────────

    #[test]
    fn rejects_the_injection_shaped_username_from_the_field_report() {
        // The exact string the automated explorer persisted, which locked the
        // admin account out of login.
        let e = err_of("admin' OR '1'='1; DROP TABLE users;--");
        assert_eq!(e.error_code(), "INVALID_USERNAME");
        assert_eq!(e.status_code(), 400);
    }

    #[test]
    fn rejects_injection_shape_even_without_whitespace() {
        // Negative control for the whitespace rule: the charset allowlist —
        // not the whitespace check — must be what rejects this.
        let e = err_of("admin'OR'1'='1;--");
        assert_eq!(e.error_code(), "INVALID_USERNAME");
        assert!(
            format!("{e:?}").contains("only letters, digits"),
            "expected the charset branch (not the whitespace branch) to fire, got: {e:?}"
        );
    }

    #[test]
    fn rejects_whitespace_control_and_bidi() {
        for u in [
            "has space",
            "tab\there",
            "new\nline",
            "trailing ",
            " leading",
            "rtl\u{202E}override",
            "zero\u{200B}width",
        ] {
            assert_eq!(err_of(u).error_code(), "INVALID_USERNAME", "input {u:?}");
        }
    }

    #[test]
    fn rejects_markup_and_shell_metacharacters() {
        for u in [
            "<script>alert(1)</script>",
            "user;rm",
            "user`id`",
            "user$(id)",
            "back\\slash",
            "quote\"mark",
            "pipe|char",
        ] {
            assert_eq!(err_of(u).error_code(), "INVALID_USERNAME", "input {u:?}");
        }
    }

    #[test]
    fn accepts_realistic_usernames() {
        for u in [
            "admin",
            "alice.smith",
            "bob_jones",
            "carol-doe",
            "dave+tag@example.com",
            "user123",
            "\u{4e2d}\u{6587}\u{7528}\u{6237}", // international
            "\u{0414}\u{043C}\u{0438}\u{0442}\u{0440}\u{0438}\u{0439}",
        ] {
            assert!(validate_username(u).is_ok(), "should accept {u:?}");
        }
    }
}
