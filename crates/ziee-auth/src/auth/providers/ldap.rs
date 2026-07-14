// Auth provider infrastructure - part of future auth system

use async_trait::async_trait;
use ldap3::{Ldap, LdapConnAsync, Scope, SearchEntry};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use super::{AuthError, AuthProvider, AuthProviderTrait, AuthResult, UserAttributes};

/// Escape a string for safe inclusion in an LDAP search filter, per
/// RFC 4515 § 3 (Search Filter String Representation).
///
/// LDAP filter parsers treat `*`, `(`, `)`, `\`, and NUL as syntax.
/// Without escaping, a username like `admin)(uid=*` slips past
/// `(uid={username})` and turns the filter into `(uid=admin)(uid=*)`
/// — every user in the directory matches. Closes 01-auth F-04 (High).
pub fn escape_ldap_filter(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'\\' => out.push_str("\\5c"),
            b'*' => out.push_str("\\2a"),
            b'(' => out.push_str("\\28"),
            b')' => out.push_str("\\29"),
            0 => out.push_str("\\00"),
            // ASCII printable (per RFC 4515 — only the 5 above need escape).
            // Multibyte UTF-8 is passed through; LDAP filter syntax
            // requires the directory's encoding to handle it.
            _ => out.push(b as char),
        }
    }
    out
}

/// Escape a string for safe inclusion in an LDAP DN attribute value,
/// per RFC 4514 § 2.4 (Converting an AttributeValue from ASN.1 to a
/// String).
///
/// DN parsers treat `,`, `+`, `"`, `\`, `<`, `>`, `;` as RDN separators
/// or special syntax; leading space / `#` and trailing space also need
/// escaping. Without this, a username like `admin,ou=admins,dc=corp`
/// in `uid={username},ou=users,dc=corp` becomes a different RDN entirely.
/// Closes 01-auth F-04 (High).
pub fn escape_ldap_dn(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        let is_first = i == 0;
        let is_last = i == chars.len() - 1;
        match c {
            ',' | '+' | '"' | '\\' | '<' | '>' | ';' | '=' => {
                out.push('\\');
                out.push(*c);
            }
            ' ' if is_first || is_last => out.push_str("\\20"),
            '#' if is_first => out.push_str("\\23"),
            '\0' => out.push_str("\\00"),
            _ => out.push(*c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_escape_blocks_classic_bypass() {
        // The classic LDAP injection that bypasses a username filter.
        let attacker = "admin)(uid=*";
        let safe = escape_ldap_filter(attacker);
        assert!(!safe.contains('('), "unescaped '(' would close the filter");
        assert!(!safe.contains(')'), "unescaped ')' would inject syntax");
        assert!(!safe.contains('*'), "unescaped '*' would wildcard-match");
        assert_eq!(safe, "admin\\29\\28uid=\\2a");
    }

    #[test]
    fn filter_escape_blocks_null_byte_truncation() {
        let attacker = "admin\0and_extra";
        let safe = escape_ldap_filter(attacker);
        assert_eq!(safe, "admin\\00and_extra");
    }

    #[test]
    fn filter_escape_passes_through_safe_input() {
        assert_eq!(escape_ldap_filter("alice"), "alice");
        assert_eq!(escape_ldap_filter("user.name+tag"), "user.name+tag");
    }

    #[test]
    fn dn_escape_blocks_rdn_injection() {
        // Attacker tries to break out of uid={username},ou=users,dc=corp
        // and land in ou=admins,dc=corp instead. The fix prefixes every
        // `,` and `=` with `\` so the LDAP DN parser keeps them as data,
        // not as RDN syntax.
        let attacker = "admin,ou=admins,dc=corp";
        let safe = escape_ldap_dn(attacker);
        assert_eq!(safe, "admin\\,ou\\=admins\\,dc\\=corp");
        // Verify no unescaped comma remains (every `,` must be preceded by `\`).
        let bytes = safe.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            if *b == b',' {
                assert!(
                    i > 0 && bytes[i - 1] == b'\\',
                    "unescaped comma at index {i} in {safe:?}"
                );
            }
        }
    }

    #[test]
    fn dn_escape_handles_leading_space_and_hash() {
        assert_eq!(escape_ldap_dn(" admin"), "\\20admin");
        assert_eq!(escape_ldap_dn("#admin"), "\\23admin");
        assert_eq!(escape_ldap_dn("admin "), "admin\\20");
    }

    #[test]
    fn dn_escape_passes_through_safe_input() {
        assert_eq!(escape_ldap_dn("alice"), "alice");
    }
}

/// LDAP provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapConfig {
    /// LDAP server URL (ldap://hostname:port or ldaps://hostname:port)
    pub url: String,
    /// Base DN for user searches
    pub base_dn: String,
    /// Search filter template (use {username} as placeholder)
    pub search_filter: String,
    /// Bind DN template (use {username} as placeholder), or search then bind
    pub bind_dn_template: Option<String>,
    /// Admin bind DN for search-then-bind flow
    pub admin_bind_dn: Option<String>,
    /// Admin password for search-then-bind flow
    pub admin_password: Option<String>,
    /// Attribute mapping
    pub attribute_mapping: LdapAttributeMapping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapAttributeMapping {
    pub username: String,             // Default: "sAMAccountName" or "uid"
    pub email: String,                // Default: "mail"
    pub display_name: Option<String>, // Default: "displayName"
    pub first_name: Option<String>,   // Default: "givenName"
    pub last_name: Option<String>,    // Default: "sn"
    pub groups: Option<String>,       // Default: "memberOf"
}

impl Default for LdapAttributeMapping {
    fn default() -> Self {
        Self {
            username: "sAMAccountName".to_string(),
            email: "mail".to_string(),
            display_name: Some("displayName".to_string()),
            first_name: Some("givenName".to_string()),
            last_name: Some("sn".to_string()),
            groups: Some("memberOf".to_string()),
        }
    }
}

pub struct LdapAuthProvider {
    name: String,
    config: LdapConfig,
    raw_config: serde_json::Value,
}

impl LdapAuthProvider {
    pub fn new(provider: &AuthProvider, _pool: PgPool) -> Result<Self, AuthError> {
        let config: LdapConfig = serde_json::from_value(provider.config.clone()).map_err(|e| {
            AuthError::ConfigurationError(format!("Invalid LDAP configuration: {}", e))
        })?;

        Ok(Self {
            name: provider.name.clone(),
            config,
            raw_config: provider.config.clone(),
        })
    }

    /// Establish an LDAP connection, retrying ONLY the connection step on
    /// transient network failures with exponential backoff. Bind/search are
    /// deliberately NOT retried here — those failures are auth/config errors
    /// (retrying a bind could lock the account), not transient.
    async fn connect_with_retry(&self) -> Result<(LdapConnAsync, Ldap), AuthError> {
        const MAX_ATTEMPTS: u32 = 3;
        let mut attempt = 0;
        loop {
            attempt += 1;
            match LdapConnAsync::new(&self.config.url).await {
                Ok(pair) => return Ok(pair),
                Err(e) if attempt < MAX_ATTEMPTS => {
                    let delay =
                        std::time::Duration::from_millis(300 * 2u64.pow(attempt - 1));
                    tracing::warn!(
                        "LDAP connect to {} failed: {e}; retrying in {delay:?} (attempt {attempt}/{MAX_ATTEMPTS})",
                        self.config.url
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    return Err(AuthError::ConnectionFailed(format!(
                        "Failed to connect to LDAP after {MAX_ATTEMPTS} attempts: {e}"
                    )));
                }
            }
        }
    }

    async fn search_user(&self, username: &str) -> Result<Option<SearchEntry>, AuthError> {
        // Connect to LDAP (with transient-failure retry)
        let (conn, mut ldap) = self.connect_with_retry().await?;

        ldap3::drive!(conn);

        // Bind as admin if configured
        if let (Some(admin_dn), Some(admin_pw)) =
            (&self.config.admin_bind_dn, &self.config.admin_password)
        {
            ldap.simple_bind(admin_dn, admin_pw)
                .await
                .map_err(|e| AuthError::ConfigurationError(format!("Admin bind failed: {}", e)))?;
        }

        // Search for user. RFC 4515 escape on the username so an
        // attacker can't break out of the filter — closes 01-auth F-04.
        let filter = self
            .config
            .search_filter
            .replace("{username}", &escape_ldap_filter(username));
        let (rs, _res) = ldap
            .search(&self.config.base_dn, Scope::Subtree, &filter, vec!["*"])
            .await
            .map_err(|e| AuthError::InternalError(format!("LDAP search failed: {}", e)))?
            .success()
            .map_err(|e| AuthError::InternalError(format!("LDAP search error: {}", e)))?;

        let _ = ldap.unbind().await;

        // Return first result
        Ok(rs.into_iter().next().map(SearchEntry::construct))
    }
}

#[async_trait]
impl AuthProviderTrait for LdapAuthProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn provider_type(&self) -> &str {
        "ldap"
    }

    async fn authenticate(&self, username: &str, password: &str) -> Result<AuthResult, AuthError> {
        // Determine bind DN
        let bind_dn = if let Some(template) = &self.config.bind_dn_template {
            // Direct bind with template. RFC 4514 escape on the username
            // so an attacker can't break out of the RDN — closes
            // 01-auth F-04.
            template.replace("{username}", &escape_ldap_dn(username))
        } else {
            // Search-then-bind flow
            let user_entry = self.search_user(username).await?.ok_or_else(|| {
                AuthError::UserNotFound(format!("User '{}' not found in LDAP", username))
            })?;

            user_entry.dn
        };

        // Connect and bind as user (connection retried on transient failure)
        let (conn, mut ldap) = self.connect_with_retry().await?;

        ldap3::drive!(conn);

        // Attempt bind with user credentials
        ldap.simple_bind(&bind_dn, password)
            .await
            .map_err(|e| AuthError::InvalidCredentials(format!("LDAP bind failed: {}", e)))?;

        // If bind succeeded, search for user attributes
        let user_entry = self.search_user(username).await?.ok_or_else(|| {
            AuthError::UserNotFound(format!(
                "User '{}' not found after authentication",
                username
            ))
        })?;

        let _ = ldap.unbind().await;

        // Extract attributes
        let attrs = &user_entry.attrs;
        let get_attr = |name: &str| -> Option<String> {
            attrs
                .get(name)
                .and_then(|v| v.first())
                .map(|s| s.to_string())
        };

        let username_attr = get_attr(&self.config.attribute_mapping.username)
            .unwrap_or_else(|| username.to_string());
        let email = get_attr(&self.config.attribute_mapping.email).unwrap_or_default();

        let display_name = self
            .config
            .attribute_mapping
            .display_name
            .as_ref()
            .and_then(|attr| get_attr(attr));
        let first_name = self
            .config
            .attribute_mapping
            .first_name
            .as_ref()
            .and_then(|attr| get_attr(attr));
        let last_name = self
            .config
            .attribute_mapping
            .last_name
            .as_ref()
            .and_then(|attr| get_attr(attr));

        // Extract groups
        let groups = self
            .config
            .attribute_mapping
            .groups
            .as_ref()
            .and_then(|attr| attrs.get(attr))
            .map(|v| v.iter().map(String::from).collect())
            .unwrap_or_default();

        Ok(AuthResult {
            external_id: user_entry.dn.clone(),
            external_username: Some(username_attr.clone()),
            external_email: Some(email.clone()),
            metadata: serde_json::json!({
                "provider": "ldap",
                "dn": user_entry.dn,
                "auth_method": "bind"
            }),
            attributes: UserAttributes {
                username: username_attr,
                email,
                display_name,
                first_name,
                last_name,
                groups,
            },
        })
    }

    async fn test_connection(&self) -> Result<String, AuthError> {
        // Test connection by attempting to connect and bind (if admin credentials provided)
        let (conn, mut ldap) = LdapConnAsync::new(&self.config.url).await.map_err(|e| {
            AuthError::ConnectionFailed(format!("Failed to connect to LDAP: {}", e))
        })?;

        ldap3::drive!(conn);

        let mut msg = "LDAP server reachable".to_string();
        if let (Some(admin_dn), Some(admin_pw)) =
            (&self.config.admin_bind_dn, &self.config.admin_password)
        {
            ldap.simple_bind(admin_dn, admin_pw)
                .await
                .map_err(|e| AuthError::ConfigurationError(format!("Test bind failed: {}", e)))?;
            msg.push_str("; admin bind succeeded");
        }

        let _ = ldap.unbind().await;
        Ok(msg)
    }

    fn get_config(&self) -> &serde_json::Value {
        &self.raw_config
    }
}
