//! Secret-handling primitives for the application.
//!
//! Two layers:
//!
//! 1. `SecretView<T>` — type-safe wrapper for secrets in memory. The
//!    `Serialize` impl always emits `"<redacted>"` so accidentally
//!    returning a `SecretView<String>` in a JSON response can't leak the
//!    value. Callers that genuinely need the plaintext (outbound HTTP
//!    clients) call `.expose_secret()`.
//!
//! 2. `encrypt_secret` / `decrypt_secret` — pgcrypto round-trip helpers
//!    used by repository write/read paths. Uses pgp_sym_encrypt with
//!    the storage_key from `SecretsConfig`. Storage is `bytea`; in-memory
//!    is `SecretView<String>`.
//!
//! Closes the cross-cutting plaintext-secret-storage finding the audit
//! flagged in 06-llm-provider F-02 (Critical), 09-llm-repository F-02
//! (High), and adjacent items.

use serde::{Deserialize, Serialize};
use ziee_core::AppError;
use sqlx::PgPool;

const MIN_STORAGE_KEY_LENGTH: usize = 32;

/// Wrapper around a secret value that refuses to serialize the plaintext.
///
/// The inner type is intentionally `String` for now — full generic
/// `T: Serialize` is overkill for the current use cases (api_key, token,
/// password). Add the generic form when a real second type shows up.
#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub struct SecretView<T> {
    inner: T,
}

impl<T> SecretView<T> {
    // Intended secret-wrapper API: constructed + read only at outbound
    // boundaries + in this module's tests today (no production caller wired
    // yet), so the lib build sees these as dead. Kept (not deleted) — this is
    // the deliberate primitive documented at the top of the module.
    #[allow(dead_code)]
    pub fn new(value: T) -> Self {
        Self { inner: value }
    }

    /// Return a reference to the plaintext. Use sparingly — only at the
    /// boundary where the secret needs to leave the process (outbound
    /// HTTP header, subprocess argv, etc.).
    #[allow(dead_code)]
    pub fn expose_secret(&self) -> &T {
        &self.inner
    }

    /// Consume the wrapper and return the plaintext.
    #[allow(dead_code)]
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T> Serialize for SecretView<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("<redacted>")
    }
}

// Note: no `JsonSchema` impl. If SecretView ever needs to appear in a
// JsonSchema-derived DTO, the recommendation is to wrap a `String` field
// at the boundary rather than baking SecretView into the schema —
// JsonSchema is for response shape, and SecretView's response shape is
// always the `<redacted>` literal which is well-modeled as `String`.

/// Encrypt a plaintext secret to a bytea blob using pgcrypto's
/// pgp_sym_encrypt. Returns Ok(Some(bytes)) on success, Ok(None) when
/// the storage key is unset (compat mode for unconfigured deployments).
/// Errors only on actual DB / encryption failure.
pub async fn encrypt_secret(
    pool: &PgPool,
    plaintext: &str,
    storage_key: Option<&str>,
) -> Result<Option<Vec<u8>>, AppError> {
    let key = match storage_key {
        Some(k) if k.len() >= MIN_STORAGE_KEY_LENGTH => k,
        Some(_) => {
            return Err(AppError::internal_error(
                "Secret storage key configured but shorter than 32 chars",
            ));
        }
        None => return Ok(None),
    };

    let row: (Vec<u8>,) = sqlx::query_as("SELECT pgp_sym_encrypt($1, $2)")
        .bind(plaintext)
        .bind(key)
        .fetch_one(pool)
        .await
        .map_err(AppError::database_error)?;

    Ok(Some(row.0))
}

/// Decrypt a bytea blob to plaintext using pgcrypto's pgp_sym_decrypt.
/// Returns Ok(plaintext) on success. Errors on DB / decryption failure
/// (typically a wrong key — fail closed and surface a generic error to
/// the caller).
pub async fn decrypt_secret(
    pool: &PgPool,
    ciphertext: &[u8],
    storage_key: &str,
) -> Result<String, AppError> {
    if storage_key.len() < MIN_STORAGE_KEY_LENGTH {
        return Err(AppError::internal_error(
            "Secret storage key shorter than 32 chars",
        ));
    }

    let row: (String,) = sqlx::query_as("SELECT pgp_sym_decrypt($1, $2)")
        .bind(ciphertext)
        .bind(storage_key)
        .fetch_one(pool)
        .await
        .map_err(AppError::database_error)?;

    Ok(row.0)
}

/// Repository-side resolver for the encrypted/plaintext dual-column
/// pattern. Prefers the encrypted column when storage_key is available
/// and decryption succeeds; otherwise falls back to the plaintext
/// column (compat mode + not-yet-backfilled rows).
///
/// Logs a `tracing::error!` if decryption fails (wrong key / corrupted
/// data) but does NOT propagate the error — the caller gets the
/// plaintext fallback (which is None on freshly-encrypted rows whose
/// plaintext column was cleared on write, so the secret simply
/// becomes unavailable rather than 500-ing every read).
pub async fn resolve_optional_secret(
    pool: &PgPool,
    encrypted: Option<Vec<u8>>,
    plaintext: Option<String>,
) -> Option<String> {
    // (Nested `if let` — equivalent to the source crate's edition-2024 let-chain;
    // the SDK workspace is edition 2021. BG-2 TRANSFORM, behaviour identical.)
    if let Some(ref enc) = encrypted {
        if let Some(key) = crate::secrets::storage_key() {
            match decrypt_secret(pool, enc, key).await {
                Ok(plain) => return Some(plain),
                Err(e) => {
                    tracing::error!(
                        error = ?e,
                        "Failed to decrypt secret column; falling back to plaintext column \
                         (likely wrong storage_key or corrupted ciphertext)"
                    );
                }
            }
        }
    }
    plaintext
}

/// Mask a secret for display: the first 4 characters followed by `***`, or
/// just `***` when the value is absent or ≤4 chars. Uses `.chars()` (not byte
/// indexing) so a multibyte/emoji-leading key never panics on a mid-UTF-8
/// boundary (closes the class of bug tracked as `06-llm-provider F-14`). This
/// is the single masking helper for every per-user API-key surface.
pub fn mask_secret(value: Option<&str>) -> String {
    match value {
        Some(key) if key.chars().count() > 4 => {
            let prefix: String = key.chars().take(4).collect();
            format!("{prefix}***")
        }
        _ => "***".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_secret_shows_first_four_then_stars() {
        assert_eq!(mask_secret(Some("sk-1234567890")), "sk-1***");
        // ≤4 chars or absent → fully masked (never leaks a short key wholesale).
        assert_eq!(mask_secret(Some("abcd")), "***");
        assert_eq!(mask_secret(Some("")), "***");
        assert_eq!(mask_secret(None), "***");
    }

    #[test]
    fn mask_secret_is_char_safe_on_multibyte() {
        // A leading multibyte char must not panic on a mid-UTF-8 byte boundary
        // (byte index 4 would split "🔑"); .chars() keeps it safe.
        assert_eq!(mask_secret(Some("🔑abcdef")), "🔑abc***");
    }

    #[test]
    fn secret_view_serializes_redacted() {
        let s = SecretView::new("sk-real-api-key-12345".to_string());
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"<redacted>\"");
        assert!(!json.contains("sk-real"));
    }

    #[test]
    fn secret_view_expose_returns_real_value() {
        let s = SecretView::new("plaintext".to_string());
        assert_eq!(s.expose_secret(), "plaintext");
    }

    #[test]
    fn secret_view_in_struct_redacts() {
        #[derive(Serialize)]
        struct Provider {
            name: String,
            api_key: SecretView<String>,
        }
        let p = Provider {
            name: "OpenAI".to_string(),
            api_key: SecretView::new("sk-secret".to_string()),
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("sk-secret"));
        assert!(json.contains("<redacted>"));
        assert!(json.contains("OpenAI"));
    }

    // encrypt_secret's two pre-DB branches — the encryption-FAILURE path that
    // repository write paths (e.g. llm_repository) propagate as
    // "secret encryption failed", and the encryption-DISABLED (no key) path.
    // Both return before touching the pool, so a lazy pool suffices.
    fn lazy_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgresql://postgres:password@127.0.0.1:54321/unused")
            .expect("lazy pool")
    }

    #[tokio::test]
    async fn encrypt_secret_errors_when_key_is_too_short() {
        let pool = lazy_pool();
        // A storage key shorter than 32 chars is a misconfiguration → Err. This
        // is exactly the failure the repository create/update paths surface as
        // "secret encryption failed" instead of silently storing plaintext.
        let err = encrypt_secret(&pool, "my-secret", Some("too-short-key"))
            .await
            .expect_err("a sub-32-char storage key must fail encryption");
        assert!(
            err.to_string().to_lowercase().contains("storage key")
                || err.to_string().to_lowercase().contains("32"),
            "error must cite the bad storage key; got: {err}"
        );
    }

    #[tokio::test]
    async fn encrypt_secret_returns_none_when_encryption_disabled() {
        let pool = lazy_pool();
        // No storage key configured → encryption is disabled, caller stores the
        // value as plaintext JSON (Ok(None)), never an error.
        let out = encrypt_secret(&pool, "my-secret", None)
            .await
            .expect("disabled encryption must not error");
        assert!(out.is_none(), "no key → Ok(None) (plaintext fallback)");
    }
}
