// JWT authentication infrastructure

use aide::OperationIo;
use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use std::sync::Arc;

use crate::auth::context::AuthContext;
use crate::auth::jwt::{Claims, JwtService};
use ziee_core::AppError;

/// JWT extractor for protected routes
/// This extracts and validates the JWT token from the Authorization header
#[derive(Clone, OperationIo)]
#[aide(input)]
pub struct JwtAuth {
    pub claims: Claims,
}

impl<S> FromRequestParts<S> for JwtAuth
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, AppError);

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            // Get JWT service from app state
            let jwt_service = parts.extensions.get::<Arc<JwtService>>().ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    AppError::internal_error("JWT service not configured"),
                )
            })?;

            // Extract Authorization header
            let auth_header = parts
                .headers
                .get("Authorization")
                .and_then(|h| h.to_str().ok())
                .ok_or_else(|| {
                    (
                        StatusCode::UNAUTHORIZED,
                        AppError::unauthorized("MISSING_TOKEN", "Authorization header is missing"),
                    )
                })?;

            // Extract token from header
            let token = JwtService::extract_token_from_header(auth_header)
                .map_err(|e| (StatusCode::UNAUTHORIZED, e))?;

            // Validate token and extract claims
            let claims = jwt_service
                .validate_access_token(token)
                .map_err(|e| (StatusCode::UNAUTHORIZED, e))?;

            assert_token_version_current(parts, &claims).await?;

            Ok(JwtAuth { claims })
        }
    }
}

/// THE access-token revocation rule.
///
/// `validate_access_token` proves a token was signed by us and has not
/// expired; this proves the SESSION it belongs to has not been torn down.
/// Logout bumps `users.token_version`, so a token minted before it no longer
/// matches and is rejected immediately instead of staying valid for the rest
/// of its (24h-by-default) TTL.
///
/// A token with NO `ver` claim was minted before this shipped; it maps to `0`
/// and so matches the column's `DEFAULT 0` — those sessions keep working until
/// they expire, and the user's first logout kills them. Deploying forces zero
/// logouts.
///
/// INVARIANT — every caller of `JwtService::validate_access_token` that GATES a
/// route MUST verify the epoch. There are exactly two, and they read it two
/// different ways:
///   1. `JwtAuth` / `OptionalJwtAuth` (this file) — via
///      `assert_token_version_current`, which does its own scalar read because
///      these extractors never load the user.
///   2. the framework permission extractor (`ZieeIdentityResolver::authenticate`)
///      — via `get_by_id_with_token_version`, folding the read into the user
///      load it already performs, so the hot path pays no extra round-trip.
/// The only other `validate_access_token` callers (`chat/stream/handler.rs`,
/// `sync/handlers.rs`) read `exp` solely for a stream deadline and their routes
/// are gated by `RequirePermissions`, i.e. by (2).
pub fn verify_token_version(
    claims_ver: Option<i32>,
    db_version: i32,
) -> Result<(), (StatusCode, AppError)> {
    if claims_ver.unwrap_or(0) == db_version {
        return Ok(());
    }
    Err((
        StatusCode::UNAUTHORIZED,
        AppError::unauthorized(
            "SESSION_REVOKED",
            "Session has been revoked; please sign in again",
        ),
    ))
}

/// Look up the user's current epoch and apply [`verify_token_version`].
///
/// The `AuthContext` (the per-request pool + repo handle the app layers onto
/// the router) is the scalar-read source — these extractors never load the
/// user, so unlike the permission extractor there is nothing to fold the read
/// into. Fail-CLOSED: an absent context or a DB error is a 500, never an
/// implicit pass — a revocation check that fails open is not a revocation
/// check.
async fn assert_token_version_current(
    parts: &Parts,
    claims: &Claims,
) -> Result<(), (StatusCode, AppError)> {
    let user_id = uuid::Uuid::parse_str(&claims.sub).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            AppError::unauthorized("INVALID_TOKEN", "Invalid user id in token"),
        )
    })?;

    let ctx = parts.extensions.get::<AuthContext>().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            AppError::internal_error("Auth context not configured"),
        )
    })?;

    let db_version = ctx
        .user()
        .get_token_version(user_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                AppError::unauthorized("USER_NOT_FOUND", "User not found"),
            )
        })?;

    verify_token_version(claims.ver, db_version)
}

/// Optional JWT extractor - doesn't fail if token is missing/invalid
/// Useful for endpoints that can work with or without authentication
// Real axum `FromRequestParts` extractor API (impl below) for optional-auth
// routes; no route consumes it yet. Narrow allow (was module blanket) rather
// than delete a public extractor surface.
#[allow(dead_code)]
#[derive(Clone, OperationIo)]
#[aide(input)]
pub struct OptionalJwtAuth {
    pub claims: Option<Claims>,
}

impl<S> FromRequestParts<S> for OptionalJwtAuth
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, AppError);

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            // Get JWT service from app state
            let jwt_service = parts.extensions.get::<Arc<JwtService>>();

            if jwt_service.is_none() {
                return Ok(OptionalJwtAuth { claims: None });
            }

            let jwt_service = jwt_service.unwrap();

            // Try to extract Authorization header
            let auth_header = parts
                .headers
                .get("Authorization")
                .and_then(|h| h.to_str().ok());

            if auth_header.is_none() {
                return Ok(OptionalJwtAuth { claims: None });
            }

            // Try to extract and validate token
            let token_result = JwtService::extract_token_from_header(auth_header.unwrap());
            if token_result.is_err() {
                return Ok(OptionalJwtAuth { claims: None });
            }

            let claims_result = jwt_service.validate_access_token(token_result.unwrap());
            if let Ok(claims) = claims_result {
                // A revoked session is no better than an invalid token: fall
                // back to anonymous, per this extractor's contract. No route
                // consumes it today, but leaving the one unchecked validation
                // path in the tree is exactly the gap this module closes.
                if assert_token_version_current(parts, &claims).await.is_err() {
                    return Ok(OptionalJwtAuth { claims: None });
                }
                Ok(OptionalJwtAuth {
                    claims: Some(claims),
                })
            } else {
                Ok(OptionalJwtAuth { claims: None })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TEST-3: the single comparison rule both read paths share
    /// (`JwtAuth`'s scalar read and the permission extractor's folded read).
    /// Pure — no DB, no HTTP.
    #[test]
    fn matching_version_is_accepted() {
        assert!(verify_token_version(Some(3), 3).is_ok());
        assert!(verify_token_version(Some(0), 0).is_ok());
    }

    /// A token minted before a logout carries the OLD epoch → rejected. This
    /// is the whole feature: without it the token stays valid for its full
    /// (24h-default) TTL after logout.
    #[test]
    fn stale_version_is_rejected_as_session_revoked() {
        let (status, err) = verify_token_version(Some(3), 4).unwrap_err();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // 401 (not 403) is what the client's on-401 interceptor treats as a
        // teardown signal; the code is what the UI/tests key on.
        assert!(
            format!("{err:?}").contains("SESSION_REVOKED"),
            "expected SESSION_REVOKED, got {err:?}"
        );
    }

    /// A token minted at a LOWER epoch than the DB is the normal stale case;
    /// a HIGHER one should never occur, but must also fail closed rather than
    /// be treated as "close enough".
    #[test]
    fn version_from_the_future_is_also_rejected() {
        assert!(verify_token_version(Some(9), 4).is_err());
    }

    /// Back-compat: a token minted before `ver` existed has no claim. It maps
    /// to 0 and so matches a user still at the column's DEFAULT 0 — those
    /// sessions keep working until they expire, so deploying forces zero
    /// logouts. Once that user logs out (epoch ≥ 1) the token dies.
    #[test]
    fn ver_less_token_matches_default_zero_but_dies_after_a_logout() {
        assert!(verify_token_version(None, 0).is_ok());
        assert!(verify_token_version(None, 1).is_err());
    }
}
