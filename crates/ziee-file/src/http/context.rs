//! Per-request dependency handle for the mountable file HTTP surface.
//!
//! The store-generic handlers pull this from `Extension<FileContext>` instead of
//! reaching app globals (`Repos.file`, the file-module JWT config,
//! `sync::publish`/`file_rag`). The app assembles + installs it once at boot.

use std::sync::Arc;

use uuid::Uuid;

use crate::models::File;
use crate::repository::FileRepository;
use crate::seams::{FileAccess, FileAccessPolicy, FileEvents};
use ziee_core::AppError;

/// Signing material for file download tokens (`generate_download_token`). The app
/// installs the same issuer/secret its access-token JWT validator uses so a
/// download token verifies (ziee's still-app-side `download_with_token` re-checks
/// iss + `aud = DOWNLOAD_TOKEN_AUDIENCE`).
#[derive(Clone)]
pub struct DownloadTokenSigner {
    /// JWT `iss` claim baked into issued tokens.
    pub issuer: String,
    /// HMAC secret the token is signed with.
    pub secret: String,
}

/// Per-request handle the mountable file handlers pull from
/// `Extension<FileContext>` instead of reaching app globals. Cheaply cloneable
/// (everything behind `Arc`).
#[derive(Clone)]
pub struct FileContext {
    /// The file store repository (replaces `Repos.file`).
    pub files: Arc<FileRepository>,
    /// Post-mutation notifications (replaces the direct
    /// `sync::publish_file_*` + `file_rag::spawn_reindex` calls); the app
    /// installs a `sync`/`file_rag`-backed [`FileEvents`] impl.
    pub events: Arc<dyn FileEvents>,
    /// Download-token signing material (replaces the file-module JWT global).
    pub download_token: DownloadTokenSigner,
    /// Whether a principal may reach a file at all — see [`FileAccessPolicy`].
    ///
    /// REQUIRED, and deliberately so. There is no `Default` for `FileContext` and
    /// this is not an `Option`, so a host cannot end up with a permissive fallback
    /// by writing nothing: omitting it is a compile error that names this field.
    /// The store's `files.user_id` scope answers "who uploaded these bytes", which
    /// is not the same question as "who may read them" in any host where a file is
    /// reachable through a shared resource.
    pub access: Arc<dyn FileAccessPolicy>,
}

impl FileContext {
    /// Resolve a file FOR a principal, or fail with the same `404` an absent file
    /// produces.
    ///
    /// This is the single choke point every per-file route in
    /// [`crate::http::routes::file_routes`] goes through, and the only place the
    /// authorization rule is written. Both conditions must hold:
    ///
    /// 1. the store's owner scope (`files.user_id = principal`), and
    /// 2. the host's [`FileAccessPolicy`] assent for `access`.
    ///
    /// Keeping (1) as a conjunct rather than replacing it with (2) means a host
    /// policy can only ever NARROW access. A buggy policy cannot hand one
    /// principal another's file, because the store never offered it in the first
    /// place.
    ///
    /// Both failures render as `404`, never `403`: a distinguishable status would
    /// confirm the file exists to a principal not entitled to know it, which is
    /// the enumeration signal this seam exists to close.
    pub async fn authorized_file(
        &self,
        principal: Uuid,
        file_id: Uuid,
        access: FileAccess,
    ) -> Result<File, AppError> {
        let file = self
            .files
            .get_by_id_and_user(file_id, principal)
            .await?
            .ok_or_else(|| AppError::not_found("File"))?;

        if !self.access.can_access(principal, file_id, access).await? {
            return Err(AppError::not_found("File"));
        }

        Ok(file)
    }

    /// [`Self::authorized_file`] for the routes that do not need the [`File`] row
    /// itself — `delete`, and the version lookups that resolve through
    /// `file_versions` rather than `files`.
    ///
    /// Those routes historically ran their own `… AND user_id = $n` query and
    /// never touched `get_by_id_and_user` at all, which is exactly why they were
    /// the easiest surfaces to leave behind: nothing about their shape hinted
    /// that an ownership check was the whole authorization.
    ///
    /// It applies BOTH conjuncts, not just the policy. An earlier shape checked
    /// only the policy and relied on each caller's own repository method to carry
    /// `f.user_id` — which they all happen to do, so it was correct, but only
    /// incidentally. [`FileRepository::get_by_id`] is `pub` and unscoped, so a
    /// future route pairing it with a policy-only check would have made the host
    /// policy the SOLE authority without anything looking wrong. Resolving the
    /// owner scope here makes "a policy can only narrow" a property of this
    /// method rather than of its callers.
    pub async fn authorize(
        &self,
        principal: Uuid,
        file_id: Uuid,
        access: FileAccess,
    ) -> Result<(), AppError> {
        self.authorized_file(principal, file_id, access).await?;
        Ok(())
    }

    /// The authorized, paged file list for a principal.
    ///
    /// Filters BEFORE paging and counting, which is the whole point: applying a
    /// per-item check to an already-paged result returns short pages and a
    /// `total` that still counts the rows it just hid. A count that says "7" over
    /// a page showing 5 tells the caller two files exist that they may not see —
    /// the same existence leak the per-file `404` is careful to avoid.
    ///
    /// Costs one id-column scan of the principal's own files plus one batch
    /// policy round-trip. That is `O(files uploaded by this principal)`, not
    /// `O(all files)`, and it is what buys an exact `total`.
    pub async fn authorized_list(
        &self,
        principal: Uuid,
        page: i32,
        per_page: i32,
    ) -> Result<(Vec<File>, i64), AppError> {
        let candidates = self.files.list_ids_by_user(principal).await?;
        let readable = self
            .access
            .filter(principal, &candidates, FileAccess::ReadMetadata)
            .await?;

        // Intersect with what we actually offered. A policy is only ever allowed
        // to NARROW, and this makes that structural instead of a property the
        // store happens to get for free from `list_by_user_filtered`'s retained
        // `user_id` conjunct: an impl that returned ids it was never given cannot
        // widen the result even by accident.
        let offered: std::collections::HashSet<Uuid> = candidates.into_iter().collect();
        let readable: Vec<Uuid> = readable
            .into_iter()
            .filter(|id| offered.contains(id))
            .collect();

        self.files
            .list_by_user_filtered(principal, &readable, page, per_page)
            .await
    }
}
