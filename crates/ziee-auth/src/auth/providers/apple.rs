// Sign in with Apple — Apple-specific provider.
//
// Apple cannot reuse `OAuth2Provider` because of three quirks:
//   1. `client_secret` is an ES256 JWT signed with a `.p8` ECDSA
//      private key — Apple does NOT accept a static secret. We
//      regenerate it per token exchange (max 6-month lifetime;
//      regenerating each time is the simplest correctness story).
//   2. `response_mode=form_post` is mandatory when scope includes
//      `name` or `email`. The callback arrives as a POST form body,
//      not a query string.
//   3. The first-time-only `user` JSON arrives in the callback POST
//      body, NOT in the ID token. Handled in `handlers.rs`
//      (`oauth_callback_post`) by merging into UserAttributes after
//      this provider returns its base AuthResult.
//
// Apple JWKS for ID-token verification: https://appleid.apple.com/auth/keys
//
// All of this is fetched at runtime; no Apple-specific code paths in
// the rest of the codebase.

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, encode};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::auth::AuthRepository;
use super::{AuthError, AuthProvider, AuthProviderTrait, AuthResult, OAuthResult, OAuthSession, UserAttributes};

const DEFAULT_APPLE_BASE_URL: &str = "https://appleid.apple.com";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppleConfig {
    /// Apple Developer Team ID (10-char alphanumeric).
    pub team_id: String,
    /// Apple Services ID — used as the `client_id` on the wire.
    pub services_id: String,
    /// Key ID for the `.p8` private key (10-char).
    pub key_id: String,
    /// Filesystem path to the `AuthKey_<KEY_ID>.p8` ECDSA private
    /// key. NOT uploaded through the admin UI — filesystem perms
    /// are the right control for a private key. Operators drop the
    /// file on disk and reference it here.
    pub private_key_path: PathBuf,
    /// Scopes to request. Defaults to `["name", "email"]`. Including
    /// either of these triggers Apple's `form_post` requirement.
    pub scopes: Vec<String>,
    /// Session timeout in seconds (default: 300 = 5 minutes)
    #[serde(default)]
    pub session_timeout_seconds: Option<i64>,
    /// Base URL for Apple's auth endpoints. Defaults to
    /// `https://appleid.apple.com`. Override is exposed so
    /// integration tests can point at a local wiremock; operators
    /// should never set this in production.
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Cached Apple JWKS. Apple rotates keys infrequently; refresh on
/// kid-miss with a small backoff to avoid hammering Apple if a token
/// references a kid that doesn't exist.
#[derive(Default, Clone)]
struct JwksCache {
    keys: Vec<AppleJwk>,
    fetched_at: Option<DateTime<Utc>>,
}

/// Minimum seconds between successive JWKS refreshes when the kid
/// lookup misses. Without a gate, a forged token with a random kid
/// value turns every login attempt into an Apple JWKS round-trip — at
/// the global rate limit that's still hundreds per minute and may
/// trip Apple's per-IP throttling. 30 seconds is long enough to
/// absorb a burst and short enough to pick up a real key rotation on
/// the next retry.
const JWKS_MIN_REFRESH_INTERVAL_SECS: i64 = 30;

#[derive(Debug, Clone, Deserialize)]
struct AppleJwks {
    keys: Vec<AppleJwk>,
}

#[derive(Debug, Clone, Deserialize)]
struct AppleJwk {
    kid: String,
    n: String,
    e: String,
    #[serde(default)]
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    r#use: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AppleTokenResponse {
    #[allow(dead_code)]
    access_token: String,
    #[allow(dead_code)]
    #[serde(default)]
    expires_in: Option<i64>,
    id_token: String,
    #[allow(dead_code)]
    #[serde(default)]
    refresh_token: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    token_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AppleIdTokenClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    /// Apple sends `email_verified` as the STRING `"true"` (not bool).
    /// `StringOrBool` handles both shapes.
    #[serde(default)]
    email_verified: Option<StringOrBool>,
    #[serde(default)]
    is_private_email: Option<StringOrBool>,
    #[serde(default)]
    nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringOrBool {
    Bool(bool),
    Str(String),
}

impl StringOrBool {
    fn as_bool(&self) -> bool {
        match self {
            StringOrBool::Bool(b) => *b,
            StringOrBool::Str(s) => s.eq_ignore_ascii_case("true"),
        }
    }
}

pub struct AppleProvider {
    // Used via the `AuthProviderTrait::name()` method — dyn-dispatched
    // through `Box<dyn AuthProviderTrait>`, so the dead-code analyzer
    // can't see the use-site.
    #[allow(dead_code)]
    name: String,
    provider_id: Uuid,
    config: AppleConfig,
    // Used via the `AuthProviderTrait::get_config()` method (same
    // dyn-dispatch story as `name`).
    #[allow(dead_code)]
    raw_config: serde_json::Value,
    #[allow(dead_code)]
    pool: PgPool,
    jwks_cache: Arc<RwLock<JwksCache>>,
    /// Single-flight gate for JWKS refresh. Held across the network
    /// fetch so concurrent kid-misses coalesce into one upstream
    /// request — without this, N parallel logins on a key rotation
    /// each issue their own GET to Apple.
    jwks_refresh_lock: Arc<tokio::sync::Mutex<()>>,
    /// EncodingKey is built once from the .p8 PEM and reused. The
    /// previous implementation re-read the file + re-parsed PEM on
    /// every token exchange (cheap, but pointless I/O + a path leak
    /// in error messages on every failed login).
    signing_key: Arc<EncodingKey>,
    http: reqwest::Client,
}

impl AppleProvider {
    fn base_url(&self) -> &str {
        self.config
            .base_url
            .as_deref()
            .unwrap_or(DEFAULT_APPLE_BASE_URL)
    }
    fn authorize_url(&self) -> String {
        format!("{}/auth/authorize", self.base_url())
    }
    fn token_url(&self) -> String {
        format!("{}/auth/token", self.base_url())
    }
    fn jwks_url(&self) -> String {
        format!("{}/auth/keys", self.base_url())
    }
    fn issuer(&self) -> String {
        self.base_url().to_string()
    }

    pub fn new(provider: &AuthProvider, pool: PgPool) -> Result<Self, AuthError> {
        let config: AppleConfig = serde_json::from_value(provider.config.clone()).map_err(|e| {
            AuthError::ConfigurationError(format!("Invalid Apple configuration: {}", e))
        })?;

        // SECURITY: defense against an admin pasting a tainted
        // `base_url` (e.g. `http://localhost:1234/`). Apple is HTTPS-
        // only on real deployments; the override exists for tests but
        // gets force-disabled in release builds so an admin who can
        // POST a provider config can't repoint Apple at a private-net
        // probe.
        if let Some(custom) = config.base_url.as_deref() {
            if cfg!(not(debug_assertions))
                && custom != DEFAULT_APPLE_BASE_URL
            {
                return Err(AuthError::ConfigurationError(
                    "Apple `base_url` override is only permitted in debug builds (for tests)".to_string(),
                ));
            }
            if !custom.starts_with("https://") && !custom.starts_with("http://") {
                return Err(AuthError::ConfigurationError(
                    "Apple `base_url` must be http(s)".to_string(),
                ));
            }
        }

        // Build the ES256 signing key ONCE at construction so the per-
        // login token exchange doesn't re-read the .p8 from disk + re-
        // parse PEM. Surfacing the path on error here (during admin
        // create/update) is acceptable because admin already knows
        // it; the bigger concern was the original code leaking it on
        // EVERY login failure via `Debug` of PathBuf.
        //
        // NOTE: std::fs::read is blocking, and AppleProvider::new is
        // sync, called from async handlers via create_provider. The
        // .p8 file is ~300 bytes and OS-cached after the first read,
        // so the block is microseconds in practice and acceptable.
        // If we ever need to support remote-mounted key paths
        // (NFS, SMB, /run/secrets/...) this should move to
        // spawn_blocking or a process-wide cache keyed by provider_id.
        let pem = std::fs::read(&config.private_key_path).map_err(|e| {
            AuthError::ConfigurationError(format!(
                "Apple private key unreadable at the configured path: {}",
                e
            ))
        })?;
        let signing_key = EncodingKey::from_ec_pem(&pem).map_err(|e| {
            AuthError::ConfigurationError(format!(
                "Apple .p8 private key failed to parse: {}",
                e
            ))
        })?;

        // SSRF: route through build_validated_client so the IP
        // allowlist (no RFC1918, no link-local) is enforced AT
        // REQUEST TIME including redirect targets. The previous
        // reqwest::Client::builder() + Policy::none() only disabled
        // redirects — it did not block an admin's `base_url`
        // override from pointing at internal infra. The policy is
        // DEV_LOCAL only in debug builds (the wiremock-based tests
        // bind loopback); in release we use PUBLIC_HTTP_OR_HTTPS
        // (loopback blocked).
        let policy = if cfg!(debug_assertions) {
            ziee_framework::url_validator::OutboundUrlPolicy::DEV_LOCAL
        } else {
            ziee_framework::url_validator::OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS
        };
        let http = ziee_framework::url_validator::build_validated_client(policy)
            .or_else(|_| {
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
            })
            .map_err(|e| {
                AuthError::ConfigurationError(format!("Failed to create HTTP client: {}", e))
            })?;

        Ok(Self {
            name: provider.name.clone(),
            provider_id: provider.id,
            config,
            raw_config: provider.config.clone(),
            pool,
            jwks_cache: Arc::new(RwLock::new(JwksCache::default())),
            jwks_refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            signing_key: Arc::new(signing_key),
            http,
        })
    }

    /// Generate the ES256 client_secret JWT that Apple expects on
    /// every token exchange. 5-minute lifetime — short to limit
    /// replay window; Apple's max is 6 months but there's no benefit
    /// to keeping it longer when regeneration is essentially free.
    fn generate_client_secret_jwt(&self) -> Result<String, AuthError> {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.config.key_id.clone());

        let now = Utc::now();
        let claims = serde_json::json!({
            "iss": self.config.team_id,
            "iat": now.timestamp(),
            "exp": (now + Duration::minutes(5)).timestamp(),
            "aud": self.issuer(),
            "sub": self.config.services_id,
        });

        encode(&header, &claims, self.signing_key.as_ref()).map_err(|e| {
            AuthError::InternalError(format!("Failed to sign Apple client_secret JWT: {}", e))
        })
    }

    async fn fetch_jwks(&self) -> Result<Vec<AppleJwk>, AuthError> {
        let resp = self.http.get(&self.jwks_url()).send().await.map_err(|e| {
            AuthError::ConnectionFailed(format!("Failed to fetch Apple JWKS: {}", e))
        })?;
        if !resp.status().is_success() {
            return Err(AuthError::ConnectionFailed(format!(
                "Apple JWKS endpoint returned {}",
                resp.status()
            )));
        }
        let jwks: AppleJwks = resp.json().await.map_err(|e| {
            AuthError::InternalError(format!("Failed to parse Apple JWKS: {}", e))
        })?;
        // SECURITY: enforce kty=RSA + alg=RS256 (Apple is documented
        // to use RSA / RS256 only). Without this, a malicious JWKS
        // response could include an HS256 key whose `n` is a
        // public-key-shaped string — jsonwebtoken's signature check
        // would then verify a token signed with that string as MAC,
        // bypassing asymmetric verification. Drop any key that
        // doesn't conform.
        let filtered: Vec<AppleJwk> = jwks
            .keys
            .into_iter()
            .filter(|k| {
                k.kty.eq_ignore_ascii_case("RSA")
                    // Require an explicit RS256 alg — Apple always
                    // sets it. unwrap_or(false): defense in depth
                    // against a confused-deputy JWKS-substitution
                    // where keys lack alg and could be reinterpreted
                    // as HS256 (n as MAC secret).
                    && k.alg.as_deref().map(|a| a.eq_ignore_ascii_case("RS256")).unwrap_or(false)
            })
            .collect();
        if filtered.is_empty() {
            return Err(AuthError::InternalError(
                "Apple JWKS response had no RSA/RS256 keys".to_string(),
            ));
        }
        Ok(filtered)
    }

    /// Look up the JWK by `kid`. Refresh the cache once if the kid
    /// isn't found (Apple may have rotated) — but gate the refresh
    /// behind both (a) a min-interval timer and (b) a single-flight
    /// mutex, so a forged token with a random kid can't turn N
    /// concurrent login attempts into N Apple GETs.
    async fn get_jwk_for_kid(&self, kid: &str) -> Result<AppleJwk, AuthError> {
        // Cached hit?
        {
            let cache = self.jwks_cache.read().await;
            if let Some(jwk) = cache.keys.iter().find(|j| j.kid == kid) {
                return Ok(jwk.clone());
            }
        }

        // Min-interval gate. If we fetched recently and the kid still
        // doesn't exist, don't bother Apple again.
        {
            let cache = self.jwks_cache.read().await;
            if let Some(fetched_at) = cache.fetched_at {
                let elapsed = (Utc::now() - fetched_at).num_seconds();
                if elapsed < JWKS_MIN_REFRESH_INTERVAL_SECS {
                    return Err(AuthError::InvalidCredentials(format!(
                        "Apple kid '{}' not found in JWKS (refresh throttled)",
                        kid
                    )));
                }
            }
        }

        // Single-flight: only one coroutine fetches at a time. After
        // acquiring the lock, re-check the cache in case another
        // coroutine just refreshed it.
        let _refresh_guard = self.jwks_refresh_lock.lock().await;
        {
            let cache = self.jwks_cache.read().await;
            if let Some(jwk) = cache.keys.iter().find(|j| j.kid == kid) {
                return Ok(jwk.clone());
            }
        }

        // SECURITY: bump fetched_at BEFORE awaiting the fetch so the
        // min-interval gate applies even if Apple is unreachable
        // (timeout / 5xx / DNS). Otherwise an Apple outage turns
        // every login into an Apple retry — exactly the storm the
        // gate was meant to prevent, just shifted from "wrong kid"
        // to "Apple down."
        {
            let mut cache = self.jwks_cache.write().await;
            cache.fetched_at = Some(Utc::now());
        }
        let fresh = self.fetch_jwks().await?;
        let found = fresh.iter().find(|j| j.kid == kid).cloned();
        let mut cache = self.jwks_cache.write().await;
        cache.keys = fresh;
        // fetched_at was already updated above; re-stamp on success
        // so the throttle starts from "successful refresh," not
        // "attempted refresh."
        cache.fetched_at = Some(Utc::now());
        found.ok_or_else(|| {
            AuthError::InvalidCredentials(format!("Apple kid '{}' not found in JWKS", kid))
        })
    }

    /// Verify the Apple-issued ID token: signature via JWKS, audience
    /// = our services_id, issuer = appleid.apple.com, nonce matches
    /// the one we sent in init_oauth_flow.
    async fn verify_id_token(
        &self,
        id_token_jwt: &str,
        expected_nonce: Option<&str>,
    ) -> Result<AppleIdTokenClaims, AuthError> {
        // Pull kid out of the JOSE header without verifying signature.
        let header = jsonwebtoken::decode_header(id_token_jwt).map_err(|e| {
            AuthError::InvalidCredentials(format!("Failed to decode ID token header: {}", e))
        })?;
        let kid = header.kid.ok_or_else(|| {
            AuthError::InvalidCredentials("Apple ID token header missing kid".to_string())
        })?;

        let jwk = self.get_jwk_for_kid(&kid).await?;
        let key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e).map_err(|e| {
            // Distinct from "JWK unparseable" — the JWK parsed fine
            // upstream; failure here means the n/e components couldn't
            // be assembled into a usable RSA public key.
            AuthError::InternalError(format!(
                "Apple JWK RSA components rejected: {}",
                e
            ))
        })?;

        let mut validation = Validation::new(Algorithm::RS256);
        let issuer = self.issuer();
        validation.set_issuer(&[issuer.as_str()]);
        validation.set_audience(&[self.config.services_id.as_str()]);

        let decoded = jsonwebtoken::decode::<AppleIdTokenClaims>(id_token_jwt, &key, &validation)
            .map_err(|e| {
                AuthError::InvalidCredentials(format!("Apple ID token verification failed: {}", e))
            })?;

        if let Some(expected) = expected_nonce {
            let token_nonce = decoded.claims.nonce.as_deref().unwrap_or_default();
            if token_nonce != expected {
                return Err(AuthError::InvalidCredentials(
                    "Apple ID token nonce mismatch".to_string(),
                ));
            }
        }

        Ok(decoded.claims)
    }
}

#[async_trait]
impl AuthProviderTrait for AppleProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn provider_type(&self) -> &str {
        "apple"
    }

    async fn authenticate(
        &self,
        _username: &str,
        _password: &str,
    ) -> Result<AuthResult, AuthError> {
        Err(AuthError::NotSupported(
            "Apple Sign In does not support password authentication".to_string(),
        ))
    }

    async fn init_oauth_flow(
        &self,
        redirect_uri: &str,
        return_to: Option<&str>,
    ) -> Result<OAuthResult, AuthError> {
        let state = Uuid::new_v4().to_string();
        let nonce = Uuid::new_v4().to_string();

        let scope_str = self.config.scopes.join(" ");
        let needs_form_post = self
            .config
            .scopes
            .iter()
            .any(|s| s == "name" || s == "email");
        let response_mode = if needs_form_post { "form_post" } else { "query" };

        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("response_type", "code")
            .append_pair("response_mode", response_mode)
            .append_pair("client_id", &self.config.services_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", &scope_str)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .finish();
        let auth_url = format!("{}?{}", self.authorize_url(), query);

        let timeout = self.config.session_timeout_seconds.unwrap_or(300);
        let expires_at = Utc::now() + Duration::seconds(timeout);

        let session = OAuthSession {
            id: Uuid::new_v4(),
            state: state.clone(),
            provider_id: self.provider_id,
            pkce_verifier: None, // Apple does not require PKCE for confidential clients
            nonce: Some(nonce),
            redirect_uri: redirect_uri.to_string(),
            created_at: Utc::now(),
            expires_at,
            return_to: return_to.map(|s| s.to_string()),
        };
        AuthRepository::new(self.pool.clone()).create_oauth_session(&session).await.map_err(|e| {
            AuthError::InternalError(format!("Failed to create oauth session: {}", e))
        })?;

        Ok(OAuthResult {
            redirect_url: auth_url,
            session_key: state,
        })
    }

    async fn handle_oauth_callback(
        &self,
        code: &str,
        state: &str,
        _session_key: &str,
    ) -> Result<AuthResult, AuthError> {
        let session = AuthRepository::new(self.pool.clone())
            .get_oauth_session_by_state(state)
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to load session: {}", e)))?
            .ok_or_else(|| {
                AuthError::InvalidCredentials("Invalid or expired session".to_string())
            })?;

        if session.provider_id != self.provider_id {
            return Err(AuthError::InvalidCredentials(
                "Provider mismatch".to_string(),
            ));
        }

        let client_secret = self.generate_client_secret_jwt()?;

        let form = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", session.redirect_uri.as_str()),
            ("client_id", self.config.services_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ];

        let resp = self
            .http
            .post(&self.token_url())
            .form(&form)
            .send()
            .await
            .map_err(|e| {
                AuthError::ConnectionFailed(format!("Apple token request failed: {}", e))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AuthError::InvalidCredentials(format!(
                "Apple token endpoint returned {}: {}",
                status, body
            )));
        }

        let token_response: AppleTokenResponse = resp.json().await.map_err(|e| {
            AuthError::InternalError(format!("Failed to parse Apple token response: {}", e))
        })?;

        let claims = self
            .verify_id_token(&token_response.id_token, session.nonce.as_deref())
            .await?;

        // Clean up session — single-use.
        let _ = AuthRepository::new(self.pool.clone()).delete_oauth_session(state).await;

        let mut email = claims.email.clone().unwrap_or_default();
        let email_verified = claims
            .email_verified
            .as_ref()
            .map(|v| v.as_bool())
            .unwrap_or(false);
        let is_private = claims
            .is_private_email
            .as_ref()
            .map(|v| v.as_bool())
            .unwrap_or(false);

        // Defense in depth: if Apple didn't assert email_verified,
        // drop the email at the provider boundary too. The handler
        // also has this guard, but enforcing it here means even a
        // future caller that bypasses the handler's check can't get
        // an unverified email through.
        if !email_verified {
            email.clear();
        }

        // Apple gives us no username; derive one. For private-relay
        // emails the local-part is an unstable hash that Apple may
        // re-derive — use the stable `sub` instead so a user's
        // username doesn't churn on key rotation events. For real
        // emails the local-part is fine.
        let username = if is_private || email.is_empty() {
            claims.sub.clone()
        } else {
            email
                .split('@')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(&claims.sub)
                .to_string()
        };

        Ok(AuthResult {
            external_id: claims.sub.clone(),
            external_username: Some(username.clone()),
            external_email: if email.is_empty() { None } else { Some(email.clone()) },
            metadata: serde_json::json!({
                "provider": "apple",
                "auth_method": "apple_sign_in",
                "email_verified": email_verified,
                "is_private_email": is_private,
            }),
            attributes: UserAttributes {
                username,
                email,
                display_name: None,
                first_name: None,
                last_name: None,
                groups: vec![],
            },
        })
    }

    async fn test_connection(&self) -> Result<String, AuthError> {
        // Layered checks so the admin sees what passed + what didn't:
        //   1. JWKS reachable
        //   2. .p8 readable + signs ES256
        //   3. dummy token exchange with our client_secret JWT —
        //      proves Apple recognizes our team_id/services_id/key_id
        let mut messages: Vec<String> = Vec::new();

        self.fetch_jwks().await?;
        messages.push("Apple JWKS reachable".to_string());

        let client_secret = self.generate_client_secret_jwt()?;
        messages.push("private key valid (.p8 signs ES256)".to_string());

        // Refuse to probe with obvious placeholder values — the admin
        // hasn't entered real credentials yet.
        if self.config.team_id.trim().is_empty()
            || self.config.services_id.trim().is_empty()
            || self.config.key_id.trim().is_empty()
        {
            messages.push(
                "team_id/services_id/key_id not fully configured; skipped credential probe"
                    .to_string(),
            );
            return Ok(messages.join("; "));
        }

        let form = [
            ("grant_type", "authorization_code"),
            ("code", "ziee-test-connection-probe-dummy-code"),
            ("redirect_uri", "http://localhost/ziee-config-probe"),
            ("client_id", self.config.services_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ];
        let resp = match self
            .http
            .post(&self.token_url())
            .form(&form)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Err(AuthError::ConnectionFailed(format!(
                    "Apple token endpoint unreachable: {}",
                    e
                )));
            }
        };
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        let parsed: serde_json::Value =
            serde_json::from_str(&body_text).unwrap_or(serde_json::Value::Null);
        let error_code = parsed.get("error").and_then(|v| v.as_str()).unwrap_or("");

        match (status.as_u16(), error_code) {
            (400, "invalid_grant") => messages.push(
                "credentials accepted (Apple returned invalid_grant for our dummy code — proves team_id/services_id/key_id are recognized)"
                    .to_string(),
            ),
            (400 | 401, "invalid_client") => {
                return Err(AuthError::InvalidCredentials(format!(
                    "Apple rejected the client_secret JWT — verify team_id ({}), services_id ({}), and key_id ({}) match what's registered in Apple Developer",
                    self.config.team_id, self.config.services_id, self.config.key_id
                )));
            }
            (s, code) if s == 400 || s == 401 => messages.push(format!(
                "Apple returned unexpected error '{}' (HTTP {}) — body: {}",
                code,
                s,
                body_text.chars().take(160).collect::<String>()
            )),
            (s, _) => messages.push(format!(
                "Apple returned HTTP {} (unexpected) — body: {}",
                s,
                body_text.chars().take(160).collect::<String>()
            )),
        }
        Ok(messages.join("; "))
    }

    fn get_config(&self) -> &serde_json::Value {
        &self.raw_config
    }
}

/// Apple's `email_verified` claim — a private helper for testability.
#[cfg(test)]
mod string_or_bool_tests {
    use super::StringOrBool;

    #[test]
    fn parses_string_true() {
        let v: StringOrBool = serde_json::from_str(r#""true""#).unwrap();
        assert!(v.as_bool());
    }

    #[test]
    fn parses_bool_true() {
        let v: StringOrBool = serde_json::from_str("true").unwrap();
        assert!(v.as_bool());
    }

    #[test]
    fn parses_string_false() {
        let v: StringOrBool = serde_json::from_str(r#""false""#).unwrap();
        assert!(!v.as_bool());
    }

    #[test]
    fn parses_bool_false() {
        let v: StringOrBool = serde_json::from_str("false").unwrap();
        assert!(!v.as_bool());
    }
}

