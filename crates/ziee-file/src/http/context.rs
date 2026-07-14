//! Per-request dependency handle for the mountable file HTTP surface.
//!
//! The store-generic handlers pull this from `Extension<FileContext>` instead of
//! reaching app globals (`Repos.file`, the file-module JWT config,
//! `sync::publish`/`file_rag`). The app assembles + installs it once at boot.

use std::sync::Arc;

use crate::repository::FileRepository;
use crate::seams::FileEvents;

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
}
