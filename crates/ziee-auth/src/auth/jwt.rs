use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use ziee_core::AppError;

/// The JWT signer's settings — an auth-owned mirror of the app's
/// `JwtConfig` (Chunk BG). Threading this instead of naming
/// the app config's `JwtConfig` is what lets the JWT service compile
/// without reaching the app config crate; the app installs the values via
/// `From<JwtConfig>` at the two construction sites (`lib.rs` / `main.rs`).
/// Field-for-field identical to `JwtConfig`, so the conversion is a pure
/// move — behaviour is unchanged.
#[derive(Debug, Clone)]
pub struct JwtSettings {
    pub secret: String,
    pub issuer: String,
    pub audience: String,
    pub access_token_expiry_hours: i64,
    pub refresh_token_expiry_days: i64,
    pub access_token_expiry_seconds: Option<i64>,
}

/// The app-config → auth-settings bridge. `ziee-auth` owns `JwtSettings` and
/// depends on `ziee-core` (which owns `JwtConfig`), so this conversion lives
/// here (Chunk BA-full: it moved out of the app's `core/config.rs`, where it
/// became an orphan-rule violation once `JwtSettings` left the app crate).
/// Field-for-field identical — behaviour unchanged; every
/// `JwtService::try_new(config.jwt.into())` call site keeps working.
impl From<ziee_core::config::JwtConfig> for JwtSettings {
    fn from(c: ziee_core::config::JwtConfig) -> Self {
        JwtSettings {
            secret: c.secret,
            issuer: c.issuer,
            audience: c.audience,
            access_token_expiry_hours: c.access_token_expiry_hours,
            refresh_token_expiry_days: c.refresh_token_expiry_days,
            access_token_expiry_seconds: c.access_token_expiry_seconds,
        }
    }
}

/// JWT claims structure
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,      // Subject (user ID)
    pub exp: i64,         // Expiration time
    pub iat: i64,         // Issued at
    pub iss: String,      // Issuer
    pub aud: String,      // Audience
    pub username: String, // Username
    pub email: String,    // Email
    pub is_admin: bool,   // Admin flag
    /// JWT ID — populated only on refresh tokens (used for the whitelist
    /// lookup in modules/auth/refresh_tokens.rs that closes 01-auth F-02
    /// + F-03). Optional + default so existing tests that hand-mint
    /// claims without a jti continue to deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// Access-token revocation epoch — a snapshot of `users.token_version`
    /// taken at mint time. Populated only on ACCESS tokens; refresh tokens
    /// are revoked via the `refresh_tokens` whitelist instead, so a second
    /// gate there would be redundant.
    ///
    /// Compared for EQUALITY against the live column on every authenticated
    /// request (see `jwt_extractor::verify_token_version`); logout bumps the
    /// column, so every token minted before it stops validating at once.
    /// Optional + `default` so tokens minted BEFORE this shipped keep working
    /// for the rest of their TTL (absent => `unwrap_or(0)` => matches the
    /// column's `DEFAULT 0`) — deploying this forces zero logouts. Same
    /// rationale as `jti` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ver: Option<i32>,
}

/// JWT token pair (access + refresh)
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
}

/// TokenPair + the refresh token's jti + expires_at.
///
/// Returned by JwtService::generate_tokens_with_jti so the caller can
/// register the refresh-token row in the `refresh_tokens` whitelist
/// before the token is handed back to the user. See the comment on
/// generate_tokens_with_jti for the two-step protocol that closes
/// 01-auth F-02 + F-03.
#[derive(Debug)]
pub struct TokenPairWithJti {
    pub pair: TokenPair,
    pub refresh_jti: Uuid,
    pub refresh_expires_at: chrono::DateTime<Utc>,
}

/// JWT service for token generation and validation
#[derive(Clone)]
pub struct JwtService {
    config: JwtSettings,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

/// Minimum acceptable JWT secret length (bytes). HMAC-SHA256 ideally
/// uses ≥ 32 bytes of entropy. Closes 01-auth F-10 + 14-core F-03.
const MIN_JWT_SECRET_LEN: usize = 32;

/// `exp`/`nbf` validation slack, in seconds. Small because the issuer and
/// validator are the same process (no cross-host clock skew); see
/// `validate_access_token`.
const JWT_LEEWAY_SECONDS: u64 = 5;

/// Known shipped placeholder secrets. Refuse to boot with any of these
/// — if an operator's config still contains a template value they
/// almost certainly forgot to override it, and the secret is in public
/// source control. Plain string match, not substring, so genuine
/// 32+-char operator secrets aren't accidentally rejected.
const BANNED_JWT_SECRETS: &[&str] = &[
    "dev-secret-change-in-production-min-32-chars-long",
    "REPLACE_ME_WITH_A_LONG_RANDOM_SECRET_AT_LEAST_32_CHARS",
    "your-secret-key-here",
    "change-me",
    "secret",
    "changeme",
];

impl JwtService {
    /// Create a new JWT service. Errors if the secret is shorter than
    /// MIN_JWT_SECRET_LEN bytes or matches a known shipped placeholder.
    /// Closes 01-auth F-10 + 14-core F-03 (weak/default JWT secret
    /// accepted at runtime). Callers (main.rs, lib.rs) propagate the
    /// error so the server refuses to boot rather than continuing with
    /// a weak signer.
    pub fn try_new(config: impl Into<JwtSettings>) -> Result<Self, AppError> {
        let config: JwtSettings = config.into();
        if config.secret.len() < MIN_JWT_SECRET_LEN {
            return Err(AppError::internal_error(format!(
                "JWT secret is {} bytes; minimum is {}. Set jwt.secret in \
                 your config to a random ≥32-char string (e.g. \
                 `openssl rand -base64 48`).",
                config.secret.len(),
                MIN_JWT_SECRET_LEN
            )));
        }
        if BANNED_JWT_SECRETS.iter().any(|p| *p == config.secret) {
            return Err(AppError::internal_error(
                "JWT secret matches a shipped placeholder value. Set \
                 jwt.secret in your config to a unique random ≥32-char \
                 string (e.g. `openssl rand -base64 48`).",
            ));
        }

        let encoding_key = EncodingKey::from_secret(config.secret.as_bytes());
        let decoding_key = DecodingKey::from_secret(config.secret.as_bytes());

        Ok(Self {
            config,
            encoding_key,
            decoding_key,
        })
    }

    /// Infallible constructor preserved for tests / callers that have
    /// already validated the secret. Production code MUST use `try_new`
    /// so a weak secret aborts boot. This thin wrapper panics on a bad
    /// secret so misuse can't go unnoticed.
    ///
    /// Used cross-crate by the `ziee-desktop` integration tests, so it
    /// appears unused from the `ziee` crate's own build — keep it.
    #[allow(dead_code)]
    pub fn new(config: impl Into<JwtSettings>) -> Self {
        Self::try_new(config).expect("JWT secret validation failed; use try_new for graceful errors")
    }

    /// The YAML-config lifetimes — the mint-time FALLBACK used when the
    /// `session_settings` DB read fails (see `mint_session_tokens` in
    /// refresh_tokens.rs). Returns `(access_hours, refresh_days)`.
    pub(crate) fn config_expiries(&self) -> (i64, i64) {
        (
            self.config.access_token_expiry_hours,
            self.config.refresh_token_expiry_days,
        )
    }

    /// Access-token TTL as (duration, whole_seconds), honoring the
    /// DEBUG-ONLY `jwt.access_token_expiry_seconds` test seam. The seam is
    /// physically inert in release builds (`cfg!(debug_assertions)`), so a
    /// production config carrying the field cannot shorten tokens.
    fn access_expiry(&self, access_hours: i64) -> (Duration, i64) {
        if cfg!(debug_assertions)
            && let Some(secs) = self.config.access_token_expiry_seconds
        {
            return (Duration::seconds(secs), secs);
        }
        (Duration::hours(access_hours), access_hours * 3600)
    }

    /// Generate access and refresh tokens with explicit lifetimes
    /// (`access_hours` for the access token, `refresh_days` for the
    /// refresh token — normally the `session_settings` values resolved
    /// by `session_expiries`).
    ///
    /// Returns the TokenPair plus the refresh token's `jti` and
    /// `expires_at`. The caller MUST then write a row to
    /// `refresh_tokens` so the new refresh token is whitelisted; without
    /// that follow-up write, the whitelist check will reject the
    /// freshly-issued token. The two-step protocol (mint then register)
    /// is deliberate — it lets callers fail closed if the DB write fails,
    /// without minting a usable secret. Closes 01-auth F-02 + F-03.
    ///
    /// Most callers should use `mint_session_tokens` (refresh_tokens.rs),
    /// which resolves the lifetimes AND handles the registration; the
    /// refresh handler is the one caller that sequences the steps itself
    /// (generate → revoke_rotated → register).
    pub fn generate_tokens_with_jti_expiry(
        &self,
        user_id: Uuid,
        username: &str,
        email: &str,
        is_admin: bool,
        access_hours: i64,
        refresh_days: i64,
        token_version: i32,
    ) -> Result<TokenPairWithJti, AppError> {
        let (_, expires_in) = self.access_expiry(access_hours);
        let access_token = self.generate_access_token(
            user_id,
            username,
            email,
            is_admin,
            access_hours,
            token_version,
        )?;
        let (refresh_token, refresh_jti, refresh_expires_at) =
            self.generate_refresh_token_with_jti(user_id, refresh_days)?;

        Ok(TokenPairWithJti {
            pair: TokenPair {
                access_token,
                refresh_token,
                token_type: "Bearer".to_string(),
                expires_in,
            },
            refresh_jti,
            refresh_expires_at,
        })
    }

    /// Re-issue a session token pair BINDING the refresh token to an
    /// EXISTING, already-whitelisted `refresh_jti` (the rotation-grace
    /// successor) with its `refresh_expires_at`, rather than minting a new
    /// jti. Used ONLY by the refresh handler's grace path so a racing /
    /// replayed-within-grace presentation converges onto the successor
    /// family instead of forking an independent chain — no new
    /// `refresh_tokens` row is created (the jti is already registered).
    /// The access token is fresh (new exp); the refresh token simply
    /// re-encodes the existing jti + exp.
    pub fn reissue_tokens_for_jti(
        &self,
        user_id: Uuid,
        username: &str,
        email: &str,
        is_admin: bool,
        access_hours: i64,
        refresh_jti: Uuid,
        refresh_expires_at: chrono::DateTime<Utc>,
        token_version: i32,
    ) -> Result<TokenPair, AppError> {
        let (_, expires_in) = self.access_expiry(access_hours);
        let access_token = self.generate_access_token(
            user_id,
            username,
            email,
            is_admin,
            access_hours,
            token_version,
        )?;

        let now = Utc::now();
        let claims = Claims {
            sub: user_id.to_string(),
            exp: refresh_expires_at.timestamp(),
            iat: now.timestamp(),
            iss: self.config.issuer.clone(),
            aud: format!("{}-refresh", self.config.audience),
            username: String::new(),
            email: String::new(),
            is_admin: false,
            jti: Some(refresh_jti.to_string()),
            // Refresh tokens are revoked via the `refresh_tokens` whitelist,
            // not the epoch — see the `ver` doc on `Claims`.
            ver: None,
        };
        let refresh_token = encode(&Header::default(), &claims, &self.encoding_key).map_err(|e| {
            AppError::internal_error(format!("Failed to re-issue refresh token: {}", e))
        })?;

        Ok(TokenPair {
            access_token,
            refresh_token,
            token_type: "Bearer".to_string(),
            expires_in,
        })
    }

    /// Generate an access token with the given TTL in hours.
    ///
    /// `token_version` is the caller's snapshot of `users.token_version`; it
    /// is stamped as the `ver` claim and is what makes the token revocable by
    /// a logout. Callers MUST read it from the DB rather than pass a constant.
    fn generate_access_token(
        &self,
        user_id: Uuid,
        username: &str,
        email: &str,
        is_admin: bool,
        access_hours: i64,
        token_version: i32,
    ) -> Result<String, AppError> {
        let now = Utc::now();
        let (ttl, _) = self.access_expiry(access_hours);
        let exp = now + ttl;

        let claims = Claims {
            sub: user_id.to_string(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
            iss: self.config.issuer.clone(),
            aud: self.config.audience.clone(),
            username: username.to_string(),
            email: email.to_string(),
            is_admin,
            jti: None,
            ver: Some(token_version),
        };

        encode(&Header::default(), &claims, &self.encoding_key).map_err(|e| {
            AppError::internal_error(format!("Failed to generate access token: {}", e))
        })
    }

    /// Generate a refresh token (simpler claims, longer expiry, carries
    /// a jti for whitelist tracking) with the given TTL in days.
    ///
    /// Returns (token, jti, expires_at). Callers must register the jti
    /// in the `refresh_tokens` table — see generate_tokens_with_jti for
    /// the two-step protocol.
    fn generate_refresh_token_with_jti(
        &self,
        user_id: Uuid,
        refresh_days: i64,
    ) -> Result<(String, Uuid, chrono::DateTime<Utc>), AppError> {
        let now = Utc::now();
        let exp = now + Duration::days(refresh_days);
        let jti = Uuid::new_v4();

        let claims = Claims {
            sub: user_id.to_string(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
            iss: self.config.issuer.clone(),
            aud: format!("{}-refresh", self.config.audience),
            username: String::new(),
            email: String::new(),
            is_admin: false,
            jti: Some(jti.to_string()),
            // Refresh tokens are revoked via the `refresh_tokens` whitelist,
            // not the epoch — see the `ver` doc on `Claims`.
            ver: None,
        };

        let token = encode(&Header::default(), &claims, &self.encoding_key).map_err(|e| {
            AppError::internal_error(format!("Failed to generate refresh token: {}", e))
        })?;

        Ok((token, jti, exp))
    }

    /// Validate and decode an access token
    pub fn validate_access_token(&self, token: &str) -> Result<Claims, AppError> {
        let mut validation = Validation::default();
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&self.config.audience]);
        // jsonwebtoken defaults `leeway` to 60s (clock-skew slack between a
        // separate issuer and validator). Here the issuer IS the validator
        // (same process), so skew is ~0 — a 60s grace on `exp` would make a
        // configured short access TTL (the admin "shorter is safer" knob)
        // effectively 60s longer and delay cutting off a refreshed/deactivated
        // session. A small cushion still absorbs sub-second scheduling.
        validation.leeway = JWT_LEEWAY_SECONDS;

        decode::<Claims>(token, &self.decoding_key, &validation)
            .map(|data| data.claims)
            .map_err(|e| {
                AppError::unauthorized("INVALID_TOKEN", format!("Invalid or expired token: {}", e))
            })
    }

    /// Validate and decode a refresh token
    pub fn validate_refresh_token(&self, token: &str) -> Result<Claims, AppError> {
        let mut validation = Validation::default();
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&format!("{}-refresh", self.config.audience)]);
        validation.leeway = JWT_LEEWAY_SECONDS;

        decode::<Claims>(token, &self.decoding_key, &validation)
            .map(|data| data.claims)
            .map_err(|e| {
                AppError::unauthorized(
                    "INVALID_REFRESH_TOKEN",
                    format!("Invalid or expired refresh token: {}", e),
                )
            })
    }

    /// Extract token from Authorization header
    pub fn extract_token_from_header(auth_header: &str) -> Result<&str, AppError> {
        if !auth_header.starts_with("Bearer ") {
            return Err(AppError::unauthorized(
                "INVALID_AUTH_HEADER",
                "Authorization header must start with 'Bearer '",
            ));
        }

        let token = &auth_header[7..];
        if token.is_empty() {
            return Err(AppError::unauthorized(
                "MISSING_TOKEN",
                "Token is missing from Authorization header",
            ));
        }

        Ok(token)
    }
}

// Chunk B1b: the concrete `JwtService` (HMAC keys, issuer/audience config,
// jsonwebtoken decode + leeway, AppError mapping) STAYS in ziee and implements
// the framework's JWT-verify INTERFACE. Framework enforcement depends only on
// `ziee_identity::TokenVerifier`, never on this concrete service or on
// jsonwebtoken/AppError; the associated types carry ziee's concrete `Claims`
// and `AppError`. This is a thin delegation to the existing methods — the
// validation logic is unchanged.
impl ziee_identity::TokenVerifier for JwtService {
    type Claims = Claims;
    type Error = AppError;

    fn verify_access_token(&self, token: &str) -> Result<Self::Claims, Self::Error> {
        self.validate_access_token(token)
    }

    fn verify_refresh_token(&self, token: &str) -> Result<Self::Claims, Self::Error> {
        self.validate_refresh_token(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(access_seconds: Option<i64>) -> JwtSettings {
        JwtSettings {
            secret: "unit-test-jwt-secret-with-at-least-32-chars!".to_string(),
            issuer: "ziee".to_string(),
            audience: "ziee-api".to_string(),
            access_token_expiry_hours: 24,
            refresh_token_expiry_days: 30,
            access_token_expiry_seconds: access_seconds,
        }
    }

    /// TEST-1: the access token carries the `token_version` it was minted
    /// with as the `ver` claim — the value the extractors compare against
    /// `users.token_version` to reject a logged-out session. The refresh
    /// token deliberately carries NO `ver` (the `refresh_tokens` whitelist
    /// revokes it instead).
    #[test]
    fn access_token_carries_the_minted_ver_claim() {
        let svc = JwtService::try_new(test_config(None)).unwrap();
        let user = Uuid::new_v4();
        let minted = svc
            .generate_tokens_with_jti_expiry(user, "u", "u@x", false, 2, 7, 7)
            .unwrap();

        let access = svc.validate_access_token(&minted.pair.access_token).unwrap();
        assert_eq!(
            access.ver,
            Some(7),
            "access token must carry the minted token_version as `ver`"
        );

        let refresh = svc
            .validate_refresh_token(&minted.pair.refresh_token)
            .unwrap();
        assert_eq!(
            refresh.ver, None,
            "refresh tokens are revoked via the whitelist, not the epoch"
        );
    }

    /// TEST-2: the back-compat contract that makes deploying this a
    /// zero-forced-logout change. A token minted BEFORE `ver` existed has no
    /// such claim; it must still deserialize, with `ver == None` (which
    /// `verify_token_version` maps to 0 → matches the column's DEFAULT 0).
    /// Encoded by hand rather than via the minter, because the minter can no
    /// longer produce a `ver`-less access token.
    #[test]
    fn a_ver_less_token_deserializes_as_none() {
        let cfg = test_config(None);
        let svc = JwtService::try_new(cfg.clone()).unwrap();
        let now = Utc::now();

        // A pre-upgrade access token: same claims, minus `ver`.
        let legacy = serde_json::json!({
            "sub": Uuid::new_v4().to_string(),
            "exp": (now + Duration::hours(1)).timestamp(),
            "iat": now.timestamp(),
            "iss": cfg.issuer,
            "aud": cfg.audience,
            "username": "u",
            "email": "u@x",
            "is_admin": false,
        });
        let token = encode(
            &Header::default(),
            &legacy,
            &EncodingKey::from_secret(cfg.secret.as_bytes()),
        )
        .unwrap();

        let claims = svc
            .validate_access_token(&token)
            .expect("a ver-less token must still validate");
        assert_eq!(claims.ver, None);
        assert_eq!(claims.jti, None);
    }

    /// The explicit-lifetime mint honors its `access_hours` /
    /// `refresh_days` args (the session_settings values) rather than
    /// the config defaults, including in `expires_in`.
    #[test]
    fn expiry_override_variant_honored() {
        let svc = JwtService::try_new(test_config(None)).unwrap();
        let user = Uuid::new_v4();
        let minted = svc
            .generate_tokens_with_jti_expiry(user, "u", "u@x", false, 2, 7, 0)
            .unwrap();

        assert_eq!(minted.pair.expires_in, 2 * 3600);

        let now = Utc::now().timestamp();
        let access = svc.validate_access_token(&minted.pair.access_token).unwrap();
        let access_ttl = access.exp - now;
        assert!(
            (2 * 3600 - 60..=2 * 3600 + 60).contains(&access_ttl),
            "access exp ≈ now+2h, got ttl {access_ttl}s"
        );

        let refresh = svc
            .validate_refresh_token(&minted.pair.refresh_token)
            .unwrap();
        let refresh_ttl = refresh.exp - now;
        let seven_days = 7 * 24 * 3600;
        assert!(
            (seven_days - 60..=seven_days + 60).contains(&refresh_ttl),
            "refresh exp ≈ now+7d, got ttl {refresh_ttl}s"
        );
        // The refresh token carries a jti and its expires_at matches exp.
        assert!(refresh.jti.is_some());
        assert_eq!(minted.refresh_expires_at.timestamp(), refresh.exp);
    }

    /// DEBUG-ONLY seam: `jwt.access_token_expiry_seconds` overrides the
    /// hour-granularity TTL (and `expires_in`) so integration/e2e suites
    /// can exercise real expiry in seconds. This test only asserts the
    /// debug behavior — the release build compiles the seam out.
    #[test]
    fn debug_seconds_override_wins() {
        let svc = JwtService::try_new(test_config(Some(5))).unwrap();
        let user = Uuid::new_v4();
        let minted = svc
            .generate_tokens_with_jti_expiry(user, "u", "u@x", false, 24, 30, 0)
            .unwrap();

        assert_eq!(minted.pair.expires_in, 5);
        let now = Utc::now().timestamp();
        let access = svc.validate_access_token(&minted.pair.access_token).unwrap();
        let ttl = access.exp - now;
        assert!(
            (0..=6).contains(&ttl),
            "access exp ≈ now+5s under the debug seam, got ttl {ttl}s"
        );
        // The refresh token is NOT affected by the seam.
        let refresh = svc
            .validate_refresh_token(&minted.pair.refresh_token)
            .unwrap();
        let refresh_ttl = refresh.exp - now;
        let thirty_days = 30 * 24 * 3600;
        assert!(
            (thirty_days - 60..=thirty_days + 60).contains(&refresh_ttl),
            "refresh unaffected by the seconds seam, got ttl {refresh_ttl}s"
        );
    }

    /// Weak/placeholder secrets are refused at construction.
    #[test]
    fn weak_secret_refused() {
        let mut cfg = test_config(None);
        cfg.secret = "short".to_string();
        assert!(JwtService::try_new(cfg).is_err());

        let mut cfg = test_config(None);
        cfg.secret = "dev-secret-change-in-production-min-32-chars-long".to_string();
        assert!(JwtService::try_new(cfg).is_err());
    }

    /// REJECT PATH (gap E-1): a signature-tampered access token must fail
    /// validation. A JWT is `header.payload.signature`; flipping a char in the
    /// signature breaks the HMAC and `decode` must error rather than trust it.
    #[test]
    fn tampered_signature_is_rejected() {
        let svc = JwtService::try_new(test_config(None)).unwrap();
        let user = Uuid::new_v4();
        let minted = svc
            .generate_tokens_with_jti_expiry(user, "u", "u@x", false, 24, 30, 0)
            .unwrap();
        let token = minted.pair.access_token;

        // A pristine token still validates (control).
        assert!(svc.validate_access_token(&token).is_ok());

        // Flip the last char of the signature segment to a definitely-different
        // base64url char.
        let (head, sig) = token.rsplit_once('.').expect("jwt has 3 segments");
        let last = sig.chars().last().expect("non-empty signature");
        let replacement = if last == 'A' { 'B' } else { 'A' };
        let mut new_sig: String = sig[..sig.len() - 1].to_string();
        new_sig.push(replacement);
        let tampered = format!("{head}.{new_sig}");
        assert_ne!(tampered, token, "the tamper actually changed the token");

        assert!(
            svc.validate_access_token(&tampered).is_err(),
            "a token with a corrupted signature must be rejected"
        );
    }

    /// REJECT PATH (gap E-1): audience separation. A refresh token (audience
    /// `ziee-api-refresh`) must NOT pass `validate_access_token` (audience
    /// `ziee-api`), and vice-versa — otherwise a long-lived refresh token would
    /// be usable as an access token.
    #[test]
    fn refresh_token_is_rejected_as_access_token() {
        let svc = JwtService::try_new(test_config(None)).unwrap();
        let user = Uuid::new_v4();
        let minted = svc
            .generate_tokens_with_jti_expiry(user, "u", "u@x", false, 24, 30, 0)
            .unwrap();

        // The refresh token is a valid refresh token...
        assert!(svc.validate_refresh_token(&minted.pair.refresh_token).is_ok());
        // ...but must be rejected on the access-token path (wrong audience).
        assert!(
            svc.validate_access_token(&minted.pair.refresh_token).is_err(),
            "a refresh token must not be accepted as an access token"
        );
        // And symmetrically: an access token is not a valid refresh token.
        assert!(
            svc.validate_refresh_token(&minted.pair.access_token).is_err(),
            "an access token must not be accepted as a refresh token"
        );
    }

    /// REJECT PATH (gap E-1): an already-expired access token is rejected.
    /// The debug-only seconds seam mints with a NEGATIVE TTL so `exp` lands in
    /// the past (well beyond the 5s leeway), and validation must fail.
    #[test]
    fn expired_access_token_is_rejected() {
        let svc = JwtService::try_new(test_config(Some(-3600))).unwrap();
        let user = Uuid::new_v4();
        let minted = svc
            .generate_tokens_with_jti_expiry(user, "u", "u@x", false, 24, 30, 0)
            .unwrap();

        // Access token's exp is ~now-3600 → expired.
        assert!(
            svc.validate_access_token(&minted.pair.access_token).is_err(),
            "an access token whose exp is in the past must be rejected"
        );
        // The refresh token is unaffected by the access-only seconds seam, so it
        // remains valid — proving the rejection is expiry, not a broken signer.
        assert!(
            svc.validate_refresh_token(&minted.pair.refresh_token).is_ok(),
            "the refresh token (30d TTL) is still valid"
        );
    }
}
