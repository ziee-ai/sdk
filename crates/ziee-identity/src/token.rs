//! `TokenVerifier` — the JWT-verify INTERFACE (token → claims).
//!
//! Only the interface lives here. The concrete `JwtService` (HMAC keys,
//! issuer/audience config, `jsonwebtoken` decode, `AppError` mapping, leeway)
//! STAYS in ziee and implements this trait. Associated `Claims`/`Error` types
//! keep the interface free of any `jsonwebtoken`/`AppError`/config dependency,
//! so an app can plug in its own token scheme while the framework enforcement
//! layer depends only on this trait.

/// Verifies bearer tokens and yields the decoded claims.
///
/// Access and refresh tokens are validated separately because they carry
/// different audiences (ziee's refresh audience is `<audience>-refresh`); a
/// concrete implementation applies its own signature, issuer, audience, and
/// expiry validation.
pub trait TokenVerifier {
    /// The decoded claims type produced on successful verification.
    type Claims;
    /// The error type returned on invalid/expired/malformed tokens.
    type Error;

    /// Validate and decode an ACCESS token, returning its claims.
    fn verify_access_token(&self, token: &str) -> Result<Self::Claims, Self::Error>;

    /// Validate and decode a REFRESH token, returning its claims.
    fn verify_refresh_token(&self, token: &str) -> Result<Self::Claims, Self::Error>;
}
