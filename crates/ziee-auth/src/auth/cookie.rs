//! httpOnly refresh-token cookie helpers.
//!
//! Web sessions carry the refresh token in an `HttpOnly` cookie scoped to
//! `Path=/api/auth` (set on login/register/refresh/link-account/setup and
//! the OAuth callback redirect) so it is unreadable by page JavaScript —
//! an XSS on the SPA cannot exfiltrate it. Desktop/tunnel clients keep the
//! JSON-body token (they opt in to cookie mode via the `X-Refresh-Cookie`
//! request header, which browsers send and Tauri does not).
//!
//! Manual `Set-Cookie` header construction — deliberately no cookie crate:
//! aide's typed handlers aren't compiled with axum-extra's cookie support,
//! and two attribute strings don't justify a dependency. Handlers return
//! `ApiResult<Response>` to attach the header (precedent:
//! `file/handlers/download.rs`).

use http::{HeaderMap, HeaderValue, header};

/// Cookie name. `ziee_` prefix so it's obviously ours in devtools.
pub const REFRESH_COOKIE_NAME: &str = "ziee_refresh";

/// Path scope: the refresh token is only ever needed by `/api/auth/*`
/// (refresh + logout), so the browser never attaches it to any other
/// request — CSRF surface stays bounded to the auth endpoints, which are
/// additionally covered by `SameSite=Strict`.
const COOKIE_PATH: &str = "/api/auth";

/// Request header a client sends to opt in to cookie-mode token delivery
/// (`X-Refresh-Cookie: 1`). Sent by the web SPA; absent from desktop
/// Tauri / tunnel clients, which therefore keep body-token behavior.
pub const REFRESH_COOKIE_OPTIN_HEADER: &str = "x-refresh-cookie";

/// Build the `Set-Cookie` value carrying a freshly-minted refresh token.
///
/// `secure` appends the `Secure` attribute — pass
/// `is_secure_request(...)` so https deployments (behind a trusted
/// proxy) get it while plain-http localhost/LAN self-hosts still work.
pub fn build_refresh_cookie(token: &str, max_age_secs: i64, secure: bool) -> HeaderValue {
    let mut cookie = format!(
        "{REFRESH_COOKIE_NAME}={token}; HttpOnly; SameSite=Strict; Path={COOKIE_PATH}; Max-Age={max_age_secs}"
    );
    if secure {
        cookie.push_str("; Secure");
    }
    // A JWT is base64url segments joined by '.', all valid header bytes;
    // from_str can only fail on a malformed token, in which case sending
    // no cookie is the safe degradation.
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// Build the clearing `Set-Cookie` value (empty value, `Max-Age=0`).
/// Attributes must match the setter's for the browser to replace it.
pub fn clear_refresh_cookie(secure: bool) -> HeaderValue {
    let mut cookie = format!(
        "{REFRESH_COOKIE_NAME}=; HttpOnly; SameSite=Strict; Path={COOKIE_PATH}; Max-Age=0"
    );
    if secure {
        cookie.push_str("; Secure");
    }
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// Extract the refresh token from the request's `Cookie` header(s).
/// Returns `None` when absent or empty.
pub fn read_refresh_cookie(headers: &HeaderMap) -> Option<String> {
    for value in headers.get_all(header::COOKIE) {
        let Ok(s) = value.to_str() else { continue };
        for pair in s.split(';') {
            let pair = pair.trim();
            if let Some(token) = pair.strip_prefix(REFRESH_COOKIE_NAME)
                && let Some(token) = token.strip_prefix('=')
                && !token.is_empty()
            {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// True when the client opted in to cookie-mode delivery
/// (`X-Refresh-Cookie: 1`).
pub fn wants_cookie(headers: &HeaderMap) -> bool {
    headers
        .get(REFRESH_COOKIE_OPTIN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim() == "1")
        .unwrap_or(false)
}

/// True when the ORIGINAL client request arrived over https, i.e. the
/// deployment runs behind a trusted reverse proxy that set
/// `X-Forwarded-Proto: https`. The server itself only terminates plain
/// HTTP, so without a trusted proxy the answer is always false (and the
/// cookie is sent without `Secure` — correct for localhost/LAN
/// self-hosts). Gated on the same `trust_forwarded_headers` flag the
/// OAuth redirect-uri derivation uses, so a spoofed XFP header from a
/// direct client can't flip the attribute.
pub fn is_secure_request(headers: &HeaderMap) -> bool {
    super::trust_forwarded_headers()
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("https"))
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_refresh_cookie_attrs() {
        let v = build_refresh_cookie("abc.def.ghi", 2_592_000, false);
        let s = v.to_str().unwrap();
        assert!(s.starts_with("ziee_refresh=abc.def.ghi;"));
        assert!(s.contains("HttpOnly"));
        assert!(s.contains("SameSite=Strict"));
        assert!(s.contains("Path=/api/auth"));
        assert!(s.contains("Max-Age=2592000"));
        assert!(!s.contains("Secure"));

        let v = build_refresh_cookie("abc.def.ghi", 60, true);
        assert!(v.to_str().unwrap().ends_with("; Secure"));
    }

    #[test]
    fn clear_cookie_is_max_age_zero() {
        let v = clear_refresh_cookie(false);
        let s = v.to_str().unwrap();
        assert!(s.starts_with("ziee_refresh=;"));
        assert!(s.contains("Max-Age=0"));
        assert!(s.contains("Path=/api/auth"));
    }

    #[test]
    fn read_refresh_cookie_parses_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=1; ziee_refresh=tok.en.x; theme=dark"),
        );
        assert_eq!(read_refresh_cookie(&headers).as_deref(), Some("tok.en.x"));

        // Name-prefix must not false-match a longer cookie name.
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("ziee_refresh_other=nope"),
        );
        assert_eq!(read_refresh_cookie(&headers), None);

        // Absent / empty-value → None.
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("ziee_refresh="));
        assert_eq!(read_refresh_cookie(&headers), None);
        assert_eq!(read_refresh_cookie(&HeaderMap::new()), None);
    }

    #[test]
    fn wants_cookie_checks_optin_header() {
        let mut headers = HeaderMap::new();
        assert!(!wants_cookie(&headers));
        headers.insert(REFRESH_COOKIE_OPTIN_HEADER, HeaderValue::from_static("1"));
        assert!(wants_cookie(&headers));
        headers.insert(REFRESH_COOKIE_OPTIN_HEADER, HeaderValue::from_static("0"));
        assert!(!wants_cookie(&headers));
    }

    #[test]
    fn is_secure_requires_trusted_proxy() {
        // trust_forwarded_headers() is false in unit tests (OnceCell unset),
        // so even an https XFP header must NOT mark the request secure.
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert!(!is_secure_request(&headers));
    }
}
