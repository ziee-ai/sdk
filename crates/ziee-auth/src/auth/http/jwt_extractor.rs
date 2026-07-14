// JWT authentication infrastructure

use aide::OperationIo;
use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use std::sync::Arc;

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

            Ok(JwtAuth { claims })
        }
    }
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
                Ok(OptionalJwtAuth {
                    claims: Some(claims),
                })
            } else {
                Ok(OptionalJwtAuth { claims: None })
            }
        }
    }
}
