//! Self-service account recovery for accounts that have no email.
//!
//! Two optional capabilities, each switched on by the `auth:` config block:
//!
//! * **recovery codes** — ten one-time codes shown once, stored as bcrypt
//!   hashes, any unused one resets the password and is consumed;
//! * **security questions** — two or three picks from a FIXED catalogue,
//!   answers normalised then hashed like a password, a reset needs all of them.
//!
//! This file is the pure half: the catalogue, the normalisation and code
//! formats, the decoy-question derivation and the limit arithmetic, all
//! testable without a database. [`repository`] is the SQL half and
//! `auth::http::recovery` is the HTTP half.
//!
//! ## What the module refuses to reveal
//!
//! A reset answers one body for every failure: an unknown username, an account
//! with nothing configured, a wrong code, a wrong answer (never which one) and
//! a deactivated account are indistinguishable. The only different answer is
//! the lockout, and that is keyed by what the CALLER typed, so it exists for a
//! username that matches nobody exactly as it does for one that does.

use sha2::{Digest, Sha256};

pub mod repository;
pub mod types;

use std::net::IpAddr;
use std::sync::{Arc, OnceLock};

pub use repository::RecoveryRepository;

// ───────────────────────────── recovery codes ─────────────────────────────

/// Codes in a freshly generated set.
pub const CODES_PER_SET: usize = 10;
/// Symbols per code (60 bits from a 32-symbol alphabet).
pub const CODE_LEN: usize = 12;
/// bcrypt cost for code hashes. The codes carry 60 bits, so the default cost
/// buys nothing against guessing and would make the ten-hash scan a reset
/// performs slow enough to hurt.
pub const CODE_BCRYPT_COST: u32 = 10;

/// Crockford base32: no `I`, `L`, `O` or `U`, so a code read off paper cannot be
/// confused with a digit or a rude word.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A new random code, formatted `XXXX-XXXX-XXXX`.
pub fn generate_code() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let mut out = String::with_capacity(CODE_LEN + 2);
    for i in 0..CODE_LEN {
        if i > 0 && i % 4 == 0 {
            out.push('-');
        }
        out.push(ALPHABET[rng.random_range(0..ALPHABET.len())] as char);
    }
    out
}

/// The canonical form a code is hashed and compared in: uppercase, separators
/// dropped, and Crockford's confusable letters folded (`I`,`L` to `1`; `O` to
/// `0`). `None` when the input cannot be a code at all (wrong length or a
/// character outside the alphabet).
pub fn normalize_code(input: &str) -> Option<String> {
    let mut out = String::with_capacity(CODE_LEN);
    for c in input.chars() {
        let c = c.to_ascii_uppercase();
        if c == '-' || c.is_whitespace() {
            continue;
        }
        let c = match c {
            'I' | 'L' => '1',
            'O' => '0',
            other => other,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == CODE_LEN).then_some(out)
}

// ───────────────────────────── security questions ─────────────────────────────

/// A question in the fixed catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogueQuestion {
    pub key: &'static str,
    pub prompt: &'static str,
}

/// The fixed catalogue. There is deliberately no free-text question: a custom
/// question lets the question carry its own answer and hands an attacker a field
/// to craft. Keys are stable identifiers; prompts may be reworded.
pub const QUESTIONS: &[CatalogueQuestion] = &[
    CatalogueQuestion { key: "first_pet", prompt: "What was the name of your first pet?" },
    CatalogueQuestion { key: "childhood_street", prompt: "What street did you grow up on?" },
    CatalogueQuestion { key: "first_school", prompt: "What was the name of your first school?" },
    CatalogueQuestion { key: "first_job", prompt: "What was your first job?" },
    CatalogueQuestion { key: "favourite_teacher", prompt: "Who was your favourite teacher?" },
    CatalogueQuestion { key: "childhood_nickname", prompt: "What was your childhood nickname?" },
    CatalogueQuestion { key: "first_concert", prompt: "What was the first concert you attended?" },
    CatalogueQuestion { key: "first_phone", prompt: "What was the model of your first phone?" },
    CatalogueQuestion { key: "grandmother_maiden", prompt: "What was your maternal grandmother's maiden name?" },
    CatalogueQuestion { key: "first_car", prompt: "What was the make of your first car?" },
    CatalogueQuestion { key: "favourite_book", prompt: "What was your favourite book as a child?" },
    CatalogueQuestion { key: "met_best_friend", prompt: "In what town did you meet your best friend?" },
];

/// Fewest and most questions a user may configure.
pub const MIN_QUESTIONS: usize = 2;
pub const MAX_QUESTIONS: usize = 3;
/// Normalised-answer length bounds, in bytes. The upper bound is bcrypt's input
/// limit with margin; the lower bound refuses one-letter answers.
pub const MIN_ANSWER_BYTES: usize = 3;
pub const MAX_ANSWER_BYTES: usize = 64;

/// The catalogue entry for `key`.
pub fn question(key: &str) -> Option<&'static CatalogueQuestion> {
    QUESTIONS.iter().find(|q| q.key == key)
}

/// The canonical form of an answer: trimmed, Unicode-lowercased, runs of
/// whitespace collapsed to one space. Idempotent.
pub fn normalize_answer(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Why an answer set was refused, as a client-safe message.
pub fn validate_answers(picks: &[(&str, String)]) -> Result<(), &'static str> {
    if !(MIN_QUESTIONS..=MAX_QUESTIONS).contains(&picks.len()) {
        return Err("Choose two or three security questions");
    }
    for (i, (key, answer)) in picks.iter().enumerate() {
        if question(key).is_none() {
            return Err("Unknown security question");
        }
        if picks[..i].iter().any(|(k, _)| k == key) {
            return Err("Choose each question only once");
        }
        let n = normalize_answer(answer);
        if n.len() < MIN_ANSWER_BYTES {
            return Err("Each answer must be at least 3 characters");
        }
        if n.len() > MAX_ANSWER_BYTES {
            return Err("Each answer must be at most 64 characters");
        }
        if picks[..i].iter().any(|(_, a)| normalize_answer(a) == n) {
            return Err("Each answer must be different");
        }
    }
    Ok(())
}

// ───────────────────────────── decoys ─────────────────────────────

/// Key material for [`decoy_questions`], derived from the JWT secret.
pub fn derive_pepper(jwt_secret: &str) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(b"ziee-auth/recovery/decoy/v1");
    h.update(jwt_secret.as_bytes());
    h.finalize().to_vec()
}

/// The question list shown for a username that has no real one (it matches
/// nobody, or the account has none configured). Deterministic per username and
/// pepper, so asking twice gives the same answer; two or three entries so its
/// shape matches a real list.
pub fn decoy_questions(pepper: &[u8], username: &str) -> Vec<&'static CatalogueQuestion> {
    let mut h = Sha256::new();
    h.update(pepper);
    // The EXACT trimmed name, never case-folded: usernames are case-sensitive,
    // so folding here would give `alice` and `ALICE` the same decoy while only
    // one of them exists, and the pair would tell the two apart.
    h.update(username.trim().as_bytes());
    let d = h.finalize();
    let want = MIN_QUESTIONS + (d[0] as usize % (MAX_QUESTIONS - MIN_QUESTIONS + 1));
    let mut picked: Vec<&'static CatalogueQuestion> = Vec::with_capacity(want);
    let mut i = 1;
    while picked.len() < want {
        let q = &QUESTIONS[d[i] as usize % QUESTIONS.len()];
        if !picked.iter().any(|p| p.key == q.key) {
            picked.push(q);
        }
        i += 1;
        if i >= d.len() {
            // 31 bytes cover 3 distinct picks from 12 with overwhelming
            // probability; fall back to filling in catalogue order rather than
            // loop on a pathological digest.
            for q in QUESTIONS {
                if picked.len() == want {
                    break;
                }
                if !picked.iter().any(|p| p.key == q.key) {
                    picked.push(q);
                }
            }
        }
    }
    picked
}

// ───────────────────────────── client address ─────────────────────────────

/// How the app wants "who is calling" decided for the per-address limit.
///
/// An app that sits behind a CDN and a proxy has its own de-proxying rule (how
/// many trusted hops to skip, how an IPv6 address collapses to a subject); the
/// SDK cannot know it. The app installs a resolver once at boot; without one the
/// SDK uses the rightmost `X-Forwarded-For` entry when
/// `server.trust_forwarded_headers` is on, else the peer address.
pub trait ClientAddressResolver: Send + Sync {
    /// The string a per-address counter is keyed on, or `None` when no address
    /// is known (the per-address limit is then skipped; the per-name one stays).
    fn client_key(&self, headers: &http::HeaderMap, peer: Option<IpAddr>) -> Option<String>;
}

static ADDRESS_RESOLVER: OnceLock<Arc<dyn ClientAddressResolver>> = OnceLock::new();

/// Install the app's client-address rule. Call once at boot; the first install
/// wins and a second is ignored with a warning.
pub fn install_client_address_resolver(resolver: Arc<dyn ClientAddressResolver>) {
    if ADDRESS_RESOLVER.set(resolver).is_err() {
        tracing::warn!("ziee-auth: a client-address resolver was already installed; the FIRST stands");
    }
}

/// The key the per-address counter uses for this request.
pub(crate) fn client_key(headers: &http::HeaderMap, peer: Option<IpAddr>) -> Option<String> {
    if let Some(r) = ADDRESS_RESOLVER.get() {
        return r.client_key(headers, peer);
    }
    if crate::auth::trust_forwarded_headers()
        && let Some(v) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(last) = v.rsplit(',').next()
        && let Ok(ip) = last.trim().parse::<IpAddr>()
    {
        return Some(ip.to_string());
    }
    peer.map(|p| p.to_string())
}

// ───────────────────────────── limits ─────────────────────────────

/// A counter as stored: attempts so far in the current window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counter {
    pub failures: i32,
    pub window_started_at: chrono::DateTime<chrono::Utc>,
}

/// What reserving one attempt at `now` does to `prev`: the new count, and
/// whether the attempt may proceed (the count has not passed `max_attempts`).
/// Pure mirror of the SQL in [`RecoveryRepository::begin_attempt`] for the
/// within-window and window-expiry arithmetic, kept so it is asserted without a
/// database and the SQL has a reference to be tested against.
pub fn next_counter(
    prev: Option<Counter>,
    now: chrono::DateTime<chrono::Utc>,
    window_minutes: i64,
    max_attempts: i32,
) -> (Counter, bool) {
    let window = chrono::Duration::minutes(window_minutes);
    let next = match prev {
        Some(p) if now - p.window_started_at < window => Counter {
            failures: p.failures + 1,
            window_started_at: p.window_started_at,
        },
        _ => Counter { failures: 1, window_started_at: now },
    };
    (next, next.failures <= max_attempts)
}

/// The key a 'name' counter is stored under: the EXACT trimmed name.
///
/// Never case-folded. Usernames are case-sensitive (`alice` and `Alice` are two
/// accounts), so folding would give two accounts one counter, and a SUCCESS on
/// the attacker's own `Alice` (which clears the counter) would launder attempts
/// against the victim's `alice`.
pub fn name_key(username: &str) -> String {
    username.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    /// TEST-26: normalisation, code format, decoy stability, limit arithmetic.
    #[test]
    fn answers_normalise_idempotently_and_fold_case_and_space() {
        assert_eq!(normalize_answer("  Fluffy   The  CAT "), "fluffy the cat");
        for raw in ["  Fluffy   The  CAT ", "ÉCOLE  Primaire", "x"] {
            let once = normalize_answer(raw);
            assert_eq!(normalize_answer(&once), once, "idempotent for {raw:?}");
        }
        assert_eq!(normalize_answer("\tA\n b\u{00a0}c"), "a b c");
    }

    #[test]
    fn codes_are_well_formed_without_ambiguous_symbols() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..500 {
            let c = generate_code();
            assert_eq!(c.len(), CODE_LEN + 2, "{c}");
            assert_eq!(c.matches('-').count(), 2);
            assert!(
                c.chars().all(|ch| ch == '-' || ALPHABET.contains(&(ch as u8))),
                "{c}"
            );
            assert!(!c.contains(['I', 'L', 'O', 'U']));
            assert!(seen.insert(c), "500 random 60-bit codes must not collide");
        }
    }

    #[test]
    fn codes_match_regardless_of_dashes_case_and_confusables() {
        let c = generate_code();
        let canon = normalize_code(&c).expect("a generated code normalises");
        assert_eq!(canon.len(), CODE_LEN);
        assert_eq!(normalize_code(&c.to_lowercase()).as_deref(), Some(canon.as_str()));
        assert_eq!(normalize_code(&c.replace('-', " ")).as_deref(), Some(canon.as_str()));
        assert_eq!(normalize_code("0000-0000-0000"), normalize_code("OOOO-oooo-0000"));
        assert_eq!(normalize_code("1111-1111-1111"), normalize_code("IIII-llll-1111"));
        assert_eq!(normalize_code("short"), None);
        assert_eq!(normalize_code("UUUU-UUUU-UUUU"), None, "U is not in the alphabet");
        assert_eq!(normalize_code("0000-0000-0000-0"), None, "too long");
    }

    #[test]
    fn answer_validation_enforces_count_distinctness_and_bounds() {
        let ok = vec![("first_pet", "Rex".to_string()), ("first_job", "Cashier".to_string())];
        assert!(validate_answers(&ok).is_ok());
        assert!(validate_answers(&ok[..1]).is_err(), "one question is too few");
        let four: Vec<_> = QUESTIONS.iter().take(4).map(|q| (q.key, "answer".to_string() + q.key)).collect();
        assert!(validate_answers(&four).is_err(), "four is too many");
        let dup = vec![("first_pet", "Rex".to_string()), ("first_pet", "Max".to_string())];
        assert!(validate_answers(&dup).is_err());
        let unknown = vec![("first_pet", "Rex".to_string()), ("not_a_key", "Max".to_string())];
        assert!(validate_answers(&unknown).is_err());
        let short = vec![("first_pet", "Rex".to_string()), ("first_job", "ab".to_string())];
        assert!(validate_answers(&short).is_err());
        let long = vec![("first_pet", "Rex".to_string()), ("first_job", "x".repeat(65))];
        assert!(validate_answers(&long).is_err());
        let same = vec![("first_pet", "Rex  Jr".to_string()), ("first_job", " rex jr ".to_string())];
        assert!(validate_answers(&same).is_err(), "the same normalised answer twice");
    }

    #[test]
    fn decoys_are_stable_per_username_and_vary_across_usernames() {
        let pepper = derive_pepper("0123456789abcdef0123456789abcdef-strong");
        let a1 = decoy_questions(&pepper, "ghost");
        let a2 = decoy_questions(&pepper, "  ghost ");
        assert_eq!(
            a1.iter().map(|q| q.key).collect::<Vec<_>>(),
            a2.iter().map(|q| q.key).collect::<Vec<_>>(),
            "same name, same decoy (padding trimmed)"
        );
        let upper = decoy_questions(&pepper, "GHOST");
        let upper_keys: Vec<_> = upper.iter().map(|q| q.key).collect();
        let lower_keys: Vec<_> = a1.iter().map(|q| q.key).collect();
        // Usernames are case-sensitive, so a decoy must not equate two spellings
        // (spot check over several names: at least one pair must differ).
        let differs = ["ghost", "alice", "bob", "carol"].iter().any(|n| {
            decoy_questions(&pepper, n).iter().map(|q| q.key).collect::<Vec<_>>()
                != decoy_questions(&pepper, &n.to_uppercase()).iter().map(|q| q.key).collect::<Vec<_>>()
        });
        assert!(differs, "case must be part of the decoy's input");
        let _ = (upper_keys, lower_keys);
        assert!((MIN_QUESTIONS..=MAX_QUESTIONS).contains(&a1.len()));
        let mut distinct = std::collections::HashSet::new();
        for n in 0..40 {
            let d = decoy_questions(&pepper, &format!("user{n}"));
            assert!((MIN_QUESTIONS..=MAX_QUESTIONS).contains(&d.len()));
            let keys: std::collections::HashSet<_> = d.iter().map(|q| q.key).collect();
            assert_eq!(keys.len(), d.len(), "no repeated question in one decoy");
            distinct.insert(d.iter().map(|q| q.key).collect::<Vec<_>>());
        }
        assert!(distinct.len() > 10, "decoys must vary across usernames");
        let other = derive_pepper("another-secret-another-secret-another");
        assert_ne!(
            decoy_questions(&pepper, "ghost").iter().map(|q| q.key).collect::<Vec<_>>(),
            decoy_questions(&other, "ghost").iter().map(|q| q.key).collect::<Vec<_>>(),
            "a different pepper must give a different decoy (spot check)"
        );
    }

    #[test]
    fn the_limit_admits_exactly_the_configured_count() {
        let t0 = Utc::now();
        let mut prev = None;
        for n in 1..=8 {
            let (c, allowed) = next_counter(prev, t0, 15, 5);
            assert_eq!(c.failures, n);
            assert_eq!(allowed, n <= 5, "attempts 1..=5 proceed, the sixth does not");
            prev = Some(c);
        }
    }

    #[test]
    fn an_attempt_after_the_window_starts_a_new_count() {
        let t0 = Utc::now();
        let (c, _) = next_counter(None, t0, 15, 5);
        let (c, _) = next_counter(Some(c), t0 + Duration::minutes(5), 15, 5);
        assert_eq!(c.failures, 2);
        let (c, allowed) = next_counter(Some(c), t0 + Duration::minutes(16), 15, 5);
        assert_eq!(c.failures, 1, "the old window expired");
        assert!(allowed);
    }

    #[test]
    fn the_name_key_is_the_exact_trimmed_name() {
        assert_eq!(name_key("  Alice "), "Alice");
        assert_ne!(name_key("alice"), name_key("Alice"), "two accounts, two counters");
    }
}
