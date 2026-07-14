//! Apple Sign In mock server, used by the apple_test integration tests.
//!
//! Why a hand-rolled mock instead of an off-the-shelf one: Apple's
//! quirks (ES256 client_secret JWTs, response_mode=form_post, the
//! first-auth-only `user` JSON field, the string `"true"` vs boolean
//! `email_verified`) have no production-grade OSS mock. The
//! industry-standard pattern (used by gameroasters/sign-in-with-apple
//! and Directus) is exactly this: spin up a wiremock instance,
//! generate an RSA keypair, stub the JWKS + token endpoints.
//!
//! How the test uses it:
//! ```ignore
//! let apple_mock = AppleMockServer::start().await;
//! // Build an id_token with whatever Apple-quirk claims you want:
//! let id_token = apple_mock.sign_id_token(&serde_json::json!({
//!     "iss": apple_mock.base_url,
//!     "aud": "com.example.services-id",
//!     "sub": "001234.abc.5678",
//!     "iat": now,
//!     "exp": now + 3600,
//!     "email": "user@privaterelay.appleid.com",
//!     "email_verified": "true",          // Apple's string quirk
//!     "is_private_email": "true",
//!     "nonce": "<nonce-from-oauth-session>",
//! }));
//! apple_mock.queue_token_response(&id_token).await;
//!
//! // ... POST to /api/auth/oauth/apple/callback (form_post) ...
//! ```

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Test KID used in the JWKS + every signed id_token's JOSE header.
const TEST_KID: &str = "apple-mock-kid-1";

pub struct AppleMockServer {
    /// Underlying wiremock instance. Stays alive for the lifetime of
    /// AppleMockServer.
    #[allow(dead_code)]
    pub server: MockServer,
    /// Base URL — pass into the AppleProvider's config as `base_url`
    /// so its calls hit this mock instead of appleid.apple.com.
    pub base_url: String,
    /// PKCS#8 PEM encoding of the RSA-2048 private key used to sign
    /// every mock id_token. Tests can use it directly via
    /// `sign_id_token` instead of holding it.
    private_key_pem: String,
    /// Per-test queue. The next call to POST /auth/token returns
    /// this id_token. Re-set per test scenario.
    next_id_token: Arc<Mutex<Option<String>>>,
}

impl AppleMockServer {
    pub async fn start() -> Self {
        // RSA-2048 keygen takes ~1-2s on a modern CPU. Acceptable
        // for the once-per-test-process startup cost. OsRng (rand 0.8)
        // implements CryptoRngCore which the `rsa` crate requires.
        let mut rng = rsa::rand_core::OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
        let public_key = RsaPublicKey::from(&private_key);
        let private_key_pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("pkcs8 pem encode")
            .to_string();

        // JWKS modulus + exponent, base64url-no-pad per RFC 7518.
        let n_b64 = URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
        let e_b64 = URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());

        let server = MockServer::start().await;
        let base_url = server.uri();

        // /auth/keys — Apple's JWKS endpoint. Single key, the one
        // we'll sign id_tokens with.
        let jwks = serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "kid": TEST_KID,
                "use": "sig",
                "alg": "RS256",
                "n": n_b64,
                "e": e_b64,
            }]
        });
        Mock::given(method("GET"))
            .and(path("/auth/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(jwks))
            .mount(&server)
            .await;

        // /auth/token — Apple's token-exchange endpoint. Returns
        // whatever id_token the test queued. Other fields are
        // boilerplate so the AppleTokenResponse deser succeeds.
        let next_id_token: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        Mock::given(method("POST"))
            .and(path("/auth/token"))
            .respond_with(TokenResponder {
                next: next_id_token.clone(),
            })
            .mount(&server)
            .await;

        Self {
            server,
            base_url,
            private_key_pem,
            next_id_token,
        }
    }

    /// Sign a custom claims set with the mock's RSA private key.
    /// Tests use this to construct id_tokens with the specific
    /// quirks they want to exercise.
    pub fn sign_id_token(&self, claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_string());
        let key =
            EncodingKey::from_rsa_pem(self.private_key_pem.as_bytes()).expect("encoding key");
        encode(&header, claims, &key).expect("sign id_token")
    }

    /// Queue the next /auth/token response. Single-use; the responder
    /// takes the value on read so subsequent requests would 500
    /// (intentional — each test scenario should set this explicitly).
    pub async fn queue_token_response(&self, id_token: &str) {
        *self.next_id_token.lock().unwrap() = Some(id_token.to_string());
    }

    /// Path to the committed P-256 EC `.p8` fixture key, suitable
    /// for AppleConfig.private_key_path. Generated once via openssl;
    /// not a real Apple key — only valid for signing tests'
    /// client_secret JWTs locally.
    pub fn fixture_p8_path() -> PathBuf {
        // tests/fixtures/apple_test_key.p8 relative to the server crate.
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests");
        p.push("fixtures");
        p.push("apple_test_key.p8");
        p
    }
}

struct TokenResponder {
    next: Arc<Mutex<Option<String>>>,
}

impl Respond for TokenResponder {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        // wiremock's Respond is sync; std::sync::Mutex is the right
        // primitive here. Contention is the test's setup call vs.
        // this read — essentially uncontended.
        let token = self
            .next
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| "missing-test-queue-call".to_string());
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "test-mock-access-token",
            "expires_in": 3600,
            "id_token": token,
            "refresh_token": "test-mock-refresh-token",
            "token_type": "Bearer",
        }))
    }
}
