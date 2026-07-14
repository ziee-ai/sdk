// OAuth2/OIDC authentication provider implementation

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    HttpRequest as OAuth2HttpRequest, HttpResponse as OAuth2HttpResponse, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl, basic::BasicClient,
};
use openidconnect::{
    HttpRequest, HttpResponse, IssuerUrl, Nonce,
    core::{
        CoreAuthenticationFlow, CoreClient, CoreIdToken, CoreIdTokenVerifier, CoreProviderMetadata,
    },
};
// Import TokenResponse trait separately to avoid conflict with oauth2::TokenResponse
use openidconnect::TokenResponse as _;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthRepository;
use super::{
    AuthError, AuthProvider, AuthProviderTrait, AuthResult, OAuthResult, OAuthSession,
    UserAttributes,
};

/// Reject an issuer URL that points at a private/loopback IP or
/// non-http(s) scheme BEFORE the openidconnect crate fires its
/// discovery GET. `build_validated_client` validates redirect
/// *targets*, but the initial URL is never checked — without this,
/// an admin with manage permission could set
/// `issuer_url=http://10.0.0.5:8200/v1/identity/oidc` and observe
/// the response via the 5xx/error surface.
///
/// DEV_LOCAL in debug builds so the testcontainer mock at 127.0.0.1
/// works; PUBLIC_HTTP_OR_HTTPS in release (loopback blocked).
fn validate_issuer_url(url: &str) -> Result<(), AuthError> {
    let policy = if cfg!(debug_assertions) {
        ziee_framework::url_validator::OutboundUrlPolicy::DEV_LOCAL
    } else {
        ziee_framework::url_validator::OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS
    };
    ziee_framework::url_validator::validate_outbound_url(url, &policy)
        .map(|_| ())
        .map_err(|e| {
            AuthError::ConfigurationError(format!(
                "issuer_url failed safety check: {}",
                e
            ))
        })
}

/// Create an HTTP client for OAuth2/OIDC requests.
///
/// SECURITY: this routes through `build_validated_client` so every
/// outbound URL — issuer discovery, token exchange, userinfo, JWKS
/// fetch — is checked against the IP allowlist (no RFC 1918, no
/// link-local) AT REQUEST TIME, *including* redirect targets.
/// Policy is DEV_LOCAL in debug builds (loopback allowed — the
/// testcontainer mock binds 127.0.0.1) and PUBLIC_HTTP_OR_HTTPS in
/// release (loopback blocked). Matches `validate_issuer_url` so the
/// pre-flight check and the actual request use the same policy.
///
/// Falls back to the redirect-disabled bare client only if
/// `build_validated_client` somehow fails (TLS config error).
fn create_http_client() -> reqwest::Client {
    let policy = if cfg!(debug_assertions) {
        ziee_framework::url_validator::OutboundUrlPolicy::DEV_LOCAL
    } else {
        ziee_framework::url_validator::OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS
    };
    ziee_framework::url_validator::build_validated_client(policy)
        .unwrap_or_else(|_| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("Failed to create HTTP client")
        })
}

/// Process-wide cached OAuth2 HTTP client. `reqwest::Client` holds an internal
/// connection pool behind an `Arc`, so building it once and cloning (cheap Arc
/// bump) reuses pooled connections instead of constructing a fresh client +
/// pool on every token/userinfo request.
fn http_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(create_http_client).clone()
}

/// Async HTTP client implementation for openidconnect
async fn async_http_client(request: HttpRequest) -> Result<HttpResponse, reqwest::Error> {
    let client = http_client();

    let method = request.method().clone();
    let url = request.uri().to_string();
    let headers = request.headers().clone();
    let body = request.body().clone();

    let mut request_builder = client.request(method, url).body(body);

    for (name, value) in headers.iter() {
        request_builder = request_builder.header(name, value);
    }

    let response = request_builder.send().await?;

    let status_code = response.status();
    let response_headers = response.headers().clone();
    let response_body = response.bytes().await?;

    let mut builder = http::Response::builder().status(status_code);

    for (name, value) in response_headers.iter() {
        builder = builder.header(name, value);
    }

    Ok(builder
        .body(response_body.to_vec())
        .expect("Failed to build HTTP response"))
}

/// Async HTTP client implementation for oauth2
async fn oauth2_http_client(
    request: OAuth2HttpRequest,
) -> Result<OAuth2HttpResponse, reqwest::Error> {
    let client = http_client();

    let method = request.method().clone();
    let url = request.uri().to_string();
    let headers = request.headers().clone();
    let body = request.body().clone();

    let mut request_builder = client.request(method, url).body(body);

    for (name, value) in headers.iter() {
        request_builder = request_builder.header(name, value);
    }

    let response = request_builder.send().await?;

    let status_code = response.status();
    let response_headers = response.headers().clone();
    let response_body = response.bytes().await?;

    let mut builder = http::Response::builder().status(status_code);

    for (name, value) in response_headers.iter() {
        builder = builder.header(name, value);
    }

    Ok(builder
        .body(response_body.to_vec())
        .expect("Failed to build HTTP response"))
}

/// OAuth2/OIDC provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Config {
    /// OAuth 2.0 client ID
    pub client_id: String,
    /// OAuth 2.0 client secret
    pub client_secret: String,
    /// Authorization endpoint URL. Optional for OIDC providers
    /// (discovery returns it); required for plain OAuth2.
    #[serde(default)]
    pub authorization_url: Option<String>,
    /// Token endpoint URL. Optional for OIDC providers (discovery
    /// returns it); required for plain OAuth2.
    #[serde(default)]
    pub token_url: Option<String>,
    /// OIDC issuer URL (required for OIDC providers; omit for plain OAuth2)
    pub issuer_url: Option<String>,
    /// User info endpoint URL (for OAuth 2.0 providers without OIDC)
    pub userinfo_url: Option<String>,
    /// Scopes to request
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Attribute mapping for user info. Defaults to standard OIDC
    /// claim names (sub/email/preferred_username/etc.).
    #[serde(default)]
    pub attribute_mapping: OAuth2AttributeMapping,
    /// Session timeout in seconds (default: 300 = 5 minutes)
    pub session_timeout_seconds: Option<i64>,
    /// Microsoft Entra ONLY: allowlist of tenant IDs (the `tid` claim
    /// on the ID token). When `Some`, the callback REJECTS tokens
    /// whose `tid` is not in this list — critical for safety when
    /// using the `https://login.microsoftonline.com/common/v2.0`
    /// (multi-tenant) issuer, because `common`'s discovery returns
    /// the templated issuer `https://login.microsoftonline.com/{tenantid}/v2.0`,
    /// which `openidconnect` cannot equality-check. Without this
    /// allowlist, ANY Microsoft tenant in the world is a valid login.
    /// `None` = accept any tenant (only safe for true consumer apps
    /// using the `consumers` endpoint).
    #[serde(default)]
    pub allowed_tenant_ids: Option<Vec<String>>,
}

/// Outcome of `probe_oidc_credentials`. Interpreted by `test_connection`
/// to build the admin-facing message.
enum ProbeResult {
    /// Token endpoint replied with `invalid_grant` (or equivalent
    /// 400-class error specifically about the code being invalid).
    /// Means: client_id + client_secret are recognized; only the
    /// code itself was bad — which it was, because we sent a dummy.
    CredentialsOk,
    /// Token endpoint replied with `invalid_client` or 401 — the
    /// provider didn't recognize our client_id / client_secret pair.
    CredentialsBad(String),
    /// Provider rejected the redirect_uri (it's not registered).
    /// Credentials format is fine but the admin needs to register
    /// the callback URL in the provider's console.
    RedirectUriMismatch,
    /// Couldn't reach the token endpoint at all.
    NetworkError(String),
    /// Provider replied with something we don't recognize. Surface
    /// it verbatim to the admin.
    Unexpected(String),
}

/// POST `grant_type=authorization_code` to the token endpoint with a
/// hand-crafted dummy code. Used by `test_connection` to distinguish:
///   - bad credentials (provider rejects client_id/client_secret)
///   - good credentials but bad redirect_uri (admin must register URL)
///   - good credentials, only the code was bad (our dummy → expected)
/// See https://datatracker.ietf.org/doc/html/rfc6749#section-5.2 for
/// the standard error codes.
async fn probe_oidc_credentials(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
) -> ProbeResult {
    let client = http_client();
    let body = [
        ("grant_type", "authorization_code"),
        ("code", "ziee-test-connection-probe-dummy-code"),
        ("redirect_uri", "http://localhost/ziee-config-probe"),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ];
    let resp = match client.post(token_url).form(&body).send().await {
        Ok(r) => r,
        Err(e) => return ProbeResult::NetworkError(e.to_string()),
    };
    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(&body_text).unwrap_or(serde_json::Value::Null);
    let error_code = parsed.get("error").and_then(|v| v.as_str()).unwrap_or("");
    let error_desc = parsed
        .get("error_description")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match (status.as_u16(), error_code) {
        (400, "invalid_grant") => ProbeResult::CredentialsOk,
        (400 | 401, "invalid_client") | (401, _) => {
            ProbeResult::CredentialsBad(if error_desc.is_empty() {
                format!("HTTP {} {}", status, error_code)
            } else {
                format!("{}: {}", error_code, error_desc)
            })
        }
        (_, code) if code.contains("redirect_uri") => ProbeResult::RedirectUriMismatch,
        (_, "invalid_request") if error_desc.to_lowercase().contains("redirect") => {
            ProbeResult::RedirectUriMismatch
        }
        // 429 (rate-limited) and 5xx are transient — surface them as
        // a clear NetworkError rather than as `Unexpected` so the
        // admin sees an actionable message and (for tests) we can
        // distinguish "bad config" from "provider had a bad day."
        (429, _) => ProbeResult::NetworkError(
            "Provider returned HTTP 429 (rate-limited). Wait and retry.".to_string(),
        ),
        (s, _) if (500..=599).contains(&s) => ProbeResult::NetworkError(format!(
            "Provider returned HTTP {} (upstream error). Try again later.",
            s
        )),
        _ => ProbeResult::Unexpected(format!(
            "HTTP {} body={}",
            status,
            body_text.chars().take(200).collect::<String>()
        )),
    }
}

/// Extract a string claim from a verified JWT compact serialization.
/// Safe because openidconnect has already validated signature +
/// standard claims before this is called; we just need to read one
/// additional claim (`tid`) that isn't in `CoreIdTokenClaims`.
fn extract_string_claim(id_token_jwt: &str, claim: &str) -> Option<String> {
    let parts: Vec<&str> = id_token_jwt.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload_bytes = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    payload.get(claim).and_then(|v| v.as_str()).map(String::from)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2AttributeMapping {
    pub user_id: String,              // Default: "sub"
    pub username: String,             // Default: "preferred_username" or "email"
    pub email: String,                // Default: "email"
    pub display_name: Option<String>, // Default: "name"
    pub first_name: Option<String>,   // Default: "given_name"
    pub last_name: Option<String>,    // Default: "family_name"
    pub groups: Option<String>,       // Default: "groups"
}

impl Default for OAuth2AttributeMapping {
    fn default() -> Self {
        Self {
            user_id: "sub".to_string(),
            username: "preferred_username".to_string(),
            email: "email".to_string(),
            display_name: Some("name".to_string()),
            first_name: Some("given_name".to_string()),
            last_name: Some("family_name".to_string()),
            groups: Some("groups".to_string()),
        }
    }
}

pub struct OAuth2Provider {
    name: String,
    provider_id: Uuid,
    config: OAuth2Config,
    raw_config: serde_json::Value,
    // Chunk BG: retained so OAuth-session queries build an `AuthRepository`
    // from this pool instead of reaching the global `Repos` aggregator.
    pool: PgPool,
}

impl OAuth2Provider {
    pub fn new(provider: &AuthProvider, pool: PgPool) -> Result<Self, AuthError> {
        let config: OAuth2Config =
            serde_json::from_value(provider.config.clone()).map_err(|e| {
                AuthError::ConfigurationError(format!("Invalid OAuth2 configuration: {}", e))
            })?;

        Ok(Self {
            name: provider.name.clone(),
            provider_id: provider.id,
            config,
            raw_config: provider.config.clone(),
            pool,
        })
    }

    async fn get_user_info_from_token(
        &self,
        id_token: &CoreIdToken,
        verifier: &CoreIdTokenVerifier<'_>,
        nonce: &Nonce,
    ) -> Result<serde_json::Value, AuthError> {
        let claims = id_token.claims(verifier, nonce).map_err(|e| {
            AuthError::InvalidCredentials(format!("ID token verification failed: {}", e))
        })?;

        Ok(serde_json::json!({
            "sub": claims.subject().to_string(),
            "email": claims.email().map(|e| e.as_str()),
            "email_verified": claims.email_verified(),
            "name": claims.name().and_then(|n| n.get(None)).map(|n| n.as_str()),
            "given_name": claims.given_name().and_then(|n| n.get(None)).map(|n| n.as_str()),
            "family_name": claims.family_name().and_then(|n| n.get(None)).map(|n| n.as_str()),
            "preferred_username": claims.preferred_username().map(|u| u.as_str()),
        }))
    }

    async fn get_user_info_from_api(
        &self,
        access_token: &str,
    ) -> Result<serde_json::Value, AuthError> {
        let userinfo_url = self.config.userinfo_url.as_ref().ok_or_else(|| {
            AuthError::ConfigurationError("UserInfo URL not configured".to_string())
        })?;

        // SECURITY: SSRF-validated client + pre-flight validate_outbound_url
        // on the configured userinfo URL. Two layers because:
        //   1) build_validated_client checks redirect TARGETS at runtime
        //      (so a malicious OIDC provider can't 302-bounce us into
        //      AWS IMDS at 169.254.169.254 to exfiltrate the bearer token —
        //      closes 01-auth F-18 High).
        //   2) validate_outbound_url on the URL itself catches an admin
        //      who configures `userinfo_url=http://internal-vault:8200`
        //      at provider-create time.
        let policy = if cfg!(debug_assertions) {
            ziee_framework::url_validator::OutboundUrlPolicy::DEV_LOCAL
        } else {
            ziee_framework::url_validator::OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS
        };
        ziee_framework::url_validator::validate_outbound_url(userinfo_url, &policy)
            .map_err(|e| {
                AuthError::ConfigurationError(format!(
                    "userinfo_url failed safety check: {}",
                    e
                ))
            })?;
        let client = http_client();
        let response = client
            .get(userinfo_url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| {
                AuthError::ConnectionFailed(format!("Failed to fetch user info: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(AuthError::InternalError(format!(
                "UserInfo request failed with status: {}",
                response.status()
            )));
        }

        response
            .json::<serde_json::Value>()
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to parse user info: {}", e)))
    }

    fn extract_user_attributes(
        &self,
        user_info: &serde_json::Value,
    ) -> Result<UserAttributes, AuthError> {
        let get_str = |key: &str| -> Option<String> {
            user_info
                .get(key)
                .and_then(|v| v.as_str())
                .map(String::from)
        };

        let get_str_array = |key: &str| -> Vec<String> {
            user_info
                .get(key)
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default()
        };

        let username = get_str(&self.config.attribute_mapping.username)
            .or_else(|| get_str(&self.config.attribute_mapping.email))
            .ok_or_else(|| {
                AuthError::InternalError("No username found in user info".to_string())
            })?;

        let email = get_str(&self.config.attribute_mapping.email).unwrap_or_default();

        let display_name = self
            .config
            .attribute_mapping
            .display_name
            .as_ref()
            .and_then(|attr| get_str(attr));
        let first_name = self
            .config
            .attribute_mapping
            .first_name
            .as_ref()
            .and_then(|attr| get_str(attr));
        let last_name = self
            .config
            .attribute_mapping
            .last_name
            .as_ref()
            .and_then(|attr| get_str(attr));

        let groups = self
            .config
            .attribute_mapping
            .groups
            .as_ref()
            .map(|attr| get_str_array(attr))
            .unwrap_or_default();

        Ok(UserAttributes {
            username,
            email,
            display_name,
            first_name,
            last_name,
            groups,
        })
    }
}

#[async_trait]
impl AuthProviderTrait for OAuth2Provider {
    fn name(&self) -> &str {
        &self.name
    }

    fn provider_type(&self) -> &str {
        if self.config.issuer_url.is_some() {
            "oidc"
        } else {
            "oauth2"
        }
    }

    async fn authenticate(
        &self,
        _username: &str,
        _password: &str,
    ) -> Result<AuthResult, AuthError> {
        Err(AuthError::NotSupported(
            "OAuth2/OIDC does not support password authentication".to_string(),
        ))
    }

    async fn init_oauth_flow(
        &self,
        redirect_uri: &str,
        return_to: Option<&str>,
    ) -> Result<OAuthResult, AuthError> {
        // Generate PKCE challenge
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        // Generate state and nonce
        let state = CsrfToken::new_random();
        let nonce = Nonce::new_random();

        // Build authorization URL
        let auth_url = if let Some(issuer_url) = &self.config.issuer_url {
            // OIDC flow - create client inline to avoid type parameter issues
            // SSRF: validate the issuer URL BEFORE handing it to
            // discover_async (closes the gap where build_validated_client
            // only checks redirects, not the initial GET).
            validate_issuer_url(issuer_url)?;
            let issuer = IssuerUrl::new(issuer_url.clone())
                .map_err(|e| AuthError::ConfigurationError(format!("Invalid issuer URL: {}", e)))?;

            let metadata = CoreProviderMetadata::discover_async(issuer, &async_http_client)
                .await
                .map_err(|e| {
                    AuthError::ConfigurationError(format!(
                        "Failed to discover OIDC metadata: {}",
                        e
                    ))
                })?;

            let client_id = ClientId::new(self.config.client_id.clone());
            let client_secret = ClientSecret::new(self.config.client_secret.clone());
            let redirect_url = RedirectUrl::new(redirect_uri.to_string()).map_err(|e| {
                AuthError::ConfigurationError(format!("Invalid redirect URL: {}", e))
            })?;

            let client =
                CoreClient::from_provider_metadata(metadata, client_id, Some(client_secret))
                    .set_redirect_uri(redirect_url);

            let state_clone = CsrfToken::new(state.secret().clone());
            let nonce_clone = Nonce::new(nonce.secret().clone());
            let mut auth_request = client
                .authorize_url(
                    CoreAuthenticationFlow::AuthorizationCode,
                    move || state_clone.clone(),
                    move || nonce_clone.clone(),
                )
                .set_pkce_challenge(pkce_challenge);

            // Add scopes
            for scope in &self.config.scopes {
                auth_request = auth_request.add_scope(Scope::new(scope.clone()));
            }

            let (url, _, _) = auth_request.url();
            url
        } else {
            // OAuth 2.0 flow - create client inline to avoid type parameter issues
            let client_id = ClientId::new(self.config.client_id.clone());
            let client_secret = ClientSecret::new(self.config.client_secret.clone());
            let auth_url_str = self.config.authorization_url.clone().ok_or_else(|| {
                AuthError::ConfigurationError(
                    "OAuth2 provider requires `authorization_url` (or set `issuer_url` for OIDC)".to_string(),
                )
            })?;
            let token_url_str = self.config.token_url.clone().ok_or_else(|| {
                AuthError::ConfigurationError(
                    "OAuth2 provider requires `token_url` (or set `issuer_url` for OIDC)".to_string(),
                )
            })?;
            let auth_url = AuthUrl::new(auth_url_str).map_err(|e| {
                AuthError::ConfigurationError(format!("Invalid authorization URL: {}", e))
            })?;
            let token_url = TokenUrl::new(token_url_str)
                .map_err(|e| AuthError::ConfigurationError(format!("Invalid token URL: {}", e)))?;
            let redirect_url = RedirectUrl::new(redirect_uri.to_string()).map_err(|e| {
                AuthError::ConfigurationError(format!("Invalid redirect URL: {}", e))
            })?;

            let client = BasicClient::new(client_id)
                .set_client_secret(client_secret)
                .set_auth_uri(auth_url)
                .set_token_uri(token_url)
                .set_redirect_uri(redirect_url);

            let state_clone = CsrfToken::new(state.secret().clone());
            let mut auth_request = client
                .authorize_url(move || state_clone.clone())
                .set_pkce_challenge(pkce_challenge);

            // Add scopes
            for scope in &self.config.scopes {
                auth_request = auth_request.add_scope(Scope::new(scope.clone()));
            }

            let (url, _) = auth_request.url();
            url
        };

        // Create OAuth session
        let timeout_seconds = self.config.session_timeout_seconds.unwrap_or(300);
        let expires_at: DateTime<Utc> = Utc::now() + Duration::seconds(timeout_seconds);

        let session = OAuthSession {
            id: Uuid::new_v4(),
            state: state.secret().clone(),
            provider_id: self.provider_id,
            pkce_verifier: Some(pkce_verifier.secret().clone()),
            nonce: Some(nonce.secret().clone()),
            redirect_uri: redirect_uri.to_string(),
            created_at: Utc::now(),
            expires_at,
            return_to: return_to.map(|s| s.to_string()),
        };

        AuthRepository::new(self.pool.clone()).create_oauth_session(&session)
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to create session: {}", e)))?;

        Ok(OAuthResult {
            redirect_url: auth_url.to_string(),
            session_key: state.secret().clone(), // Use state as session key for callback
        })
    }

    async fn handle_oauth_callback(
        &self,
        code: &str,
        state: &str,
        _session_key: &str, // Not used - we use state directly
    ) -> Result<AuthResult, AuthError> {
        // Get and validate session by state
        let session = AuthRepository::new(self.pool.clone()).get_oauth_session_by_state(state)
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to get session: {}", e)))?
            .ok_or_else(|| {
                AuthError::InvalidCredentials("Invalid or expired session".to_string())
            })?;

        if session.provider_id != self.provider_id {
            return Err(AuthError::InvalidCredentials(
                "Provider mismatch".to_string(),
            ));
        }

        let redirect_uri = &session.redirect_uri;

        let pkce_verifier = session
            .pkce_verifier
            .as_ref()
            .ok_or_else(|| AuthError::InternalError("No PKCE verifier in session".to_string()))?;

        // Exchange code for token
        let (_access_token, user_info) = if let Some(issuer_url) = &self.config.issuer_url {
            // OIDC flow - create client inline to avoid type parameter issues
            validate_issuer_url(issuer_url)?;
            let issuer = IssuerUrl::new(issuer_url.clone())
                .map_err(|e| AuthError::ConfigurationError(format!("Invalid issuer URL: {}", e)))?;

            let metadata = CoreProviderMetadata::discover_async(issuer, &async_http_client)
                .await
                .map_err(|e| {
                    AuthError::ConfigurationError(format!(
                        "Failed to discover OIDC metadata: {}",
                        e
                    ))
                })?;

            let client_id = ClientId::new(self.config.client_id.clone());
            let client_secret = ClientSecret::new(self.config.client_secret.clone());
            let redirect_url = RedirectUrl::new(redirect_uri.to_string()).map_err(|e| {
                AuthError::ConfigurationError(format!("Invalid redirect URL: {}", e))
            })?;

            let client =
                CoreClient::from_provider_metadata(metadata, client_id, Some(client_secret))
                    .set_redirect_uri(redirect_url);

            let nonce_str = session
                .nonce
                .as_ref()
                .ok_or_else(|| AuthError::InternalError("No nonce in session".to_string()))?;

            let token_request = client
                .exchange_code(AuthorizationCode::new(code.to_string()))
                .map_err(|e| {
                    AuthError::ConfigurationError(format!(
                        "Failed to create token request: {:?}",
                        e
                    ))
                })?;

            let token_response = token_request
                .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.clone()))
                .request_async(&async_http_client)
                .await
                .map_err(|e| {
                    AuthError::InvalidCredentials(format!("Token exchange failed: {}", e))
                })?;

            let id_token = token_response
                .id_token()
                .ok_or_else(|| AuthError::InternalError("No ID token in response".to_string()))?;

            // Peek at `tid` from the JWT payload BEFORE verification.
            // Required for two reasons:
            //   (a) Microsoft's `common` endpoint discovery returns the
            //       templated issuer `https://login.microsoftonline.com/{tenantid}/v2.0`
            //       (literal curly-brace placeholder), which fails
            //       openidconnect's strict issuer-equality check. We
            //       substitute `{tenantid}` with the token's tid and
            //       re-do discovery against the resulting single-tenant
            //       URL before verifying.
            //   (b) Tenant-allowlist enforcement when configured.
            // The peek is safe — it reads JSON from the verified-signature
            // payload only AFTER we re-verify below. We never trust the
            // peeked value beyond using it to pick the right verifier.
            let id_token_jwt = id_token.to_string();
            let token_tid = extract_string_claim(&id_token_jwt, "tid");

            let needs_substitution = issuer_url.contains("{tenantid}");

            // Enforce the allowlist + substitute issuer URL if templated.
            // Order matters: reject BEFORE the expensive re-discovery.
            let (verify_client, _verify_client_holder);
            let nonce = Nonce::new(nonce_str.clone());

            if needs_substitution {
                let tid = token_tid.as_deref().ok_or_else(|| {
                    AuthError::InvalidCredentials(
                        "Microsoft templated-issuer flow requires a `tid` claim on the ID token".to_string(),
                    )
                })?;
                // A templated issuer means a multi-tenant endpoint; without
                // an explicit allowlist ANY Microsoft tenant in the world
                // can log in. Refuse to operate in this footgun configuration.
                // Treat `Some(empty vec)` exactly like `None` — an empty
                // allowlist is almost always a UI mistake (saved without
                // any entries) and silently producing "no tenant can log
                // in" is more confusing than a clear error.
                let allowed = self
                    .config
                    .allowed_tenant_ids
                    .as_ref()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| {
                        AuthError::ConfigurationError(format!(
                            "Provider '{}' uses templated issuer URL but `allowed_tenant_ids` is empty or missing — any Microsoft tenant could log in. Configure `allowed_tenant_ids` with at least one tenant ID.",
                            self.name
                        ))
                    })?;
                if !allowed.iter().any(|t| t.eq_ignore_ascii_case(tid)) {
                    // SECURITY: don't echo the raw tid into the error
                    // string. Tenant IDs are stable per-organization
                    // identifiers; one slipping into a third-party
                    // log aggregator could ID a customer org by name.
                    // Truncate to a non-reversible prefix purely for
                    // debugging operator-side allowlist typos; the
                    // operator can confirm by comparison.
                    let preview: String = tid.chars().take(8).collect();
                    return Err(AuthError::InvalidCredentials(format!(
                        "Tenant (id prefix '{}…') is not in the allowlist for provider '{}'",
                        preview, self.name
                    )));
                }
                // SECURITY: refuse tid values that aren't strictly
                // alphanumeric + hyphen — anything with a slash or
                // dot could path-inject into the substituted URL
                // (`a/../../evil`) and bend discovery toward a
                // different host after url normalization. Reject
                // empty tids too: `"".chars().all(...)` is vacuously
                // true and would let `issuer_url.replace("{tenantid}", "")`
                // produce a `//` URL that may normalize unexpectedly.
                if tid.is_empty()
                    || !tid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    return Err(AuthError::InvalidCredentials(
                        "Tenant ID contains invalid characters".to_string(),
                    ));
                }
                let substituted = issuer_url.replace("{tenantid}", tid);
                // SSRF: validate the substituted URL before discovery
                // (issuer_url itself was checked at config time, but
                // an attacker-controlled tid could in theory redirect
                // through different DNS).
                validate_issuer_url(&substituted)?;
                let sub_issuer = IssuerUrl::new(substituted).map_err(|e| {
                    AuthError::ConfigurationError(format!(
                        "Invalid substituted issuer URL: {}",
                        e
                    ))
                })?;
                let sub_metadata =
                    CoreProviderMetadata::discover_async(sub_issuer, &async_http_client)
                        .await
                        .map_err(|e| {
                            AuthError::ConfigurationError(format!(
                                "Failed to discover OIDC metadata for substituted issuer: {}",
                                e
                            ))
                        })?;
                let sub_client_id = ClientId::new(self.config.client_id.clone());
                let sub_client_secret = ClientSecret::new(self.config.client_secret.clone());
                let sub_redirect_url =
                    RedirectUrl::new(redirect_uri.to_string()).map_err(|e| {
                        AuthError::ConfigurationError(format!("Invalid redirect URL: {}", e))
                    })?;
                _verify_client_holder = CoreClient::from_provider_metadata(
                    sub_metadata,
                    sub_client_id,
                    Some(sub_client_secret),
                )
                .set_redirect_uri(sub_redirect_url);
                verify_client = &_verify_client_holder;
            } else {
                // Single-tenant or non-MS provider: still enforce the
                // allowlist if one is configured, but no substitution.
                if let Some(allowed) = &self.config.allowed_tenant_ids {
                    let tid = token_tid.as_deref().ok_or_else(|| {
                        AuthError::InvalidCredentials(
                            "Tenant allowlist configured but ID token has no `tid` claim".to_string(),
                        )
                    })?;
                    if !allowed.iter().any(|t| t.eq_ignore_ascii_case(tid)) {
                        return Err(AuthError::InvalidCredentials(format!(
                            "Tenant '{}' is not in the allowlist for provider '{}'",
                            tid, self.name
                        )));
                    }
                }
                verify_client = &client;
            }

            // Now verify against the correct (possibly substituted) client.
            let verifier = verify_client.id_token_verifier();
            let _claims = id_token.claims(&verifier, &nonce).map_err(|e| {
                AuthError::InvalidCredentials(format!("ID token verification failed: {}", e))
            })?;

            // Note: AccessTokenHash verification skipped - requires JWK key which is complex to obtain.
            // The ID token verification above provides sufficient security.

            let user_info = self
                .get_user_info_from_token(id_token, &verifier, &nonce)
                .await?;
            (token_response.access_token().secret().clone(), user_info)
        } else {
            // OAuth 2.0 flow - create client inline to avoid type parameter issues
            let client_id = ClientId::new(self.config.client_id.clone());
            let client_secret = ClientSecret::new(self.config.client_secret.clone());
            let auth_url_str = self.config.authorization_url.clone().ok_or_else(|| {
                AuthError::ConfigurationError(
                    "OAuth2 provider requires `authorization_url` (or set `issuer_url` for OIDC)".to_string(),
                )
            })?;
            let token_url_str = self.config.token_url.clone().ok_or_else(|| {
                AuthError::ConfigurationError(
                    "OAuth2 provider requires `token_url` (or set `issuer_url` for OIDC)".to_string(),
                )
            })?;
            let auth_url = AuthUrl::new(auth_url_str).map_err(|e| {
                AuthError::ConfigurationError(format!("Invalid authorization URL: {}", e))
            })?;
            let token_url = TokenUrl::new(token_url_str)
                .map_err(|e| AuthError::ConfigurationError(format!("Invalid token URL: {}", e)))?;
            let redirect_url = RedirectUrl::new(redirect_uri.to_string()).map_err(|e| {
                AuthError::ConfigurationError(format!("Invalid redirect URL: {}", e))
            })?;

            let client = BasicClient::new(client_id)
                .set_client_secret(client_secret)
                .set_auth_uri(auth_url)
                .set_token_uri(token_url)
                .set_redirect_uri(redirect_url);

            let token_response = client
                .exchange_code(AuthorizationCode::new(code.to_string()))
                .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.clone()))
                .request_async(&oauth2_http_client)
                .await
                .map_err(|e| {
                    AuthError::InvalidCredentials(format!("Token exchange failed: {}", e))
                })?;

            let access_token = token_response.access_token().secret().clone();
            let user_info = self.get_user_info_from_api(&access_token).await?;
            (access_token, user_info)
        };

        // SECURITY: require email_verified to be true (or absent — some
        // providers don't include the claim but only return verified
        // emails) before treating the email as authoritative for user
        // matching. If a provider explicitly returns email_verified=false,
        // the email belongs to someone who hasn't yet proven control of
        // it; matching on it would let an attacker take over an account
        // by signing up with someone else's email at a provider that
        // doesn't verify. Closes 01-auth F-09 (High).
        if let Some(verified) = user_info.get("email_verified").and_then(|v| v.as_bool())
            && !verified {
                return Err(AuthError::InvalidCredentials(
                    "OAuth provider returned email_verified=false; refusing to provision".to_string(),
                ));
            }

        // Extract user attributes
        let attributes = self.extract_user_attributes(&user_info)?;

        let external_id = user_info
            .get(&self.config.attribute_mapping.user_id)
            .and_then(|v| v.as_str())
            .ok_or_else(|| AuthError::InternalError("No user ID in token".to_string()))?
            .to_string();

        // Delete session after successful authentication
        let _ = AuthRepository::new(self.pool.clone()).delete_oauth_session(state).await;

        Ok(AuthResult {
            external_id,
            external_username: Some(attributes.username.clone()),
            external_email: Some(attributes.email.clone()),
            metadata: serde_json::json!({
                "provider": self.provider_type(),
                "auth_method": "oauth2",
                "user_info": user_info,
            }),
            attributes,
        })
    }

    async fn test_connection(&self) -> Result<String, AuthError> {
        // Layered checks so the success message tells the admin
        // exactly what was verified — config error vs credential
        // error becomes obvious instead of a single opaque pass/fail.
        let mut messages: Vec<String> = Vec::new();

        // Layer 1: structural validation of config fields.
        if self.config.client_id.trim().is_empty() {
            return Err(AuthError::ConfigurationError(
                "client_id is empty".to_string(),
            ));
        }

        // Layer 2: OIDC discovery (proves issuer URL is reachable + serves a valid doc).
        let (token_url, original_issuer_url): (String, Option<String>) =
            if let Some(issuer_url) = &self.config.issuer_url {
                // For templated Microsoft `common` URLs we can't run
                // real discovery against the literal `{tenantid}`
                // placeholder — `openidconnect` only accepts a fully-
                // qualified issuer. Substitute with a benign placeholder
                // (matches `common` itself, which is what's reachable).
                let probe_issuer_url = issuer_url.replace("{tenantid}", "common");
                validate_issuer_url(&probe_issuer_url)?;
                let issuer = IssuerUrl::new(probe_issuer_url.clone()).map_err(|e| {
                    AuthError::ConfigurationError(format!("Invalid issuer URL: {}", e))
                })?;
                let metadata =
                    CoreProviderMetadata::discover_async(issuer, &async_http_client)
                        .await
                        .map_err(|e| {
                            AuthError::ConnectionFailed(format!(
                                "OIDC discovery failed for {}: {}",
                                probe_issuer_url, e
                            ))
                        })?;
                messages.push("OIDC discovery succeeded".to_string());
                let token_endpoint = metadata.token_endpoint().ok_or_else(|| {
                    AuthError::ConfigurationError(
                        "OIDC metadata missing token_endpoint".to_string(),
                    )
                })?;
                (
                    token_endpoint.url().to_string(),
                    Some(issuer_url.clone()),
                )
            } else {
                // OAuth 2.0 (non-OIDC): URL syntax only.
                let auth_url_str = self.config.authorization_url.clone().ok_or_else(|| {
                    AuthError::ConfigurationError(
                        "OAuth2 provider requires `authorization_url` (or set `issuer_url` for OIDC)".to_string(),
                    )
                })?;
                let token_url_str = self.config.token_url.clone().ok_or_else(|| {
                    AuthError::ConfigurationError(
                        "OAuth2 provider requires `token_url` (or set `issuer_url` for OIDC)".to_string(),
                    )
                })?;
                AuthUrl::new(auth_url_str).map_err(|e| {
                    AuthError::ConfigurationError(format!("Invalid authorization URL: {}", e))
                })?;
                TokenUrl::new(token_url_str.clone()).map_err(|e| {
                    AuthError::ConfigurationError(format!("Invalid token URL: {}", e))
                })?;
                messages.push("OAuth2 URLs are valid".to_string());
                (token_url_str, None)
            };

        // Layer 3: dummy token-exchange probe — distinguishes wrong
        // credentials from wrong URL. Skipped if client_secret is
        // empty (we'd just see a guaranteed invalid_client and the
        // admin hasn't actually entered anything to probe).
        if self.config.client_secret.trim().is_empty() {
            messages.push("client_secret empty; skipped credential probe".to_string());
            return Ok(messages.join("; "));
        }

        match probe_oidc_credentials(
            &token_url,
            &self.config.client_id,
            &self.config.client_secret,
        )
        .await
        {
            ProbeResult::CredentialsOk => messages.push(
                "credentials accepted (token endpoint returned `invalid_grant` for our dummy probe — proves client_id/secret are recognized)"
                    .to_string(),
            ),
            ProbeResult::CredentialsBad(detail) => {
                return Err(AuthError::InvalidCredentials(format!(
                    "token endpoint rejected client_id/client_secret: {}",
                    detail
                )));
            }
            ProbeResult::RedirectUriMismatch => messages.push(
                "credentials format OK; redirect URI not registered with provider (register `<your-server>/api/auth/oauth/<name>/callback` in the provider's console)"
                    .to_string(),
            ),
            ProbeResult::NetworkError(e) => {
                return Err(AuthError::ConnectionFailed(format!(
                    "token endpoint unreachable: {}",
                    e
                )));
            }
            ProbeResult::Unexpected(s) => {
                messages.push(format!("token endpoint returned unexpected response: {}", s))
            }
        }

        // For templated `common` issuers, note that the per-tenant
        // validation only kicks in at real login time.
        if original_issuer_url
            .as_deref()
            .map(|s| s.contains("{tenantid}"))
            .unwrap_or(false)
        {
            messages.push(
                "templated issuer uses `{tenantid}`; per-tenant validation happens at real login via the allowed_tenant_ids allowlist"
                    .to_string(),
            );
        }

        Ok(messages.join("; "))
    }

    fn get_config(&self) -> &serde_json::Value {
        &self.raw_config
    }
}
