//! `SyncOrigin` request extractor (chunk B5, moved verbatim from ziee's
//! `modules::sync::extractor`).
//!
//! Reads the originating SSE connection id from the `X-Sync-Connection-Id`
//! header (set by the SyncClient on mutating requests) so the fan-out can skip
//! echoing a change back to the tab that made it. Never fails: an absent/invalid
//! header is `None`, which simply means "no self-echo suppression for this
//! request". App-agnostic — depends only on axum + uuid.

use std::convert::Infallible;

use aide::OperationIo;
use axum::{extract::FromRequestParts, http::request::Parts};
use uuid::Uuid;

/// Header carrying the caller's current sync connection id.
pub const SYNC_CONNECTION_HEADER: &str = "X-Sync-Connection-Id";

#[derive(Debug, Clone, Copy, OperationIo)]
#[aide(input)]
pub struct SyncOrigin(pub Option<Uuid>);

impl FromRequestParts<()> for SyncOrigin {
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &(),
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let id = parts
            .headers
            .get(SYNC_CONNECTION_HEADER)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| Uuid::parse_str(s).ok());
        async move { Ok(SyncOrigin(id)) }
    }
}
