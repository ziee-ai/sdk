//! Account recovery — REST handlers.
//!
//! Two public endpoints (the questions lookup and the reset itself: the caller
//! is by definition not signed in) and five signed-in management endpoints.
//! The management ones are gated on `profile::edit` like the other
//! self-service account routes and require the CURRENT password in the body.
//!
//! Every public failure is the same body. See [`crate::auth::recovery`].

use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;

use aide::transform::TransformOperation;
use axum::extract::ConnectInfo;
use axum::{Extension, Json, debug_handler, http::{HeaderMap, StatusCode}};
use uuid::Uuid;

use ziee_core::{ApiResult, AppError};
use ziee_framework::permissions::{IdentityResolver, RequirePermissions, with_permission};
use ziee_framework::sync::{Audience, SyncOrigin};

use crate::auth::context::{AuthContext, AuthSyncAction, AuthSyncEntity};
use crate::auth::password;
use crate::auth::recovery::types::*;
use crate::auth::recovery::{
    self, CODE_BCRYPT_COST, CODES_PER_SET, MAX_QUESTIONS, QUESTIONS, RecoveryRepository,
};
use crate::auth::refresh_tokens;
use crate::user::events::UserEvent;
use crate::user::permissions::ProfileEdit;
use crate::user::{Group, User};

const SCOPE_NAME: &str = "name";
const SCOPE_IP: &str = "ip";
const SCOPE_REAUTH: &str = "reauth";

type Fail = (StatusCode, AppError);

fn internal<E: std::fmt::Display>(what: &str, e: E) -> Fail {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        AppError::internal_with_id(format!("{what}: {e}")),
    )
}

fn db(e: AppError) -> Fail {
    (StatusCode::INTERNAL_SERVER_ERROR, e)
}

/// The ONE body every failed reset returns.
fn recovery_failed() -> Fail {
    (
        StatusCode::BAD_REQUEST,
        AppError::bad_request(
            "RECOVERY_FAILED",
            "That did not work. Check what you entered and try again.",
        ),
    )
}

fn rate_limited(minutes: u32) -> Fail {
    (
        StatusCode::TOO_MANY_REQUESTS,
        AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "RECOVERY_RATE_LIMITED",
            format!("Too many attempts. Try again in about {minutes} minutes."),
        ),
    )
}

fn not_available() -> Fail {
    (
        StatusCode::NOT_FOUND,
        AppError::new(
            StatusCode::NOT_FOUND,
            "RECOVERY_NOT_AVAILABLE",
            "This recovery method is not available on this site",
        ),
    )
}

/// The caller's address: the peer, or with `trust_forwarded_headers` the
/// RIGHTMOST `X-Forwarded-For` entry (the one the nearest trusted proxy
/// appended; earlier entries are client-supplied). `None` when neither is
/// available, in which case only the per-name limit applies.
fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<IpAddr> {
    if crate::auth::trust_forwarded_headers()
        && let Some(v) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(last) = v.rsplit(',').next()
        && let Ok(ip) = last.trim().parse::<IpAddr>()
    {
        return Some(ip);
    }
    peer.map(|p| p.ip())
}

fn dummy_code_hash() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| {
        bcrypt::hash(Uuid::new_v4().to_string(), CODE_BCRYPT_COST).expect("bcrypt dummy hash")
    })
}

fn dummy_answer_hash() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| {
        bcrypt::hash(Uuid::new_v4().to_string(), bcrypt::DEFAULT_COST).expect("bcrypt dummy hash")
    })
}

// ───────────────────────────── public ─────────────────────────────

/// GET /api/auth/recovery/capabilities
pub async fn capabilities(Extension(ctx): Extension<AuthContext>) -> ApiResult<Json<RecoveryCapabilities>> {
    let c = &ctx.options().config;
    Ok((
        StatusCode::OK,
        Json(RecoveryCapabilities {
            recovery_codes: c.recovery_codes.enabled,
            security_questions: c.security_questions.enabled,
        }),
    ))
}

pub fn capabilities_docs(op: TransformOperation) -> TransformOperation {
    op.description("Which self-service recovery methods this site offers (public).")
        .id("Auth.recoveryCapabilities")
        .tag("auth")
        .response::<200, Json<RecoveryCapabilities>>()
}

fn prompts<'a>(it: impl Iterator<Item = &'a recovery::CatalogueQuestion>) -> Vec<QuestionPrompt> {
    it.map(|q| QuestionPrompt { key: q.key.to_string(), prompt: q.prompt.to_string() })
        .collect()
}

/// POST /api/auth/recovery/questions
#[debug_handler]
pub async fn lookup_questions(
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<QuestionsLookupRequest>,
) -> ApiResult<Json<QuestionsLookupResponse>> {
    let cfg = &ctx.options().config;
    if !cfg.security_questions.enabled {
        return Ok((StatusCode::OK, Json(QuestionsLookupResponse { questions: vec![] })));
    }
    let name = req.username.trim();
    let real = if name.is_empty() || name.chars().count() > 100 {
        Vec::new()
    } else {
        match ctx.user().get_by_username(name).await.map_err(db)? {
            Some(u) if u.is_active && !u.is_admin => RecoveryRepository::new(ctx.pool().clone())
                .questions(u.id)
                .await
                .map_err(db)?,
            _ => Vec::new(),
        }
    };
    let list: Vec<QuestionPrompt> = if real.is_empty() {
        prompts(recovery::decoy_questions(ctx.options().pepper(), name).into_iter())
    } else {
        prompts(real.iter().filter_map(|r| recovery::question(&r.question_key)))
    };
    Ok((StatusCode::OK, Json(QuestionsLookupResponse { questions: list })))
}

pub fn lookup_questions_docs(op: TransformOperation) -> TransformOperation {
    op.description(concat!(
        "The security questions to ask for a username (public).\n\n",
        "For a name that has none configured, or no account at all, the list is a stable ",
        "decoy of the same shape, so this endpoint does not reveal whether an account exists.",
    ))
    .id("Auth.recoveryQuestions")
    .tag("auth")
    .response::<200, Json<QuestionsLookupResponse>>()
}

/// Verify a recovery code against the user's unused set. ALWAYS performs
/// `CODES_PER_SET` bcrypt verifications whatever the account holds, so the
/// timing does not say whether the account exists or has codes.
async fn match_code(unused: Vec<recovery::repository::CodeRow>, input: String) -> Option<Uuid> {
    let canon = recovery::normalize_code(&input).unwrap_or_else(|| "000000000000".to_string());
    let well_formed = recovery::normalize_code(&input).is_some();
    tokio::task::spawn_blocking(move || {
        let mut found = None;
        for i in 0..CODES_PER_SET {
            let (hash, id) = match unused.get(i) {
                Some(r) => (r.code_hash.as_str(), Some(r.id)),
                None => (dummy_code_hash(), None),
            };
            let ok = bcrypt::verify(&canon, hash).unwrap_or(false);
            if ok && well_formed && found.is_none() {
                found = id;
            }
        }
        found
    })
    .await
    .unwrap_or(None)
}

/// Verify ALL configured answers. Always performs `MAX_QUESTIONS` bcrypt
/// verifications. `true` only when the answered keys are exactly the configured
/// keys and every answer matches.
async fn match_answers(
    stored: Vec<recovery::repository::QuestionRow>,
    given: Vec<QuestionAnswer>,
) -> bool {
    tokio::task::spawn_blocking(move || {
        let mut all_ok = !stored.is_empty() && given.len() == stored.len();
        for i in 0..MAX_QUESTIONS {
            let (hash, expected_key) = match stored.get(i) {
                Some(r) => (r.answer_hash.as_str(), Some(r.question_key.as_str())),
                None => (dummy_answer_hash(), None),
            };
            let answer = expected_key
                .and_then(|k| given.iter().find(|g| g.key == k))
                .map(|g| recovery::normalize_answer(&g.answer))
                .unwrap_or_default();
            let ok = bcrypt::verify(&answer, hash).unwrap_or(false);
            if expected_key.is_some() && !ok {
                all_ok = false;
            }
            if expected_key.is_some()
                && !given.iter().any(|g| Some(g.key.as_str()) == expected_key)
            {
                all_ok = false;
            }
        }
        all_ok
    })
    .await
    .unwrap_or(false)
}

/// POST /api/auth/recovery/reset
#[debug_handler]
pub async fn reset_password(
    Extension(ctx): Extension<AuthContext>,
    origin: SyncOrigin,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(req): Json<ResetPasswordRequest>,
) -> ApiResult<()> {
    let cfg = ctx.options().config.clone();
    let lim = cfg.recovery.clone();
    let window = lim.lockout_minutes as i32;

    // Shape checks that say nothing about any account and are not counted.
    let name = req.username.trim().to_string();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(recovery_failed());
    }
    if let Err(msg) = password::validate_password_strength(&req.new_password) {
        return Err((StatusCode::BAD_REQUEST, AppError::bad_request("WEAK_PASSWORD", msg)));
    }
    if req.code.as_deref().is_some_and(|c| c.len() > 64)
        || req.answers.as_ref().is_some_and(|a| a.len() > MAX_QUESTIONS)
        || req.answers.as_ref().is_some_and(|a| {
            a.iter().any(|x| x.key.len() > 64 || x.answer.len() > 512)
        })
    {
        return Err(recovery_failed());
    }

    let repo = RecoveryRepository::new(ctx.pool().clone());
    let name_key = recovery::name_key(&name);
    let ip = client_ip(&headers, peer.map(|Extension(ConnectInfo(a))| a)).map(|i| i.to_string());

    // Locks first: a locked key answers 429 without touching the account.
    if repo.locked_until(SCOPE_NAME, &name_key).await.map_err(db)?.is_some() {
        return Err(rate_limited(lim.lockout_minutes));
    }
    if let Some(ip) = &ip
        && repo.locked_until(SCOPE_IP, ip).await.map_err(db)?.is_some()
    {
        return Err(rate_limited(lim.lockout_minutes));
    }

    let user = ctx.user().get_by_username(&name).await.map_err(db)?;
    let user = user.filter(|u| u.is_active);

    // Verify. The enabled flags and the account's state decide `proven`, but the
    // same amount of hashing runs on every path.
    let mut consume: Option<Uuid> = None;
    let proven = match req.method {
        RecoveryMethod::Code => {
            let rows = match (&user, cfg.recovery_codes.enabled) {
                (Some(u), true) => repo.unused_codes(u.id).await.map_err(db)?,
                _ => Vec::new(),
            };
            let hit = match_code(rows, req.code.clone().unwrap_or_default()).await;
            consume = hit;
            hit.is_some() && user.is_some() && cfg.recovery_codes.enabled
        }
        RecoveryMethod::Questions => {
            let rows = match (&user, cfg.security_questions.enabled) {
                (Some(u), true) if !u.is_admin => repo.questions(u.id).await.map_err(db)?,
                _ => Vec::new(),
            };
            match_answers(rows, req.answers.clone().unwrap_or_default()).await
                && user.is_some()
                && cfg.security_questions.enabled
        }
    };

    let user = match (proven, user) {
        (true, Some(u)) => u,
        _ => {
            let name_locked = repo
                .record_failure(SCOPE_NAME, &name_key, window, lim.max_failures_per_name as i32)
                .await
                .map_err(db)?;
            let ip_locked = match &ip {
                Some(ip) => repo
                    .record_failure(SCOPE_IP, ip, window, lim.max_failures_per_ip as i32)
                    .await
                    .map_err(db)?,
                None => false,
            };
            if name_locked || ip_locked {
                tracing::info!(name_locked, ip_locked, "recovery: a key was locked out");
            }
            tracing::info!("recovery: reset refused");
            return Err(recovery_failed());
        }
    };

    let new_hash = password::hash_password(&req.new_password)
        .map_err(|e| internal("hash password", e))?;
    let won = repo
        .complete_reset(user.id, consume, &new_hash)
        .await
        .map_err(db)?;
    if !won {
        // Lost a race for the same code: indistinguishable from a wrong one.
        return Err(recovery_failed());
    }

    // Whoever held the old password is signed out everywhere: refresh tokens
    // revoked and the access-token epoch bumped in one transaction.
    refresh_tokens::end_session_atomically(ctx.pool(), user.id)
        .await
        .map_err(db)?;
    repo.clear_attempts(SCOPE_NAME, &name_key).await.map_err(db)?;

    ctx.events.emit_user(UserEvent::Updated { user: ctx.options().scrub_user(user.clone()) });
    ctx.sync.publish(
        AuthSyncEntity::Session,
        AuthSyncAction::Update,
        user.id,
        Audience::owner(user.id),
        origin.0,
    );
    tracing::info!(user_id = %user.id, method = ?req.method, "recovery: password reset");
    Ok((StatusCode::NO_CONTENT, ()))
}

pub fn reset_password_docs(op: TransformOperation) -> TransformOperation {
    op.description(concat!(
        "Reset a forgotten password with a recovery code or the answers to ALL of the ",
        "account's security questions (public).\n\n",
        "Error codes (in `error_code`):\n",
        "- `RECOVERY_FAILED` (400) - ANY failure: unknown username, nothing configured, ",
        "wrong code, wrong answer (never which one), deactivated account\n",
        "- `WEAK_PASSWORD` (400) - the new password fails the strength check\n",
        "- `RECOVERY_RATE_LIMITED` (429) - too many failed attempts for this username or ",
        "client address; the lock lasts the configured window\n\n",
        "On success every existing session of the account is revoked.",
    ))
    .id("Auth.recoveryReset")
    .tag("auth")
    .response::<204, ()>()
    .response_with::<400, (), _>(|r| r.description("Reset refused (generic)"))
    .response_with::<429, (), _>(|r| r.description("Locked out after repeated failures"))
}

// ───────────────────────────── signed in ─────────────────────────────

/// Re-authenticate the caller with their current password, with a per-account
/// failure limit. Returns the verified user's id.
async fn reauth(ctx: &AuthContext, user: &User, current_password: &str) -> Result<(), Fail> {
    let repo = RecoveryRepository::new(ctx.pool().clone());
    let lim = &ctx.options().config.recovery;
    let key = user.id.to_string();
    if repo.locked_until(SCOPE_REAUTH, &key).await.map_err(db)?.is_some() {
        return Err(rate_limited(lim.lockout_minutes));
    }
    let hash = user.password_hash.clone().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            AppError::bad_request(
                "NO_LOCAL_PASSWORD",
                "This account has no local password (you sign in via an external provider).",
            ),
        )
    })?;
    let pw = current_password.to_string();
    let ok = tokio::task::spawn_blocking(move || password::verify_password(&pw, &hash))
        .await
        .map_err(|e| internal("verify password", e))?
        .map_err(|e| internal("verify password", e))?;
    if !ok {
        repo.record_failure(
            SCOPE_REAUTH,
            &key,
            lim.lockout_minutes as i32,
            lim.max_failures_per_account_reauth as i32,
        )
        .await
        .map_err(db)?;
        return Err((
            StatusCode::UNAUTHORIZED,
            AppError::unauthorized("INVALID_CREDENTIALS", "Current password is incorrect"),
        ));
    }
    repo.clear_attempts(SCOPE_REAUTH, &key).await.map_err(db)?;
    Ok(())
}

async fn status_for(ctx: &AuthContext, user: &User) -> Result<RecoveryStatus, Fail> {
    let repo = RecoveryRepository::new(ctx.pool().clone());
    let cfg = &ctx.options().config;
    let (remaining, generated_at) = repo.codes_status(user.id).await.map_err(db)?;
    let stored = repo.questions(user.id).await.map_err(db)?;
    let questions = prompts(stored.iter().filter_map(|r| recovery::question(&r.question_key)));
    let blocked = user.is_admin;
    Ok(RecoveryStatus {
        recovery_codes_enabled: cfg.recovery_codes.enabled,
        security_questions_enabled: cfg.security_questions.enabled,
        codes_remaining: if cfg.recovery_codes.enabled { remaining } else { 0 },
        codes_generated_at: if cfg.recovery_codes.enabled { generated_at } else { None },
        has_recovery: (cfg.recovery_codes.enabled && remaining > 0)
            || (cfg.security_questions.enabled && !blocked && !questions.is_empty()),
        questions: if cfg.security_questions.enabled { questions } else { vec![] },
        available_questions: if cfg.security_questions.enabled && !blocked {
            prompts(QUESTIONS.iter())
        } else {
            vec![]
        },
        questions_blocked: blocked,
    })
}

fn announce(ctx: &AuthContext, user: &User, origin: &SyncOrigin) {
    ctx.events.emit_user(UserEvent::Updated { user: ctx.options().scrub_user(user.clone()) });
    ctx.sync.publish(
        AuthSyncEntity::Profile,
        AuthSyncAction::Update,
        user.id,
        Audience::owner(user.id),
        origin.0,
    );
}

/// GET /api/auth/recovery
pub async fn recovery_status<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (ProfileEdit,)>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<RecoveryStatus>> {
    Ok((StatusCode::OK, Json(status_for(&ctx, &auth.user).await?)))
}

pub fn recovery_status_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(ProfileEdit,)>(op)
        .description("The signed-in user's recovery configuration: codes left, configured question prompts (never answers) and the catalogue.")
        .id("Auth.recoveryStatus")
        .tag("auth")
        .response::<200, Json<RecoveryStatus>>()
}

/// POST /api/auth/recovery/codes
pub async fn generate_codes<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (ProfileEdit,)>,
    Extension(ctx): Extension<AuthContext>,
    origin: SyncOrigin,
    Json(req): Json<ReauthRequest>,
) -> ApiResult<Json<GeneratedCodes>> {
    if !ctx.options().config.recovery_codes.enabled {
        return Err(not_available());
    }
    reauth(&ctx, &auth.user, &req.current_password).await?;
    let codes: Vec<String> = (0..CODES_PER_SET).map(|_| recovery::generate_code()).collect();
    let to_hash: Vec<String> = codes
        .iter()
        .map(|c| recovery::normalize_code(c).expect("a generated code normalises"))
        .collect();
    let hashes = tokio::task::spawn_blocking(move || {
        to_hash
            .iter()
            .map(|c| bcrypt::hash(c, CODE_BCRYPT_COST))
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(|e| internal("hash codes", e))?
    .map_err(|e| internal("hash codes", e))?;
    let generated_at = RecoveryRepository::new(ctx.pool().clone())
        .replace_codes(auth.user.id, &hashes)
        .await
        .map_err(db)?;
    announce(&ctx, &auth.user, &origin);
    Ok((StatusCode::CREATED, Json(GeneratedCodes { codes, generated_at })))
}

pub fn generate_codes_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(ProfileEdit,)>(op)
        .description(concat!(
            "Generate (or regenerate) the user's ten one-time recovery codes. Requires the ",
            "current password. The codes are returned ONCE and never again; generating a new ",
            "set invalidates every code of the previous one.\n\n",
            "Error codes (in `error_code`):\n",
            "- `INVALID_CREDENTIALS` (401) - wrong current password\n",
            "- `RECOVERY_RATE_LIMITED` (429) - too many wrong passwords\n",
            "- `RECOVERY_NOT_AVAILABLE` (404) - recovery codes are off on this site",
        ))
        .id("Auth.recoveryGenerateCodes")
        .tag("auth")
        .response::<201, Json<GeneratedCodes>>()
        .response_with::<401, (), _>(|r| r.description("Current password is incorrect"))
        .response_with::<404, (), _>(|r| r.description("Recovery codes are not enabled"))
        .response_with::<429, (), _>(|r| r.description("Too many wrong passwords"))
}

/// POST /api/auth/recovery/codes/clear
pub async fn clear_codes<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (ProfileEdit,)>,
    Extension(ctx): Extension<AuthContext>,
    origin: SyncOrigin,
    Json(req): Json<ReauthRequest>,
) -> ApiResult<()> {
    if !ctx.options().config.recovery_codes.enabled {
        return Err(not_available());
    }
    reauth(&ctx, &auth.user, &req.current_password).await?;
    RecoveryRepository::new(ctx.pool().clone())
        .delete_codes(auth.user.id)
        .await
        .map_err(db)?;
    announce(&ctx, &auth.user, &origin);
    Ok((StatusCode::NO_CONTENT, ()))
}

pub fn clear_codes_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(ProfileEdit,)>(op)
        .description("Delete every recovery code (used or not). Requires the current password.")
        .id("Auth.recoveryClearCodes")
        .tag("auth")
        .response::<204, ()>()
        .response_with::<401, (), _>(|r| r.description("Current password is incorrect"))
        .response_with::<404, (), _>(|r| r.description("Recovery codes are not enabled"))
        .response_with::<429, (), _>(|r| r.description("Too many wrong passwords"))
}

/// PUT /api/auth/recovery/questions
pub async fn set_questions<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (ProfileEdit,)>,
    Extension(ctx): Extension<AuthContext>,
    origin: SyncOrigin,
    Json(req): Json<SetQuestionsRequest>,
) -> ApiResult<Json<RecoveryStatus>> {
    if !ctx.options().config.security_questions.enabled {
        return Err(not_available());
    }
    if auth.user.is_admin {
        return Err((
            StatusCode::BAD_REQUEST,
            AppError::bad_request(
                "QUESTIONS_NOT_ALLOWED_FOR_ADMIN",
                "Security questions cannot protect an administrator account. Use recovery codes.",
            ),
        ));
    }
    let picks: Vec<(&str, String)> =
        req.questions.iter().map(|q| (q.key.as_str(), q.answer.clone())).collect();
    recovery::validate_answers(&picks)
        .map_err(|m| (StatusCode::BAD_REQUEST, AppError::bad_request("INVALID_QUESTIONS", m)))?;
    reauth(&ctx, &auth.user, &req.current_password).await?;
    let normalised: Vec<(String, String)> = req
        .questions
        .iter()
        .map(|q| (q.key.clone(), recovery::normalize_answer(&q.answer)))
        .collect();
    let hashed = tokio::task::spawn_blocking(move || {
        normalised
            .into_iter()
            .map(|(k, a)| bcrypt::hash(&a, bcrypt::DEFAULT_COST).map(|h| (k, h)))
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(|e| internal("hash answers", e))?
    .map_err(|e| internal("hash answers", e))?;
    RecoveryRepository::new(ctx.pool().clone())
        .replace_questions(auth.user.id, &hashed)
        .await
        .map_err(db)?;
    announce(&ctx, &auth.user, &origin);
    Ok((StatusCode::OK, Json(status_for(&ctx, &auth.user).await?)))
}

pub fn set_questions_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(ProfileEdit,)>(op)
        .description(concat!(
            "Set (replace) the user's security questions: two or three distinct picks from the ",
            "fixed catalogue with an answer each. Requires the current password. A reset needs ",
            "ALL answers.\n\n",
            "Error codes (in `error_code`):\n",
            "- `INVALID_QUESTIONS` (400) - wrong count, unknown or repeated question, answer too ",
            "short/long or repeated\n",
            "- `QUESTIONS_NOT_ALLOWED_FOR_ADMIN` (400)\n",
            "- `INVALID_CREDENTIALS` (401) - wrong current password\n",
            "- `RECOVERY_RATE_LIMITED` (429)\n",
            "- `RECOVERY_NOT_AVAILABLE` (404) - security questions are off on this site",
        ))
        .id("Auth.recoverySetQuestions")
        .tag("auth")
        .response::<200, Json<RecoveryStatus>>()
        .response_with::<400, (), _>(|r| r.description("Invalid question set"))
        .response_with::<401, (), _>(|r| r.description("Current password is incorrect"))
        .response_with::<404, (), _>(|r| r.description("Security questions are not enabled"))
        .response_with::<429, (), _>(|r| r.description("Too many wrong passwords"))
}

/// POST /api/auth/recovery/questions/clear
pub async fn clear_questions<R: IdentityResolver<User = User, Group = Group>>(
    auth: RequirePermissions<R, (ProfileEdit,)>,
    Extension(ctx): Extension<AuthContext>,
    origin: SyncOrigin,
    Json(req): Json<ReauthRequest>,
) -> ApiResult<()> {
    if !ctx.options().config.security_questions.enabled {
        return Err(not_available());
    }
    reauth(&ctx, &auth.user, &req.current_password).await?;
    RecoveryRepository::new(ctx.pool().clone())
        .delete_questions(auth.user.id)
        .await
        .map_err(db)?;
    announce(&ctx, &auth.user, &origin);
    Ok((StatusCode::NO_CONTENT, ()))
}

pub fn clear_questions_docs(op: TransformOperation) -> TransformOperation {
    with_permission::<(ProfileEdit,)>(op)
        .description("Remove the user's security questions. Requires the current password.")
        .id("Auth.recoveryClearQuestions")
        .tag("auth")
        .response::<204, ()>()
        .response_with::<401, (), _>(|r| r.description("Current password is incorrect"))
        .response_with::<404, (), _>(|r| r.description("Security questions are not enabled"))
        .response_with::<429, (), _>(|r| r.description("Too many wrong passwords"))
}
