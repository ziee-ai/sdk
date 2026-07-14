//! Mountable realtime-sync SSE subscribe route (chunk sdk-surfaces).
//!
//! Moved from ziee's `modules::sync::handlers`: the per-user
//! `GET /api/sync/subscribe` Server-Sent-Events handler — the
//! `tokio::select!` over {channel recv, periodic re-check, JWT-`exp` deadline}
//! that is the genuinely app-agnostic half of the subscribe endpoint. It is
//! generic over
//! - an app [`IdentityResolver`] (`R`) — the pluggable auth mechanism (subscribe
//!   gate + the access token's `exp`), and
//! - an app [`SyncSurface`] (`S`) — the concrete, schema-bearing wire/entity
//!   surface. `SyncSurface` is the seam replacing the handler's direct references
//!   to ziee's `SyncConnPrincipal`, its singleton `registry()`, its
//!   `SyncSseEvent` handshake, and its `Repos`-backed periodic re-check. The
//!   framework never names any of those concrete types: the wire enum
//!   (`SyncSseEvent`, in the OpenAPI/`types.ts` surface) stays concrete in the
//!   app and is surfaced here only as the opaque `S::Wire` schema type for the
//!   200 response.
//!
//! An app mounts it with `sync_routes::<AppResolver, AppSurface>()` — ziee wires
//! `sync_routes::<ZieeIdentityResolver, SyncEntity>()` — and gets the SSE
//! subscribe endpoint (path/response/re-check/deadline) byte-identical to the
//! former handler.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use aide::axum::{ApiRouter, routing::get_with};
use aide::transform::TransformOperation;
use axum::{
    Json,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::stream::Stream;
use schemars::JsonSchema;
use serde::Serialize;
use uuid::Uuid;

use ziee_core::ApiResult;
use ziee_identity::{PermissionList, Principal};

use crate::permissions::{IdentityResolver, RequirePermissions, with_permission};

use super::registry::{ClientConn, SyncRegistry, SYNC_CHANNEL_CAPACITY};

/// Re-resolve `is_active` + group permissions this often while a stream
/// is open, so a deactivation / permission change is picked up within the
/// window (bounded staleness) without a per-event DB hit.
const RECHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Resolve the re-check interval. A debug-only env override
/// (`SYNC_RECHECK_TICK_MS`, compiled out of release builds via
/// `cfg!(debug_assertions)` — same testability-seam pattern as
/// `LLM_RUNTIME_REAPER_TICK_MS`) lets the mid-stream deactivation /
/// permission-revocation integration test observe the teardown in
/// milliseconds instead of the 60s production cadence. Ignored in release.
fn recheck_interval() -> Duration {
    #[cfg(debug_assertions)]
    if let Ok(ms) = std::env::var("SYNC_RECHECK_TICK_MS") {
        if let Ok(n) = ms.parse::<u64>() {
            if n > 0 {
                return Duration::from_millis(n);
            }
        }
    }
    RECHECK_INTERVAL
}

/// The outcome of an app's periodic connection re-check ([`SyncSurface::recheck`]).
/// Mirrors the three arms of the former inline re-check in ziee's handler.
pub enum RecheckOutcome<P> {
    /// Still valid: refresh the connection's cached permission snapshot.
    Refresh(P),
    /// Account deactivated / removed, or lost the baseline subscribe
    /// permission → tear the stream down.
    TearDown,
    /// Transient (DB) error → keep the stream and retry on the next tick.
    Transient,
}

/// The app's concrete realtime-sync surface: everything the mountable subscribe
/// handler needs that the framework will not name — the per-connection principal
/// snapshot, the process-wide registry singleton, the SSE wire schema, the
/// baseline subscribe permission, the handshake event, and the periodic
/// re-check. ziee implements this on its `SyncEntity`.
#[async_trait::async_trait]
pub trait SyncSurface: Send + Sync + 'static {
    /// The per-connection permission snapshot registered in the pool
    /// (ziee: `SyncConnPrincipal { user, groups }`).
    type Principal: Principal + Send + Sync + 'static;

    /// The SSE wire enum — the 200 response schema AND the handshake payload's
    /// carrier (ziee: `SyncSseEvent`, which keeps its `JsonSchema` derive so the
    /// OpenAPI/`types.ts` surface is byte-unchanged).
    type Wire: JsonSchema + Serialize + Send + Sync + 'static;

    /// The baseline permission gating both subscribe (the `RequirePermissions`
    /// extractor) and the periodic re-check (ziee: `(ProfileRead,)` → `profile::read`).
    type BaselinePerms: PermissionList;

    /// The process-wide registry singleton keyed to this surface's principal.
    fn registry() -> &'static SyncRegistry<Self::Principal>;

    /// The user id a principal snapshot belongs to (used as the registry key).
    fn principal_user_id(principal: &Self::Principal) -> Uuid;

    /// Build the handshake SSE event handed to the client on connect, carrying
    /// its `conn_id` to echo back for self-echo suppression (ziee:
    /// `SyncSseEvent::Connected(SyncConnectedData { connection_id })`).
    fn connected_signal(conn_id: Uuid) -> Event;

    /// Re-resolve the connection's identity by user id: reload the active user +
    /// groups, re-check the baseline permission, and produce a refreshed
    /// principal — or a teardown/transient signal. Byte-equivalent to the
    /// former inline re-check (`Repos.user.get_by_id` → is_active → admin-empty
    /// groups → baseline `check_permission_union`).
    async fn recheck(user_id: Uuid) -> RecheckOutcome<Self::Principal>;
}

/// Extractor yielding the access token's unix `exp` (or `None`) for bounding the
/// SSE stream deadline. Pulls the app-installed [`IdentityResolver`] out of the
/// request extensions and delegates to
/// [`IdentityResolver::access_token_exp`]; a missing resolver → `None` (the
/// stream falls back to the far-future deadline, never a hard rejection).
struct AccessTokenExp<R>(Option<i64>, PhantomData<R>);

/// Contributes nothing to the OpenAPI operation (it reads the `Authorization`
/// header the auth extractor already documents), so its `OperationInput` is the
/// empty default — keeping the generated `Sync.subscribe` operation byte-identical.
impl<R> aide::OperationInput for AccessTokenExp<R> {}

impl<R: IdentityResolver> FromRequestParts<()> for AccessTokenExp<R> {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &(),
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let exp = parts
                .extensions
                .get::<Arc<R>>()
                .cloned()
                .and_then(|r| r.access_token_exp(parts));
            Ok(Self(exp, PhantomData))
        }
    }
}

/// `GET /api/sync/subscribe` — per-user realtime change stream, generic over the
/// app's resolver `R` and sync surface `S`.
async fn subscribe_sync<R, S>(
    auth: RequirePermissions<R, S::BaselinePerms>,
    exp: AccessTokenExp<R>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, axum::Error>>>>
where
    R: IdentityResolver,
    S: SyncSurface,
    S::Principal: From<(R::User, Vec<R::Group>)>,
{
    // Bound the stream by the access token's expiry: when it lapses the
    // client reconnects with a fresh token, which re-runs the auth
    // extractor (re-checking is_active + permissions from scratch).
    let exp_unix = exp.0;

    let conn_id = Uuid::new_v4();
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<Result<Event, axum::Error>>(SYNC_CHANNEL_CAPACITY);

    let principal: S::Principal = (auth.user, auth.groups).into();
    let user_id = S::principal_user_id(&principal);

    S::registry()
        .register(
            conn_id,
            ClientConn {
                user_id,
                principal,
                sender: tx.clone(),
            },
        )
        .map_err(|e| e.to_api_error())?;

    // Handshake: hand the client its connection id for echo suppression.
    let _ = tx.try_send(Ok(S::connected_signal(conn_id)));

    // Deadline = token exp (fallback far future if somehow absent).
    let secs_remaining = exp_unix
        .map(|exp| (exp - chrono::Utc::now().timestamp()).max(0) as u64)
        .unwrap_or(24 * 60 * 60);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs_remaining);

    let stream = async_stream::stream! {
        // Unregister on ANY stream termination — client disconnect, exp,
        // or deactivation. Drop runs even when the client vanishes
        // mid-await (axum drops the stream future on disconnect).
        let _guard = ConnGuard::<S>(conn_id, PhantomData);

        let mut recheck =
            tokio::time::interval_at(tokio::time::Instant::now() + recheck_interval(), recheck_interval());
        // After a stall, don't fire missed ticks back-to-back (each does DB work).
        recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let sleep = tokio::time::sleep_until(deadline);
        tokio::pin!(sleep);

        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    match maybe {
                        Some(ev) => yield ev,
                        None => break,
                    }
                }
                _ = recheck.tick() => {
                    // Re-resolve is_active + permissions. Tear the stream
                    // down if the account was deactivated/removed OR lost the
                    // baseline subscribe permission; otherwise refresh the
                    // snapshot used to route Permission-audience events (so a
                    // user who loses an admin perm stops receiving its events).
                    match S::recheck(user_id).await {
                        RecheckOutcome::Refresh(principal) => {
                            S::registry().refresh(conn_id, principal);
                        }
                        // Account removed/deactivated or baseline lost → tear down.
                        RecheckOutcome::TearDown => break,
                        // Transient DB error → keep the stream; retry next tick
                        // (don't drop an otherwise-valid connection on a blip).
                        RecheckOutcome::Transient => {}
                    }
                }
                _ = &mut sleep => break,
            }
        }
    };

    Ok((
        StatusCode::OK,
        Sse::new(stream).keep_alive(KeepAlive::default()),
    ))
}

/// Unregisters its connection from the app's registry on drop, covering every
/// way a stream can end (including a client that simply disappears).
struct ConnGuard<S: SyncSurface>(Uuid, PhantomData<S>);

impl<S: SyncSurface> Drop for ConnGuard<S> {
    fn drop(&mut self) {
        S::registry().unregister(self.0);
    }
}

fn subscribe_sync_docs<S: SyncSurface>(op: TransformOperation) -> TransformOperation {
    with_permission::<S::BaselinePerms>(op)
        .id("Sync.subscribe")
        .tag("Sync")
        .summary("Subscribe to realtime cross-device sync events via SSE")
        .description(
            "Server-Sent Events stream of `{entity, action, id}` change \
             notifications scoped to the authenticated user (and the \
             permission-/group-visibility rules per entity). The client \
             refetches the changed entity via its normal REST endpoint, so \
             no row data crosses this channel. The first frame (`connected`) \
             carries a connection id to echo back as `X-Sync-Connection-Id` \
             on mutations for self-echo suppression. The stream closes when \
             the access token expires or the account loses access.",
        )
        .response::<200, Json<S::Wire>>()
        .response_with::<401, (), _>(|res| res.description("Unauthorized"))
        .response_with::<429, (), _>(|res| res.description("Too many open sync connections"))
}

/// Mountable realtime-sync router: `GET /sync/subscribe`, generic over the app's
/// identity resolver `R` and sync surface `S`. Mount it under `/api`.
pub fn sync_routes<R, S>() -> ApiRouter
where
    R: IdentityResolver,
    S: SyncSurface,
    S::Principal: From<(R::User, Vec<R::Group>)>,
{
    ApiRouter::new().api_route(
        "/sync/subscribe",
        get_with(subscribe_sync::<R, S>, subscribe_sync_docs::<S>),
    )
}
