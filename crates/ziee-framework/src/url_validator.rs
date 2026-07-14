//! Outbound URL validation to prevent SSRF.
//!
//! Server-wide guard for any outbound HTTP request whose target URL is
//! derived from user input (OAuth UserInfo URLs, MCP resource_link URIs,
//! LLM provider base URLs, model download URLs, repository URLs, hub
//! catalog URLs, etc.).
//!
//! Blocks:
//! - non-allowlisted schemes (no `file://`, `ftp://`, `git://`, `gopher://`, `data:`)
//! - private/loopback/link-local/multicast IPs (RFC 1918, RFC 6890)
//!   - IPv4: `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`,
//!     `169.254.0.0/16`, `100.64.0.0/10` (CGNAT), `0.0.0.0`, `255.255.255.255`,
//!     `224.0.0.0/4`, documentation ranges
//!   - IPv6: `::1`, `fc00::/7` (ULA), `fe80::/10` (link-local), `ff00::/8` (multicast),
//!     IPv4-mapped forms of any blocked v4
//! - credentials embedded in the URL (`https://user:pass@host`) so tokens
//!   stored in DB columns can't accidentally turn into URL components that
//!   reqwest forwards to redirect targets
//!
//! Callers wire this in by:
//! 1. Calling [`validate_outbound_url`] when accepting the URL from user
//!    input (typically at request handler level) — fail-fast with a 4xx if
//!    the URL fails policy.
//! 2. Using a [`reqwest::Client`] built via [`build_validated_client`] for
//!    the actual fetch — its custom redirect policy re-validates each
//!    `Location` hop so that an attacker-controlled redirect cannot bypass
//!    the original check.

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::Policy;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum OutboundUrlError {
    #[error("URL parse failed: {0}")]
    Parse(#[from] url::ParseError),

    #[error("scheme '{scheme}' not allowed; permitted: {allowed:?}")]
    DisallowedScheme {
        scheme: String,
        allowed: &'static [&'static str],
    },

    #[error("URL must not embed credentials")]
    CredentialsInUrl,

    #[error("URL host is missing")]
    MissingHost,

    #[error("DNS resolution failed for host '{host}': {source}")]
    DnsFailed {
        host: String,
        source: std::io::Error,
    },

    #[error(
        "URL host resolves to a private/loopback/link-local address ({0}); \
         SSRF protection rejected the request"
    )]
    BlockedAddress(IpAddr),
}

/// Policy for one outbound-URL site. Most call sites use [`STRICT`]
/// (https-only, public IPs only). Specific call sites that genuinely need
/// to reach a loopback service (test harnesses, sandbox-internal calls)
/// opt into [`DEV_LOCAL`] explicitly.
///
/// Construct your own via the struct literal when you need a custom scheme
/// allow-list (e.g., the git LFS client may need both `https` and `http`
/// for local mirror testing).
#[derive(Debug, Clone, Copy)]
pub struct OutboundUrlPolicy {
    pub allow_schemes: &'static [&'static str],
    pub allow_localhost: bool,
    pub allow_private: bool,
    /// Whether link-local addresses (IPv4 169.254.0.0/16 — which includes the
    /// cloud metadata endpoint 169.254.169.254 — and IPv6 fe80::/10) are
    /// permitted. Independent of `allow_private` so a policy can allow LAN
    /// RFC1918 hosts while still blocking IMDS/link-local (the MCP case).
    pub allow_link_local: bool,
}

impl OutboundUrlPolicy {
    /// Production default: https only, no localhost, no private subnets.
    pub const STRICT: Self = Self {
        allow_schemes: &["https"],
        allow_localhost: false,
        allow_private: false,
        allow_link_local: false,
    };

    /// HTTP+HTTPS public-only. Used by clients that need plain HTTP for
    /// legacy reasons (e.g., self-hosted upstreams without TLS).
    pub const PUBLIC_HTTP_OR_HTTPS: Self = Self {
        allow_schemes: &["http", "https"],
        allow_localhost: false,
        allow_private: false,
        allow_link_local: false,
    };

    /// HTTP+HTTPS for user-configured MCP servers: localhost (built-in servers)
    /// and RFC1918 LAN hosts (self-hosted MCP servers) are allowed, but
    /// link-local / cloud-metadata (169.254.0.0/16, fe80::/10) is always
    /// blocked. Low-breakage SSRF hardening for the admin-trusted MCP surface.
    pub const MCP_USER: Self = Self {
        allow_schemes: &["http", "https"],
        allow_localhost: true,
        allow_private: true,
        allow_link_local: false,
    };

    /// HTTP+HTTPS allowing localhost (sandbox-internal, dev). Still blocks
    /// RFC 1918 / link-local / ULA.
    pub const DEV_LOCAL: Self = Self {
        allow_schemes: &["http", "https"],
        allow_localhost: true,
        allow_private: false,
        allow_link_local: false,
    };
}

/// Validate that `url_str` can be safely fetched under [`OutboundUrlPolicy`].
/// Resolves DNS once and rejects any IP that violates policy. Returns the
/// parsed [`Url`] so callers can construct a request without re-parsing.
///
/// **Race-condition caveat:** DNS rebinding is partially mitigated by the
/// fact that the calling [`build_validated_client`] resolves through the
/// system resolver again at request time. For total protection a caller
/// must either pin the resolved IP into the request (which `reqwest`
/// supports via `.resolve()`) or use a resolver wrapper. Most server-side
/// SSRF surfaces in this codebase are admin-only, so single-resolution is
/// the chosen trade-off.
pub fn validate_outbound_url(
    url_str: &str,
    policy: &OutboundUrlPolicy,
) -> Result<Url, OutboundUrlError> {
    let url = Url::parse(url_str)?;

    if !url.username().is_empty() || url.password().is_some() {
        return Err(OutboundUrlError::CredentialsInUrl);
    }

    if !policy.allow_schemes.contains(&url.scheme()) {
        return Err(OutboundUrlError::DisallowedScheme {
            scheme: url.scheme().to_string(),
            allowed: policy.allow_schemes,
        });
    }

    let host = url.host_str().ok_or(OutboundUrlError::MissingHost)?;

    // For IPv6 literals, url's host_str returns the bracketed form `[::1]`
    // which doesn't parse as IpAddr and isn't a valid DNS name. Strip the
    // brackets for the IP-literal short-circuit; DNS resolution still uses
    // the un-bracketed form too.
    let host_for_parse = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);

    // If the host is an IP literal, check it directly (no DNS).
    if let Ok(ip) = host_for_parse.parse::<IpAddr>() {
        if is_blocked_ip(&ip, policy) {
            return Err(OutboundUrlError::BlockedAddress(ip));
        }
        return Ok(url);
    }

    // Resolve hostname; reject if ANY resolved IP is blocked. This is
    // intentionally strict — a hostname that resolves to both a public
    // IP and a private IP (split-horizon DNS) gets rejected.
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = (host_for_parse, port)
        .to_socket_addrs()
        .map_err(|e| OutboundUrlError::DnsFailed {
            host: host.to_string(),
            source: e,
        })?;

    for sock in addrs {
        let ip = sock.ip();
        if is_blocked_ip(&ip, policy) {
            return Err(OutboundUrlError::BlockedAddress(ip));
        }
    }

    Ok(url)
}

fn is_blocked_ip(ip: &IpAddr, policy: &OutboundUrlPolicy) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4, policy),
        IpAddr::V6(v6) => is_blocked_v6(v6, policy),
    }
}

fn is_blocked_v4(ip: &Ipv4Addr, policy: &OutboundUrlPolicy) -> bool {
    if ip.is_loopback() {
        return !policy.allow_localhost;
    }
    if ip.is_link_local() {
        return !policy.allow_link_local;
    }
    if ip.is_private()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
    {
        return !policy.allow_private;
    }
    // CGNAT 100.64.0.0/10 (RFC 6598) — `Ipv4Addr` doesn't have a stdlib
    // helper for this yet, so we test the octets directly.
    let oct = ip.octets();
    if oct[0] == 100 && (64..=127).contains(&oct[1]) {
        return !policy.allow_private;
    }
    false
}

fn is_blocked_v6(ip: &Ipv6Addr, policy: &OutboundUrlPolicy) -> bool {
    if ip.is_loopback() {
        return !policy.allow_localhost;
    }
    if ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    let segments = ip.segments();
    // Link-local fe80::/10
    if segments[0] & 0xffc0 == 0xfe80 {
        return !policy.allow_link_local;
    }
    // Unique local fc00::/7
    if segments[0] & 0xfe00 == 0xfc00 {
        return !policy.allow_private;
    }
    // IPv4-mapped ::ffff:0:0/96 — unwrap and recurse
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_v4(&v4, policy);
    }
    false
}

/// Build a `reqwest::Client` whose redirect policy re-validates each hop
/// against the supplied policy — including targets given as a HOSTNAME
/// (resolved + checked), not just IP literals. Without this, a 302 to an
/// attacker-chosen `Location: http://evil.example/` whose host resolves to
/// `169.254.169.254` / loopback / an RFC1918 address would bypass the one-shot
/// pre-flight check on the original URL. The new untrusted page-fetch path
/// (`web_search::fetch`) is the first surface exposing this to genuinely
/// attacker-controlled redirect chains, so hostname hops MUST be checked.
///
/// Credential safety on the initial URL is enforced separately by
/// [`validate_outbound_url`], which rejects any URL embedding `username:password@`
/// before the fetch begins — this builder does not itself strip credentials.
pub fn build_validated_client(policy: OutboundUrlPolicy) -> reqwest::Result<reqwest::Client> {
    validated_client_builder(policy).build()
}

/// SSRF-guarded [`reqwest::ClientBuilder`] for callers that need to layer extra
/// options on top of the DNS-rebinding guard — e.g. a credential-carrying
/// request that wants `.no_proxy()` + `.redirect(Policy::none())` while still
/// pinning every resolved address at connect time. The returned builder has the
/// connect-time [`GuardingResolver`] + per-hop redirect policy installed;
/// callers may override `.redirect(...)` (the last call wins) and add
/// `.timeout(...)`/`.no_proxy()` before `.build()`.
pub fn validated_client_builder(policy: OutboundUrlPolicy) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .dns_resolver(Arc::new(GuardingResolver { policy }))
        .redirect(Policy::custom(move |attempt| {
            // Hard cap on hops to avoid infinite-redirect DoS, in addition
            // to the per-hop policy check.
            if attempt.previous().len() >= 10 {
                return attempt.error("too many redirects (>10)");
            }
            // Extract owned bits before any branch that moves `attempt`.
            let scheme = attempt.url().scheme().to_string();
            let host = attempt.url().host_str().map(str::to_string);
            let port = attempt.url().port_or_known_default().unwrap_or(443);
            let host = match host {
                Some(h) => h,
                None => return attempt.error("redirect target has no host"),
            };
            match redirect_blocked_reason(&scheme, &host, port, &policy) {
                Some(reason) => attempt.error(reason),
                None => attempt.follow(),
            }
        }))
}

/// Decide whether a redirect target is blocked under `policy`. Returns
/// `Some(reason)` to refuse the hop, `None` to allow it. Mirrors
/// [`validate_outbound_url`]: rejects disallowed schemes, blocked IP literals,
/// AND hostnames that RESOLVE to any blocked address (closing the
/// redirect-to-hostname SSRF hole). DNS resolution here is blocking, which is
/// acceptable on the rare redirect path; the originating call already resolved
/// the first hop in `validate_outbound_url`.
fn redirect_blocked_reason(
    scheme: &str,
    host: &str,
    port: u16,
    policy: &OutboundUrlPolicy,
) -> Option<String> {
    if !policy.allow_schemes.iter().any(|s| *s == scheme) {
        return Some(format!("redirect to disallowed scheme '{scheme}'"));
    }
    // Strip brackets from IPv6 literals (matches validate_outbound_url).
    let host_for_parse = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = host_for_parse.parse::<IpAddr>() {
        return is_blocked_ip(&ip, policy).then(|| format!("redirect to blocked address {ip}"));
    }
    // Hostname: resolve and reject if ANY resolved IP is blocked.
    match (host_for_parse, port).to_socket_addrs() {
        Ok(addrs) => addrs
            .map(|sock| sock.ip())
            .find(|ip| is_blocked_ip(ip, policy))
            .map(|ip| format!("redirect to blocked address {ip}")),
        Err(_) => Some(format!(
            "redirect target DNS resolution failed for host '{host}'"
        )),
    }
}

/// A reqwest DNS resolver that filters resolved addresses through the SSRF
/// [`OutboundUrlPolicy`] at connect time. Rejects the whole resolution if ANY
/// returned address is blocked (matching [`validate_outbound_url`]'s
/// split-horizon paranoia), so an attacker cannot rebind a hostname to a
/// blocked IP between the pre-flight check and the socket connect.
#[derive(Debug, Clone, Copy)]
struct GuardingResolver {
    policy: OutboundUrlPolicy,
}

impl Resolve for GuardingResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.policy;
        let host = name.as_str().to_string();
        Box::pin(async move {
            let host_for_resolve = host.clone();
            // Blocking system resolution, off the async runtime. Port 0 — the
            // connector overrides it with the request's real port.
            let resolved: Vec<SocketAddr> = tokio::task::spawn_blocking(move || {
                (host_for_resolve.as_str(), 0u16)
                    .to_socket_addrs()
                    .map(|it| it.collect::<Vec<_>>())
            })
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

            if let Some(bad) = resolved.iter().find(|s| is_blocked_ip(&s.ip(), &policy)) {
                return Err(format!(
                    "SSRF policy blocked '{host}' resolving to {}",
                    bad.ip()
                )
                .into());
            }
            Ok(Box::new(resolved.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Closes 01-auth F-18, 04-chat F-07, 06-llm-provider F-03,
    /// 07-llm-model F-01, 09-llm-repository F-01, 11-hub F-01 (and others
    /// that flagged `Url::parse(s).is_ok()` as the only validation).
    #[test]
    fn rejects_aws_imds_ip() {
        let err = validate_outbound_url("http://169.254.169.254/latest/", &OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS)
            .expect_err("AWS IMDS link-local IP must be blocked");
        assert!(matches!(err, OutboundUrlError::BlockedAddress(_)), "got {err:?}");
    }

    #[test]
    fn rejects_loopback_v4() {
        let err = validate_outbound_url("http://127.0.0.1/", &OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS)
            .expect_err("loopback must be blocked");
        assert!(matches!(err, OutboundUrlError::BlockedAddress(_)));
    }

    #[test]
    fn rejects_loopback_v6() {
        let err = validate_outbound_url("http://[::1]/", &OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS)
            .expect_err("::1 must be blocked");
        assert!(matches!(err, OutboundUrlError::BlockedAddress(_)), "got: {err:?}");
    }

    #[test]
    fn rejects_rfc1918_ranges() {
        for ip in ["10.0.0.1", "10.255.255.255", "172.16.0.1", "172.31.255.255", "192.168.0.1"] {
            let url = format!("http://{ip}/");
            let err = validate_outbound_url(&url, &OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS)
                .expect_err(&format!("RFC1918 IP {ip} must be rejected"));
            assert!(matches!(err, OutboundUrlError::BlockedAddress(_)), "{ip} not blocked");
        }
    }

    #[test]
    fn rejects_cgnat_range() {
        let err = validate_outbound_url("http://100.64.0.1/", &OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS)
            .expect_err("CGNAT 100.64.0.0/10 must be blocked");
        assert!(matches!(err, OutboundUrlError::BlockedAddress(_)));
    }

    #[test]
    fn rejects_ipv4_mapped_loopback() {
        // ::ffff:127.0.0.1 — the IPv4-mapped form of the v4 loopback.
        let err = validate_outbound_url(
            "http://[::ffff:127.0.0.1]/",
            &OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS,
        )
        .expect_err("IPv4-mapped loopback must be blocked");
        assert!(matches!(err, OutboundUrlError::BlockedAddress(_)));
    }

    #[test]
    fn rejects_ipv6_link_local() {
        let err = validate_outbound_url(
            "http://[fe80::1]/",
            &OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS,
        )
        .expect_err("fe80::/10 must be blocked");
        assert!(matches!(err, OutboundUrlError::BlockedAddress(_)));
    }

    #[test]
    fn rejects_ipv6_ula() {
        let err = validate_outbound_url(
            "http://[fc00::1]/",
            &OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS,
        )
        .expect_err("fc00::/7 must be blocked");
        assert!(matches!(err, OutboundUrlError::BlockedAddress(_)));
    }

    #[test]
    fn rejects_disallowed_schemes_under_strict() {
        for url in &[
            "file:///etc/passwd",
            "ftp://example.com/",
            "git://example.com/repo.git",
            "gopher://example.com/",
            "data:text/plain,leaked",
        ] {
            let err = validate_outbound_url(url, &OutboundUrlPolicy::STRICT)
                .expect_err(&format!("{url} must be rejected"));
            assert!(
                matches!(err, OutboundUrlError::DisallowedScheme { .. }),
                "{url} got {err:?}"
            );
        }
    }

    #[test]
    fn rejects_http_under_strict() {
        let err = validate_outbound_url("http://example.com/", &OutboundUrlPolicy::STRICT)
            .expect_err("STRICT must reject plain http");
        assert!(matches!(err, OutboundUrlError::DisallowedScheme { .. }));
    }

    #[test]
    fn rejects_url_credentials() {
        let err = validate_outbound_url("https://user:pass@example.com/", &OutboundUrlPolicy::STRICT)
            .expect_err("credentials in URL must be blocked");
        assert!(matches!(err, OutboundUrlError::CredentialsInUrl));
    }

    #[test]
    fn accepts_localhost_when_policy_permits() {
        let ok = validate_outbound_url("http://127.0.0.1:8080/", &OutboundUrlPolicy::DEV_LOCAL);
        assert!(ok.is_ok(), "DEV_LOCAL must accept 127.0.0.1: {ok:?}");
    }

    #[test]
    fn rejects_localhost_under_strict() {
        let err = validate_outbound_url("https://localhost:8080/", &OutboundUrlPolicy::STRICT)
            .expect_err("localhost hostname must resolve and be blocked");
        // Either DNS resolves it (localhost → 127.0.0.1) and we get BlockedAddress,
        // or it fails DNS — both are acceptable rejections under STRICT.
        assert!(matches!(
            err,
            OutboundUrlError::BlockedAddress(_) | OutboundUrlError::DnsFailed { .. }
        ));
    }

    #[test]
    fn accepts_known_public_host() {
        // example.com is reserved (RFC 2606) but resolves to a real public IP.
        // We're not actually fetching it — just verifying policy accepts.
        let res = validate_outbound_url("https://example.com/", &OutboundUrlPolicy::STRICT);
        // DNS may flake; only fail the test on policy violation, not on
        // an unrelated DNS error in CI.
        match res {
            Ok(_) => {}
            Err(OutboundUrlError::DnsFailed { .. }) => {
                eprintln!("note: DNS resolution unavailable in this environment; skipping");
            }
            Err(other) => panic!("public host wrongly rejected: {other:?}"),
        }
    }

    #[test]
    fn rejects_url_missing_host() {
        // The url crate parses "file:" as a relative URL — testing this
        // path requires a scheme + no host which is unusual. Use a
        // pathological case to exercise the missing-host arm.
        let err = validate_outbound_url("https:///", &OutboundUrlPolicy::STRICT)
            .expect_err("URL without host must be rejected");
        // url 2.x parses this as having empty host; we treat that as missing.
        assert!(matches!(
            err,
            OutboundUrlError::MissingHost
                | OutboundUrlError::Parse(_)
                | OutboundUrlError::DnsFailed { .. }
        ), "got {err:?}");
    }

    #[test]
    fn ip_literal_skips_dns() {
        // A bogus IP that wouldn't resolve via DNS should still be blocked
        // immediately via the IP-literal short-circuit.
        let err = validate_outbound_url("http://10.255.255.255/", &OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS)
            .expect_err("RFC1918 IP literal must be blocked without DNS");
        assert!(matches!(err, OutboundUrlError::BlockedAddress(_)));
    }

    // ─── redirect re-validation (redirect_blocked_reason) ────────────────
    // Closes the SSRF hole where a redirect Location given as a HOSTNAME that
    // resolves to a private/loopback/IMDS address was followed unchecked.

    #[test]
    fn redirect_blocks_disallowed_scheme() {
        // Scheme is checked first (no DNS needed).
        assert!(
            redirect_blocked_reason("ftp", "example.com", 21, &OutboundUrlPolicy::STRICT).is_some()
        );
    }

    #[test]
    fn redirect_blocks_ip_literal_imds_and_private() {
        for host in ["169.254.169.254", "127.0.0.1", "10.0.0.1", "[::1]"] {
            assert!(
                redirect_blocked_reason("https", host, 443, &OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS)
                    .is_some(),
                "redirect to {host} must be blocked"
            );
        }
    }

    #[test]
    fn redirect_blocks_hostname_resolving_to_loopback() {
        // The core fix: a HOSTNAME (not an IP literal) that resolves to a
        // blocked address must be refused. `localhost` deterministically
        // resolves to loopback, so this is hermetic.
        assert!(
            redirect_blocked_reason("https", "localhost", 443, &OutboundUrlPolicy::STRICT).is_some(),
            "redirect to a hostname resolving to loopback must be blocked"
        );
        // Under DEV_LOCAL, loopback is intentionally allowed.
        assert!(
            redirect_blocked_reason("https", "localhost", 443, &OutboundUrlPolicy::DEV_LOCAL)
                .is_none(),
            "DEV_LOCAL allows loopback"
        );
    }

    #[test]
    fn redirect_allows_public_ip_literal() {
        assert!(
            redirect_blocked_reason("https", "1.1.1.1", 443, &OutboundUrlPolicy::PUBLIC_HTTP_OR_HTTPS)
                .is_none()
        );
    }

    // ─── DNS-rebinding guard (GuardingResolver) ──────────────────────────
    // The connect-time resolver refuses a hostname that resolves to a blocked
    // address, closing the TOCTOU between the pre-flight check and the socket.

    #[tokio::test]
    async fn guarding_resolver_blocks_loopback_hostname() {
        use std::str::FromStr;
        // localhost deterministically resolves to loopback → blocked under STRICT.
        let strict = GuardingResolver {
            policy: OutboundUrlPolicy::STRICT,
        };
        assert!(
            strict.resolve(Name::from_str("localhost").unwrap()).await.is_err(),
            "STRICT must refuse a hostname resolving to loopback"
        );
        // DEV_LOCAL intentionally allows loopback.
        let dev = GuardingResolver {
            policy: OutboundUrlPolicy::DEV_LOCAL,
        };
        assert!(
            dev.resolve(Name::from_str("localhost").unwrap()).await.is_ok(),
            "DEV_LOCAL allows loopback"
        );
    }
}
