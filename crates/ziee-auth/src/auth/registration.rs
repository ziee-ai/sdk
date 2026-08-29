//! The **registration policy** seam — how a deployment closes account creation.
//!
//! `ziee-auth` owns every path that creates an account, and until this seam
//! existed an app had no way to stop one. That is not a theoretical gap: an app
//! can store, audit and publicly serve a "registration is closed" setting and
//! still create accounts on every request, because the handlers that create
//! them live in this crate and never asked anybody. A control an operator can
//! move that changes nothing is worse than no control at all — it reports a
//! posture the deployment does not have.
//!
//! ## Why this cannot be a middleware in front of the routes
//!
//! Two of the three creation paths share a URL with SIGN-IN:
//!
//! | path | URL | also used by |
//! |---|---|---|
//! | local sign-up | `POST /auth/register` | nothing else |
//! | external first login (LDAP) | `POST /auth/login` | every external sign-in |
//! | OAuth first login | `GET`/`POST /auth/oauth/{provider}/callback` | every OAuth sign-in |
//!
//! Whether an OAuth callback is a returning user, a link-to-an-existing-account
//! confirmation, or a brand-new account is only knowable AFTER the one-shot code
//! exchange with the identity provider — i.e. inside
//! [`oauth_complete_inner`](crate::auth::http). A route-layer guard therefore has
//! to choose between missing OAuth sign-UP and breaking OAuth sign-IN. Closing
//! sign-up must never close sign-in, so the decision belongs where the branch is
//! taken, and that is here.
//!
//! ## The default is OPEN, deliberately
//!
//! Unlike [`AuthSyncWiring`](super::context::AuthSyncWiring) — where doing
//! nothing bought a SILENT drop and is therefore refused at boot — an app that
//! installs no policy gets this crate's historical behaviour: registration is
//! open. That absence is *visible*: accounts get created, which is exactly what
//! an operator who never touched the setting expects. Refusing to boot without a
//! declaration would break every existing consumer to prevent a state that
//! announces itself.
//!
//! ## Installing one
//!
//! ```ignore
//! struct SiteRegistrationPolicy { pool: PgPool }
//!
//! #[async_trait::async_trait]
//! impl RegistrationPolicy for SiteRegistrationPolicy {
//!     async fn allow_registration(
//!         &self,
//!         _channel: RegistrationChannel,
//!     ) -> Result<(), RegistrationRefused> {
//!         if site_settings(&self.pool).await.signups_open {
//!             Ok(())
//!         } else {
//!             Err(RegistrationRefused::closed(
//!                 "REGISTRATION_CLOSED",
//!                 "This site is not accepting new accounts right now.",
//!             ))
//!         }
//!     }
//! }
//!
//! // once at boot, before `initialize_modules`
//! ziee_auth::install_registration_policy(Arc::new(SiteRegistrationPolicy { pool }));
//! ```

use std::sync::{Arc, OnceLock};

use http::StatusCode;
use ziee_core::AppError;

/// Which account-creation path is asking.
///
/// A policy may treat them differently — a deployment can reasonably keep
/// LDAP auto-provisioning open for staff while closing public sign-up — so the
/// channel is a parameter rather than a single boolean question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationChannel {
    /// `POST /auth/register` — self-service local sign-up with a password.
    LocalPassword,
    /// `POST /auth/login` with `provider != "local"`, where the external
    /// provider authenticated somebody who has no account yet, so the handler
    /// would auto-provision one (the LDAP first-login path).
    ExternalFirstLogin,
    /// `GET`/`POST /auth/oauth/{provider}/callback` where the callback resolved
    /// to no existing link and no linkable local account, so the handler would
    /// auto-provision one.
    ///
    /// Reached only on the branch that CREATES: a returning user (existing
    /// auth link) and a first-broker-link confirmation both return before this.
    OauthFirstLogin,
}

impl RegistrationChannel {
    /// A stable, loggable name for the channel.
    pub fn as_str(self) -> &'static str {
        match self {
            RegistrationChannel::LocalPassword => "local_password",
            RegistrationChannel::ExternalFirstLogin => "external_first_login",
            RegistrationChannel::OauthFirstLogin => "oauth_first_login",
        }
    }
}

impl std::fmt::Display for RegistrationChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A policy's refusal, carried all the way to the client.
///
/// It owns its `status` and `code` so the refusal is something a UI can ACT on
/// — "registration is closed" and "you need an invite" are different states a
/// sign-up form renders differently. A refusal that arrives as an opaque 500 is
/// indistinguishable from the server being broken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationRefused {
    /// The HTTP status the client sees. Must be a client error (4xx); anything
    /// else is coerced by [`RegistrationRefused::into_api_error`], because a
    /// refusal that reads as a server fault trains clients to retry it.
    pub status: StatusCode,
    /// The machine-readable `error_code` (e.g. `REGISTRATION_CLOSED`).
    pub code: String,
    /// The reader-facing sentence.
    pub message: String,
}

impl RegistrationRefused {
    /// A refusal with an explicit status.
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
        }
    }

    /// The usual shape: `403 Forbidden` with a code the sign-up form can branch
    /// on. 403 rather than 404/401 because the endpoint EXISTS and the caller is
    /// not being asked to authenticate — the deployment is declining.
    pub fn closed(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }

    /// The handler-shaped error. A non-4xx status is coerced to 403 rather than
    /// trusted: a policy that returns 500 (or 200) would turn a deliberate
    /// refusal into something a client reads as a fault or a success.
    pub fn into_api_error(self) -> (StatusCode, AppError) {
        let status = if self.status.is_client_error() {
            self.status
        } else {
            tracing::warn!(
                status = %self.status,
                code = %self.code,
                "ziee-auth: a RegistrationPolicy returned a non-4xx refusal status; \
                 coercing to 403 so the client does not read a deliberate refusal as a fault"
            );
            StatusCode::FORBIDDEN
        };
        (status, AppError::new(status, self.code, self.message))
    }
}

/// Decide whether an account may be created right now.
///
/// Called on every creation attempt (registration is rare; this is not a hot
/// path), so an implementation that reads a settings row per call is correct and
/// a cached one risks an operator closing registration and watching accounts
/// keep appearing until the cache expires.
#[async_trait::async_trait]
pub trait RegistrationPolicy: Send + Sync {
    /// `Ok(())` to let the account be created; `Err` to refuse with a status and
    /// code the client can act on.
    ///
    /// An implementation that cannot reach its own state (a DB read fails) must
    /// decide deliberately which way to fail and say so at the call site —
    /// this trait does not choose for it.
    async fn allow_registration(
        &self,
        channel: RegistrationChannel,
    ) -> Result<(), RegistrationRefused>;
}

/// A policy that allows everything — the shape of "no policy installed", made
/// nameable so an app can install it explicitly to say *"we looked at this and
/// we are open"*.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenRegistrationPolicy;

#[async_trait::async_trait]
impl RegistrationPolicy for OpenRegistrationPolicy {
    async fn allow_registration(
        &self,
        _channel: RegistrationChannel,
    ) -> Result<(), RegistrationRefused> {
        Ok(())
    }
}

/// The process-wide installed policy. `OnceLock` (not a swappable slot) for the
/// same reason the sync wiring is one: a policy swapped mid-run would split one
/// deployment's registration decisions across two implementations.
static POLICY: OnceLock<Arc<dyn RegistrationPolicy>> = OnceLock::new();

/// Install this process's registration policy. Call ONCE at boot, before
/// `initialize_modules`.
///
/// First install wins; a second is ignored and reported at `warn` (silently
/// replacing the first would make "which policy is live" unanswerable).
pub fn install_registration_policy(policy: Arc<dyn RegistrationPolicy>) {
    if POLICY.set(policy).is_err() {
        tracing::warn!(
            "ziee-auth: a registration policy was already installed; the FIRST one stands \
             and this one is ignored"
        );
    }
}

/// The installed policy, if any. `None` means registration is open.
pub fn registration_policy() -> Option<&'static Arc<dyn RegistrationPolicy>> {
    POLICY.get()
}

/// The decision, taken against an EXPLICIT policy rather than the process-wide
/// one — so the whole rule, including "no policy means open", is testable
/// without touching the `OnceLock`.
pub async fn resolve_registration(
    policy: Option<&Arc<dyn RegistrationPolicy>>,
    channel: RegistrationChannel,
) -> Result<(), RegistrationRefused> {
    match policy {
        Some(p) => p.allow_registration(channel).await,
        None => Ok(()),
    }
}

/// The call every account-creating handler makes immediately before it writes.
///
/// Placed so that a refusal happens BEFORE any row is created and AFTER the
/// branch that decides this really is a creation — so closing sign-up never
/// closes sign-in.
pub async fn check_registration_allowed(
    channel: RegistrationChannel,
) -> Result<(), (StatusCode, AppError)> {
    match resolve_registration(registration_policy(), channel).await {
        Ok(()) => Ok(()),
        Err(refusal) => {
            tracing::info!(
                channel = %channel,
                code = %refusal.code,
                "ziee-auth: account creation refused by the installed registration policy"
            );
            Err(refusal.into_api_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records the channels it was asked about, so "the policy was consulted for
    /// THIS path" is a behavioural assertion rather than a compile-time one.
    #[derive(Default)]
    struct RecordingPolicy {
        seen: Mutex<Vec<RegistrationChannel>>,
        refuse: Option<RegistrationRefused>,
    }

    #[async_trait::async_trait]
    impl RegistrationPolicy for RecordingPolicy {
        async fn allow_registration(
            &self,
            channel: RegistrationChannel,
        ) -> Result<(), RegistrationRefused> {
            self.seen.lock().unwrap().push(channel);
            match &self.refuse {
                Some(r) => Err(r.clone()),
                None => Ok(()),
            }
        }
    }

    /// POSITIVE CONTROL, asserted first: with a permissive policy every channel
    /// is allowed. Without this, the refusal tests below would pass just as well
    /// against a seam that refuses unconditionally.
    #[tokio::test]
    async fn a_permissive_policy_allows_every_channel() {
        let policy: Arc<dyn RegistrationPolicy> = Arc::new(RecordingPolicy::default());
        for channel in [
            RegistrationChannel::LocalPassword,
            RegistrationChannel::ExternalFirstLogin,
            RegistrationChannel::OauthFirstLogin,
        ] {
            assert!(
                resolve_registration(Some(&policy), channel).await.is_ok(),
                "a permissive policy must allow {channel}"
            );
        }
    }

    /// The channel reaches the policy unchanged — a policy that wants to keep
    /// LDAP open while closing public sign-up can only do that if it is told
    /// which path is asking.
    #[tokio::test]
    async fn the_channel_reaches_the_policy() {
        let recorder = Arc::new(RecordingPolicy::default());
        let policy: Arc<dyn RegistrationPolicy> = recorder.clone();
        resolve_registration(Some(&policy), RegistrationChannel::OauthFirstLogin)
            .await
            .expect("permissive");
        resolve_registration(Some(&policy), RegistrationChannel::LocalPassword)
            .await
            .expect("permissive");
        assert_eq!(
            *recorder.seen.lock().unwrap(),
            vec![
                RegistrationChannel::OauthFirstLogin,
                RegistrationChannel::LocalPassword
            ]
        );
    }

    #[tokio::test]
    async fn a_refusing_policy_refuses_with_its_own_code() {
        let policy: Arc<dyn RegistrationPolicy> = Arc::new(RecordingPolicy {
            seen: Mutex::default(),
            refuse: Some(RegistrationRefused::closed(
                "REGISTRATION_CLOSED",
                "not accepting new accounts",
            )),
        });
        let err = resolve_registration(Some(&policy), RegistrationChannel::LocalPassword)
            .await
            .expect_err("a refusing policy must refuse");
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        assert_eq!(err.code, "REGISTRATION_CLOSED");
    }

    /// NO POLICY MEANS OPEN. This is the backwards-compatibility contract: an
    /// app that never heard of this seam behaves exactly as it did before.
    #[tokio::test]
    async fn no_policy_means_open() {
        assert!(
            resolve_registration(None, RegistrationChannel::LocalPassword)
                .await
                .is_ok(),
            "an app that installs no policy must keep the historical open behaviour"
        );
    }

    /// A refusal becomes a 4xx the client can branch on — never a 500, which
    /// reads as "the server is broken, retry" rather than "we are closed".
    #[test]
    fn a_refusal_becomes_a_client_error_with_its_code() {
        let (status, err) = RegistrationRefused::closed("REGISTRATION_CLOSED", "closed")
            .into_api_error();
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(status.is_client_error());
        let body = serde_json::to_value(&err).expect("AppError serializes");
        assert_eq!(body["error_code"], "REGISTRATION_CLOSED");
    }

    /// A policy that hands back a 5xx (or a success status) is coerced, not
    /// trusted — otherwise one buggy policy turns every refusal into a fault.
    #[test]
    fn a_non_client_error_status_is_coerced_to_403() {
        for bad in [StatusCode::INTERNAL_SERVER_ERROR, StatusCode::OK] {
            let (status, _) =
                RegistrationRefused::new(bad, "REGISTRATION_CLOSED", "closed").into_api_error();
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "a {bad} refusal must be coerced to 403"
            );
        }
    }

    /// `OpenRegistrationPolicy` is the nameable form of "we are open" — an app
    /// that installs it must behave exactly like one that installs nothing.
    #[tokio::test]
    async fn the_open_policy_is_indistinguishable_from_no_policy() {
        let policy: Arc<dyn RegistrationPolicy> = Arc::new(OpenRegistrationPolicy);
        for channel in [
            RegistrationChannel::LocalPassword,
            RegistrationChannel::ExternalFirstLogin,
            RegistrationChannel::OauthFirstLogin,
        ] {
            assert_eq!(
                resolve_registration(Some(&policy), channel).await,
                resolve_registration(None, channel).await
            );
        }
    }
}
