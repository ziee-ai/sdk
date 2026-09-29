use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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
    /// JWT ID (RFC 7519 §4.1.7, REQUIRED by RFC 9068 §2.2) — unique per
    /// token. On refresh tokens it is the whitelist key (the `refresh_tokens`
    /// lookup that closes 01-auth F-02 + F-03); on access tokens it is a fresh
    /// v4 UUID. Optional + default so tokens minted before access tokens
    /// carried one (and hand-minted test claims) continue to deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// Access-token revocation epoch — a snapshot of the token's SESSION
    /// epoch (`auth_sessions.ver`, itself initialised from
    /// `users.token_version` at sign-in) taken at mint time; for a legacy
    /// session-less mint, a snapshot of `users.token_version`. Populated only
    /// on ACCESS tokens; refresh tokens are revoked via the `refresh_tokens`
    /// whitelist instead, so a second gate there would be redundant.
    ///
    /// Compared for EQUALITY against the live session row (or, for a token
    /// with no `sid`, the live `users.token_version`) on every authenticated
    /// request (see `jwt_extractor::verify_token_version`); logout ends the
    /// session rows and bumps the column, so every token minted before it
    /// stops validating at once.
    /// Optional + `default` so tokens minted BEFORE this shipped keep working
    /// for the rest of their TTL (absent => `unwrap_or(0)` => matches the
    /// column's `DEFAULT 0`) — deploying this forces zero logouts. Same
    /// rationale as `jti` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ver: Option<i32>,
    /// Session id (`sid`, the OpenID Connect session-id claim) — the
    /// `auth_sessions` row this token belongs to. Stamped on the access AND
    /// the refresh token by `mint_session_tokens` / the refresh handler.
    ///
    /// For an access token carrying `sid`, the revocation epoch `ver` is
    /// compared against THAT session row's `ver` (`sessions::get_session_version`),
    /// not against `users.token_version` — see
    /// `jwt_extractor::assert_session_epoch_current`. Absent on tokens minted
    /// before the session record existed; those keep the `users.token_version`
    /// compare (same back-compat rationale as `ver`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sid: Option<Uuid>,
    /// RFC 9068 §2.2 `client_id` — the client the token was issued to.
    /// Supplied by the app's [`TokenClaimsSource`] at sign-in; `None` from the
    /// default source (today's behaviour) and on pre-delta tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// RFC 9068 §2.2.1 / RFC 8176 authentication methods references. Fixed for
    /// every token derived from one sign-in (RFC 9068 §2.2.1): supplied once by
    /// the app's source and COPIED from the refresh token on every refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amr: Option<Vec<String>>,
    /// RFC 9068 §2.2.1 `auth_time` — when the user authenticated (seconds since
    /// the epoch). Fixed across refreshes exactly like `amr`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_time: Option<i64>,
    /// RFC 9449 §6 / RFC 7800 confirmation — the proof-of-possession key the
    /// token is bound to, when the app has one. Fixed across refreshes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cnf: Option<Cnf>,
    /// REFRESH tokens only: the non-default `aud` the session's ACCESS tokens
    /// carry (the refresh token's own `aud` is the refresh audience), so a
    /// refresh re-issues access tokens with the same audience without
    /// re-consulting the app. `None` = the configured audience.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_aud: Option<String>,
}

/// RFC 7800 `cnf` confirmation claim, in the RFC 9449 (DPoP) shape: `jkt` is
/// the base64url JWK SHA-256 thumbprint of the key the token is bound to.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Cnf {
    pub jkt: String,
}

/// The per-sign-in claim values an app supplies at minting — the ones the SDK
/// cannot know by itself. Everything else on the access token is SDK-owned:
/// `sub` (the user id), `iss`/`iat`/`exp`, `jti` (fresh per token), `sid` (the
/// session row the SDK creates) and `ver` (that session's epoch).
///
/// Every field is optional; all-`None` is today's token (see
/// [`DefaultTokenClaimsSource`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessTokenClaimValues {
    /// Overrides the access token's `aud` (default: the configured audience).
    /// A non-default `aud` only VALIDATES if the same source's
    /// [`TokenClaimsSource::accepts_audience`] names it — RFC 9068 §4: the
    /// resource server rejects a token whose `aud` does not name it.
    pub aud: Option<String>,
    pub client_id: Option<String>,
    pub amr: Option<Vec<String>>,
    pub auth_time: Option<i64>,
    pub cnf: Option<Cnf>,
}

/// How the user authenticated for the sign-in being minted — the fact an app
/// needs to supply an honest RFC 8176 `amr` (and to know `auth_time` is a real
/// authentication event). Set by the SDK mint site that performed the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// The user proved the account's LOCAL password (`login` without a
    /// provider or with provider `local`, or a provider row of type `local`).
    Password,
    /// The account was just created and signed in (registration, first-run
    /// setup): the user CHOSE a credential rather than proving one, so this is
    /// a sign-in event without an RFC 8176 authentication method.
    NewAccount,
    /// The user proved a password against an external directory provider
    /// (`login` with a provider row of a non-`local` type, e.g. LDAP).
    DirectoryPassword,
    /// A federated sign-in (OAuth2/OIDC/Apple): the SDK does not learn how the
    /// identity provider authenticated the user.
    Federated,
    /// Linking an external identity to an existing account, proven by the
    /// account's local password.
    LinkAccountPassword,
    /// No authentication event is known to the mint site (e.g. the one-time
    /// upgrade of a legacy jti-less refresh token). An honest source supplies
    /// no `amr`/`auth_time` for it.
    Unspecified,
}

/// What the SDK tells the app's [`TokenClaimsSource`] about the sign-in it is
/// minting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MintContext {
    pub user_id: Uuid,
    /// The session row created for this sign-in (the token's `sid`); it exists
    /// in the mint's transaction when the source is consulted.
    pub session_id: Uuid,
    /// That session's epoch (`auth_sessions.ver`, the token's `ver`), so a
    /// claim record the app writes for the session carries the same value
    /// (memo identity-and-access-339 §1 point 2).
    pub ver: i32,
    /// How the user authenticated (see [`AuthMethod`]).
    pub method: AuthMethod,
}

/// The app's minting hook: supplies the RFC 9068 claim values the SDK cannot
/// know (who the client is, how the user authenticated, when, and any PoP key).
///
/// Consulted ONCE per session, inside the transaction that creates it
/// (`refresh_tokens::mint_session_tokens_for`, and the refresh handler's
/// adoption of a legacy family), AFTER the session row is inserted and while
/// that transaction holds the user's `users` row share lock — so whatever the
/// app records for the session is atomic with the session row and strictly
/// ordered with a logout (which updates that `users` row). Two hooks:
///   * [`claims_for`](Self::claims_for) returns the values; it must NOT touch the
///     database through its own connection (it would wait on the very lock the
///     mint holds);
///   * [`record_session`](Self::record_session) writes the app's claim record
///     through the mint's OWN connection; an `Err` aborts the whole mint (no
///     session, no refresh row, no token).
///
/// It is NEVER consulted on refresh: the refresh token carries the values and the
/// refresh handler copies them forward, so they stay fixed for every token
/// derived from one authorization (RFC 9068 §2.2.1). Install with
/// [`JwtService::with_token_claims_source`]; the SDK default is
/// [`DefaultTokenClaimsSource`].
#[async_trait::async_trait]
pub trait TokenClaimsSource: Send + Sync {
    /// The claim values for the sign-in described by `ctx`. PURE: it runs
    /// inside the mint's open transaction (a pooled connection held, the
    /// user's `users` row share-locked), so it must do no I/O of its own —
    /// database work belongs in [`record_session`](Self::record_session), on
    /// the connection it is given.
    async fn claims_for(&self, ctx: &MintContext) -> AccessTokenClaimValues;

    /// Record the app's side of the new session (its claim record, keyed by
    /// `ctx.session_id` and carrying `ctx.ver`) on `conn` — the mint's own
    /// transaction, in which the `auth_sessions` row already exists — and
    /// return the FINAL claim values to stamp (default: `values` unchanged;
    /// an app may derive values here from what it reads on `conn`, e.g. a
    /// per-session `aud`). An `Err` aborts the whole mint (surfaced as 500).
    ///
    /// Constraints: do NOT write the `users` row (the mint holds it `FOR
    /// SHARE`; two concurrent sign-ins of one user would deadlock upgrading
    /// it) and keep it short (it delays that user's logout while it runs).
    ///
    /// ONE epoch authority: the SDK session row. SDK logout ends
    /// `auth_sessions` rows and `sessions::bump_session_version` bumps
    /// `auth_sessions.ver`; neither touches the app's record. So an app
    /// resolver must refuse a token whose `auth_sessions` row is absent, ended
    /// OR whose `ver` differs from the token's — e.g. read its record joined
    /// to `auth_sessions` by the same `sid` and compare `auth_sessions.ver`,
    /// or call `jwt_extractor::assert_session_epoch_current`.
    async fn record_session(
        &self,
        _ctx: &MintContext,
        values: AccessTokenClaimValues,
        _conn: &mut sqlx::PgConnection,
    ) -> Result<AccessTokenClaimValues, AppError> {
        Ok(values)
    }

    /// Whether `aud` (a value this source minted, other than the configured
    /// audience) names this resource server. Default: accept nothing beyond
    /// the configured audience. The refresh audience is refused before this is
    /// consulted, whatever it answers.
    fn accepts_audience(&self, _aud: &str) -> bool {
        false
    }
}

/// The SDK default source: supplies nothing, so tokens carry exactly today's
/// claims plus the SDK-owned `jti`/`sid`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultTokenClaimsSource;

#[async_trait::async_trait]
impl TokenClaimsSource for DefaultTokenClaimsSource {
    async fn claims_for(&self, _ctx: &MintContext) -> AccessTokenClaimValues {
        AccessTokenClaimValues::default()
    }
}

/// Everything session-scoped the mint stamps on a pair: the session id, its
/// epoch, and the fixed per-sign-in values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionClaims {
    pub sid: Option<Uuid>,
    pub ver: i32,
    pub values: AccessTokenClaimValues,
}

impl SessionClaims {
    /// The session claims a presented (already signature-verified) REFRESH
    /// token carries forward to its successor pair — `sid` and the fixed
    /// per-sign-in values — with `ver` supplied by the caller from the
    /// session row. The refresh token's own `aud` is the refresh audience, so a
    /// non-default ACCESS `aud` rides in its `access_aud` claim and is restored
    /// here (`None` → the configured audience), keeping it fixed as well.
    pub fn carried_from_refresh(refresh: &Claims, sid: Option<Uuid>, ver: i32) -> Self {
        SessionClaims {
            sid,
            ver,
            values: AccessTokenClaimValues {
                aud: refresh.access_aud.clone(),
                client_id: refresh.client_id.clone(),
                amr: refresh.amr.clone(),
                auth_time: refresh.auth_time,
                cnf: refresh.cnf.clone(),
            },
        }
    }
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
    /// The session (`auth_sessions.id`, the tokens' `sid`) this pair belongs
    /// to. `Some` for every pair minted by `mint_session_tokens`; `None` only
    /// from the legacy session-less `generate_tokens_with_jti_expiry`.
    pub session_id: Option<Uuid>,
}

/// JWT service for token generation and validation
#[derive(Clone)]
pub struct JwtService {
    config: JwtSettings,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    /// The app's minting hook (see [`TokenClaimsSource`]). Defaults to
    /// [`DefaultTokenClaimsSource`] so `try_new` alone keeps today's tokens.
    claims_source: Arc<dyn TokenClaimsSource>,
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
            claims_source: Arc::new(DefaultTokenClaimsSource),
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

    /// Install the app's minting hook (builder style, so `try_new`/`new` keep
    /// their signatures):
    /// `JwtService::try_new(cfg)?.with_token_claims_source(Arc::new(MySource))`.
    pub fn with_token_claims_source(mut self, source: Arc<dyn TokenClaimsSource>) -> Self {
        self.claims_source = source;
        self
    }

    /// The installed minting hook — consulted by `mint_session_tokens` once per
    /// sign-in.
    pub fn claims_source(&self) -> &Arc<dyn TokenClaimsSource> {
        &self.claims_source
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
        self.generate_session_tokens(
            user_id,
            username,
            email,
            is_admin,
            access_hours,
            refresh_days,
            &SessionClaims {
                sid: None,
                ver: token_version,
                values: AccessTokenClaimValues::default(),
            },
        )
    }

    /// Mint a session token pair stamped with `session`: the `sid`, the epoch
    /// `ver` (the session row's), and the fixed per-sign-in values. The ACCESS
    /// token carries the full RFC 9068 §2.2 set (`iss`, `exp`, `aud`, `sub`,
    /// `client_id` when supplied, `iat`, a fresh `jti`) plus `amr`/`auth_time`/
    /// `cnf` when supplied; the REFRESH token carries `sid` and the same fixed
    /// values so a refresh can copy them forward (RFC 9068 §2.2.1).
    ///
    /// Same two-step protocol as `generate_tokens_with_jti_expiry`: the caller
    /// must register `refresh_jti` before handing the pair out.
    pub fn generate_session_tokens(
        &self,
        user_id: Uuid,
        username: &str,
        email: &str,
        is_admin: bool,
        access_hours: i64,
        refresh_days: i64,
        session: &SessionClaims,
    ) -> Result<TokenPairWithJti, AppError> {
        let (_, expires_in) = self.access_expiry(access_hours);
        let access_token =
            self.generate_access_token(user_id, username, email, is_admin, access_hours, session)?;
        let (refresh_token, refresh_jti, refresh_expires_at) =
            self.generate_refresh_token_with_jti(user_id, refresh_days, session)?;

        Ok(TokenPairWithJti {
            pair: TokenPair {
                access_token,
                refresh_token,
                token_type: "Bearer".to_string(),
                expires_in,
            },
            refresh_jti,
            refresh_expires_at,
            session_id: session.sid,
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
        self.reissue_session_tokens_for_jti(
            user_id,
            username,
            email,
            is_admin,
            access_hours,
            refresh_jti,
            refresh_expires_at,
            &SessionClaims {
                sid: None,
                ver: token_version,
                values: AccessTokenClaimValues::default(),
            },
        )
    }

    /// `reissue_tokens_for_jti`, stamping `session` (the successor family's
    /// session and the fixed per-sign-in values) on both tokens.
    pub fn reissue_session_tokens_for_jti(
        &self,
        user_id: Uuid,
        username: &str,
        email: &str,
        is_admin: bool,
        access_hours: i64,
        refresh_jti: Uuid,
        refresh_expires_at: chrono::DateTime<Utc>,
        session: &SessionClaims,
    ) -> Result<TokenPair, AppError> {
        let (_, expires_in) = self.access_expiry(access_hours);
        let access_token =
            self.generate_access_token(user_id, username, email, is_admin, access_hours, session)?;

        let claims = self.refresh_claims(user_id, refresh_jti, refresh_expires_at, session);
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
    /// `session.ver` is the caller's snapshot of the epoch (the session row's
    /// `ver`, or `users.token_version` for a session-less legacy mint); it is
    /// stamped as the `ver` claim and is what makes the token revocable.
    /// Callers MUST read it from the DB rather than pass a constant. Every
    /// access token gets a fresh `jti` (RFC 9068 §2.2 REQUIRED; RFC 7519
    /// §4.1.7 unique per token).
    fn generate_access_token(
        &self,
        user_id: Uuid,
        username: &str,
        email: &str,
        is_admin: bool,
        access_hours: i64,
        session: &SessionClaims,
    ) -> Result<String, AppError> {
        let now = Utc::now();
        let (ttl, _) = self.access_expiry(access_hours);
        let exp = now + ttl;
        let v = &session.values;

        let claims = Claims {
            sub: user_id.to_string(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
            iss: self.config.issuer.clone(),
            aud: v.aud.clone().unwrap_or_else(|| self.config.audience.clone()),
            username: username.to_string(),
            email: email.to_string(),
            is_admin,
            jti: Some(Uuid::new_v4().to_string()),
            ver: Some(session.ver),
            sid: session.sid,
            client_id: v.client_id.clone(),
            amr: v.amr.clone(),
            auth_time: v.auth_time,
            cnf: v.cnf.clone(),
            access_aud: None,
        };

        encode(&Header::default(), &claims, &self.encoding_key).map_err(|e| {
            AppError::internal_error(format!("Failed to generate access token: {}", e))
        })
    }

    /// The refresh-token claim set: the refresh audience, the whitelist `jti`,
    /// and the session-scoped values the refresh handler carries forward
    /// (`sid`, `client_id`, `amr`, `auth_time`, `cnf`, and a non-default
    /// access `aud` as `access_aud`). No `ver`: refresh tokens are revoked via
    /// the `refresh_tokens` whitelist, not the epoch — see the `ver` doc.
    fn refresh_claims(
        &self,
        user_id: Uuid,
        jti: Uuid,
        expires_at: chrono::DateTime<Utc>,
        session: &SessionClaims,
    ) -> Claims {
        let v = &session.values;
        Claims {
            sub: user_id.to_string(),
            exp: expires_at.timestamp(),
            iat: Utc::now().timestamp(),
            iss: self.config.issuer.clone(),
            aud: format!("{}-refresh", self.config.audience),
            username: String::new(),
            email: String::new(),
            is_admin: false,
            jti: Some(jti.to_string()),
            ver: None,
            sid: session.sid,
            client_id: v.client_id.clone(),
            amr: v.amr.clone(),
            auth_time: v.auth_time,
            cnf: v.cnf.clone(),
            access_aud: v.aud.clone(),
        }
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
        session: &SessionClaims,
    ) -> Result<(String, Uuid, chrono::DateTime<Utc>), AppError> {
        let exp = Utc::now() + Duration::days(refresh_days);
        let jti = Uuid::new_v4();
        let claims = self.refresh_claims(user_id, jti, exp, session);

        let token = encode(&Header::default(), &claims, &self.encoding_key).map_err(|e| {
            AppError::internal_error(format!("Failed to generate refresh token: {}", e))
        })?;

        Ok((token, jti, exp))
    }

    /// Validate and decode an access token.
    ///
    /// Pins signature, `iss`, `exp` (5 s leeway) and `aud`. The `aud` rule
    /// (RFC 9068 §4 — the resource server rejects a token whose `aud` does not
    /// name it): the refresh audience `{audience}-refresh` is ALWAYS refused
    /// (a refresh token is never an access token); otherwise `aud` must be the
    /// configured audience, or a value the installed [`TokenClaimsSource`]
    /// names as its own via `accepts_audience` (default: none).
    pub fn validate_access_token(&self, token: &str) -> Result<Claims, AppError> {
        let mut validation = Validation::default();
        validation.set_issuer(&[&self.config.issuer]);
        // `aud` is checked below, against the configured audience AND the
        // app's accepted set, which jsonwebtoken's fixed list cannot express.
        validation.validate_aud = false;
        // jsonwebtoken defaults `leeway` to 60s (clock-skew slack between a
        // separate issuer and validator). Here the issuer IS the validator
        // (same process), so skew is ~0 — a 60s grace on `exp` would make a
        // configured short access TTL (the admin "shorter is safer" knob)
        // effectively 60s longer and delay cutting off a refreshed/deactivated
        // session. A small cushion still absorbs sub-second scheduling.
        validation.leeway = JWT_LEEWAY_SECONDS;

        let claims = decode::<Claims>(token, &self.decoding_key, &validation)
            .map(|data| data.claims)
            .map_err(|e| {
                AppError::unauthorized("INVALID_TOKEN", format!("Invalid or expired token: {}", e))
            })?;

        let refresh_aud = format!("{}-refresh", self.config.audience);
        let aud_ok = claims.aud != refresh_aud
            && (claims.aud == self.config.audience
                || self.claims_source.accepts_audience(&claims.aud));
        if !aud_ok {
            return Err(AppError::unauthorized(
                "INVALID_TOKEN",
                "Invalid or expired token: InvalidAudience",
            ));
        }
        Ok(claims)
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

    /// A test claim source: fixed values, records every context it is asked
    /// about, and optionally accepts one extra audience.
    struct FakeSource {
        values: AccessTokenClaimValues,
        accept: Option<String>,
        seen: std::sync::Mutex<Vec<MintContext>>,
    }

    #[async_trait::async_trait]
    impl TokenClaimsSource for FakeSource {
        async fn claims_for(&self, ctx: &MintContext) -> AccessTokenClaimValues {
            self.seen.lock().unwrap().push(*ctx);
            self.values.clone()
        }
        fn accepts_audience(&self, aud: &str) -> bool {
            self.accept.as_deref() == Some(aud)
        }
    }

    fn fake(values: AccessTokenClaimValues, accept: Option<&str>) -> Arc<FakeSource> {
        Arc::new(FakeSource {
            values,
            accept: accept.map(str::to_string),
            seen: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn raw_payload(token: &str) -> serde_json::Value {
        use base64::Engine;
        let payload = token.split('.').nth(1).expect("jwt has a payload segment");
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .expect("payload is base64url");
        serde_json::from_slice(&bytes).expect("payload is json")
    }

    /// TEST-1 (queue fr3-417sess, INV-1): the access token carries the RFC 9068
    /// §2.2 REQUIRED set (`iss`, `exp`, `aud`, `sub`, `client_id`, `iat`, `jti`)
    /// plus `amr`/`auth_time`/`cnf`, and the session claims `sid`/`ver` —
    /// asserted on the DECODED token, not on "mint returned ok".
    #[test]
    fn access_token_carries_the_rfc9068_required_claim_set() {
        let cfg = test_config(None);
        let svc = JwtService::try_new(cfg.clone()).unwrap();
        let user = Uuid::new_v4();
        let sid = Uuid::new_v4();
        let session = SessionClaims {
            sid: Some(sid),
            ver: 4,
            values: AccessTokenClaimValues {
                aud: None,
                client_id: Some("client-1".into()),
                amr: Some(vec!["pwd".into()]),
                auth_time: Some(1_700_000_000),
                cnf: Some(Cnf { jkt: "thumb".into() }),
            },
        };
        let minted = svc
            .generate_session_tokens(user, "u", "u@x", false, 2, 7, &session)
            .unwrap();
        assert_eq!(minted.session_id, Some(sid));

        let a = svc.validate_access_token(&minted.pair.access_token).unwrap();
        assert_eq!(a.iss, cfg.issuer);
        assert_eq!(a.aud, cfg.audience);
        assert_eq!(a.sub, user.to_string());
        assert!(a.iat > 0 && a.exp > a.iat);
        assert_eq!(a.client_id.as_deref(), Some("client-1"));
        let jti = a.jti.clone().expect("RFC 9068 §2.2: jti is REQUIRED on the access token");
        assert!(Uuid::parse_str(&jti).is_ok());
        assert_eq!(a.amr, Some(vec!["pwd".to_string()]));
        assert_eq!(a.auth_time, Some(1_700_000_000));
        assert_eq!(a.cnf, Some(Cnf { jkt: "thumb".into() }));
        assert_eq!(a.sid, Some(sid));
        assert_eq!(a.ver, Some(4));

        // RFC 7519 §4.1.7: unique per token.
        let again = svc
            .generate_session_tokens(user, "u", "u@x", false, 2, 7, &session)
            .unwrap();
        let b = svc.validate_access_token(&again.pair.access_token).unwrap();
        assert_ne!(b.jti, a.jti, "each access token gets its own jti");

        // The refresh token carries the session + fixed values forward (the
        // RFC 9068 §2.2.1 fixedness carrier) but no epoch.
        let r = svc.validate_refresh_token(&minted.pair.refresh_token).unwrap();
        assert_eq!(r.sid, Some(sid));
        assert_eq!(r.amr, a.amr);
        assert_eq!(r.auth_time, a.auth_time);
        assert_eq!(r.client_id, a.client_id);
        assert_eq!(r.cnf, a.cnf);
        assert_eq!(r.ver, None);
    }

    /// TEST-2 (queue fr3-417sess, INV-2): additive + versioned. (a) a PRE-DELTA
    /// access token (none of the new claims) still validates and every new
    /// field reads `None`; (b) a token minted with no session and the default
    /// source puts NONE of the new keys on the wire (`skip_serializing_if`).
    #[test]
    fn legacy_token_without_new_claims_still_validates_and_new_claims_are_skipped_on_the_wire() {
        let cfg = test_config(None);
        let svc = JwtService::try_new(cfg.clone()).unwrap();
        let now = Utc::now();
        let legacy = serde_json::json!({
            "sub": Uuid::new_v4().to_string(),
            "exp": (now + Duration::hours(1)).timestamp(),
            "iat": now.timestamp(),
            "iss": cfg.issuer,
            "aud": cfg.audience,
            "username": "u",
            "email": "u@x",
            "is_admin": false,
            "ver": 0,
        });
        let token = encode(
            &Header::default(),
            &legacy,
            &EncodingKey::from_secret(cfg.secret.as_bytes()),
        )
        .unwrap();
        let c = svc
            .validate_access_token(&token)
            .expect("a pre-delta token must still validate");
        assert_eq!(c.sid, None);
        assert_eq!(c.client_id, None);
        assert_eq!(c.amr, None);
        assert_eq!(c.auth_time, None);
        assert_eq!(c.cnf, None);
        assert_eq!(c.access_aud, None);

        let minted = svc
            .generate_tokens_with_jti_expiry(Uuid::new_v4(), "u", "u@x", false, 1, 1, 0)
            .unwrap();
        for tok in [&minted.pair.access_token, &minted.pair.refresh_token] {
            let raw = raw_payload(tok);
            for key in ["sid", "client_id", "amr", "auth_time", "cnf", "access_aud"] {
                assert!(
                    raw.get(key).is_none(),
                    "an unset `{key}` must not appear on the wire (got {raw})"
                );
            }
        }
    }

    /// TEST-3 (queue fr3-417sess, INV-3): the seam. (a) `try_new` alone carries
    /// the default source — no values, accepts no extra audience; (b) the
    /// builder installs the app's source, which receives the mint context.
    #[tokio::test]
    async fn token_claims_source_is_consulted_and_default_is_todays_behaviour() {
        let ctx = MintContext {
            user_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            ver: 0,
            method: AuthMethod::Password,
        };
        let default_svc = JwtService::try_new(test_config(None)).unwrap();
        assert_eq!(
            default_svc.claims_source().claims_for(&ctx).await,
            AccessTokenClaimValues::default()
        );
        assert!(!default_svc.claims_source().accepts_audience("anything"));

        let values = AccessTokenClaimValues {
            client_id: Some("c".into()),
            amr: Some(vec!["pwd".into()]),
            auth_time: Some(42),
            ..Default::default()
        };
        let src = fake(values.clone(), None);
        let svc = JwtService::try_new(test_config(None))
            .unwrap()
            .with_token_claims_source(src.clone());
        assert_eq!(svc.claims_source().claims_for(&ctx).await, values);
        assert_eq!(*src.seen.lock().unwrap(), vec![ctx]);
    }

    /// TEST-6 (queue fr3-417sess, ITEM-5 / DEC-4): the access-token `aud` rule.
    #[test]
    fn validate_access_token_aud_rule() {
        let cfg = test_config(None);
        let key = EncodingKey::from_secret(cfg.secret.as_bytes());
        let now = Utc::now();
        let token_with = |aud: &str, client_id: bool| {
            let mut c = serde_json::json!({
                "sub": Uuid::new_v4().to_string(),
                "exp": (now + Duration::hours(1)).timestamp(),
                "iat": now.timestamp(),
                "iss": cfg.issuer,
                "aud": aud,
                "username": "u", "email": "u@x", "is_admin": false,
            });
            if client_id {
                c["client_id"] = serde_json::json!("client-1");
            }
            encode(&Header::default(), &c, &key).unwrap()
        };
        let plain = JwtService::try_new(cfg.clone()).unwrap();
        let clinic = JwtService::try_new(cfg.clone())
            .unwrap()
            .with_token_claims_source(fake(Default::default(), Some("clinic-7")));
        let everything = {
            struct AcceptAll;
            #[async_trait::async_trait]
            impl TokenClaimsSource for AcceptAll {
                async fn claims_for(&self, _: &MintContext) -> AccessTokenClaimValues {
                    Default::default()
                }
                fn accepts_audience(&self, _: &str) -> bool {
                    true
                }
            }
            JwtService::try_new(cfg.clone())
                .unwrap()
                .with_token_claims_source(Arc::new(AcceptAll))
        };

        // (a) a foreign aud is refused by default, `client_id` or not.
        assert!(plain.validate_access_token(&token_with("clinic-7", true)).is_err());
        assert!(plain.validate_access_token(&token_with("clinic-7", false)).is_err());
        // (b) the app, as resource server, can name its own audiences.
        assert!(clinic.validate_access_token(&token_with("clinic-7", false)).is_ok());
        assert!(clinic.validate_access_token(&token_with("clinic-8", true)).is_err());
        // (c) the refresh audience is never an access token.
        let refresh_aud = format!("{}-refresh", cfg.audience);
        assert!(everything.validate_access_token(&token_with(&refresh_aud, true)).is_err());
        // (d) the configured audience validates everywhere.
        assert!(plain.validate_access_token(&token_with(&cfg.audience, false)).is_ok());
        assert!(clinic.validate_access_token(&token_with(&cfg.audience, true)).is_ok());
    }

}
