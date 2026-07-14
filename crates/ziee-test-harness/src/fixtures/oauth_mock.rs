use testcontainers::{
    ContainerAsync, GenericImage,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};

/// OAuth2/OIDC mock server using navikt/mock-oauth2-server
///
/// This provides a lightweight, scriptable mock OAuth2/OpenID Connect server
/// for testing OAuth flows without requiring a full Keycloak instance.
pub struct OAuthMockServer {
    #[allow(dead_code)]
    container: ContainerAsync<GenericImage>,
    /// Host address. Only read by `base_url()` which is pub but never called
    /// (tests use `issuer_url` directly). Kept as convenience for debugging.
    #[allow(dead_code)]
    pub host: String,
    /// Dynamically assigned port. Same status as `host`.
    #[allow(dead_code)]
    pub port: u16,
    pub issuer_url: String,
}

impl OAuthMockServer {
    /// Start a new OAuth mock server
    ///
    /// The server exposes these endpoints:
    /// - `/.well-known/openid-configuration` - OpenID Connect discovery
    /// - `/token` - Token endpoint
    /// - `/authorize` - Authorization endpoint
    /// - `/jwks` - JSON Web Key Set
    pub async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        // Use the mock-oauth2-server Docker image
        // The OAuth server doesn't log a "ready" message, so we use a simple duration wait
        let image = GenericImage::new("ghcr.io/navikt/mock-oauth2-server", "2.1.10")
            .with_exposed_port(ContainerPort::Tcp(8080))
            .with_wait_for(WaitFor::seconds(5));

        let container = image.start().await?;
        let host = "127.0.0.1".to_string();
        let port = container.get_host_port_ipv4(8080).await?;
        let issuer_url = format!("http://{}:{}/default", host, port);

        // Wait for the server to be ready with retry logic
        let well_known_url = format!("{}/.well-known/openid-configuration", issuer_url);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()?;

        let max_retries = 10;
        let mut retry_count = 0;
        let mut last_error = None;

        while retry_count < max_retries {
            match client.get(&well_known_url).send().await {
                Ok(response) if response.status().is_success() => {
                    // Server is ready
                    break;
                }
                Ok(_) | Err(_) => {
                    last_error = Some(format!("Attempt {} failed", retry_count + 1));
                    retry_count += 1;
                    if retry_count < max_retries {
                        // Exponential backoff: 100ms, 200ms, 400ms, 800ms, etc.
                        let delay =
                            std::time::Duration::from_millis(100 * 2_u64.pow(retry_count.min(5)));
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        if retry_count >= max_retries {
            return Err(format!(
                "OAuth mock server failed to become ready after {} attempts. Last error: {:?}",
                max_retries, last_error
            )
            .into());
        }

        Ok(Self {
            container,
            host,
            port,
            issuer_url,
        })
    }

    /// Get the base URL of the mock server
    #[allow(dead_code)]
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    /// Get the well-known configuration URL
    pub fn well_known_url(&self) -> String {
        format!("{}/.well-known/openid-configuration", self.issuer_url)
    }

    /// Get the token endpoint URL
    pub fn token_url(&self) -> String {
        format!("{}/token", self.issuer_url)
    }

    /// Get the authorization endpoint URL
    pub fn authorize_url(&self) -> String {
        format!("{}/authorize", self.issuer_url)
    }

    /// Get the JWKS endpoint URL
    #[allow(dead_code)]
    pub fn jwks_url(&self) -> String {
        format!("{}/.well-known/jwks.json", self.issuer_url)
    }

    /// Create a mock OAuth provider configuration for testing
    /// Returns JSON that can be inserted into the database
    pub fn create_test_provider_config(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "client_id": client_id,
            "client_secret": client_secret,
            "auth_url": self.authorize_url(),
            "token_url": self.token_url(),
            "redirect_url": "http://localhost:3000/api/auth/oauth/test-oauth/callback",
            "scopes": ["openid", "profile", "email"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_oauth_mock_server_starts() {
        let server = OAuthMockServer::start()
            .await
            .expect("Failed to start OAuth mock server");

        // Verify the server is responding
        let client = reqwest::Client::new();
        let response = client
            .get(server.well_known_url())
            .send()
            .await
            .expect("Failed to connect to OAuth mock server");

        assert!(response.status().is_success());

        // Verify the OpenID configuration is valid
        let config: serde_json::Value = response.json().await.expect("Invalid JSON response");
        assert_eq!(config["issuer"], server.issuer_url);
        assert!(config["authorization_endpoint"].is_string());
        assert!(config["token_endpoint"].is_string());
        assert!(config["jwks_uri"].is_string());
    }
}
