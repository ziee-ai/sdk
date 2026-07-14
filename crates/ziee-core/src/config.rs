//! Framework-level server configuration (`ServerConfig`) — the Config split.
//!
//! Chunk B2 fleshes out the B1 placeholder. `ServerConfig` groups the
//! **app-agnostic** server settings — `postgresql` / `server` (host/port/CORS/
//! rate-limit) / `logging` / `jwt` — that every framework consumer needs
//! regardless of which domain modules it links. ziee's monolithic `Config`
//! composes this via `#[serde(flatten)]` + `Deref`, so the serialized (YAML)
//! shape stays byte-identical and every `config.postgresql` / `config.server` /
//! `config.jwt` call site keeps working unchanged.
//!
//! `ModuleContext` (in `ziee-framework`) carries `Arc<ServerConfig>`, never the
//! app's monolithic `Config` — the app injects its full config through the
//! opaque `app_config` slot instead (see `ziee-framework::ModuleContext`).
//!
//! NAMING: ziee's former `ServerConfig` (host/port/api_prefix/…) is the HTTP
//! server sub-config; it is renamed `HttpServerConfig` here so the grouping
//! type can own the `ServerConfig` name. ziee re-exports both.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Minimum acceptable JWT secret length (bytes). HMAC-SHA256 ideally uses ≥ 32
/// bytes of entropy. Mirrors ziee-auth's `JwtService::try_new` guard so an app
/// using the SDK config loader gets the same refusal without depending on the
/// auth crate.
pub const MIN_JWT_SECRET_LEN: usize = 32;

/// Known shipped placeholder secrets — refuse to boot with any of these (an
/// operator whose config still contains a template value almost certainly forgot
/// to override it, and the value is in public source control). Plain string
/// match (not substring) so genuine 32+-char operator secrets aren't rejected.
/// Kept in lockstep with ziee-auth's `jwt::BANNED_JWT_SECRETS`; the first entry
/// is the `config/dev.example.yaml` placeholder the docs call out.
const BANNED_JWT_SECRETS: &[&str] = &[
    "dev-secret-change-in-production-min-32-chars-long",
    "REPLACE_ME_WITH_A_LONG_RANDOM_SECRET_AT_LEAST_32_CHARS",
    "your-secret-key-here",
    "change-me",
    "secret",
    "changeme",
];

/// Framework server configuration: the app-agnostic settings a framework
/// consumer needs. ziee's `Config` flattens this in, so on the wire the
/// `postgresql` / `server` / `logging` / `jwt` keys sit at the top level
/// exactly as before the split.
#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub postgresql: PostgreSqlConfig,
    pub server: HttpServerConfig,
    #[serde(default)]
    pub logging: Option<LoggingConfig>,
    pub jwt: JwtConfig,
}

impl ServerConfig {
    pub fn database_url(&self) -> String {
        if self.postgresql.use_embedded {
            let embedded = self
                .postgresql
                .embedded
                .as_ref()
                .expect("embedded config must be present when use_embedded is true");
            format!(
                "postgresql://{}:{}@{}:{}/{}",
                embedded.username,
                embedded.password,
                embedded.bind_address,
                embedded.port,
                embedded.database
            )
        } else {
            let external = self
                .postgresql
                .external
                .as_ref()
                .expect("external config must be present when use_embedded is false");
            format!(
                "postgresql://{}:{}@{}:{}/{}",
                external.username,
                external.password,
                external.host,
                external.port,
                external.database
            )
        }
    }

    pub fn server_address(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }

    /// Load + validate a `ServerConfig` from a YAML file (chunk sdk-batteries /
    /// G2). Parses the framework keys (`postgresql`/`server`/`logging`/`jwt`);
    /// any extra app-specific top-level keys are ignored (serde default). Refuses
    /// a weak/placeholder `jwt.secret` so an app never boots with a known signer
    /// — the same guard ziee documents in its own `Config::load_from`.
    ///
    /// ADDITIVE: ziee keeps its own monolithic `Config` loader; this is the SDK
    /// helper a fresh app calls instead of re-implementing YAML load + validation.
    pub fn load_from(path: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read config file '{}': {}", path.display(), e))?;
        let config: ServerConfig = serde_norway::from_str(&content)
            .map_err(|e| format!("failed to parse config file '{}': {}", path.display(), e))?;
        config.validate_jwt_secret()?;
        Ok(config)
    }

    /// Discover a config file: the `CONFIG_FILE` env var takes precedence, then a
    /// short list of conventional locations relative to the current dir. Returns
    /// the first existing path, or `None`.
    pub fn discover() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("CONFIG_FILE") {
            let path = PathBuf::from(p);
            if path.exists() {
                return Some(path);
            }
        }
        for candidate in ["config/dev.yaml", "config/config.yaml", "config.yaml"] {
            let path = PathBuf::from(candidate);
            if path.exists() {
                return Some(path);
            }
        }
        None
    }

    /// Discover (via [`discover`](Self::discover)) then [`load_from`](Self::load_from).
    /// Errors if no config file is found.
    pub fn load() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let path = Self::discover().ok_or_else(|| {
            "no config file found — set CONFIG_FILE or place one at \
             config/dev.yaml (e.g. CONFIG_FILE=config/dev.yaml)"
                .to_string()
        })?;
        Self::load_from(&path)
    }

    /// Refuse a weak/placeholder `jwt.secret`. Mirrors ziee-auth's
    /// `JwtService::try_new`.
    pub fn validate_jwt_secret(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let secret = &self.jwt.secret;
        if secret.len() < MIN_JWT_SECRET_LEN {
            return Err(format!(
                "jwt.secret is {} bytes; minimum is {}. Set jwt.secret to a random \
                 ≥32-char string (e.g. `openssl rand -base64 48`).",
                secret.len(),
                MIN_JWT_SECRET_LEN
            )
            .into());
        }
        if BANNED_JWT_SECRETS.contains(&secret.as_str()) {
            return Err("jwt.secret matches a shipped placeholder value. Set jwt.secret to a \
                        unique random ≥32-char string (e.g. `openssl rand -base64 48`)."
                .into());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct PostgreSqlConfig {
    pub use_embedded: bool,
    #[serde(default)]
    pub embedded: Option<EmbeddedPostgreSqlConfig>,
    #[serde(default)]
    pub external: Option<ExternalPostgreSqlConfig>,
    #[serde(default)]
    pub pool: Option<PoolConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct EmbeddedPostgreSqlConfig {
    pub version: String,
    pub port: u16,
    pub bind_address: String,
    pub username: String,
    pub password: String,
    pub database: String,
    /// Postgres install tree (bin/lib/share). Default
    /// `<app.data_dir>/postgres/` (filled by `resolve_paths`).
    /// postgresql_embedded skips re-extraction if the version matches —
    /// safe to share across server upgrades.
    #[serde(default)]
    pub installation_dir: Option<String>,
    /// PGDATA cluster (pg_wal, base, postgresql.conf). Default
    /// `<app.data_dir>/postgres-data/`. Operators commonly override
    /// to put the cluster on a fast disk.
    #[serde(default)]
    pub data_dir: Option<String>,
    pub timezone: String,
    pub log_timezone: String,
    pub logging: LoggingConfigPostgres,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExternalPostgreSqlConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub database: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfigPostgres {
    pub collector: bool,
    pub directory: String,
    pub filename: String,
    pub statement: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PoolConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_secs: u64,
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_lifetime_secs: Option<u64>,
}

/// HTTP server sub-config (formerly ziee's `ServerConfig`). Renamed so the
/// grouping [`ServerConfig`] can own the `ServerConfig` name; ziee re-exports
/// this as `HttpServerConfig`.
#[derive(Debug, Deserialize, Clone)]
pub struct HttpServerConfig {
    pub host: String,
    pub port: u16,
    pub api_prefix: String,
    #[serde(default)]
    pub cors: Option<CorsConfig>,
    /// Rate-limit configuration (tower-governor). Optional — defaults
    /// to 50 req/s sustained, 500-burst (enough for a normal SPA
    /// cold-load without 429s). Hardened deployments behind a real
    /// reverse proxy should override downward here; tests override
    /// upward since sequential sweeps against 127.0.0.1 share a
    /// single peer-IP bucket.
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
    /// Honor X-Forwarded-Host / X-Forwarded-Proto in OAuth
    /// redirect_uri derivation.
    ///
    /// **Default false.** Only set true when the server is behind a
    /// reverse proxy that STRIPS inbound X-Forwarded-* headers and
    /// sets them itself (nginx `proxy_set_header`, Caddy `header_up`,
    /// Cloudflare / Vercel / Fly defaults, the Vite dev proxy in
    /// this repo's vite.config.ts). When the server is exposed
    /// directly, this MUST stay false — otherwise an attacker can
    /// send `X-Forwarded-Host: evil.com` to the backend and a
    /// permissive IdP (Keycloak wildcard, Dex, Authentik) will hand
    /// the OAuth `code` to evil.com. F-07 attack class.
    #[serde(default)]
    pub trust_forwarded_headers: bool,
    /// Per-file upload size cap, in MiB (binary — `N * 1024 * 1024`).
    ///
    /// **Default 128.** Enforced by `upload_file_inner` for every upload
    /// entry point (`/files/upload` + `/projects/{id}/files/upload`); the
    /// per-route body limit is derived as `cap + 16 MiB` so the request is
    /// rejected before buffering an over-cap body into RAM. Keep the layers
    /// consistent: `nginx client_max_body_size ≥ body limit ≥ this cap`
    /// (nginx defaults to 1 GiB, so any cap up to ~1000 fits). Raise this
    /// for deployments handling large scientific/genomics files
    /// (`.rds`/`.csv` commonly 80–200 MB+); via the docker image, set
    /// `ZIEE_MAX_FILE_UPLOAD_MB`.
    #[serde(default = "default_max_file_upload_mb")]
    pub max_file_upload_mb: u64,
}

/// Default per-file upload cap in MiB. 128 MiB comfortably covers the
/// common 80–200 MB genomics-file range while staying well under nginx's
/// 1 GiB body limit.
fn default_max_file_upload_mb() -> u64 {
    128
}

#[derive(Debug, Deserialize, Clone)]
pub struct RateLimitConfig {
    /// Master on/off switch for the global tower-governor rate limiter.
    /// Defaults to `true` (preserve the A3 DoS-protection posture).
    ///
    /// Set `false` on trusted / non-public deployments. The built-in
    /// code_sandbox + memory MCP servers are reached over loopback
    /// (`http://127.0.0.1`), so every internal tool call shares the same
    /// peer-IP bucket as user traffic and a rapid agent tool loop makes the
    /// server self-throttle (HTTP 429 "Too Many Requests"). When this is
    /// `false` the `GovernorLayer` is not installed at all, so NO traffic —
    /// internal or external — is rate-limited.
    #[serde(default = "default_rate_limit_enabled")]
    pub enabled: bool,
    /// Sustained requests-per-second per peer IP.
    #[serde(default = "default_rate_limit_per_second")]
    pub per_second: u64,
    /// Token-bucket burst capacity.
    #[serde(default = "default_rate_limit_burst_size")]
    pub burst_size: u32,
}

fn default_rate_limit_enabled() -> bool {
    true
}

fn default_rate_limit_per_second() -> u64 {
    50
}

fn default_rate_limit_burst_size() -> u32 {
    500
}

#[derive(Debug, Deserialize, Clone)]
pub struct CorsConfig {
    pub allow_origins: Vec<String>,
    pub allow_methods: Vec<String>,
    pub allow_headers: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    pub level: String,
    pub format: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct JwtConfig {
    pub secret: String,
    pub issuer: String,
    pub audience: String,
    /// Initial seed + DB-read fallback since migration 129: the live value
    /// is the `session_settings.access_token_expiry_hours` row (admin-
    /// configurable), copied from this field exactly once at first boot.
    pub access_token_expiry_hours: i64,
    /// Initial seed + DB-read fallback (see access_token_expiry_hours).
    #[serde(default = "default_refresh_token_expiry")]
    pub refresh_token_expiry_days: i64,
    /// DEBUG-ONLY test seam: overrides the access-token TTL with a
    /// seconds-granularity value so integration/e2e suites can exercise
    /// token expiry without waiting hours. Honored only under
    /// `cfg!(debug_assertions)` — physically inert in release builds
    /// (same pattern as `SYNC_RECHECK_TICK_MS`).
    #[serde(default)]
    pub access_token_expiry_seconds: Option<i64>,
}

fn default_refresh_token_expiry() -> i64 {
    30
}

#[cfg(test)]
mod rate_limit_config_tests {
    use super::RateLimitConfig;

    // serde_json (not yaml) keeps the test dependency-free; RateLimitConfig
    // derives Deserialize so the format is irrelevant to what we assert.

    #[test]
    fn enabled_defaults_true_when_field_omitted() {
        // The pre-existing block shape (per_second/burst_size only, e.g.
        // tests/common/mod.rs) must keep the limiter enabled.
        let cfg: RateLimitConfig =
            serde_json::from_str(r#"{"per_second":1,"burst_size":2}"#).unwrap();
        assert!(cfg.enabled, "enabled should default to true");
        assert_eq!(cfg.per_second, 1);
        assert_eq!(cfg.burst_size, 2);
    }

    #[test]
    fn can_disable_with_just_enabled_flag() {
        // `enabled: false` alone is enough; per_second/burst_size fall back
        // to their serde defaults.
        let cfg: RateLimitConfig = serde_json::from_str(r#"{"enabled":false}"#).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.per_second, 50);
        assert_eq!(cfg.burst_size, 500);
    }

    #[test]
    fn full_block_parses_all_fields() {
        let cfg: RateLimitConfig =
            serde_json::from_str(r#"{"enabled":true,"per_second":100,"burst_size":200}"#).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.per_second, 100);
        assert_eq!(cfg.burst_size, 200);
    }
}

#[cfg(test)]
mod load_from_tests {
    use super::ServerConfig;
    use std::io::Write;

    const STRONG_SECRET: &str = "0123456789abcdef0123456789abcdef-strong";

    fn yaml_with_secret(secret: &str) -> String {
        format!(
            "postgresql:\n  use_embedded: true\n  embedded:\n    version: '18.3.0'\n    port: 0\n    bind_address: 127.0.0.1\n    username: postgres\n    password: pw\n    database: app\n    timezone: UTC\n    log_timezone: UTC\n    logging:\n      collector: false\n      directory: log\n      filename: pg.log\n      statement: none\nserver:\n  host: 127.0.0.1\n  port: 3000\n  api_prefix: /api\njwt:\n  secret: '{secret}'\n  issuer: app\n  audience: app-api\n  access_token_expiry_hours: 24\n"
        )
    }

    fn write_tmp(contents: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f
    }

    #[test]
    fn loads_valid_config() {
        let f = write_tmp(&yaml_with_secret(STRONG_SECRET));
        let cfg = ServerConfig::load_from(f.path()).expect("valid config loads");
        assert_eq!(cfg.server.port, 3000);
        assert_eq!(cfg.jwt.issuer, "app");
        assert!(cfg.postgresql.use_embedded);
    }

    #[test]
    fn refuses_example_placeholder_secret() {
        let f = write_tmp(&yaml_with_secret("dev-secret-change-in-production-min-32-chars-long"));
        let err = ServerConfig::load_from(f.path()).unwrap_err().to_string();
        assert!(err.contains("placeholder"), "got: {err}");
    }

    #[test]
    fn refuses_too_short_secret() {
        let f = write_tmp(&yaml_with_secret("short"));
        let err = ServerConfig::load_from(f.path()).unwrap_err().to_string();
        assert!(err.contains("minimum is 32"), "got: {err}");
    }

    #[test]
    fn ignores_extra_app_specific_keys() {
        // A real app YAML carries many keys ServerConfig doesn't model; they must
        // be ignored, not rejected.
        let mut yaml = yaml_with_secret(STRONG_SECRET);
        yaml.push_str("app:\n  data_dir: /tmp/app\nmemory:\n  enabled: false\n");
        let f = write_tmp(&yaml);
        ServerConfig::load_from(f.path()).expect("extra keys ignored");
    }

    #[test]
    fn discover_prefers_config_file_env() {
        let f = write_tmp(&yaml_with_secret(STRONG_SECRET));
        // SAFETY: single-threaded test; restore after.
        unsafe { std::env::set_var("CONFIG_FILE", f.path()) };
        let discovered = ServerConfig::discover();
        unsafe { std::env::remove_var("CONFIG_FILE") };
        assert_eq!(discovered.as_deref(), Some(f.path()));
    }
}

#[cfg(test)]
mod max_file_upload_tests {
    use super::{default_max_file_upload_mb, HttpServerConfig};

    #[test]
    fn default_is_128() {
        assert_eq!(default_max_file_upload_mb(), 128);
    }

    #[test]
    fn omitted_key_deserializes_to_default() {
        let yaml = "host: 127.0.0.1\nport: 3000\napi_prefix: /api\n";
        let cfg: HttpServerConfig = serde_norway::from_str(yaml).expect("parse HttpServerConfig");
        assert_eq!(cfg.max_file_upload_mb, 128);
    }

    #[test]
    fn explicit_key_overrides_default() {
        let yaml = "host: 127.0.0.1\nport: 3000\napi_prefix: /api\nmax_file_upload_mb: 256\n";
        let cfg: HttpServerConfig = serde_norway::from_str(yaml).expect("parse HttpServerConfig");
        assert_eq!(cfg.max_file_upload_mb, 256);
    }
}
