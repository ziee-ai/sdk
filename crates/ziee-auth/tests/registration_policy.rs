//! The process-wide registration policy, proved in its OWN PROCESS.
//!
//! `install_registration_policy` writes a `OnceLock`, so "the first install
//! wins, and until one is made registration is open" is order-dependent and
//! cannot be asserted from a test that shares a binary with siblings that
//! install. This file owns the lock for one process and drives the whole
//! sequence in order.
//!
//! The pure decision (`resolve_registration`) is unit-tested in
//! `auth::registration`; this is the GLOBAL half.

use std::sync::Arc;

use ziee_auth::auth::registration::{
    RegistrationChannel, RegistrationPolicy, RegistrationRefused, check_registration_allowed,
    install_registration_policy, registration_policy,
};

struct ClosedPolicy;

#[async_trait::async_trait]
impl RegistrationPolicy for ClosedPolicy {
    async fn allow_registration(
        &self,
        _channel: RegistrationChannel,
    ) -> Result<(), RegistrationRefused> {
        Err(RegistrationRefused::closed(
            "REGISTRATION_CLOSED",
            "This site is not accepting new accounts right now.",
        ))
    }
}

struct SecondPolicy;

#[async_trait::async_trait]
impl RegistrationPolicy for SecondPolicy {
    async fn allow_registration(
        &self,
        _channel: RegistrationChannel,
    ) -> Result<(), RegistrationRefused> {
        Err(RegistrationRefused::closed("SECOND_POLICY", "second"))
    }
}

#[tokio::test]
async fn the_whole_declaration_sequence_in_order() {
    // ── 1. POSITIVE CONTROL, first: before anything is installed, every
    //       channel is allowed. Asserted BEFORE the refusal legs so a seam
    //       that refused unconditionally could not pass this file.
    assert!(registration_policy().is_none(), "nothing installed yet");
    for channel in [
        RegistrationChannel::LocalPassword,
        RegistrationChannel::ExternalFirstLogin,
        RegistrationChannel::OauthFirstLogin,
    ] {
        assert!(
            check_registration_allowed(channel).await.is_ok(),
            "with no policy installed, {channel} must be allowed — an app that never \
             heard of this seam keeps the historical behaviour"
        );
    }

    // ── 2. Install a closing policy: every creation channel is now refused
    //       with a 403 and a code the client can branch on.
    install_registration_policy(Arc::new(ClosedPolicy));
    assert!(registration_policy().is_some(), "the install took effect");

    for channel in [
        RegistrationChannel::LocalPassword,
        RegistrationChannel::ExternalFirstLogin,
        RegistrationChannel::OauthFirstLogin,
    ] {
        let (status, err) = check_registration_allowed(channel)
            .await
            .expect_err("a closing policy must refuse every creation channel");
        assert_eq!(
            status,
            http::StatusCode::FORBIDDEN,
            "{channel} must be refused with 403, not a 500 (which reads as a fault) \
             and not a 200 (which reads as success)"
        );
        let body = serde_json::to_value(&err).expect("AppError serializes");
        assert_eq!(body["error_code"], "REGISTRATION_CLOSED");
    }

    // ── 3. FIRST INSTALL WINS. A second install is ignored, so "which policy
    //       is live" stays answerable.
    install_registration_policy(Arc::new(SecondPolicy));
    let (_, err) = check_registration_allowed(RegistrationChannel::LocalPassword)
        .await
        .expect_err("still refusing");
    let body = serde_json::to_value(&err).expect("AppError serializes");
    assert_eq!(
        body["error_code"], "REGISTRATION_CLOSED",
        "the FIRST installed policy must stand; a silent replacement would make the \
         live policy unknowable"
    );
}
