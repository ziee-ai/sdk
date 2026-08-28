// Auth handlers

use aide::transform::TransformOperation;
use axum::{
    Extension, Form, Json, debug_handler,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use sqlx::PgPool;
use std::sync::Arc;

use ziee_core::{ApiResult, AppError};
use ziee_framework::permissions::{IdentityResolver, RequirePermissions, with_permission};
use crate::user::events::UserEvent;
use crate::user::permissions::ProfileEdit;
use crate::user::{Group, User, UserRepository, UserService};

use crate::auth::context::{AuthContext, AuthSyncAction, AuthSyncEntity};

use crate::auth::cookie;
use crate::auth::jwt::{JwtService, TokenPair, TokenPairWithJti};
use super::jwt_extractor::JwtAuth;
use crate::auth::password;
use crate::auth::permissions::{AuthProvidersManage, AuthProvidersRead};
use crate::auth::providers::events::AuthProviderEvent;
use crate::auth::providers::{
    AuthResult, create_provider, health as provider_health, repository as provider_repo,
};
use crate::auth::refresh_tokens;
use crate::auth::refresh_tokens::mint_session_tokens;
use crate::auth::types::{
    AppleCallbackForm, AuthProviderResponse, AuthResponse, ChangePasswordRequest,
    CreateAuthProviderRequest, CreateAuthProviderResponse, DeleteProviderResponse,
    LinkAccountRequest, LoginRequest, MeResponse, OAuthAuthorizeQuery, OAuthCallbackQuery,
    PublicProvider, PublicProvidersResponse, RefreshTokenRequest, RegisterRequest,
    TestProviderResponse, UpdateAuthProviderRequest, UpdateProfileRequest,
};
use ziee_framework::sync::{Audience, SyncOrigin};

// =====================================================
// Cookie-mode response shaping
// =====================================================

/// Build the JSON response for a token-minting endpoint, honoring the
/// client's delivery mode.
///
/// Cookie mode (request carried `X-Refresh-Cookie: 1`): the refresh token
/// moves into an httpOnly `Set-Cookie` (Max-Age = the refresh token's
/// remaining TTL) and the JSON body's `refresh_token` is blanked — page
/// JavaScript never sees it. Body mode (no header — desktop Tauri, tunnel
/// clients): the body carries the refresh token exactly as before and no
/// cookie is set.
///
/// The docs (`*_docs`) keep advertising the same JSON schema; only the
/// `refresh_token` VALUE differs between modes.
///
/// `pub(crate)` because the app module's first-run `setup_admin` (also a
/// browser flow) shares it.
pub fn token_response<T: serde::Serialize>(
    req_headers: &HeaderMap,
    status: StatusCode,
    minted: TokenPairWithJti,
    build: impl FnOnce(TokenPair) -> T,
) -> (StatusCode, Response) {
    let mut tokens = minted.pair;
    let mut set_cookie = None;
    if cookie::wants_cookie(req_headers) {
        let max_age = (minted.refresh_expires_at - chrono::Utc::now())
            .num_seconds()
            .max(0);
        set_cookie = Some(cookie::build_refresh_cookie(
            &tokens.refresh_token,
            max_age,
            cookie::is_secure_request(req_headers),
        ));
        tokens.refresh_token = String::new();
    }
    let mut resp = Json(build(tokens)).into_response();
    if let Some(c) = set_cookie {
        resp.headers_mut().append(header::SET_COOKIE, c);
    }
    (status, resp)
}

// =====================================================
// Route Handlers
// =====================================================

/// POST /api/auth/register
/// Register a new user with username, email, and password
#[debug_handler]
pub async fn register(
    Extension(jwt_service): Extension<Arc<JwtService>>,
    Extension(ctx): Extension<AuthContext>,
    headers: HeaderMap,
    Json(mut req): Json<RegisterRequest>,
) -> ApiResult<Response> {
    // Validate input fields. The username goes through the shared
    // `validate_username` bound/charset gate (length ≤ the varchar(100)
    // column, no whitespace/control/bidi, alnum + `. _ - @ +` only) — without
    // it an over-long name reached Postgres and surfaced as a generic 500,
    // and a structurally-junk name was persisted outright.
    let username = req.username.trim().to_string();
    crate::auth::username::validate_username(&username).map_err(AppError::to_api_error)?;
    req.username = username;
    if req.email.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            AppError::bad_request("INVALID_EMAIL", "Email cannot be empty"),
        ));
    }
    if let Err(msg) = password::validate_password_strength(&req.password) {
        return Err((
            StatusCode::BAD_REQUEST,
            AppError::bad_request("INVALID_PASSWORD", msg),
        ));
    }

    // Check if username or email already exists. Closes 01-auth F-13
    // (Medium): the previous "Username" vs "Email" differential let an
    // attacker probe which of two values is already registered (user
    // enumeration). We now collapse both branches into the same
    // generic "ACCOUNT_EXISTS" response, leaking nothing about which
    // field collided. Server-side logs still record which one for
    // operator debugging.
    let username_taken = ctx.user()
        .get_by_username(&req.username)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .is_some();
    let email_taken = ctx.user()
        .get_by_email(&req.email)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .is_some();
    if username_taken || email_taken {
        if username_taken {
            tracing::info!("Register conflict on username (logged for ops; client sees generic)");
        }
        if email_taken {
            tracing::info!("Register conflict on email (logged for ops; client sees generic)");
        }
        return Err((
            StatusCode::CONFLICT,
            AppError::new(
                StatusCode::CONFLICT,
                "ACCOUNT_EXISTS",
                "An account with these details already exists",
            ),
        ));
    }

    // Hash password
    let password_hash = password::hash_password(&req.password).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            AppError::internal_with_id(format!("hash password: {e}")),
        )
    })?;

    // Create user + assign the default group atomically (one transaction) so a
    // failure of the group assignment can't leave an orphan user with no group
    // membership (and hence no permissions). Mirrors the external/OAuth path's
    // `create_external_user_with_link`.
    let user = ctx.auth()
        .create_local_user_with_default_group(
            &req.username,
            &req.email,
            Some(password_hash),
            req.display_name,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Emit UserCreated event asynchronously
    ctx.events.emit_user(UserEvent::Created { user: user.clone() });

    // Mint + whitelist the session tokens (admin-configured lifetimes).
    let minted = mint_session_tokens(ctx.pool(), &jwt_service, user.id, &user.username, &user.email, user.is_admin)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(token_response(&headers, StatusCode::CREATED, minted, |tokens| {
        AuthResponse { user, tokens }
    }))
}

/// Documentation for register endpoint
pub fn register_docs(op: TransformOperation) -> TransformOperation {
    op.description("Register a new user with username, email, and password")
        .id("Auth.register")
        .tag("auth")
        .response::<201, Json<AuthResponse>>()
}

/// POST /api/auth/login
/// Login with username/email and password
#[debug_handler]
pub async fn login(
    Extension(jwt_service): Extension<Arc<JwtService>>,
    Extension(ctx): Extension<AuthContext>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Response> {
    // Check if external provider is specified
    if let Some(provider_name) = &req.provider
        && provider_name != "local" {
            // External authentication (LDAP/OAuth)
            return login_with_provider(
                &ctx,
                jwt_service,
                &headers,
                &req.username,
                &req.password,
                provider_name,
            )
            .await;
        }

    // Local password authentication.
    //
    // Closes 01-auth F-06 (Medium): the previous flow leaked
    // existence three ways: (a) returning early with no bcrypt when
    // the user didn't exist (~10ms timing differential), (b) a
    // distinct ACCOUNT_DISABLED error for valid-but-disabled accounts,
    // and (c) a distinct NO_PASSWORD error for OAuth-only accounts.
    // Combined, an attacker could enumerate registered emails.
    //
    // Defense:
    //   - Always run bcrypt verify against a precomputed dummy hash
    //     when the user / password is absent, so the timing matches a
    //     real verification call.
    //   - Collapse every failure into the same INVALID_CREDENTIALS
    //     response shape.
    //   - Log the real reason server-side for operator debugging.
    //
    // The dummy hash is precomputed once at first use; the password
    // input to bcrypt::verify is the user-supplied password (so the
    // hashing cost matches the input).
    static DUMMY_PWHASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let dummy_hash = DUMMY_PWHASH.get_or_init(|| {
        // Cost matches the application default (bcrypt::DEFAULT_COST).
        // A random unguessable value so even a length-equal guess
        // can't match.
        bcrypt::hash(uuid::Uuid::new_v4().to_string(), bcrypt::DEFAULT_COST)
            .expect("bcrypt dummy hash")
    });

    let user_opt = ctx.user()
        .get_by_username_or_email(&req.username)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Pick the hash to verify: real user hash, dummy when missing or
    // no-password. Both code paths run bcrypt to keep timing flat.
    let (hash_to_verify, real_user_active, password_was_present) = match &user_opt {
        Some(u) => match u.password_hash.as_deref() {
            Some(h) => (h.to_string(), u.is_active, true),
            None => (dummy_hash.clone(), u.is_active, false),
        },
        None => (dummy_hash.clone(), false, false),
    };

    let verify_result =
        password::verify_password(&req.password, &hash_to_verify).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                AppError::internal_with_id(format!("verify password: {e}")),
            )
        })?;

    if !verify_result || !real_user_active || !password_was_present {
        if user_opt.is_none() {
            tracing::info!("Login failed: user not found");
        } else if !real_user_active {
            tracing::info!("Login failed: account disabled");
        } else if !password_was_present {
            tracing::info!("Login failed: no password (OAuth-only account)");
        } else {
            tracing::info!("Login failed: bad password");
        }
        return Err((
            StatusCode::UNAUTHORIZED,
            AppError::unauthorized("INVALID_CREDENTIALS", "Invalid username or password"),
        ));
    }

    let user = user_opt.expect("user_opt is Some past timing-equalised checks");

    // Update last login
    ctx.user()
        .update_last_login(user.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Mint + whitelist the session tokens (admin-configured lifetimes).
    let minted = mint_session_tokens(ctx.pool(), &jwt_service, user.id, &user.username, &user.email, user.is_admin)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(token_response(&headers, StatusCode::OK, minted, |tokens| {
        AuthResponse { user, tokens }
    }))
}

/// Documentation for login endpoint
pub fn login_docs(op: TransformOperation) -> TransformOperation {
    op.description("Login with username/email and password")
        .id("Auth.login")
        .tag("auth")
        .response::<200, Json<AuthResponse>>()
}

/// Login with external provider (LDAP/OAuth)
async fn login_with_provider(
    ctx: &AuthContext,
    jwt_service: Arc<JwtService>,
    headers: &HeaderMap,
    username: &str,
    password: &str,
    provider_name: &str,
) -> ApiResult<Response> {
    use crate::auth::providers::{create_provider, repository as provider_repo};

    // Get provider configuration
    let provider_config = provider_repo::get_provider_by_name(ctx.pool(), provider_name)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                AppError::database_error(e),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                AppError::not_found("Authentication provider"),
            )
        })?;

    // Create provider instance
    let provider = create_provider(&provider_config, ctx.pool().clone()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            AppError::bad_request("PROVIDER_ERROR", format!("Provider error: {}", e)),
        )
    })?;

    // Authenticate with external provider
    let auth_result = provider
        .authenticate(username, password)
        .await
        .map_err(|_e| {
            (
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized(
                    "INVALID_CREDENTIALS",
                    "Invalid username or password".to_string(),
                ),
            )
        })?;

    // Try to find user via auth link
    let user_id = ctx.auth()
        .find_user_by_auth_link(provider_config.id, &auth_result.external_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let user = if let Some(user_id) = user_id {
        // User exists, get it
        ctx.user()
            .get_by_id(user_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
            .ok_or_else(|| (StatusCode::NOT_FOUND, AppError::not_found("User")))?
    } else {
        // User doesn't exist - create new user with auth link and default group assignment
        let display_name = auth_result
            .attributes
            .display_name
            .unwrap_or_else(|| username.to_string());
        let email = auth_result.attributes.email;

        let new_user_id = ctx.auth()
            .create_external_user_with_link(
                username,
                Some(email),
                &display_name,
                provider_config.id,
                &auth_result.external_id,
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        // Fetch the newly created user
        ctx.user()
            .get_by_id(new_user_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
            .ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    AppError::internal_error("Failed to fetch newly created user"),
                )
            })?
    };

    // Check if user is active
    if !user.is_active {
        return Err((
            StatusCode::UNAUTHORIZED,
            AppError::unauthorized("ACCOUNT_DISABLED", "User account is disabled"),
        ));
    }

    // Update last login
    ctx.user()
        .update_last_login(user.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Mint + whitelist the session tokens (admin-configured lifetimes).
    let minted = mint_session_tokens(ctx.pool(), &jwt_service, user.id, &user.username, &user.email, user.is_admin)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(token_response(headers, StatusCode::OK, minted, |tokens| {
        AuthResponse { user, tokens }
    }))
}

/// POST /api/auth/refresh
/// Refresh access token using refresh token
#[debug_handler]
pub async fn refresh(
    Extension(jwt_service): Extension<Arc<JwtService>>,
    Extension(ctx): Extension<AuthContext>,
    headers: HeaderMap,
    Json(req): Json<RefreshTokenRequest>,
) -> ApiResult<Response> {
    // Source precedence: an explicit body token wins (desktop Tauri /
    // tunnel clients); otherwise the httpOnly `ziee_refresh` cookie (web).
    // The response mirrors the source — body-in→body-out, cookie-in→
    // cookie-out — so a phone browser driving the tunnel body-path can't
    // have its token silently moved into a cookie it doesn't read.
    let (presented, from_cookie) = match req.refresh_token.filter(|t| !t.is_empty()) {
        Some(t) => (t, false),
        None => match cookie::read_refresh_cookie(&headers) {
            Some(t) => (t, true),
            None => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    AppError::unauthorized(
                        "MISSING_REFRESH_TOKEN",
                        "No refresh token in request body or cookie",
                    ),
                ));
            }
        },
    };

    // Validate refresh token (signature + exp + iss + aud)
    let claims = jwt_service
        .validate_refresh_token(&presented)
        .map_err(|e| (StatusCode::UNAUTHORIZED, e))?;

    // Parse user ID from claims
    let user_id = uuid::Uuid::parse_str(&claims.sub).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            AppError::internal_with_id(format!("parse user id from token: {e}")),
        )
    })?;

    // SECURITY: check the refresh token's jti against the whitelist
    // (refresh_tokens table). Closes 01-auth F-03 (refresh didn't rotate
    // the presented token — the old one kept minting access tokens for
    // up to 30 days).
    //
    // Tokens minted BEFORE this commit don't carry a jti claim; we let
    // those through unconditionally so existing sessions don't break on
    // the upgrade. Within ~30 days every active session is naturally
    // re-issued through the new code path and gets a jti, after which
    // unchecked legacy tokens can no longer exist.
    let presented_jti = match claims.jti.as_deref() {
        Some(jti_str) => Some(uuid::Uuid::parse_str(jti_str).map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized("INVALID_TOKEN", "Invalid refresh token jti"),
            )
        })?),
        None => None,
    };

    // Get user from database
    let user = ctx.user()
        .get_by_id(user_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized("USER_NOT_FOUND", "User not found"),
            )
        })?;

    // Check if user is still active
    if !user.is_active {
        return Err((
            StatusCode::UNAUTHORIZED,
            AppError::unauthorized("ACCOUNT_DISABLED", "User account is disabled"),
        ));
    }

    let (access_hours, refresh_days) = refresh_tokens::session_expiries(ctx.pool(), &jwt_service).await;

    // Determine the outgoing pair + the refresh token's expiry (for the
    // cookie Max-Age). Three cases:
    //
    //  1. Presented jti present (the normal path): ATOMICALLY claim the
    //     token for rotation (`claim_rotation` flips active→revoked in one
    //     UPDATE). The winner registers a fresh successor and returns it.
    //     A LOSER (concurrent double-refresh, or a replay within the grace
    //     window) is re-issued tokens bound to the EXISTING successor
    //     family via `reissue_tokens_for_jti` — NOT an independent new
    //     chain — so single-use holds even under a race and a
    //     replayed-within-grace token can never outlive the family it
    //     belongs to. No successor in grace → 401 REFRESH_TOKEN_REVOKED.
    //
    //  2. No jti (a legacy token minted before this feature): the
    //     one-time upgrade allowance — mint + register a fresh jti pair.
    //
    // The claim + successor-register happen in ONE transaction
    // (`claim_rotation_and_register`), so a losing concurrent request
    // blocks on the presented row's lock until the successor is committed
    // + visible, then falls into the grace path — no spurious 401. On a
    // mid-transaction DB failure nothing commits and the presented token
    // stays active, so the client's retry simply rotates again.
    //
    // Read the access-token revocation epoch BEFORE claiming the rotation, and
    // never after. A logout racing this request must not be able to hand back a
    // live session: if it commits first, the claim below finds the token
    // revoked → 401; if it commits after, this read (READ COMMITTED) has
    // already returned the PRE-bump value, so the token we mint carries the old
    // `ver` and is dead on arrival. Reading after the claim would instead
    // observe the NEW epoch and mint a token that survives the logout.
    // A DB failure here is 500, NOT 401: the client treats a 401 from
    // /auth/refresh as terminal (wipe + reload), so mapping a transient pool
    // blip to 401 would log active users out mid-work. Only an absent user row
    // is an auth failure. Mirrors the `get_by_id` mapping above.
    let token_version = refresh_tokens::current_token_version(ctx.pool(), user.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized("USER_NOT_FOUND", "User not found"),
            )
        })?;

    let (out_pair, out_refresh_expires_at) = if let Some(jti) = presented_jti {
        let candidate = jwt_service
            .generate_tokens_with_jti_expiry(
                user.id,
                &user.username,
                &user.email,
                user.is_admin,
                access_hours,
                refresh_days,
                token_version,
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        let won = refresh_tokens::claim_rotation_and_register(
            ctx.pool(),
            jti,
            candidate.refresh_jti,
            user.id,
            candidate.refresh_expires_at,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        if won {
            (candidate.pair, candidate.refresh_expires_at)
        } else {
            // We lost the race / the token was already rotated. `candidate`
            // is discarded (never registered). Serve the existing
            // successor family if still within grace + active.
            match refresh_tokens::rotation_grace_successor(ctx.pool(), jti)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
            {
                Some((succ_jti, succ_exp)) => {
                    let pair = jwt_service
                        .reissue_tokens_for_jti(
                            user.id,
                            &user.username,
                            &user.email,
                            user.is_admin,
                            access_hours,
                            succ_jti,
                            succ_exp,
                            token_version,
                        )
                        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
                    (pair, succ_exp)
                }
                None => {
                    return Err((
                        StatusCode::UNAUTHORIZED,
                        AppError::unauthorized(
                            "REFRESH_TOKEN_REVOKED",
                            "Refresh token has been revoked or already used",
                        ),
                    ));
                }
            }
        }
    } else {
        // Legacy jti-less token: one-time upgrade allowance.
        let minted = refresh_tokens::mint_session_tokens(
            ctx.pool(),
            &jwt_service,
            user.id,
            &user.username,
            &user.email,
            user.is_admin,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        (minted.pair, minted.refresh_expires_at)
    };

    // body-in→body-out / cookie-in→cookie-out (see the source comment).
    if from_cookie {
        let max_age = (out_refresh_expires_at - chrono::Utc::now())
            .num_seconds()
            .max(0);
        let set_cookie = cookie::build_refresh_cookie(
            &out_pair.refresh_token,
            max_age,
            cookie::is_secure_request(&headers),
        );
        let mut tokens = out_pair;
        tokens.refresh_token = String::new();
        let mut resp = Json(tokens).into_response();
        resp.headers_mut().append(header::SET_COOKIE, set_cookie);
        Ok((StatusCode::OK, resp))
    } else {
        Ok((StatusCode::OK, Json(out_pair).into_response()))
    }
}

/// Documentation for refresh endpoint
pub fn refresh_docs(op: TransformOperation) -> TransformOperation {
    op.description(
        "Refresh access token using a refresh token (JSON body, or the \
         httpOnly ziee_refresh cookie when the body token is absent)",
    )
    .id("Auth.refresh")
    .tag("auth")
    .response::<200, Json<TokenPair>>()
}

/// POST /api/auth/logout
/// Logout current user. Ends every session the user holds — with two
/// pre-existing residues noted below:
///   - bumps `users.token_version`, so every already-issued ACCESS token stops
///     validating at once (see `jwt_extractor::verify_token_version`);
///   - revokes all of the user's active refresh tokens, so /auth/refresh fails
///     with REFRESH_TOKEN_REVOKED (01-auth F-02);
///   - clears the httpOnly refresh cookie;
///   - signals the user's OTHER devices/tabs to re-bootstrap, so they tear down
///     immediately rather than lingering on a stale authenticated UI.
///
/// Sign-out-everywhere is not a new behavior — `revoke_all_for_user` has always
/// revoked every refresh token the user holds (see migration 44). The bump only
/// removes the up-to-24h delay before that intent took effect.
///
/// RESIDUE (pre-existing, unchanged here): a jti-LESS refresh token — minted
/// before migration 44 — has no `refresh_tokens` row, so there is nothing to
/// revoke, and `refresh`'s legacy-upgrade branch will mint it a fresh session
/// carrying the CURRENT epoch. Those tokens are effectively extinct (the
/// jti-less minter is deleted and the default refresh TTL is 30d), and
/// `revoke_all_for_user` never covered them either. Closing it needs the legacy
/// branch retired — its own change.
///
/// RESIDUE 2 (pre-existing): a file DOWNLOAD token (`DownloadTokenClaims`,
/// `aud: ziee-download`, 1h TTL) is a separate signed credential with no `ver`,
/// so one minted before a logout keeps serving its file until it expires. Narrow
/// — a single, already-owned file, with permissions re-checked at download.
///
/// The bump + revoke are ONE transaction, and the sync signal is published only
/// after it commits: see `refresh_tokens::end_session_atomically` for why a
/// partial logout would hand the session straight back.
#[debug_handler]
pub async fn logout(
    auth: JwtAuth,
    Extension(ctx): Extension<AuthContext>,
    origin: SyncOrigin,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let user_id = uuid::Uuid::parse_str(&auth.claims.sub).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            AppError::internal_with_id(format!("parse user id from token: {e}")),
        )
    })?;
    refresh_tokens::end_session_atomically(ctx.pool(), user_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Both writes have committed — only now tell the user's other connections,
    // so a device racing to /auth/me on this signal is guaranteed to observe
    // the bump and get its 401. `origin.0` suppresses the echo back to the tab
    // that logged out (it is tearing itself down already).
    ctx.sync.publish(
        AuthSyncEntity::Session,
        AuthSyncAction::Update,
        user_id,
        Audience::owner(user_id),
        origin.0,
    );

    // Clear the httpOnly refresh cookie on web clients (harmless no-op
    // for body-token clients that never had one).
    let mut resp = ().into_response();
    resp.headers_mut().append(
        header::SET_COOKIE,
        cookie::clear_refresh_cookie(cookie::is_secure_request(&headers)),
    );
    Ok((StatusCode::NO_CONTENT, resp))
}

/// Documentation for logout endpoint
pub fn logout_docs(op: TransformOperation) -> TransformOperation {
    op.description("Logout current user")
        .id("Auth.logout")
        .tag("auth")
        .response::<204, ()>()
}

/// GET /api/auth/me
/// Get currently authenticated user with their effective permissions
#[debug_handler]
pub async fn me(
    auth: JwtAuth,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<MeResponse>> {
    // Parse user ID from claims
    let user_id = uuid::Uuid::parse_str(&auth.claims.sub).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            AppError::internal_with_id(format!("parse user id from token: {e}")),
        )
    })?;

    // Get user from database
    let user = ctx.user()
        .get_by_id(user_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, AppError::not_found("User")))?;

    // A deactivated account must not keep reading its profile on a still-valid
    // JWT. JwtAuth only validates the token — it never re-checks is_active, so
    // reject here. 401 is the same teardown signal the session-sync path relies
    // on: delete_user / toggle-inactive emit a Session signal expecting the
    // device's /auth/me re-bootstrap to 401 and log out. Mirrors the is_active
    // gate the login + refresh handlers (and RequirePermissions) already enforce.
    if !user.is_active {
        return Err((
            StatusCode::UNAUTHORIZED,
            AppError::unauthorized("ACCOUNT_DEACTIVATED", "Account is deactivated"),
        ));
    }

    // Get effective permissions (union of user permissions + group permissions)
    let user_service = UserService::new(ctx.user());
    let permissions = user_service
        .get_effective_permissions(user_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let has_password = user.password_hash.is_some();

    Ok((
        StatusCode::OK,
        Json(MeResponse {
            user,
            permissions,
            has_password,
        }),
    ))
}

/// Documentation for me endpoint
pub fn me_docs(op: TransformOperation) -> TransformOperation {
    op.description("Get currently authenticated user with their effective permissions")
        .id("Auth.me")
        .tag("auth")
        .response::<200, Json<MeResponse>>()
}

/// POST /api/auth/profile
/// Update the authenticated user's own profile. Gated on `profile::edit`
/// (the codebase's "edit own profile" permission, held by the default
/// group) and scoped to the caller. Only `username` + `display_name`
/// are accepted — `email`, `is_active`, `is_admin`, and `permissions`
/// can NEVER be set here (the request struct doesn't carry them), which
/// keeps this path safe from privilege escalation / email-takeover.
pub async fn update_profile<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (ProfileEdit,)>,
    Extension(ctx): Extension<AuthContext>,
    origin: SyncOrigin,
    Json(req): Json<UpdateProfileRequest>,
) -> ApiResult<Json<User>> {
    let user_id = auth.user.id;

    // Trim username, then run the shared bound/charset gate. Previously only
    // the blank case was rejected, so this self-service route accepted a
    // 500-character name (→ varchar(100) overflow → generic 500) and an
    // injection-shaped name like `admin' OR '1'='1; DROP TABLE users;--`
    // (→ persisted, locking the account out of login).
    let username = req.username.map(|u| u.trim().to_string());
    if let Some(ref u) = username {
        crate::auth::username::validate_username(u).map_err(AppError::to_api_error)?;
    }

    // display_name is tri-state: absent/null → keep; a value → set
    // (trimmed); empty/whitespace → clear back to NULL.
    let set_display_name = req.display_name.is_some();
    let display_name = req.display_name.and_then(|d| {
        let t = d.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    });

    // Bound + charset gate on the trimmed value. This path previously had NO
    // display_name validation at all, so a 256-character name overflowed the
    // varchar(255) column and a name containing U+0000 could not be stored at
    // all — both reached the client as a generic 500 SYSTEM_DATABASE_ERROR.
    // Validated AFTER trimming so the bound applies to what is actually stored.
    if let Some(ref d) = display_name {
        crate::auth::username::validate_display_name(d).map_err(AppError::to_api_error)?;
    }

    // Username uniqueness friendly pre-check: only a *different* user
    // holding the name is a conflict — re-submitting your own current
    // username is a no-op. The DB UNIQUE constraint (mapped to 409 inside
    // `update_profile`) is the race-safe backstop.
    if let Some(ref u) = username
        && let Some(existing) = ctx.user()
            .get_by_username(u)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        && existing.id != user_id
    {
        return Err((StatusCode::CONFLICT, AppError::conflict("Username")));
    }

    // `to_api_error` carries the AppError's own status: the UNIQUE-violation
    // race maps to 409, anything else to 500.
    let updated_user = ctx.user()
        .update_profile(user_id, username, set_display_name, display_name)
        .await
        .map_err(AppError::to_api_error)?;

    ctx.events.emit_user(UserEvent::Updated { user: updated_user.clone() });

    // Owner-scoped realtime sync so the user's OTHER devices re-bootstrap
    // /auth/me and converge on the new username / display_name without a
    // reload. Mirrors the admin edit path (user::update_user); the self-echo
    // is suppressed via the originating connection id.
    ctx.sync.publish(
        AuthSyncEntity::Profile,
        AuthSyncAction::Update,
        updated_user.id,
        Audience::owner(updated_user.id),
        origin.0,
    );

    Ok((StatusCode::OK, Json(updated_user)))
}

/// Documentation for update_profile endpoint
pub fn update_profile_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(ProfileEdit,)>(op)
        .description("Update the authenticated user's own profile (username + display_name)")
        .id("Auth.updateProfile")
        .tag("auth")
        .response::<200, Json<User>>()
        .response_with::<409, (), _>(|r| r.description("Username already taken"))
        .response_with::<400, (), _>(|r| {
            r.description("Username or display name failed validation")
        })
}

/// POST /api/auth/password
/// Change the authenticated user's own password. Gated on `profile::edit`
/// and scoped to the caller. Only valid for local-password accounts;
/// OAuth/LDAP-only users get 400 NO_LOCAL_PASSWORD. Mirrors the desktop
/// `tunnel_auth` handler but is a separate route (`/auth/password`) so
/// the two never collide.
pub async fn change_password<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (ProfileEdit,)>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<ChangePasswordRequest>,
) -> ApiResult<()> {
    let user = auth.user;

    // Only local-password accounts can change a password.
    let current_hash = user.password_hash.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            AppError::bad_request(
                "NO_LOCAL_PASSWORD",
                "This account has no local password (you sign in via an external provider).",
            ),
        )
    })?;

    // Verify the current password as proof.
    let ok = password::verify_password(&req.current_password, current_hash).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            AppError::internal_with_id(format!("verify password: {e}")),
        )
    })?;
    if !ok {
        return Err((
            StatusCode::UNAUTHORIZED,
            AppError::unauthorized("INVALID_CREDENTIALS", "Current password is incorrect"),
        ));
    }

    // Validate the new password's strength.
    if let Err(msg) = password::validate_password_strength(&req.new_password) {
        return Err((
            StatusCode::BAD_REQUEST,
            AppError::bad_request("WEAK_PASSWORD", msg),
        ));
    }

    let new_hash = password::hash_password(&req.new_password).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            AppError::internal_with_id(format!("hash password: {e}")),
        )
    })?;

    // update_password also bumps password_changed_at.
    ctx.user()
        .update_password(user.id, &new_hash)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Revoke the user's whitelisted refresh tokens on credential rotation
    // (OWASP session-management). Mirrors `logout`. Every mint path now
    // routes through `mint_session_tokens`, so every issued refresh token
    // carries a whitelisted `jti` and is revoked here (only jti-less
    // tokens minted BEFORE this feature deployed slip through, and those
    // age out within the refresh TTL). Revoking the active successor also
    // closes the rotation-grace window (`rotation_grace_successor`
    // requires an active successor). Outstanding access tokens stay valid
    // for their short remaining TTL.
    refresh_tokens::revoke_all_for_user(ctx.pool(), user.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok((StatusCode::NO_CONTENT, ()))
}

/// Documentation for change_password endpoint
pub fn change_password_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(ProfileEdit,)>(op)
        .description("Change the authenticated user's own password (requires the current password)")
        .id("Auth.changePassword")
        .tag("auth")
        .response::<204, ()>()
        .response_with::<401, (), _>(|r| r.description("Current password is incorrect"))
        .response_with::<400, (), _>(|r| {
            r.description("New password fails strength check, or account has no local password")
        })
}

/// GET /api/auth/oauth/{provider_name}/authorize
/// Initiate OAuth flow for the specified provider
#[debug_handler]
pub async fn oauth_authorize(
    Extension(ctx): Extension<AuthContext>,
    headers: axum::http::HeaderMap,
    Path(provider_name): Path<String>,
    Query(query): Query<OAuthAuthorizeQuery>,
) -> Result<impl IntoResponse, (StatusCode, AppError)> {
    // Get provider configuration
    let provider_config = provider_repo::get_provider_by_name(ctx.pool(), &provider_name)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                AppError::database_error(e),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                AppError::not_found("Authentication provider"),
            )
        })?;

    // Create provider instance
    let provider = create_provider(&provider_config, ctx.pool().clone()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            AppError::bad_request("PROVIDER_ERROR", format!("Provider error: {}", e)),
        )
    })?;

    // SECURITY: ignore the user-supplied redirect_uri query parameter
    // and always use the server's canonical OAuth callback URL. The
    // original implementation let `?redirect_uri=https://evil.com/` flow
    // through to the OAuth authorize call; well-configured providers
    // would reject the mismatch against their registered URI, but
    // misconfigured ones (which are common with self-hosted IdP setups)
    // would happily redirect the victim's browser to evil.com WITH the
    // OAuth `code` in the query string — evil.com can then exchange
    // the code for the access + ID token. Closes 01-auth F-07 (High).
    //
    // The OAuth2 spec requires an absolute URL — derive scheme + host
    // from the inbound request. Reverse-proxy operators should ensure
    // their proxy forwards X-Forwarded-Proto so https survives the
    // hop; otherwise we fall back to http (the dev / tests default).
    // The path portion is server-controlled (provider_name comes from
    // URL routing matched against a string we built ourselves, not
    // user-controlled here).
    // SECURITY: derive redirect_uri from PROXY-SET headers ONLY when
    // the operator explicitly opted into trusting them via
    // `server.trust_forwarded_headers: true` (default false). When
    // the server is exposed directly (no reverse proxy), an attacker
    // can send `X-Forwarded-Host: evil.com` straight to the backend
    // and a permissive IdP (Keycloak wildcard, Dex, Authentik) will
    // happily hand the OAuth `code` to evil.com — same F-07 attack
    // class as the dropped Referer-derivation, just shifted to a
    // different header. With the flag off, derive from HOST only
    // (set by the client but only routable to OUR origin's IP).
    //
    // Never trust Referer in either mode — Referer is unconditionally
    // attacker-controllable.
    //
    // Dev: the Vite proxy is configured to forward X-Forwarded-Host
    // explicitly (see src-app/ui/vite.config.ts), so dev mode runs
    // with trust_forwarded_headers=true by setting it in
    // config/dev.yaml.
    fn safe_scheme(s: &str) -> Option<&str> {
        match s {
            "http" | "https" => Some(s),
            _ => None,
        }
    }
    let trust_proxy = crate::auth::trust_forwarded_headers();
    let scheme = if trust_proxy {
        headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| safe_scheme(s))
            .unwrap_or("http")
            .to_string()
    } else {
        "http".to_string()
    };
    let host = {
        let from_proxy = if trust_proxy {
            headers
                .get("x-forwarded-host")
                .and_then(|v| v.to_str().ok())
        } else {
            None
        };
        from_proxy
            .or_else(|| {
                headers
                    .get(axum::http::header::HOST)
                    .and_then(|v| v.to_str().ok())
            })
            .filter(|h| !h.is_empty())
            .map(|s| s.to_string())
    };
    // Prefer the operator-configured https public origin when set (deploy
    // behind an HTTPS edge): the header-derived scheme would be http there —
    // the edge terminates TLS and forwards plain HTTP to this container — and
    // Google rejects non-localhost http:// redirect_uris. A configured origin
    // is operator-controlled (not request-derived), so it does not reintroduce
    // the header-spoofing risk. Falls through to the header derivation below for
    // local/direct deployments (whose configured value, if any, is an http
    // loopback that fails the https gate).
    let origin = if let Some(configured) = crate::auth::configured_public_origin() {
        configured
    } else {
        match host {
            Some(h) => format!("{}://{}", scheme, h),
            None => {
                if cfg!(debug_assertions) {
                    "http://localhost".to_string()
                } else {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        AppError::bad_request(
                            "OAUTH_MISCONFIGURED",
                            "Server cannot derive redirect URL",
                        ),
                    ));
                }
            }
        }
    };
    let redirect_uri = format!(
        "{}/api/auth/oauth/{}/callback",
        origin, provider_name
    );

    // Validate + capture return_to. We never round-trip it through
    // the provider URL — it lives on `oauth_sessions.return_to`
    // (see G3 in the plan). Only same-origin paths are accepted;
    // anything else (absolute URLs, protocol-relative `//host/...`,
    // backslash tricks) is silently dropped so the callback falls
    // back to `/`.
    let validated_return_to = validate_return_to(query.return_to.as_deref());

    // Initialize OAuth flow
    let oauth_result = provider
        .init_oauth_flow(&redirect_uri, validated_return_to.as_deref())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                AppError::internal_with_id(format!("oauth init: {e}")),
            )
        })?;

    // Redirect to provider's authorization URL
    Ok(Redirect::temporary(&oauth_result.redirect_url))
}

/// Reject anything that isn't a same-origin path: must start with a
/// single `/` (not `//` — protocol-relative), no backslashes, no
/// control characters. Anything else returns None and the callback
/// falls back to `/`.
fn validate_return_to(rt: Option<&str>) -> Option<String> {
    let rt = rt?;
    if !rt.starts_with('/') || rt.starts_with("//") {
        return None;
    }
    if rt.bytes().any(|b| b == b'\\' || b < 0x20) {
        return None;
    }
    Some(rt.to_string())
}

/// GET /api/auth/oauth/{provider_name}/callback
/// Handle OAuth callback from provider (Google, Microsoft, generic
/// OIDC, etc. — anything that uses the `query` response mode).
#[debug_handler]
pub async fn oauth_callback(
    Extension(jwt_service): Extension<Arc<JwtService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(provider_name): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, AppError)> {
    // SECURITY: same enumeration concern as oauth_callback_post —
    // looking up the provider by name first would expose distinct
    // 404 (unknown name) vs 401 (known name, bad state) statuses,
    // letting an attacker probe which provider names exist. Validate
    // the state first via oauth_sessions (which proves the request
    // was solicited AND tells us the provider via provider_id), and
    // collapse failure modes into a single neutral 400.
    let session = ctx.auth()
        .get_oauth_session_by_state(&query.state)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let session = match session {
        Some(s) => s,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                AppError::bad_request(
                    "INVALID_STATE",
                    "Callback state is invalid or expired",
                ),
            ));
        }
    };
    let provider_config = provider_repo::get_provider_by_id(ctx.pool(), session.provider_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                AppError::bad_request(
                    "INVALID_STATE",
                    "Callback state is invalid or expired",
                ),
            )
        })?;
    if provider_config.name != provider_name {
        return Err((
            StatusCode::BAD_REQUEST,
            AppError::bad_request(
                "INVALID_STATE",
                "Callback state is invalid or expired",
            ),
        ));
    }
    oauth_complete(&ctx, jwt_service, &headers, provider_name, query.code, query.state, None).await
}

/// POST /api/auth/oauth/{provider_name}/callback
/// Apple Sign In's `response_mode=form_post` lands here. Same
/// decision tree as the GET path, plus first-time-only Apple `user`
/// JSON merging (Apple gives us the user's display name exactly
/// ONCE, in this body — persist it or lose it forever).
#[debug_handler]
pub async fn oauth_callback_post(
    Extension(jwt_service): Extension<Arc<JwtService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(provider_name): Path<String>,
    headers: HeaderMap,
    Form(form): Form<AppleCallbackForm>,
) -> Result<impl IntoResponse, (StatusCode, AppError)> {
    // SECURITY: only Apple uses response_mode=form_post. Reject POSTs
    // targeted at any other provider — without this gate, a hostile
    // cross-origin form could submit `(code, state)` against
    // /oauth/google/callback (cookie-less endpoint with no other
    // protection) and trigger an account-binding flow as if Google
    // had posted form_post.
    //
    // SECURITY: also single-flatten the error responses. Looking up
    // the provider by NAME then returning distinct 404/405/307s
    // would let an attacker enumerate which provider names exist
    // and which are Apple (migration 47 pre-seeds google/microsoft/
    // apple rows even when disabled). Instead: validate the state
    // first via the oauth_sessions row (which proves the request
    // was solicited and tells us the provider via provider_id), and
    // collapse all failure modes into a single neutral 400.
    let session = ctx.auth()
        .get_oauth_session_by_state(&form.state)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let session = match session {
        Some(s) => s,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                AppError::bad_request(
                    "INVALID_STATE",
                    "Callback state is invalid or expired",
                ),
            ));
        }
    };
    let provider_config = provider_repo::get_provider_by_id(ctx.pool(), session.provider_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                AppError::bad_request(
                    "INVALID_STATE",
                    "Callback state is invalid or expired",
                ),
            )
        })?;
    if provider_config.provider_type != "apple"
        || provider_config.name != provider_name
    {
        return Err((
            StatusCode::BAD_REQUEST,
            AppError::bad_request(
                "INVALID_STATE",
                "Callback state is invalid or expired",
            ),
        ));
    }
    oauth_complete(&ctx, jwt_service, &headers, provider_name, form.code, form.state, form.user).await
}

/// Shared callback completion logic. The user has bounced back from
/// the OAuth provider; figure out which of the four landing states
/// they're in (returning user / first-broker-link required / new
/// user / nothing to do) and route accordingly.
async fn oauth_complete(
    ctx: &AuthContext,
    jwt_service: Arc<JwtService>,
    headers: &HeaderMap,
    provider_name: String,
    code: String,
    state: String,
    apple_user_json: Option<String>,
) -> Result<Response, (StatusCode, AppError)> {
    // Run the inner logic, then ALWAYS try to delete the oauth_sessions
    // row keyed by `state` — providers delete on success, but every
    // error path used to leave an orphan row that would only be reaped
    // by the cleanup job (or worse, never if the cleanup job isn't
    // running). Use of `let _ = ...` is deliberate: a delete failure
    // here is non-fatal (the row will be reaped by TTL), and we don't
    // want to mask the original error.
    let result =
        oauth_complete_inner(ctx, jwt_service, headers, provider_name, code, &state, apple_user_json)
            .await;
    if result.is_err() {
        let _ = ctx.auth().delete_oauth_session(&state).await;
    }
    result
}

async fn oauth_complete_inner(
    ctx: &AuthContext,
    jwt_service: Arc<JwtService>,
    headers: &HeaderMap,
    provider_name: String,
    code: String,
    state: &str,
    apple_user_json: Option<String>,
) -> Result<Response, (StatusCode, AppError)> {
    let provider_config = provider_repo::get_provider_by_name(ctx.pool(), &provider_name)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| (
            StatusCode::NOT_FOUND,
            AppError::not_found("Authentication provider"),
        ))?;

    let provider = create_provider(&provider_config, ctx.pool().clone()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            AppError::bad_request("PROVIDER_ERROR", format!("Provider error: {}", e)),
        )
    })?;

    // PEEK at the oauth_sessions row before handing off to the
    // provider — the provider deletes it on success and we need the
    // return_to for the final redirect. Errors here are non-fatal:
    // worst case we fall back to "/".
    let return_to = ctx.auth()
        .get_oauth_session_by_state(state)
        .await
        .ok()
        .flatten()
        .and_then(|s| s.return_to);

    let mut auth_result = provider
        .handle_oauth_callback(&code, state, state)
        .await
        .map_err(|e| {
            (
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized(
                    "OAUTH_FAILED",
                    format!("OAuth authentication failed: {}", e),
                ),
            )
        })?;

    // Apple form_post: merge the first-time-only `user` JSON before
    // any DB writes so the new user gets the display name on row
    // creation. No-op for non-Apple providers.
    if let Some(user_json_str) = apple_user_json.as_deref() {
        merge_apple_user_json(&mut auth_result, user_json_str);
    }

    // SECURITY: drop the email if the provider didn't assert it as
    // verified. Without this, a sloppy IdP (or a provider that
    // simply omits the `email_verified` claim) can hand us an
    // unverified email, and our email-collision branch would later
    // bind a social identity to a victim's local account. Stripping
    // the email forces auto-provisioning with email=None — user has
    // to enter a real email out-of-band.
    if !email_verified_from_auth_result(&auth_result)
        && auth_result
            .external_email
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    {
        auth_result.external_email = None;
        auth_result.attributes.email = String::new();
    }

    let provider_id = provider_config.id;

    // ── 1. Existing link → returning user, just issue JWT ────────
    // Single UPDATE+RETURNING (`touch_auth_link_and_get_user_id`)
    // bumps last_login_at and returns the user_id in one round-trip.
    if let Some(user_id) = ctx.auth()
        .touch_auth_link_and_get_user_id(provider_id, &auth_result.external_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
    {
        let user = ctx.user()
            .get_by_id(user_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
            .ok_or_else(|| (StatusCode::NOT_FOUND, AppError::not_found("User")))?;

        if !user.is_active {
            return Err((
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized("ACCOUNT_DISABLED", "User account is disabled"),
            ));
        }

        ctx.user()
            .update_last_login(user.id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        let minted =
            mint_session_tokens(ctx.pool(), &jwt_service, user.id, &user.username, &user.email, user.is_admin)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        return Ok(success_redirect(&minted, return_to.as_deref(), headers));
    }

    // ── 2. Email collision with an existing local account ───────
    //     → First-Broker-Link: do NOT auto-link, require password.
    if email_verified_from_auth_result(&auth_result) {
        if let Some(email) = auth_result.external_email.as_deref() {
            if !email.is_empty() {
                if let Some(target_user_id) = ctx.auth()
                    .find_user_by_email_for_linking(email)
                    .await
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
                {
                    let target_user = ctx.user()
                        .get_by_id(target_user_id)
                        .await
                        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
                        .ok_or_else(|| (StatusCode::NOT_FOUND, AppError::not_found("User")))?;

                    if target_user.password_hash.is_some() {
                        // Local-password account → standard FBL flow.
                        let link_token = ctx.auth()
                            .create_pending_link(
                                provider_id,
                                target_user_id,
                                &auth_result.external_id,
                                auth_result.external_email.as_deref(),
                                Some(&auth_result.metadata),
                            )
                            .await
                            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
                        return Ok(Redirect::temporary(&format!(
                            "/auth/link-account?link_token={}",
                            url::form_urlencoded::byte_serialize(link_token.as_bytes())
                                .collect::<String>()
                        ))
                        .into_response());
                    } else {
                        // External-only account with the same email exists.
                        // Refuse with a clear error — auto-linking these
                        // would let the user hijack the account.
                        return Err((
                            StatusCode::CONFLICT,
                            AppError::new(
                                StatusCode::CONFLICT,
                                "EMAIL_TAKEN_BY_EXTERNAL_ACCOUNT",
                                "An account with this email already exists via another login method. Sign in with that method instead.",
                            ),
                        ));
                    }
                }
            }
        }
    }

    // ── 3. No link, no collision → auto-provision a new user ────
    let username = ensure_unique_username(ctx.pool(), &auth_result.attributes.username).await?;
    let display_name = auth_result
        .attributes
        .display_name
        .clone()
        .unwrap_or_else(|| username.clone());

    // A `users` row requires a non-null email. The unverified-email guard
    // above intentionally drops an email the provider didn't assert as
    // verified, so a provider that returns no verified email cannot
    // auto-create an account. Reject cleanly here rather than letting a
    // NULL reach the NOT NULL `email` column (which previously surfaced as
    // an opaque 500 from the DB constraint).
    let email = match auth_result.external_email.clone().filter(|e| !e.is_empty()) {
        Some(e) => e,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                AppError::bad_request(
                    "OAUTH_EMAIL_REQUIRED",
                    "The identity provider did not return a verified email address, \
                     which is required to create an account.",
                ),
            ));
        }
    };

    // Atomic provision: user row + auth_link + default-group
    // assignment in a single transaction. Partial failure (e.g.
    // unique-collision race on the auth_link) used to leave a
    // password-less orphan that locked the user out forever —
    // re-login would trip the email-collision branch and refuse.
    // Persist the provider's verification verdict for THIS email. The
    // guard above drops any email the provider didn't assert as
    // verified, and the `OAUTH_EMAIL_REQUIRED` branch then refuses to
    // provision without one — so this is `true` on every reachable path
    // today. Threading the computed value rather than hardcoding `true`
    // keeps the row honest if either guard is ever relaxed.
    let email_verified = email_verified_from_auth_result(&auth_result);

    let new_user_id = ctx.auth()
        .provision_external_user_atomic(
            &username,
            Some(email.as_str()),
            email_verified,
            &display_name,
            provider_id,
            &auth_result.external_id,
            Some(&auth_result.metadata),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let user = ctx.user()
        .get_by_id(new_user_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                AppError::internal_error("Failed to fetch newly created user"),
            )
        })?;

    let minted =
        mint_session_tokens(ctx.pool(), &jwt_service, user.id, &user.username, &user.email, user.is_admin)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(success_redirect(&minted, return_to.as_deref(), headers))
}

/// Was the email asserted as verified by the provider? Both shapes
/// happen in the wild: standard OIDC providers put it under
/// `metadata.user_info.email_verified` (boolean), Apple puts it
/// under `metadata.email_verified` (boolean — we coerced from
/// Apple's quirky string earlier).
fn email_verified_from_auth_result(r: &AuthResult) -> bool {
    let read = |v: &serde_json::Value| -> Option<bool> {
        v.as_bool()
            .or_else(|| v.as_str().map(|s| s.eq_ignore_ascii_case("true")))
    };
    if let Some(v) = r
        .metadata
        .get("user_info")
        .and_then(|ui| ui.get("email_verified"))
    {
        if let Some(b) = read(v) {
            return b;
        }
    }
    if let Some(v) = r.metadata.get("email_verified") {
        if let Some(b) = read(v) {
            return b;
        }
    }
    false
}

/// Append `2`, `3`, … to the username until we find one that's not
/// taken. Up to 999 attempts before giving up — a hard cap rather
/// than an infinite loop to avoid pathological cases.
#[doc(hidden)] // pub for integration tests (auto-provision username collision)
pub async fn ensure_unique_username(
    pool: &PgPool,
    base: &str,
) -> Result<String, (StatusCode, AppError)> {
    let users = UserRepository::new(pool.clone());
    let mut candidate = base.trim().to_string();
    if candidate.is_empty() {
        candidate = "user".to_string();
    }
    if users
        .get_by_username(&candidate)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .is_none()
    {
        return Ok(candidate);
    }
    for n in 2..=999u32 {
        let next = format!("{}{}", candidate, n);
        if users
            .get_by_username(&next)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
            .is_none()
        {
            return Ok(next);
        }
    }
    Err((
        StatusCode::INTERNAL_SERVER_ERROR,
        AppError::internal_error("Could not derive a unique username"),
    ))
}

/// Merge Apple's first-auth-only `user` form field into the
/// AuthResult. The id_token has `sub` and `email` but never `name`;
/// `name` arrives in this body exactly once and only on first auth.
fn merge_apple_user_json(auth_result: &mut AuthResult, user_json_str: &str) {
    #[derive(serde::Deserialize)]
    struct AppleUser {
        name: Option<AppleName>,
        email: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct AppleName {
        #[serde(rename = "firstName")]
        first_name: Option<String>,
        #[serde(rename = "lastName")]
        last_name: Option<String>,
    }
    let Ok(parsed) = serde_json::from_str::<AppleUser>(user_json_str) else {
        return;
    };
    // SECURITY: the `user` payload comes from Apple's POST body, NOT
    // the signed id_token. Only trust the email if the id_token didn't
    // contain one AND the form value looks like a private-relay
    // address. Otherwise an attacker who can capture a (code, state)
    // pair could supply an arbitrary `email` here and trigger FBL
    // against a victim's account.
    if let Some(email) = parsed.email {
        let id_token_had_email = auth_result
            .external_email
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let looks_like_relay =
            email.to_ascii_lowercase().ends_with("@privaterelay.appleid.com");
        if !id_token_had_email && looks_like_relay {
            auth_result.external_email = Some(email.clone());
            auth_result.attributes.email = email;
            // Apple only sends the `user` blob on the FIRST authorization and
            // the relay address is Apple-controlled (it can't target a victim's
            // local account), so once it passes the relay gate above treat it
            // as verified — otherwise the downstream "drop unverified email"
            // guard would strip it and force OAUTH_EMAIL_REQUIRED.
            if !auth_result.metadata.is_object() {
                auth_result.metadata = serde_json::json!({});
            }
            auth_result.metadata["email_verified"] = serde_json::json!(true);
        }
    }
    if let Some(name) = parsed.name {
        let first = name.first_name.clone();
        let last = name.last_name.clone();
        if auth_result.attributes.first_name.is_none() {
            auth_result.attributes.first_name = first.clone();
        }
        if auth_result.attributes.last_name.is_none() {
            auth_result.attributes.last_name = last.clone();
        }
        if auth_result.attributes.display_name.is_none() {
            auth_result.attributes.display_name = match (first, last) {
                (Some(f), Some(l)) => Some(format!("{} {}", f, l)),
                (Some(f), None) => Some(f),
                (None, Some(l)) => Some(l),
                _ => None,
            };
        }
    }
}

/// Build the post-auth redirect. The access token (+ its `expires_in`,
/// which the SPA uses to schedule its proactive silent refresh) rides in
/// the URL **fragment** (`#token=…`) so it does not appear in server
/// access logs, Referer headers, or browser history. The SPA's
/// `/auth/callback` page reads the fragment then immediately calls
/// `history.replaceState` to scrub it.
///
/// The refresh token NEVER touches the URL: OAuth is a browser-only flow,
/// so it always travels as the httpOnly `ziee_refresh` cookie set on this
/// redirect response. (Before migration 129 the OAuth refresh token was
/// generated and then silently discarded — OAuth sessions could never
/// refresh at all.)
fn success_redirect(
    minted: &TokenPairWithJti,
    return_to: Option<&str>,
    headers: &HeaderMap,
) -> Response {
    let target = return_to.unwrap_or("/");
    let fragment = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("token", &minted.pair.access_token)
        .append_pair("expires_in", &minted.pair.expires_in.to_string())
        .append_pair("return_to", target)
        .finish();
    let max_age = (minted.refresh_expires_at - chrono::Utc::now())
        .num_seconds()
        .max(0);
    let set_cookie = cookie::build_refresh_cookie(
        &minted.pair.refresh_token,
        max_age,
        cookie::is_secure_request(headers),
    );
    let mut resp = Redirect::temporary(&format!("/auth/callback#{}", fragment)).into_response();
    resp.headers_mut().append(header::SET_COOKIE, set_cookie);
    resp
}

/// POST /api/auth/link-account
/// First-Broker-Login confirmation. The user proves ownership of an
/// existing local account by entering its password; on success we
/// atomically create the user_auth_links row + issue a JWT. The
/// pending row is consumed (deleted) regardless of outcome on success.
#[debug_handler]
pub async fn link_account(
    Extension(jwt_service): Extension<Arc<JwtService>>,
    Extension(ctx): Extension<AuthContext>,
    headers: HeaderMap,
    Json(req): Json<LinkAccountRequest>,
) -> ApiResult<Response> {
    // Peek (don't consume) so a wrong-password attempt doesn't burn
    // the single-use token — the user gets to retry without
    // re-running the entire OAuth dance. The token is still
    // single-use: we delete it on the FIRST successful password +
    // link insertion.
    let pending = ctx.auth()
        .peek_pending_link(&req.link_token)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized(
                    "INVALID_LINK_TOKEN",
                    "Link token is invalid, already used, or expired",
                ),
            )
        })?;

    // Authorization checks BEFORE bumping the attempts counter so
    // an OAuth-only target account (no password_hash) doesn't burn
    // 5 attempts before the user sees a useful error.
    let user = ctx.user()
        .get_by_id(pending.target_user_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, AppError::not_found("User")))?;

    if !user.is_active {
        return Err((
            StatusCode::UNAUTHORIZED,
            AppError::unauthorized("ACCOUNT_DISABLED", "Account is disabled"),
        ));
    }

    let pw_hash = user.password_hash.as_deref().ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            AppError::unauthorized("INVALID_CREDENTIALS", "Invalid credentials"),
        )
    })?;

    // SECURITY: per-token brute-force gate. The global rate limiter
    // caps requests per IP; this caps requests per token, defeating
    // distributed brute-force. Hard ceiling 5 attempts — past that
    // the token is invalidated. Runs after the is_active +
    // password_hash checks so legitimate misuse-detection errors
    // don't burn attempts.
    const LINK_TOKEN_MAX_ATTEMPTS: i32 = 5;
    let attempt_n = ctx.auth()
        .bump_pending_link_attempts(&req.link_token)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        // None means the row was concurrently consumed or just
        // expired between our peek and our bump. Surface as the
        // honest "this token is no longer valid" rather than
        // masquerading as a brute-force throttle.
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized(
                    "INVALID_LINK_TOKEN",
                    "Link token is invalid, already used, or expired",
                ),
            )
        })?;
    if attempt_n > LINK_TOKEN_MAX_ATTEMPTS {
        let _ = ctx.auth().delete_pending_link(&req.link_token).await;
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "TOO_MANY_ATTEMPTS",
                "Too many failed attempts. Restart the sign-in flow.",
            ),
        ));
    }

    let ok = password::verify_password(&req.password, pw_hash).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            AppError::internal_with_id(format!("verify password: {e}")),
        )
    })?;
    if !ok {
        // Pending row intentionally preserved for retry.
        return Err((
            StatusCode::UNAUTHORIZED,
            AppError::unauthorized("INVALID_CREDENTIALS", "Invalid credentials"),
        ));
    }

    // The pending link only exists because the provider asserted this
    // email verified AND it matched this user's address (the FBL branch
    // in `oauth_callback`), and the password just proved account
    // ownership. So the provider's proof carries over to the local row:
    // link the identity and mark the email verified, atomically. The
    // repository re-checks the address match at the write.
    //
    // The `?` below is load-bearing: `ApiResult` is
    // `Result<(StatusCode, T), (StatusCode, AppError)>` and axum lets the
    // TUPLE's status win over the body's, so the `map_err(|e|
    // (INTERNAL_SERVER_ERROR, e))` this call used to carry would ship the
    // repository's typed 409 (a foreign identity already linked elsewhere)
    // under a 500 — passing a body assertion while the status stayed wrong.
    // `?` routes through `From<AppError>`, which reads `err.status_code`.
    let verification_upgraded = ctx
        .auth()
        .link_verified_external_identity(
            user.id,
            pending.provider_id,
            &pending.external_id,
            pending.external_email.as_deref(),
            pending.external_data.as_ref(),
        )
        .await?;

    // Consume the pending token now that the link is bound.
    let _ = ctx.auth().delete_pending_link(&req.link_token).await;

    ctx.user()
        .update_last_login(user.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Mint + whitelist the session tokens (admin-configured lifetimes).
    let minted = mint_session_tokens(ctx.pool(), &jwt_service, user.id, &user.username, &user.email, user.is_admin)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // `user` was read BEFORE the link write, so its `email_verified` is
    // stale when the link just verified it. Reflect the upgrade we made
    // rather than returning a payload that contradicts the row we just
    // wrote. (Today's web client happens to refetch /auth/me right after
    // this, so it would self-correct — but a response that lies about
    // state it just changed is a trap for the next consumer.)
    // `verification_upgraded` is the DELTA, not the resulting state: an
    // already-verified user upgrades nothing and keeps its own `true`.
    let mut user = user;
    if verification_upgraded {
        user.email_verified = true;
    }

    Ok(token_response(&headers, StatusCode::OK, minted, |tokens| {
        AuthResponse { user, tokens }
    }))
}

pub fn link_account_docs(op: TransformOperation) -> TransformOperation {
    op.description(
        "Confirm a First-Broker-Login pending link by proving ownership of \
         the existing local account with its password. Returns a fresh JWT \
         pair on success.",
    )
    .id("Auth.linkAccount")
    .tag("auth")
    .response::<200, Json<AuthResponse>>()
}

/// GET /api/auth/providers — public list of enabled providers for
/// the login page. Returns ONLY the fields the login UI needs;
/// never exposes config / secrets / tenant IDs.
#[debug_handler]
pub async fn list_public_providers(
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<PublicProvidersResponse>> {
    let rows = provider_repo::list_public_providers(ctx.pool())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let providers: Vec<PublicProvider> = rows
        .into_iter()
        .map(|p| {
            let display_name = p
                .config
                .get("display_name")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| default_display_name(&p.name, &p.provider_type));
            PublicProvider {
                name: p.name,
                provider_type: p.provider_type,
                display_name,
            }
        })
        .collect();
    Ok((StatusCode::OK, Json(PublicProvidersResponse { providers })))
}

pub fn list_public_providers_docs(op: TransformOperation) -> TransformOperation {
    op.description(
        "List enabled third-party auth providers for the login page. Public \
         endpoint; returns only display fields, never config or secrets.",
    )
    .id("Auth.listProviders")
    .tag("auth")
    .response::<200, Json<PublicProvidersResponse>>()
}

fn default_display_name(name: &str, provider_type: &str) -> String {
    match provider_type {
        "apple" => "Sign in with Apple".to_string(),
        _ => format!("Sign in with {}", name),
    }
}

// =====================================================
// Admin: Auth Provider CRUD
// =====================================================
//
// All handlers below are gated through the typed permission
// extractor — never hand-rolled. The list endpoint requires
// `auth_providers::read`; everything mutating + the test endpoint
// requires `auth_providers::manage`. Administrators-group members
// get both implicitly via the `*` wildcard, so no seed grants needed.

/// Sensitive keys whose values are masked in any GET / list response.
const SENSITIVE_CONFIG_KEYS: &[&str] = &["client_secret", "bind_password", "private_key_path"];

/// Mask sentinel used in admin-list GET responses. We treat the
/// literal sentinel as "empty" on writes too — otherwise an admin
/// who clicks Save in the EditDrawer without retyping the password
/// would persist the bullet string `"••••••"` as the new secret,
/// destroying the real one (HIGH bug found by 2026-05-25 audit).
const MASK_SENTINEL: &str = "••••••";

/// Mask sensitive values inside an auth_providers.config JSONB
/// payload. Returns a cloned + masked copy; the original (with real
/// secrets) stays in the DB.
fn mask_provider_config(config: &serde_json::Value) -> serde_json::Value {
    let mut masked = config.clone();
    if let serde_json::Value::Object(map) = &mut masked {
        for key in SENSITIVE_CONFIG_KEYS {
            if let Some(v) = map.get_mut(*key) {
                if v.as_str().map(|s| !s.is_empty()).unwrap_or(false) {
                    *v = serde_json::Value::String(MASK_SENTINEL.to_string());
                }
            }
        }
    }
    masked
}

fn provider_to_response(p: crate::auth::providers::models::AuthProvider) -> AuthProviderResponse {
    AuthProviderResponse {
        config: mask_provider_config(&p.config),
        id: p.id,
        name: p.name,
        provider_type: p.provider_type,
        enabled: p.enabled,
        created_at: p.created_at,
        updated_at: p.updated_at,
        last_test_at: p.last_test_at,
        last_test_ok: p.last_test_ok,
        last_test_message: p.last_test_message,
    }
}

/// GET /api/admin/auth-providers
pub async fn admin_list_providers<R: IdentityResolver<User = User, Group = Group>>(
    _: RequirePermissions<R, (AuthProvidersRead,)>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<Vec<AuthProviderResponse>>> {
    let rows = provider_repo::list_providers(ctx.pool())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let resp: Vec<AuthProviderResponse> = rows.into_iter().map(provider_to_response).collect();
    Ok((StatusCode::OK, Json(resp)))
}

pub fn admin_list_providers_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(AuthProvidersRead,)>(op)
        .id("AuthProviders.list")
        .tag("auth-providers")
        .summary("List all configured auth providers (secrets masked)")
        .response::<200, Json<Vec<AuthProviderResponse>>>()
}

/// POST /api/admin/auth-providers
pub async fn admin_create_provider<R: IdentityResolver<User = User, Group = Group>>(
    _: RequirePermissions<R, (AuthProvidersManage,)>,
    Extension(ctx): Extension<AuthContext>,
    origin: SyncOrigin,
    Json(req): Json<CreateAuthProviderRequest>,
) -> ApiResult<Json<CreateAuthProviderResponse>> {
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            AppError::bad_request("INVALID_NAME", "Provider name cannot be empty"),
        ));
    }
    // "local" is the built-in password provider — not creatable via
    // this endpoint (creating a second one leaves the login routing
    // in undefined state).
    let allowed_types = ["oidc", "oauth2", "apple", "ldap"];
    if !allowed_types.contains(&req.provider_type.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            AppError::bad_request(
                "INVALID_PROVIDER_TYPE",
                format!(
                    "provider_type must be one of: {}",
                    allowed_types.join(", ")
                ),
            ),
        ));
    }
    // Propagate the error's OWN status. `ApiResult` is
    // `Result<(StatusCode, T), (StatusCode, AppError)>` and axum's
    // `(StatusCode, impl IntoResponse)` impl lets the tuple's status WIN over the
    // body's — so the old hardcoded INTERNAL_SERVER_ERROR turned every typed
    // error from this repo call (now incl. the 409 name conflict) into a 500.
    // The `From<AppError> for (StatusCode, AppError)` impl reads `err.status_code`.
    let row = provider_repo::create_provider(
        ctx.pool(),
        req.name.trim(),
        req.provider_type.as_str(),
        req.enabled,
        &req.config,
        ctx.secret_key(),
    )
    .await?;
    let row_id = row.id;
    // If enabled=true, probe immediately; on failure the row stays
    // created but `enabled` is flipped back to false and
    // `connection_warning` carries the reason.
    let outcome = provider_health::enforce_on_create_with_enabled(row, ctx.pool(), &*ctx.events)
        .await
        .map_err(|e| {
            (
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                e,
            )
        })?;
    ctx.events.emit_auth_provider(AuthProviderEvent::created(outcome.provider.id));
    ctx.sync.publish(
        AuthSyncEntity::AuthProvider,
        AuthSyncAction::Create,
        row_id,
        Audience::perm::<AuthProvidersRead>(),
        origin.0,
    );
    Ok((
        StatusCode::CREATED,
        Json(CreateAuthProviderResponse {
            provider: provider_to_response(outcome.provider),
            connection_warning: outcome.connection_warning,
        }),
    ))
}

pub fn admin_create_provider_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(AuthProvidersManage,)>(op)
        .id("AuthProviders.create")
        .tag("auth-providers")
        .summary("Create a new auth provider")
        .response::<201, Json<CreateAuthProviderResponse>>()
}

/// PUT /api/admin/auth-providers/{id}
/// Empty `client_secret` in the patch config preserves the existing
/// value — so admins can edit other fields without re-entering
/// secrets they don't have at hand.
///
/// Enable-transition (`enabled` going false → true) runs a live
/// connection probe; on failure the row's `enabled` is forced back
/// to false in the same response and a 400
/// `AUTH_PROVIDER_ENABLE_FAILED_HEALTH_CHECK` is returned. Other
/// fields in the same PUT stay persisted — the partial save is
/// preferable to losing the admin's concurrent edits.
pub async fn admin_update_provider<R: IdentityResolver<User = User, Group = Group>>(
    _: RequirePermissions<R, (AuthProvidersManage,)>,
    Extension(ctx): Extension<AuthContext>,
    origin: SyncOrigin,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<UpdateAuthProviderRequest>,
) -> ApiResult<Json<AuthProviderResponse>> {
    // Snapshot the existing row BEFORE the update so the
    // enable-transition check below can compare. We also need the
    // existing config to preserve sensitive fields.
    let existing = provider_repo::get_provider_by_id(ctx.pool(), id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, AppError::not_found("Auth provider")))?;
    let old_enabled = existing.enabled;

    // If config is being patched, merge sensitive empty fields with
    // the existing row to preserve secrets.
    let final_config = if let Some(mut new_config) = req.config {
        preserve_sensitive_fields(&existing.config, &mut new_config);
        Some(new_config)
    } else {
        None
    };

    // Propagate the error's OWN status — see the note in `admin_create_provider`.
    // A hardcoded INTERNAL_SERVER_ERROR here would mask the 409 the repository
    // now returns for a rename onto a taken provider name.
    let row = provider_repo::update_provider(
        ctx.pool(),
        id,
        req.name.as_deref().map(str::trim),
        req.enabled,
        final_config.as_ref(),
        ctx.secret_key(),
    )
    .await?;

    // Enforce: if enabled transitioned false → true, probe live; on
    // failure this returns Err(400) which the `?` propagates.
    let enforced = provider_health::enforce_on_update_transition(row, old_enabled, ctx.pool(), &*ctx.events)
        .await
        .map_err(|e| {
            (
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::BAD_REQUEST),
                e,
            )
        })?;

    ctx.events.emit_auth_provider(AuthProviderEvent::updated(enforced.id));
    ctx.sync.publish(
        AuthSyncEntity::AuthProvider,
        AuthSyncAction::Update,
        id,
        Audience::perm::<AuthProvidersRead>(),
        origin.0,
    );
    Ok((StatusCode::OK, Json(provider_to_response(enforced))))
}

/// For each `SENSITIVE_CONFIG_KEYS`: if it's missing or empty in
/// `new_config`, copy the existing value over. Lets admins PATCH a
/// provider row without re-entering secrets.
fn preserve_sensitive_fields(
    existing: &serde_json::Value,
    new_config: &mut serde_json::Value,
) {
    let (existing_obj, new_obj) = match (existing, new_config) {
        (serde_json::Value::Object(e), serde_json::Value::Object(n)) => (e, n),
        _ => return,
    };
    for key in SENSITIVE_CONFIG_KEYS {
        // Treat absent, empty string, AND the masking sentinel as
        // "leave existing value alone." The sentinel branch matters
        // because the admin EditDrawer GETs a masked config + sends
        // it straight back on Save; without this guard the bullets
        // would replace the real secret.
        let new_should_preserve = new_obj
            .get(*key)
            .map(|v| {
                v.as_str()
                    .map(|s| s.is_empty() || s == MASK_SENTINEL)
                    .unwrap_or(false)
            })
            .unwrap_or(true);
        if new_should_preserve {
            if let Some(existing_val) = existing_obj.get(*key) {
                new_obj.insert((*key).to_string(), existing_val.clone());
            }
        }
    }
}

pub fn admin_update_provider_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(AuthProvidersManage,)>(op)
        .id("AuthProviders.update")
        .tag("auth-providers")
        .summary("Update an auth provider (empty client_secret preserves existing)")
        .response::<200, Json<AuthProviderResponse>>()
}

/// DELETE /api/admin/auth-providers/{id}
pub async fn admin_delete_provider<R: IdentityResolver<User = User, Group = Group>>(
    _: RequirePermissions<R, (AuthProvidersManage,)>,
    Extension(ctx): Extension<AuthContext>,
    origin: SyncOrigin,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<Json<DeleteProviderResponse>> {
    // Atomic: existence-lock + link count + delete in ONE transaction (row
    // locked FOR UPDATE). Serializes concurrent deletes and makes the reported
    // `affected_user_links` exactly match the FK cascade. `None` means the
    // provider doesn't exist (or a concurrent deleter already removed it) → 404.
    let (name, affected) =
        provider_repo::delete_provider_with_link_count(ctx.pool(), id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
            .ok_or_else(|| (StatusCode::NOT_FOUND, AppError::not_found("Auth provider")))?;
    ctx.events.emit_auth_provider(AuthProviderEvent::deleted(id, name));
    ctx.sync.publish(
        AuthSyncEntity::AuthProvider,
        AuthSyncAction::Delete,
        id,
        Audience::perm::<AuthProvidersRead>(),
        origin.0,
    );
    Ok((
        StatusCode::OK,
        Json(DeleteProviderResponse {
            deleted: true,
            affected_user_links: affected,
        }),
    ))
}

pub fn admin_delete_provider_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(AuthProvidersManage,)>(op)
        .id("AuthProviders.delete")
        .tag("auth-providers")
        .summary("Delete an auth provider (cascades user_auth_links)")
        .response::<200, Json<DeleteProviderResponse>>()
}

/// POST /api/admin/auth-providers/{id}/test
/// Run the provider's `test_connection`. Discovery + dummy
/// token-exchange probe for OIDC / Apple; URL-syntax check for
/// OAuth2. Returns 200 always; success / failure is in the body so
/// the admin UI can render a nicer inline message than a non-200.
/// Persists the result on the auth_providers row (`last_test_at`,
/// `last_test_ok`, `last_test_message`) so the result survives a
/// page reload.
pub async fn admin_test_provider<R: IdentityResolver<User = User, Group = Group>>(
    _: RequirePermissions<R, (AuthProvidersManage,)>,
    Extension(ctx): Extension<AuthContext>,
    origin: SyncOrigin,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<Json<TestProviderResponse>> {
    let row = provider_repo::get_provider_by_id(ctx.pool(), id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, AppError::not_found("Auth provider")))?;

    let was_enabled = row.enabled;
    let probe = provider_health::probe_provider(&row, ctx.pool()).await;
    // record_test_outcome persists last_test_* AND auto-disables the
    // row when the probe failed on a currently-enabled provider —
    // mirroring the LLM repo's pattern.
    if let Err(e) =
        provider_health::record_test_outcome(ctx.pool(), &*ctx.events, id, was_enabled, &probe).await
    {
        tracing::warn!(error = ?e, "failed to persist auth-provider test outcome");
    }
    // Emit Updated regardless of the auto-disable path so listeners
    // that don't subscribe to AutoDisabled (e.g. a future audit hook)
    // still see "the test changed this row." Mirrors LLM repo's
    // test_repository_connection_by_id pattern.
    ctx.events.emit_auth_provider(AuthProviderEvent::updated(id));
    ctx.sync.publish(
        AuthSyncEntity::AuthProvider,
        AuthSyncAction::Update,
        id,
        Audience::perm::<AuthProvidersRead>(),
        origin.0,
    );
    Ok((
        StatusCode::OK,
        Json(TestProviderResponse {
            ok: probe.ok,
            message: probe.message,
        }),
    ))
}

pub fn admin_test_provider_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(AuthProvidersManage,)>(op)
        .id("AuthProviders.test")
        .tag("auth-providers")
        .summary("Test the auth provider's connection (probes discovery + credentials)")
        .response::<200, Json<TestProviderResponse>>()
}

/// POST /api/admin/auth-providers/test-config
/// Test a provider config WITHOUT saving it to the database. Used by
/// the EditDrawer's "Test config" button so admins can verify their
/// inputs before committing. Body is the same shape as Create — we
/// just don't persist the row. Result is not stored anywhere (no row
/// to attach to).
pub async fn admin_test_provider_config<R: IdentityResolver<User = User, Group = Group>>(
    _: RequirePermissions<R, (AuthProvidersManage,)>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateAuthProviderRequest>,
) -> ApiResult<Json<TestProviderResponse>> {
    // Same allowlist as admin_create_provider — refuses "local" + any
    // unknown type. Without this, an admin could probe arbitrary
    // LDAP/OIDC servers via the test endpoint (lower-impact than
    // SSRF since admin already has manage permission, but still
    // good hygiene).
    let allowed_types = ["oidc", "oauth2", "apple", "ldap"];
    if !allowed_types.contains(&req.provider_type.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            AppError::bad_request(
                "INVALID_PROVIDER_TYPE",
                format!(
                    "provider_type must be one of: {}",
                    allowed_types.join(", ")
                ),
            ),
        ));
    }
    let transient = crate::auth::providers::models::AuthProvider {
        id: uuid::Uuid::nil(),
        name: req.name,
        provider_type: req.provider_type,
        enabled: true,
        config: req.config,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        last_test_at: None,
        last_test_ok: None,
        last_test_message: None,
    };
    let probe = provider_health::probe_provider(&transient, ctx.pool()).await;
    Ok((
        StatusCode::OK,
        Json(TestProviderResponse {
            ok: probe.ok,
            message: probe.message,
        }),
    ))
}

pub fn admin_test_provider_config_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(AuthProvidersManage,)>(op)
        .id("AuthProviders.testConfig")
        .tag("auth-providers")
        .summary("Test a provider config without saving (used by the EditDrawer)")
        .response::<200, Json<TestProviderResponse>>()
}

// `run_test_for_row` lived here pre-migration. The probe was moved
// to `crate::auth::providers::health::probe_provider`; the three callers
// (admin_test_provider, admin_test_provider_config, the enforce
// paths) all consume it from there now.

// NOTE: `get_auth_config`, `login_password_only`, and `change_password`
// all live in the desktop tauri crate (`desktop/tauri/src/modules/
// tunnel_auth/`) now — they're all part of the Remote Access feature
// which depends on the desktop-only `remote_access_settings` table
// and the `users.password_changed_at` column added by a desktop
// migration. Keeping them in this crate would orphan unreachable
// routes in server-only deployments.
