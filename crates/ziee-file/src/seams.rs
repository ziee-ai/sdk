//! Injected seams that sever the file STORE from the app's file PROCESSING and
//! realtime-sync/RAG subsystems.
//!
//! The store persists bytes + derivative blobs + version rows; it does NOT know
//! how to PRODUCE derivatives (that is the app's `ProcessingManager`) nor how to
//! notify other devices / trigger RAG re-indexing (that is the app's `sync` +
//! `file_rag`). Two traits invert those dependencies:
//!
//! - [`FileProcessor`] — the app implements it with `ProcessingManager`; the
//!   store's `ingest_bytes` calls it to turn raw bytes into a
//!   [`crate::models::ProcessingResult`]. The store never names the extraction
//!   engine.
//! - [`FileEvents`] — the app wires `on_file_changed`/`on_file_deleted` to
//!   `sync::publish` (`SyncEntity::File`, owner-scoped) and `on_committed` to
//!   `file_rag::ingest::spawn_{index,reindex}`. The store never names
//!   `SyncEntity` or `file_rag`.
//! - [`FileAccessPolicy`] — the app decides whether a principal may reach a file
//!   AT ALL. The store knows only `files.user_id`; it does not know that a host
//!   may reach the same blob through a shared folder, a team, a tenant, or any
//!   other resource of its own. See the section below — this seam is what stops
//!   ownership being the whole answer.

use async_trait::async_trait;
use uuid::Uuid;

use crate::models::ProcessingResult;
use ziee_core::AppError;

/// Produce derivative bytes (text pages / geometry / thumbnails / preview
/// images / metadata) from a file's raw bytes. Implemented app-side by
/// `ProcessingManager`; the store calls it before persisting.
#[async_trait]
pub trait FileProcessor: Send + Sync {
    /// Run the extraction pipeline. Returning `Err` lets the store degrade
    /// gracefully (store the raw original only) while logging the failure.
    async fn process(&self, bytes: &[u8], mime_type: &str) -> Result<ProcessingResult, AppError>;
}

/// Post-persist notifications. The store fires these after a durable change so
/// the app can push cross-device sync + kick off background RAG indexing —
/// without the store naming the concrete `SyncEntity`/`file_rag` symbols.
pub trait FileEvents: Send + Sync {
    /// A file's head/version set changed (append / restore / new file). Wired
    /// app-side to `sync::publish_file_changed_with_origin` (owner-scoped
    /// `SyncEntity::File` update). `origin` is the originating SSE connection
    /// (self-echo skip) when known.
    fn on_file_changed(&self, user_id: Uuid, file_id: Uuid, origin: Option<Uuid>);

    /// A file was deleted. Wired app-side to
    /// `sync::publish_file_deleted_with_origin`.
    fn on_file_deleted(&self, user_id: Uuid, file_id: Uuid, origin: Option<Uuid>);

    /// A new head was committed (new file or new version). Wired app-side to
    /// `file_rag::ingest::spawn_{index,reindex}` for background RAG indexing.
    /// `is_new` distinguishes a brand-new file (`spawn_index`) from a new
    /// version of an existing file (`spawn_reindex`). Default no-op so callers
    /// that don't need RAG (e.g. plain ingest) can ignore it.
    fn on_committed(&self, _user_id: Uuid, _file_id: Uuid, _is_new: bool) {}
}

// ---------------------------------------------------------------------------
// Authorization
// ---------------------------------------------------------------------------

/// What a principal is attempting to do with a file, so a host policy can answer
/// at the granularity it cares about.
///
/// The store itself draws no distinction between these — it asks, the host
/// decides. A host with one rule for everything can simply ignore the value; one
/// that separates "may look at it" from "may take the bytes" from "may destroy
/// it" has the vocabulary to say so without an API change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FileAccess {
    /// Metadata, version listings, extracted text, thumbnails and preview
    /// images — everything that describes the file or renders a derivative of
    /// it, but is not the original bytes.
    ReadMetadata,
    /// The ORIGINAL bytes: `download`, `raw`, a version download, and minting a
    /// download token (which is a durable, transferable capability for exactly
    /// these bytes, so it is gated as content, not as metadata).
    ///
    /// Note the limit of the token gate: it stops a principal MINTING a new
    /// token once access is revoked, but a token already minted stays valid
    /// until it expires. A host that mounts a route redeeming those tokens and
    /// wants revocation to take effect immediately must consult this policy
    /// again at redemption.
    ReadContent,
    /// Mutating the file's version chain — e.g. restoring an older version as
    /// the new head.
    Write,
    /// Destroying the file and every blob behind it.
    Delete,
}

/// Whether a principal may reach a file at all.
///
/// # Why this seam exists
///
/// The store's own scoping is `files.user_id` — the uploading principal. That is
/// a statement about PROVENANCE, not about authorization, and the two come apart
/// the moment a host lets a file be reached through anything other than its
/// uploader: a shared folder, a team, a tenant, an organization. When they come
/// apart, `user_id` alone silently keeps granting access that the host believes
/// it has revoked — the uploader keeps listing, downloading and deleting content
/// long after the host stopped considering them entitled to it.
///
/// The store cannot fix that itself, because the resource that confers the
/// entitlement is the host's, not the store's. So it asks. Every mounted route
/// resolves through [`crate::http::FileContext::authorized_file`], which requires
/// BOTH the store's owner scope AND this policy's assent; neither alone is
/// sufficient. Because the policy is only ever an additional conjunct, a host
/// policy can narrow access but can never widen it beyond what the store would
/// already have allowed.
///
/// # Fail-closed by construction
///
/// [`crate::http::FileContext`] takes this as a REQUIRED field: no `Default`, no
/// `Option`, no implicit fallback. A host that supplies nothing does not get a
/// permissive default — it does not compile. That is deliberate: a
/// silently-permissive default would leave every existing consumer exactly as
/// exposed as before while appearing to have been fixed. Hosts choose
/// [`DenyAllFileAccess`], [`OwnerOnlyFileAccess`], or their own impl, and the
/// choice is greppable at every injection site.
#[async_trait]
pub trait FileAccessPolicy: Send + Sync {
    /// May `principal` perform `access` on `file_id`?
    ///
    /// `false` is rendered by the caller as a `404`, identical to a file that
    /// does not exist — a distinguishable `403` would confirm the file's
    /// existence to a principal who is not entitled to know it.
    ///
    /// An `Err` is propagated, NOT swallowed into a permit: a policy that cannot
    /// reach its own backing store must fail the request, never open it.
    async fn can_access(
        &self,
        principal: Uuid,
        file_id: Uuid,
        access: FileAccess,
    ) -> Result<bool, AppError>;

    /// Restrict `candidates` to the subset `principal` may `access`, PRESERVING
    /// the given order.
    ///
    /// This is the batch half, and it is separate from [`Self::can_access`] on
    /// purpose. Listing must be filtered BEFORE it is paged and counted: a
    /// per-item check applied to an already-paged result yields short pages and,
    /// worse, a total that still counts the files it just hid — which leaks the
    /// very existence the filter was meant to conceal. `candidates` is always
    /// already owner-scoped by the store, so a host only ever sees ids the
    /// principal uploaded.
    ///
    /// Implementations SHOULD answer in one batch round-trip rather than looping
    /// `can_access`, which is why this is a distinct method rather than a
    /// provided one. It MUST agree with `can_access` for the same
    /// `(principal, file, access)` — a batch that drifts from the point check
    /// either hides a file the caller can still fetch, or lists one it cannot.
    ///
    /// It takes `access` for the same reason `can_access` does: a host whose
    /// answer differs by kind (say, read standing for viewing and edit standing
    /// for destroying) must be able to say so through BOTH halves of the trait.
    /// The store passes [`FileAccess::ReadMetadata`] for the file list, which is
    /// the only enumeration it exposes today.
    async fn filter(
        &self,
        principal: Uuid,
        candidates: &[Uuid],
        access: FileAccess,
    ) -> Result<Vec<Uuid>, AppError>;
}

/// Refuses everything.
///
/// The safe thing to reach for when a host has not yet modelled file
/// authorization, and the right thing to inject in a context where the file
/// routes should be mounted but inert. Every route will answer `404`.
pub struct DenyAllFileAccess;

#[async_trait]
impl FileAccessPolicy for DenyAllFileAccess {
    async fn can_access(&self, _: Uuid, _: Uuid, _: FileAccess) -> Result<bool, AppError> {
        Ok(false)
    }

    async fn filter(
        &self,
        _: Uuid,
        _: &[Uuid],
        _: FileAccess,
    ) -> Result<Vec<Uuid>, AppError> {
        Ok(Vec::new())
    }
}

/// Ownership is the whole answer: whatever the store's `files.user_id` scope
/// already admitted is admitted.
///
/// # This policy was the subject of a confirmed exploit. Read this before injecting it.
///
/// The behaviour below is what every consumer of this crate had before
/// [`FileAccessPolicy`] existed, and in a multi-tenant host it was a live,
/// reachable data leak — verified by running it, not by reading the code:
///
/// > A user uploaded files into a dataset belonging to an organization. They were
/// > then removed from that organization — their membership row deleted, every
/// > dataset and project gate correctly refusing them afterwards. Because
/// > `files.user_id` still named them as the uploader, they retained, indefinitely:
/// > `GET /files` (an enumeration of every file they had ever uploaded, which is
/// > how they found the ids), `GET /files/{id}/download` returning **200 and the
/// > exact bytes**, every metadata and version route, a mintable download token,
/// > and `DELETE /files/{id}` returning **204** — they could destroy the
/// > organization's data after being removed from it.
///
/// Nothing about that was a bug in this crate's code. It is what "ownership is
/// authorization" MEANS once a file can be reached through anything other than its
/// uploader, because `files.user_id` records who uploaded the bytes and never stops
/// recording it. No revocation the host performs can change it.
///
/// # When it is correct
///
/// Only where `files.user_id` is the complete authorization story: a single-user or
/// strictly single-tenant deployment with no sharing, no teams, no organizations,
/// and no way for a principal's entitlement to a file to be revoked while the
/// `files` row still names them. If a host has ANY resource that confers access to
/// a file — a shared folder, a dataset, a project, a workspace, a tenant — this
/// policy is wrong, and the paragraph above is what wrong looks like.
///
/// It is provided named rather than withheld because that deployment is real and
/// deserves a reviewed answer instead of a hand-rolled one. Choosing it is meant to
/// be an explicit, auditable act (`git grep OwnerOnlyFileAccess`) rather than
/// something a host falls into by writing nothing — which is why
/// [`crate::http::FileContext`] takes the policy as a required field.
pub struct OwnerOnlyFileAccess;

#[async_trait]
impl FileAccessPolicy for OwnerOnlyFileAccess {
    async fn can_access(&self, _: Uuid, _: Uuid, _: FileAccess) -> Result<bool, AppError> {
        Ok(true)
    }

    async fn filter(
        &self,
        _: Uuid,
        candidates: &[Uuid],
        _: FileAccess,
    ) -> Result<Vec<Uuid>, AppError> {
        Ok(candidates.to_vec())
    }
}
