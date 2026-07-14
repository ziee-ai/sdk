use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

/// A file attached to a conversation, surfaced by `get_conversation_files`.
/// Used by sandbox tools to expose user-uploaded files as read-only binds
/// at their original filenames.
#[derive(Debug, Clone, FromRow)]
pub struct ConversationFile {
    pub file_id: Uuid,
    /// Head version's blob storage key. NOT `file_id`: for a v2+ file, `file_id`
    /// keys v1's blob, so loading bytes by `file_id` returns STALE content.
    /// Always load originals/text/images by `blob_version_id`.
    pub blob_version_id: Uuid,
    /// Head version number + row id — for pinning resource_links emitted for
    /// this file (so the UI opens the exact version, not a guessed head).
    pub version: i32,
    pub version_id: Uuid,
    pub filename: String,
    pub user_id: Uuid,
    pub mime_type: Option<String>,
    #[allow(dead_code)]
    pub created_at: DateTime<Utc>,
}

/// Provenance row linking a per-conversation workspace path to the `files` row
/// it represents, so in-sandbox edits version-back to that file instead of
/// minting an orphan. `base_version_id` is the version the workspace copy was
/// last seeded from / committed as.
#[derive(Debug, Clone, FromRow)]
pub struct SandboxWorkspaceFile {
    #[allow(dead_code)]
    pub conversation_id: Uuid,
    pub workspace_relpath: String,
    pub file_id: Uuid,
    pub base_version_id: Uuid,
}
