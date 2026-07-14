//! Process-global access to the at-rest secret storage key.
//!
//! Initialized once at server boot from `Config::secrets::storage_key`.
//! Repository code that encrypts / decrypts api_key + token + password
//! columns reads from here rather than threading the key through every
//! function signature.
//!
//! When the key is unset, the application boots in compat mode:
//! new writes go to the plaintext columns and a tracing::warn fires.
//! Closes 06-llm-provider F-02 (Critical) once configured.

use once_cell::sync::OnceCell;

static STORAGE_KEY: OnceCell<Option<String>> = OnceCell::new();

/// Initialize the global storage key. Idempotent; first call wins (a
/// second call with a different value logs a tracing::warn and ignores
/// the new value, same as init_repositories).
pub fn init_storage_key(key: Option<String>) {
    // (Nested `if let` — equivalent to the source crate's edition-2024 let-chain;
    // the SDK workspace is edition 2021. BG-2 TRANSFORM, behaviour identical.)
    if let Err(prev) = STORAGE_KEY.set(key.clone()) {
        if prev != key {
            tracing::warn!(
                "init_storage_key called twice with different values; \
                 ignoring second call"
            );
        }
    }
    if let Some(stored) = STORAGE_KEY.get() {
        match stored.as_deref() {
            Some(k) if k.len() >= 32 => {
                tracing::info!("Secret storage key configured ({} chars)", k.len());
            }
            Some(k) => {
                tracing::warn!(
                    "Secret storage key configured but only {} chars — \
                     minimum is 32. Encryption helpers will refuse to use it.",
                    k.len()
                );
            }
            None => {
                tracing::warn!(
                    "No secret storage key configured (secrets.storage_key); \
                     api_key / token / password columns will stay in plaintext. \
                     Set secrets.storage_key in your config to encrypt at rest."
                );
            }
        }
    }
}

/// Return the configured storage key. None when unset (compat mode) or
/// when init_storage_key hasn't been called yet (test contexts that
/// don't go through the full server boot).
pub fn storage_key() -> Option<&'static str> {
    STORAGE_KEY.get().and_then(|opt| opt.as_deref())
}
