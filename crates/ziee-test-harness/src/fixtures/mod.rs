//! App-neutral integration-test fixtures — moved verbatim from the ziee
//! server crate's `tests/common/` (they carried ZERO `ziee::` references, so
//! a second app can drive the same auth/sync flows against them).
//!
//! Gated behind the `fixtures` cargo feature because they pull heavier
//! test-only deps (testcontainers, wiremock, jsonwebtoken, rsa, base64,
//! tokio-stream, serde_json) that the lean spawn/isolation core never needs.
//! An app that only wants the harness's spawn engine leaves the feature off;
//! an app that wants the mocks enables `features = ["fixtures"]`.
//!
//! - [`oauth_mock`] — a scriptable OAuth2/OIDC mock (navikt/mock-oauth2-server
//!   via testcontainers).
//! - [`ldap_mock`] — a mock LDAP server (testcontainers).
//! - [`apple_mock`] — a hand-rolled Apple Sign-In mock (wiremock + an RSA
//!   keypair + JWKS/token stubs). Its `fixture_p8_path()` resolves a committed
//!   `.p8` under THIS crate's `tests/fixtures/` via `CARGO_MANIFEST_DIR` — the
//!   one legitimate manifest-relative reference (the fixture travels WITH the
//!   fixture code, so it is self-consistent for every consuming app).
//! - [`sync_probe`] — an SSE sync-stream probe. Parses the wire frames as raw
//!   JSON (`{entity, action, id}` strings), so it names NO app-side
//!   `SyncEntity` type; it is generic over the app's `TestServer` via the
//!   [`crate::ApiUrlTarget`] seam.

pub mod apple_mock;
pub mod ldap_mock;
pub mod oauth_mock;
pub mod sync_probe;
