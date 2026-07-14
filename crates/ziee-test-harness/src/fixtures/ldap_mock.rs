use testcontainers::{
    ContainerAsync, GenericImage,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};

/// LDAP mock server using rroemhild/test-openldap
///
/// This provides an OpenLDAP server pre-configured with test data
/// from the Futurama Wiki (domain: planetexpress.com).
///
/// Default users:
/// - Professor Farnsworth (admin)
/// - Philip J. Fry
/// - Turanga Leela
/// - Bender
/// - And many more...
pub struct LdapMockServer {
    #[allow(dead_code)]
    container: ContainerAsync<GenericImage>,
    pub host: String,
    pub port: u16,
    pub bind_dn: String,
    pub bind_password: String,
    pub base_dn: String,
}

impl LdapMockServer {
    /// Start a new LDAP mock server
    ///
    /// The server is pre-configured with:
    /// - Domain: planetexpress.com
    /// - Base DN: dc=planetexpress,dc=com
    /// - Admin DN: cn=admin,dc=planetexpress,dc=com
    /// - Admin Password: GoodNewsEveryone
    pub async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        // Use the test-openldap Docker image
        let image = GenericImage::new("rroemhild/test-openldap", "latest")
            .with_exposed_port(ContainerPort::Tcp(10389))
            .with_wait_for(WaitFor::message_on_stderr("slapd starting"));

        let container = image.start().await?;
        let host = "127.0.0.1".to_string();
        let port = container.get_host_port_ipv4(10389).await?;

        Ok(Self {
            container,
            host,
            port,
            bind_dn: "cn=admin,dc=planetexpress,dc=com".to_string(),
            bind_password: "GoodNewsEveryone".to_string(),
            base_dn: "dc=planetexpress,dc=com".to_string(),
        })
    }

    /// Get the LDAP URL
    pub fn ldap_url(&self) -> String {
        format!("ldap://{}:{}", self.host, self.port)
    }

    /// Create a mock LDAP provider configuration for testing
    /// Returns JSON that can be inserted into the database
    pub fn create_test_provider_config(&self) -> serde_json::Value {
        serde_json::json!({
            "url": self.ldap_url(),
            "base_dn": "ou=people,dc=planetexpress,dc=com",
            "search_filter": "(uid={username})",
            "admin_bind_dn": self.bind_dn.clone(),
            "admin_password": self.bind_password.clone(),
            "attribute_mapping": {
                "username": "uid",
                "email": "mail",
                "display_name": "cn"
            }
        })
    }

    /// Get a test user credentials
    /// Returns (username, password) for testing
    pub fn get_test_user() -> (&'static str, &'static str) {
        // Fry's credentials from the test LDAP server
        ("fry", "fry")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldap3::{LdapConnAsync, Scope, SearchEntry};

    #[tokio::test]
    async fn test_ldap_mock_server_starts() {
        let server = LdapMockServer::start()
            .await
            .expect("Failed to start LDAP mock server");

        // Verify we can connect and bind
        let (conn, mut ldap) = LdapConnAsync::new(&server.ldap_url())
            .await
            .expect("Failed to connect to LDAP");
        ldap3::drive!(conn);

        let bind_result = ldap
            .simple_bind(&server.bind_dn, &server.bind_password)
            .await
            .expect("Failed to bind to LDAP");

        assert!(bind_result.success().is_ok());
        ldap.unbind().await.ok();
    }

    #[tokio::test]
    async fn test_ldap_mock_server_has_test_users() {
        let server = LdapMockServer::start()
            .await
            .expect("Failed to start LDAP mock server");

        let (conn, mut ldap) = LdapConnAsync::new(&server.ldap_url())
            .await
            .expect("Failed to connect");
        ldap3::drive!(conn);

        ldap.simple_bind(&server.bind_dn, &server.bind_password)
            .await
            .expect("Failed to bind")
            .success()
            .expect("Bind failed");

        // Search for Fry
        let (username, _) = LdapMockServer::get_test_user();
        let filter = format!("(uid={})", username);
        let search_base = format!("ou=people,{}", server.base_dn);

        let (results, _res) = ldap
            .search(
                &search_base,
                Scope::Subtree,
                &filter,
                vec!["uid", "cn", "mail"],
            )
            .await
            .expect("Search failed")
            .success()
            .expect("Search result failed");

        assert!(!results.is_empty(), "Should find at least one user");

        let entry = SearchEntry::construct(results[0].clone());
        assert_eq!(entry.attrs.get("uid").unwrap()[0], username);
        ldap.unbind().await.ok();
    }
}
