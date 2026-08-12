//! App-neutral integration-test fixtures — moved verbatim from the ziee
//! server crate's `tests/common/` (they carried ZERO `ziee::` references, so
//! a second app can drive the same auth/sync flows against them).
//!
//! Feature-gated because they pull test-only deps the lean spawn/isolation core
//! never needs — but in TWO independent groups, not one:
//!
//! * **`sync-probe`** → [`sync_probe`] alone (`serde_json`, `tokio-stream`,
//!   `reqwest/stream`).
//! * **`auth-mocks`** → [`oauth_mock`] / [`ldap_mock`] / [`apple_mock`]
//!   (`testcontainers`, `wiremock`, `jsonwebtoken`, `rsa`, `base64`).
//! * **`fixtures`** → both, i.e. exactly what the single flag always meant.
//!
//! The split exists because the groups have wildly different prices and only one
//! of them is a DIAGNOSTIC. `SyncProbe` is what makes a cross-user sync audience
//! bug observable at all, so an app that must import a docker client to get an
//! SSE reader will tend to hand-roll or skip it — and the bug ships.
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

#[cfg(feature = "auth-mocks")]
pub mod apple_mock;
#[cfg(feature = "auth-mocks")]
pub mod ldap_mock;
#[cfg(feature = "auth-mocks")]
pub mod oauth_mock;
#[cfg(feature = "sync-probe")]
pub mod sync_probe;
