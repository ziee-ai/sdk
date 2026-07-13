// Base application state globals (moved from ziee `core/app_state.rs`).
// Provides access to app-wide configuration like data_dir.
//
// The `~/.ziee` default is now derived from a configurable APP NAME (T-1) so
// the SDK is app-agnostic; the consuming app calls `set_app_name(..)` (ziee
// passes "ziee", preserving the `~/.ziee` default byte-for-byte).

use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Configurable application name used to derive the default data directory
/// (`~/.<app_name>`). Set once at process start by the consuming app; if unset
/// the default falls back to `"app"`. ziee sets `"ziee"` so the default data
/// dir stays `~/.ziee`, identical to the pre-extraction behavior. (T-1)
static APP_NAME: OnceLock<String> = OnceLock::new();

/// Set the application name used to derive the default data directory.
/// Idempotent (first setter wins); must run before the first
/// `get_app_data_dir`/`set_app_data_dir` for the default to reflect it.
pub fn set_app_name(name: impl Into<String>) {
    let _ = APP_NAME.set(name.into());
}

fn app_name() -> String {
    APP_NAME
        .get()
        .cloned()
        .unwrap_or_else(|| "app".to_string())
}

/// Global application data directory
/// This stores models, caches, temporary files, etc.
pub static APP_DATA_DIR: Lazy<Mutex<PathBuf>> = Lazy::new(|| {
    // Default to ~/.<app_name> if not set (was hardcoded ~/.ziee — T-1).
    let default_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(format!(".{}", app_name()));
    Mutex::new(default_path)
});

/// Set the application data directory.
///
/// Closes 14-core F-25 + F-17 (Info / Low): poison recovery rather than
/// silent log-and-continue. If a previous holder panicked while
/// mutating APP_DATA_DIR, recover the inner value and overwrite it.
/// The data dir is set-once at boot so the recovery branch should
/// never fire in practice.
pub fn set_app_data_dir(path: PathBuf) {
    let mut guard = APP_DATA_DIR.lock().unwrap_or_else(|poisoned| {
        tracing::error!("APP_DATA_DIR mutex poisoned in set_app_data_dir; recovering");
        poisoned.into_inner()
    });
    *guard = path;
    tracing::info!("Application data directory set to: {}", guard.display());
}

/// Get the current application data directory.
/// Returns a cloned PathBuf to avoid holding the mutex lock.
/// Poison recovery same as set_app_data_dir (14-core F-17 + F-25).
pub fn get_app_data_dir() -> PathBuf {
    APP_DATA_DIR
        .lock()
        .unwrap_or_else(|poisoned| {
            tracing::error!("APP_DATA_DIR mutex poisoned in get_app_data_dir; recovering");
            poisoned.into_inner()
        })
        .clone()
}

/// Server-side host + port + api_prefix, captured from `Config::server`
/// at boot. Used by the llm_local_runtime URL injection to derive the
/// proxy base_url at read time. The api_prefix matters because module
/// routes are nested under it (`app_builder.rs`), so the externally
/// reachable proxy URL is `http://host:port{api_prefix}/local-llm/v1`.
/// Default `("127.0.0.1", 3000, "/api")` so pre-boot reads work in tests.
pub static SERVER_ADDR: Lazy<Mutex<(String, u16, String)>> =
    Lazy::new(|| Mutex::new(("127.0.0.1".to_string(), 3000, "/api".to_string())));

#[allow(dead_code)]
pub fn set_server_addr(host: String, port: u16, api_prefix: String) {
    let mut guard = SERVER_ADDR.lock().unwrap_or_else(|p| p.into_inner());
    *guard = (host, port, api_prefix);
}

pub fn get_server_addr() -> (String, u16, String) {
    SERVER_ADDR
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_data_dir() {
        let test_path = PathBuf::from("/tmp/test-ziee");
        set_app_data_dir(test_path.clone());
        let retrieved_path = get_app_data_dir();
        assert_eq!(retrieved_path, test_path);
    }
}
