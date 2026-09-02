// File repository
//
// Versioning model: `files` is the parent (PK = stable file_id, referenced
// everywhere). `file_versions` holds one immutable row per version. `files`
// keeps the per-version columns as a denormalized mirror of the HEAD version
// (kept in lock-step by `append_version` / `restore_version`), so existing
// readers of `files.*` transparently see the latest version. Reads here JOIN
// the head only to surface the head `version` number and `blob_version_id`
// (the storage key, which differs from `current_version_id` for a restored
// head). Storage blobs are keyed by `blob_version_id`; v1's id == file_id so
// pre-versioning blobs resolve unchanged.

use ziee_core::AppError;
use crate::models::{File, FileCreateData, FileVersion, FileVersionCreateData};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct FileRepository {
    pool: PgPool,
}

impl FileRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a new file: inserts the parent `files` row, version 1 (id =
    /// file_id, blob_version_id = file_id so the on-disk blob path is keyed by
    /// the file_id for v1), and points the head at it — all in one transaction.
    pub async fn create(&self, data: FileCreateData) -> Result<File, AppError> {
        let mut tx = self.pool.begin().await.map_err(AppError::database_error)?;

        // Insert the parent already pointing at its (about-to-exist) v1. The
        // current_version_id FK is DEFERRABLE INITIALLY DEFERRED, so referencing
        // file_versions($1) before that row exists is fine — it's verified at
        // COMMIT. This keeps current_version_id NOT NULL at all times.
        sqlx::query!(
            r#"
            INSERT INTO files (
                id, user_id, filename, file_size, mime_type, checksum,
                has_thumbnail, preview_page_count, text_page_count,
                processing_metadata, created_by, current_version_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $1)
            "#,
            data.id,
            data.user_id,
            data.filename,
            data.file_size,
            data.mime_type,
            data.checksum,
            data.has_thumbnail,
            data.preview_page_count,
            data.text_page_count,
            data.processing_metadata,
            data.created_by,
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        sqlx::query!(
            r#"
            INSERT INTO file_versions (
                id, file_id, version, is_head, blob_version_id,
                file_size, mime_type, checksum, has_thumbnail,
                preview_page_count, text_page_count, processing_metadata,
                source_message_id, created_by
            )
            VALUES ($1, $1, 1, true, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
            data.id,
            data.file_size,
            data.mime_type,
            data.checksum,
            data.has_thumbnail,
            data.preview_page_count,
            data.text_page_count,
            data.processing_metadata,
            data.source_message_id,
            data.created_by,
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        tx.commit().await.map_err(AppError::database_error)?;

        self.get_by_id(data.id)
            .await?
            .ok_or_else(|| AppError::internal_error("file vanished after create"))
    }

    /// Like `create`, but enforces the per-user storage quota atomically.
    /// A per-user transaction-scoped advisory lock serializes concurrent
    /// uploads so two can't both pass a separate count-then-insert check and
    /// overshoot the quota (closes the TOCTOU in the upload handler). Returns
    /// `STORAGE_QUOTA_EXCEEDED` (the handler's pre-check is the fast path).
    pub async fn create_with_quota(
        &self,
        data: FileCreateData,
        quota_bytes: i64,
    ) -> Result<File, AppError> {
        let mut tx = self.pool.begin().await.map_err(AppError::database_error)?;

        // Serialize per user for the lifetime of the transaction.
        sqlx::query!(
            "SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))",
            data.user_id.to_string()
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        let used: i64 = sqlx::query_scalar!(
            r#"SELECT COALESCE(SUM(file_size), 0)::BIGINT AS "total"
               FROM files WHERE user_id = $1"#,
            data.user_id
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(AppError::database_error)?
        .unwrap_or(0);

        if used.saturating_add(data.file_size) > quota_bytes {
            return Err(AppError::bad_request(
                "STORAGE_QUOTA_EXCEEDED",
                format!(
                    "Upload would exceed the per-user storage quota \
                     ({used} bytes already used + {} bytes incoming)",
                    data.file_size
                ),
            ));
        }

        sqlx::query!(
            r#"
            INSERT INTO files (
                id, user_id, filename, file_size, mime_type, checksum,
                has_thumbnail, preview_page_count, text_page_count,
                processing_metadata, created_by, current_version_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $1)
            "#,
            data.id,
            data.user_id,
            data.filename,
            data.file_size,
            data.mime_type,
            data.checksum,
            data.has_thumbnail,
            data.preview_page_count,
            data.text_page_count,
            data.processing_metadata,
            data.created_by,
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        sqlx::query!(
            r#"
            INSERT INTO file_versions (
                id, file_id, version, is_head, blob_version_id,
                file_size, mime_type, checksum, has_thumbnail,
                preview_page_count, text_page_count, processing_metadata,
                source_message_id, created_by
            )
            VALUES ($1, $1, 1, true, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
            data.id,
            data.file_size,
            data.mime_type,
            data.checksum,
            data.has_thumbnail,
            data.preview_page_count,
            data.text_page_count,
            data.processing_metadata,
            data.source_message_id,
            data.created_by,
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        tx.commit().await.map_err(AppError::database_error)?;

        self.get_by_id(data.id)
            .await?
            .ok_or_else(|| AppError::internal_error("file vanished after create"))
    }

    // NOTE (chunk ziee-file): the former `set_workflow_run_id` /
    // `list_ids_by_workflow_run` helpers + the `files.workflow_run_id` column
    // are NOT part of this domain-agnostic store. The file↔run association lives
    // app-side in ziee's `file_workflow_runs` join table (workflow module), so
    // the store stays free of any `workflow_runs` reference.

    /// Get file by ID (internal use). Returns the head view.
    pub async fn get_by_id(&self, file_id: Uuid) -> Result<Option<File>, AppError> {
        let file = sqlx::query_as!(
            File,
            r#"
            SELECT f.id, f.user_id, f.filename, f.file_size, f.mime_type, f.checksum,
                   f.has_thumbnail, f.preview_page_count, f.text_page_count,
                   f.processing_metadata as "processing_metadata!: _",
                   f.created_by,
                   f.created_at as "created_at: _",
                   f.updated_at as "updated_at: _",
                   fv.version, f.current_version_id as "current_version_id!",
                   fv.blob_version_id
            FROM files f
            JOIN file_versions fv ON fv.id = f.current_version_id
            WHERE f.id = $1
            "#,
            file_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(file)
    }

    /// Batch variant of [`get_by_id`] — resolve many files in one query (head
    /// views). No ownership filter (callers that need it use
    /// `get_by_ids_and_user`); used where ownership is enforced downstream.
    pub async fn get_by_ids(&self, file_ids: &[Uuid]) -> Result<Vec<File>, AppError> {
        let files = sqlx::query_as!(
            File,
            r#"
            SELECT f.id, f.user_id, f.filename, f.file_size, f.mime_type, f.checksum,
                   f.has_thumbnail, f.preview_page_count, f.text_page_count,
                   f.processing_metadata as "processing_metadata!: _",
                   f.created_by,
                   f.created_at as "created_at: _",
                   f.updated_at as "updated_at: _",
                   fv.version, f.current_version_id as "current_version_id!",
                   fv.blob_version_id
            FROM files f
            JOIN file_versions fv ON fv.id = f.current_version_id
            WHERE f.id = ANY($1)
            "#,
            file_ids
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(files)
    }

    /// Get file by ID and verify user ownership. Returns the head view.
    pub async fn get_by_id_and_user(
        &self,
        file_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<File>, AppError> {
        let file = sqlx::query_as!(
            File,
            r#"
            SELECT f.id, f.user_id, f.filename, f.file_size, f.mime_type, f.checksum,
                   f.has_thumbnail, f.preview_page_count, f.text_page_count,
                   f.processing_metadata as "processing_metadata!: _",
                   f.created_by,
                   f.created_at as "created_at: _",
                   f.updated_at as "updated_at: _",
                   fv.version, f.current_version_id as "current_version_id!",
                   fv.blob_version_id
            FROM files f
            JOIN file_versions fv ON fv.id = f.current_version_id
            WHERE f.id = $1 AND f.user_id = $2
            "#,
            file_id,
            user_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(file)
    }

    /// Batch-fetch files by id, scoped to the owning user. Foreign or deleted
    /// ids simply don't appear in the result. Returns head views.
    pub async fn get_by_ids_and_user(
        &self,
        file_ids: &[Uuid],
        user_id: Uuid,
    ) -> Result<Vec<File>, AppError> {
        let files = sqlx::query_as!(
            File,
            r#"
            SELECT f.id, f.user_id, f.filename, f.file_size, f.mime_type, f.checksum,
                   f.has_thumbnail, f.preview_page_count, f.text_page_count,
                   f.processing_metadata as "processing_metadata!: _",
                   f.created_by,
                   f.created_at as "created_at: _",
                   f.updated_at as "updated_at: _",
                   fv.version, f.current_version_id as "current_version_id!",
                   fv.blob_version_id
            FROM files f
            JOIN file_versions fv ON fv.id = f.current_version_id
            WHERE f.id = ANY($1) AND f.user_id = $2
            "#,
            file_ids,
            user_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok(files)
    }

    /// List files for user with pagination (head views). One row per file —
    /// the head join attaches head metadata, so versioned files appear once.
    /// **Unfiltered owner-scoped listing — NOT the route path.**
    ///
    /// `GET /files` goes through [`crate::http::FileContext::authorized_list`],
    /// which filters through the host's [`crate::seams::FileAccessPolicy`] before
    /// paging. This method applies no policy at all, so wiring a route to it
    /// re-opens the enumeration this seam exists to close — the same hazard
    /// [`crate::http::FileContext::authorize`]'s doc records for
    /// [`Self::get_by_id`].
    ///
    /// It is kept for callers that legitimately need the full owner footprint
    /// with no principal to authorize (administrative sweeps, quota/footprint
    /// accounting). If you are serving a request, you want `authorized_list`.
    #[deprecated(
        since = "0.0.0",
        note = "unfiltered: use FileContext::authorized_list for anything serving a request"
    )]
    pub async fn list_by_user(
        &self,
        user_id: Uuid,
        page: i32,
        per_page: i32,
    ) -> Result<(Vec<File>, i64), AppError> {
        let page64 = (page as i64).max(1);
        // Clamp per_page to the shared upper bound so `?per_page=100000000`
        // can't force an unbounded LIMIT (memory amplification). Matches the
        // convention in file/handlers/versions.rs + common::PaginationQuery.
        let per_page64 =
            (per_page as i64).clamp(1, 100i64);
        let offset = (page64 - 1).saturating_mul(per_page64);

        let total: i64 = sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!" FROM files WHERE user_id = $1"#,
            user_id
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        let files = sqlx::query_as!(
            File,
            r#"
            SELECT f.id, f.user_id, f.filename, f.file_size, f.mime_type, f.checksum,
                   f.has_thumbnail, f.preview_page_count, f.text_page_count,
                   f.processing_metadata as "processing_metadata!: _",
                   f.created_by,
                   f.created_at as "created_at: _",
                   f.updated_at as "updated_at: _",
                   fv.version, f.current_version_id as "current_version_id!",
                   fv.blob_version_id
            FROM files f
            JOIN file_versions fv ON fv.id = f.current_version_id
            WHERE f.user_id = $1
            ORDER BY f.created_at DESC
            LIMIT $2 OFFSET $3
            "#,
            user_id,
            per_page64,
            offset
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok((files, total))
    }

    /// Every file id owned by `user_id`, newest first — the candidate set an
    /// authorization policy filters before the list is paged.
    ///
    /// Ids only: this runs on every list request, so it stays a narrow index-ish
    /// scan rather than materializing rows that will mostly be paged away.
    pub async fn list_ids_by_user(&self, user_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        sqlx::query_scalar!(
            r#"SELECT id as "id!" FROM files WHERE user_id = $1 ORDER BY created_at DESC"#,
            user_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// [`Self::list_by_user`] restricted to an explicit id allow-list.
    ///
    /// Both the page AND the `total` are computed over the restricted set, so a
    /// filtered listing cannot report a count that includes what it withheld.
    /// Passing an empty allow-list yields an empty page and a `total` of 0 — a
    /// principal with no readable files is indistinguishable from one with no
    /// files.
    ///
    /// `user_id` is retained as a conjunct alongside the allow-list: the ids come
    /// from a host policy, and the store does not take a caller-supplied id set
    /// as permission to leave its own owner scope.
    pub async fn list_by_user_filtered(
        &self,
        user_id: Uuid,
        allowed: &[Uuid],
        page: i32,
        per_page: i32,
    ) -> Result<(Vec<File>, i64), AppError> {
        let page64 = (page as i64).max(1);
        // Same clamp as `list_by_user` — `?per_page=100000000` must not force an
        // unbounded LIMIT.
        let per_page64 = (per_page as i64).clamp(1, 100i64);
        let offset = (page64 - 1).saturating_mul(per_page64);

        let total: i64 = sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!" FROM files
               WHERE user_id = $1 AND id = ANY($2)"#,
            user_id,
            allowed
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        let files = sqlx::query_as!(
            File,
            r#"
            SELECT f.id, f.user_id, f.filename, f.file_size, f.mime_type, f.checksum,
                   f.has_thumbnail, f.preview_page_count, f.text_page_count,
                   f.processing_metadata as "processing_metadata!: _",
                   f.created_by,
                   f.created_at as "created_at: _",
                   f.updated_at as "updated_at: _",
                   fv.version, f.current_version_id as "current_version_id!",
                   fv.blob_version_id
            FROM files f
            JOIN file_versions fv ON fv.id = f.current_version_id
            WHERE f.user_id = $1 AND f.id = ANY($2)
            ORDER BY f.created_at DESC
            LIMIT $3 OFFSET $4
            "#,
            user_id,
            allowed,
            per_page64,
            offset
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        Ok((files, total))
    }

    /// Sum the user's current uploaded file bytes (= head sizes, since
    /// `files.file_size` mirrors the head). Drives the per-user storage quota.
    /// Quota counts the current footprint (head once), not the sum of all
    /// versions.
    pub async fn count_user_bytes(&self, user_id: Uuid) -> Result<i64, AppError> {
        let total: Option<i64> = sqlx::query_scalar!(
            r#"SELECT COALESCE(SUM(file_size), 0)::BIGINT AS "total"
               FROM files
               WHERE user_id = $1"#,
            user_id
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(total.unwrap_or(0))
    }

    /// Delete a file and all its versions. Returns the DISTINCT
    /// `blob_version_id`s that were referenced so the caller can purge the
    /// on-disk blobs (restored versions share a blob → dedupe avoids a
    /// double-delete / missing-blob). The DB rows cascade via the FK.
    pub async fn delete(&self, file_id: Uuid, user_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        let blob_ids: Vec<Uuid> = sqlx::query_scalar!(
            r#"SELECT DISTINCT fv.blob_version_id
               FROM file_versions fv
               JOIN files f ON f.id = fv.file_id
               WHERE fv.file_id = $1 AND f.user_id = $2"#,
            file_id,
            user_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        let result = sqlx::query!(
            "DELETE FROM files WHERE id = $1 AND user_id = $2",
            file_id,
            user_id
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::database_error)?;

        if result.rows_affected() == 0 {
            return Err(AppError::not_found("File"));
        }

        Ok(blob_ids)
    }

    /// All distinct blob version ids owned by a user (across every file +
    /// version). Used by the user-delete path to clean up on-disk blobs that
    /// the `files` `ON DELETE CASCADE` would otherwise orphan.
    pub async fn list_all_blob_ids_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<Uuid>, AppError> {
        sqlx::query_scalar!(
            r#"SELECT DISTINCT fv.blob_version_id
               FROM file_versions fv
               JOIN files f ON f.id = fv.file_id
               WHERE f.user_id = $1"#,
            user_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)
    }

    /// Append a new immutable version and advance the head. The caller MUST
    /// have already saved the new blob keyed by `new_version_id` (the new
    /// version's `blob_version_id` == its own id). Flips the prior head via the
    /// partial-unique `is_head` index and re-syncs the `files` head mirror.
    pub async fn append_version(
        &self,
        file_id: Uuid,
        new_version_id: Uuid,
        data: FileVersionCreateData,
    ) -> Result<FileVersion, AppError> {
        let mut tx = self.pool.begin().await.map_err(AppError::database_error)?;

        // Serialize concurrent appends to this file. Without the lock, two
        // parallel edits (or an MCP edit racing a sandbox version-back) both read
        // the same MAX(version) and collide on UNIQUE(file_id, version), surfacing
        // as a 500. Locking the parent row makes per-file appends sequential.
        sqlx::query_scalar!("SELECT id FROM files WHERE id = $1 FOR UPDATE", file_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(AppError::database_error)?;

        let next: i32 = sqlx::query_scalar!(
            r#"SELECT COALESCE(MAX(version), 0) + 1 AS "v!" FROM file_versions WHERE file_id = $1"#,
            file_id
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        sqlx::query!(
            "UPDATE file_versions SET is_head = false WHERE file_id = $1 AND is_head",
            file_id
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        let version = sqlx::query_as!(
            FileVersion,
            r#"
            INSERT INTO file_versions (
                id, file_id, version, is_head, blob_version_id,
                file_size, mime_type, checksum, has_thumbnail,
                preview_page_count, text_page_count, processing_metadata,
                source_message_id, created_by
            )
            VALUES ($1, $2, $3, true, $1, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING id, file_id, version, is_head, blob_version_id,
                      file_size, mime_type, checksum, has_thumbnail,
                      preview_page_count, text_page_count,
                      processing_metadata as "processing_metadata!: _",
                      source_message_id,
                      created_by,
                      created_at as "created_at: _"
            "#,
            new_version_id,
            file_id,
            next,
            data.file_size,
            data.mime_type,
            data.checksum,
            data.has_thumbnail,
            data.preview_page_count,
            data.text_page_count,
            data.processing_metadata,
            data.source_message_id,
            data.created_by,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        self.sync_head_mirror(&mut tx, file_id, new_version_id, &version)
            .await?;

        tx.commit().await.map_err(AppError::database_error)?;
        Ok(version)
    }

    /// Restore a prior version: append a NEW head whose bytes are the target
    /// version's (no blob is copied — `blob_version_id` points at the target's
    /// blob). Append-only: the intervening versions are untouched.
    pub async fn restore_version(
        &self,
        file_id: Uuid,
        target_version: i32,
        created_by: String,
        source_message_id: Option<Uuid>,
    ) -> Result<FileVersion, AppError> {
        let mut tx = self.pool.begin().await.map_err(AppError::database_error)?;

        // Serialize against concurrent appends/restores on this file (see
        // append_version) so the new version number can't collide.
        sqlx::query_scalar!("SELECT id FROM files WHERE id = $1 FOR UPDATE", file_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(AppError::database_error)?;

        let target = sqlx::query!(
            r#"SELECT blob_version_id, file_size, mime_type, checksum, has_thumbnail,
                      preview_page_count, text_page_count,
                      processing_metadata as "processing_metadata!: serde_json::Value"
               FROM file_versions WHERE file_id = $1 AND version = $2"#,
            file_id,
            target_version
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(AppError::database_error)?
        .ok_or_else(|| AppError::not_found("File version"))?;

        let next: i32 = sqlx::query_scalar!(
            r#"SELECT COALESCE(MAX(version), 0) + 1 AS "v!" FROM file_versions WHERE file_id = $1"#,
            file_id
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        sqlx::query!(
            "UPDATE file_versions SET is_head = false WHERE file_id = $1 AND is_head",
            file_id
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        let new_id = Uuid::new_v4();
        let version = sqlx::query_as!(
            FileVersion,
            r#"
            INSERT INTO file_versions (
                id, file_id, version, is_head, blob_version_id,
                file_size, mime_type, checksum, has_thumbnail,
                preview_page_count, text_page_count, processing_metadata,
                source_message_id, created_by
            )
            VALUES ($1, $2, $3, true, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            RETURNING id, file_id, version, is_head, blob_version_id,
                      file_size, mime_type, checksum, has_thumbnail,
                      preview_page_count, text_page_count,
                      processing_metadata as "processing_metadata!: _",
                      source_message_id,
                      created_by,
                      created_at as "created_at: _"
            "#,
            new_id,
            file_id,
            next,
            target.blob_version_id,
            target.file_size,
            target.mime_type,
            target.checksum,
            target.has_thumbnail,
            target.preview_page_count,
            target.text_page_count,
            target.processing_metadata,
            source_message_id,
            created_by,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(AppError::database_error)?;

        self.sync_head_mirror(&mut tx, file_id, new_id, &version)
            .await?;

        tx.commit().await.map_err(AppError::database_error)?;
        Ok(version)
    }

    /// Re-point `files.current_version_id` and refresh the denormalized
    /// per-version mirror columns to the new head, inside an open transaction.
    async fn sync_head_mirror(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        file_id: Uuid,
        new_version_id: Uuid,
        v: &FileVersion,
    ) -> Result<(), AppError> {
        sqlx::query!(
            r#"UPDATE files SET current_version_id = $1, file_size = $2, mime_type = $3,
                   checksum = $4, has_thumbnail = $5, preview_page_count = $6,
                   text_page_count = $7, processing_metadata = $8, updated_at = NOW()
               WHERE id = $9"#,
            new_version_id,
            v.file_size,
            v.mime_type,
            v.checksum,
            v.has_thumbnail,
            v.preview_page_count,
            v.text_page_count,
            v.processing_metadata,
            file_id,
        )
        .execute(&mut **tx)
        .await
        .map_err(AppError::database_error)?;
        Ok(())
    }

    /// List all versions of a file (newest first), ownership-gated.
    pub async fn list_versions(
        &self,
        file_id: Uuid,
        user_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<FileVersion>, AppError> {
        let versions = sqlx::query_as!(
            FileVersion,
            r#"
            SELECT fv.id, fv.file_id, fv.version, fv.is_head, fv.blob_version_id,
                   fv.file_size, fv.mime_type, fv.checksum, fv.has_thumbnail,
                   fv.preview_page_count, fv.text_page_count,
                   fv.processing_metadata as "processing_metadata!: _",
                   fv.source_message_id, fv.created_by,
                   fv.created_at as "created_at: _"
            FROM file_versions fv
            JOIN files f ON f.id = fv.file_id
            WHERE fv.file_id = $1 AND f.user_id = $2
            ORDER BY fv.version DESC
            LIMIT $3 OFFSET $4
            "#,
            file_id,
            user_id,
            limit,
            offset
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(versions)
    }

    /// Get a specific version by (file_id, version number), ownership-gated.
    pub async fn get_version(
        &self,
        file_id: Uuid,
        version: i32,
        user_id: Uuid,
    ) -> Result<Option<FileVersion>, AppError> {
        let v = sqlx::query_as!(
            FileVersion,
            r#"
            SELECT fv.id, fv.file_id, fv.version, fv.is_head, fv.blob_version_id,
                   fv.file_size, fv.mime_type, fv.checksum, fv.has_thumbnail,
                   fv.preview_page_count, fv.text_page_count,
                   fv.processing_metadata as "processing_metadata!: _",
                   fv.source_message_id, fv.created_by,
                   fv.created_at as "created_at: _"
            FROM file_versions fv
            JOIN files f ON f.id = fv.file_id
            WHERE fv.file_id = $1 AND fv.version = $2 AND f.user_id = $3
            "#,
            file_id,
            version,
            user_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(v)
    }

    /// Get the head version of a file, ownership-gated.
    pub async fn get_head(
        &self,
        file_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<FileVersion>, AppError> {
        let v = sqlx::query_as!(
            FileVersion,
            r#"
            SELECT fv.id, fv.file_id, fv.version, fv.is_head, fv.blob_version_id,
                   fv.file_size, fv.mime_type, fv.checksum, fv.has_thumbnail,
                   fv.preview_page_count, fv.text_page_count,
                   fv.processing_metadata as "processing_metadata!: _",
                   fv.source_message_id, fv.created_by,
                   fv.created_at as "created_at: _"
            FROM file_versions fv
            JOIN files f ON f.id = fv.file_id
            WHERE fv.file_id = $1 AND fv.is_head AND f.user_id = $2
            "#,
            file_id,
            user_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(v)
    }

    /// Get a version by its own `file_versions.id` (pins exact bytes regardless
    /// of head), ownership-gated. Used to resolve `version_id`-pinned reads.
    pub async fn get_version_by_id(
        &self,
        version_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<FileVersion>, AppError> {
        let v = sqlx::query_as!(
            FileVersion,
            r#"
            SELECT fv.id, fv.file_id, fv.version, fv.is_head, fv.blob_version_id,
                   fv.file_size, fv.mime_type, fv.checksum, fv.has_thumbnail,
                   fv.preview_page_count, fv.text_page_count,
                   fv.processing_metadata as "processing_metadata!: _",
                   fv.source_message_id, fv.created_by,
                   fv.created_at as "created_at: _"
            FROM file_versions fv
            JOIN files f ON f.id = fv.file_id
            WHERE fv.id = $1 AND f.user_id = $2
            "#,
            version_id,
            user_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::database_error)?;
        Ok(v)
    }
}
