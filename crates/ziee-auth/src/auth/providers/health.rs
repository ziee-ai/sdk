//! Auth-provider connection-health enforcement.
//!
//! Single source of truth for "the provider may be enabled only when
//! the IdP responds." Three entry points, all funneled through one
//! probe (`probe_provider`) so the manual Test button, the
//! enable-transition on update, and the create-with-enabled path all
//! behave identically.
//!
//! Mirrors `src-app/server/src/modules/llm_repository/connection_health.rs`
//! — same shape, simpler typing (we don't yet need the HealthOps trait
//! abstraction the LLM-repo module uses for unit testing).

use uuid::Uuid;

use sqlx::PgPool;

use ziee_core::AppError;
use crate::auth::context::AuthEventSink;

use super::events::AuthProviderEvent;
use super::models::AuthProvider;
use super::repository as provider_repo;
use super::{AuthError, create_provider};

/// Outcome of one probe call. Carries the human-readable success
/// blurb on the happy path (mirroring `AuthProviderTrait::test_connection`)
/// and the failure reason on the sad path.
#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub ok: bool,
    pub message: String,
}

impl ProbeResult {
    fn ok(message: String) -> Self {
        Self { ok: true, message }
    }
    fn fail(message: String) -> Self {
        Self { ok: false, message }
    }
}

/// Build the provider in-memory (force-enabled so the factory doesn't
/// reject a row the admin is testing before flipping the switch),
/// then call `test_connection`. Never persists anything.
///
/// This is the single probe site for the manual Test endpoint and for
/// the enable-transition enforcement below — a regression in one is a
/// regression in the other.
pub async fn probe_provider(provider: &AuthProvider, pool: &PgPool) -> ProbeResult {
    // Force-enable a local copy. `create_provider` refuses
    // `enabled=false` rows because the live login path needs that
    // guard, but for tests we want to verify the config independent
    // of the kill switch.
    let mut row = provider.clone();
    row.enabled = true;
    let built = match create_provider(&row, pool.clone()) {
        Ok(p) => p,
        Err(AuthError::ConfigurationError(msg)) => {
            return ProbeResult::fail(format!("Configuration error: {}", msg));
        }
        Err(e) => {
            return ProbeResult::fail(format!("{}", e));
        }
    };
    match built.test_connection().await {
        Ok(msg) => ProbeResult::ok(msg),
        Err(e) => ProbeResult::fail(format!("{}", e)),
    }
}

/// Enforce on a PUT enable-transition. Call AFTER persisting other
/// fields but BEFORE returning the response.
///
/// - `old_enabled == new_enabled` → no-op (no transition to gate).
/// - `false → true` and probe ok → record `last_test_*`, return Ok.
/// - `false → true` and probe fail → revert `enabled=false`, record
///   the failure, emit `AutoDisabled`, return Err(400) so the handler
///   short-circuits.
///
/// Other fields the user updated in the same PUT stay persisted —
/// the partial save is preferable to losing the admin's concurrent
/// name/config edits.
pub async fn enforce_on_update_transition(
    persisted: AuthProvider,
    old_enabled: bool,
    pool: &PgPool,
    events: &dyn AuthEventSink,
) -> Result<AuthProvider, AppError> {
    let transitioned_to_enabled = persisted.enabled && !old_enabled;
    if !transitioned_to_enabled {
        return Ok(persisted);
    }

    let result = probe_provider(&persisted, pool).await;
    if result.ok {
        provider_repo::record_test_result(pool, persisted.id, true, &result.message)
            .await?;
        // Re-fetch so the response carries the recorded `last_test_*` fields.
        let refetched = provider_repo::get_provider_by_id(pool, persisted.id)
            .await
            .map_err(AppError::database_error)?
            .unwrap_or(persisted);
        return Ok(refetched);
    }

    tracing::warn!(
        provider_id = %persisted.id,
        reason = %result.message,
        "auth_provider::health: update-enable-transition probe failed; reverting to enabled=false",
    );
    provider_repo::set_enabled_false(pool, persisted.id).await?;
    provider_repo::record_test_result(pool, persisted.id, false, &result.message).await?;
    events.emit_auth_provider(AuthProviderEvent::auto_disabled(persisted.id, result.message.clone()));
    Err(AppError::bad_request(
        "AUTH_PROVIDER_ENABLE_FAILED_HEALTH_CHECK",
        format!(
            "Other changes were saved, but the provider could not be enabled because the connection probe failed: {}",
            result.message
        ),
    ))
}

/// Wrapper returned by `enforce_on_create_with_enabled`. `connection_warning`
/// is `Some(reason)` when a probe failed and the row was auto-downgraded
/// to `enabled=false`; the handler surfaces it on the create response.
#[derive(Debug, Clone)]
pub struct CreateOutcome {
    pub provider: AuthProvider,
    pub connection_warning: Option<String>,
}

/// Enforce on a POST create. Call AFTER `Repos.auth.create_provider`
/// returns the persisted row. If the row was created with `enabled=true`
/// we probe; on failure we downgrade + record + emit, but do NOT 400 —
/// the row IS created, just disabled, and the caller learns via
/// `connection_warning`.
pub async fn enforce_on_create_with_enabled(
    row: AuthProvider,
    pool: &PgPool,
    events: &dyn AuthEventSink,
) -> Result<CreateOutcome, AppError> {
    if !row.enabled {
        return Ok(CreateOutcome {
            provider: row,
            connection_warning: None,
        });
    }

    let result = probe_provider(&row, pool).await;
    if result.ok {
        provider_repo::record_test_result(pool, row.id, true, &result.message).await?;
        let refetched = provider_repo::get_provider_by_id(pool, row.id)
            .await
            .map_err(AppError::database_error)?
            .unwrap_or(row);
        return Ok(CreateOutcome {
            provider: refetched,
            connection_warning: None,
        });
    }

    tracing::warn!(
        provider_id = %row.id,
        reason = %result.message,
        "auth_provider::health: create-time probe failed; downgrading new provider to disabled",
    );
    provider_repo::set_enabled_false(pool, row.id).await?;
    provider_repo::record_test_result(pool, row.id, false, &result.message).await?;
    events.emit_auth_provider(AuthProviderEvent::auto_disabled(row.id, result.message.clone()));
    let refetched = provider_repo::get_provider_by_id(pool, row.id)
        .await
        .map_err(AppError::database_error)?
        // Concurrent delete between set_enabled_false and the
        // re-fetch: the row genuinely no longer exists, so 404 is
        // truthful (not a 500). Rare in practice — narrow window
        // between two admin tabs racing.
        .ok_or_else(|| AppError::not_found("Auth provider"))?;
    Ok(CreateOutcome {
        provider: refetched,
        connection_warning: Some(result.message),
    })
}

/// Record the outcome of a manual Test on a saved row. Always writes
/// `last_test_*` so the next list call shows the result. Additionally,
/// when the probe failed on a currently-enabled row, auto-disables it
/// + emits `AutoDisabled`. Mirrors
/// `llm_repository::connection_health::record_test_outcome`.
///
/// Returns `Some(disabled_reason)` when the row was auto-disabled —
/// the handler can use it to enrich the response if it wants to.
pub async fn record_test_outcome(
    pool: &PgPool,
    events: &dyn AuthEventSink,
    provider_id: Uuid,
    was_enabled: bool,
    result: &ProbeResult,
) -> Result<Option<String>, AppError> {
    provider_repo::record_test_result(pool, provider_id, result.ok, &result.message)
        .await?;
    if was_enabled && !result.ok {
        tracing::warn!(
            provider_id = %provider_id,
            reason = %result.message,
            "auth_provider::health: manual Test on enabled row failed; auto-disabling",
        );
        provider_repo::set_enabled_false(pool, provider_id).await?;
        events.emit_auth_provider(AuthProviderEvent::auto_disabled(provider_id, result.message.clone()));
        return Ok(Some(result.message.clone()));
    }
    Ok(None)
}
