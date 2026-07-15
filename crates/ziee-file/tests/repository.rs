//! Crate-scoped DB integration test for `ziee-file`'s `FileRepository`.
//!
//! Drives the real repository against a fresh throwaway DB migrated with the
//! crate's OWN `files`/`file_versions` migration. Covers the DB-level logic the
//! in-source unit tests (magic-sniff, zip-bomb, filesystem storage) can't reach:
//! the versioning model (create → append → restore head mirror), owner-scoped
//! reads, pagination + per-page clamp, the atomic per-user storage quota
//! (STORAGE_QUOTA_EXCEEDED), byte accounting, and delete → returned blob ids.
//! Also proves STANDALONE MIGRATION APPLY — the base schema carries no
//! cross-module FK (the `users`/self FKs stay ziee-side), so it migrates on a
//! bare DB. The assembled-app upload/download/processing flow stays in ziee.

use sqlx::{Connection, Executor, PgConnection, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;
use ziee_file::models::{FileCreateData, FileVersionCreateData};
use ziee_file::repository::FileRepository;

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:password@127.0.0.1:54321/postgres".to_string())
}

/// Provision a fresh throwaway DB and apply the crate's migration. The migration
/// applying at all IS the standalone-apply proof (no domain tables present).
async fn fresh_db() -> (PgPool, String) {
    let admin = admin_url();
    let base_full = admin.split('?').next().unwrap();
    let (base, _) = base_full.rsplit_once('/').expect("admin url has a /db suffix");
    let dbname = format!("ziee_file_test_{}", Uuid::new_v4().simple());

    let mut conn = PgConnection::connect(&admin).await.expect("connect admin db");
    conn.execute(format!("CREATE DATABASE \"{dbname}\"").as_str())
        .await
        .expect("create test db");
    conn.close().await.ok();

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&format!("{base}/{dbname}"))
        .await
        .expect("connect test db");

    pool.execute(include_str!(
        "../migrations/202607140125_file_schema.sql"
    ))
    .await
    .expect("crate migration applies standalone on a bare DB");

    (pool, dbname)
}

async fn drop_db(dbname: &str) {
    if let Ok(mut conn) = PgConnection::connect(&admin_url()).await {
        let _ = conn
            .execute(format!("DROP DATABASE IF EXISTS \"{dbname}\" WITH (FORCE)").as_str())
            .await;
        conn.close().await.ok();
    }
}

fn file_data(user_id: Uuid, filename: &str, size: i64) -> FileCreateData {
    FileCreateData {
        id: Uuid::new_v4(),
        user_id,
        filename: filename.to_string(),
        file_size: size,
        mime_type: Some("text/plain".to_string()),
        checksum: Some("abc123".to_string()),
        has_thumbnail: false,
        preview_page_count: 0,
        text_page_count: 0,
        processing_metadata: serde_json::json!({}),
        source_message_id: None,
        created_by: "user".to_string(),
    }
}

fn version_data(size: i64) -> FileVersionCreateData {
    FileVersionCreateData {
        file_size: size,
        mime_type: Some("text/plain".to_string()),
        checksum: Some("def456".to_string()),
        has_thumbnail: false,
        preview_page_count: 0,
        text_page_count: 0,
        processing_metadata: serde_json::json!({}),
        source_message_id: None,
        created_by: "user".to_string(),
    }
}

/// `create` inserts the parent + v1 with `version=1` and the head mirror wired
/// so the returned head view (`version`, `current_version_id`, `blob_version_id`)
/// all resolve to v1 (blob keyed by the file_id).
#[tokio::test]
async fn create_seeds_v1_head_view() {
    let (pool, db) = fresh_db().await;
    let repo = FileRepository::new(pool);
    let user = Uuid::new_v4();

    let data = file_data(user, "notes.txt", 123);
    let id = data.id;
    let file = repo.create(data).await.expect("create file");

    assert_eq!(file.id, id);
    assert_eq!(file.user_id, user);
    assert_eq!(file.filename, "notes.txt");
    assert_eq!(file.file_size, 123);
    assert_eq!(file.version, 1, "a fresh file is at version 1");
    assert_eq!(
        file.current_version_id, id,
        "v1's id equals the file id (blob keyed by file id)"
    );
    assert_eq!(file.blob_version_id, id, "v1 blob is keyed by the file id");

    // Round-trips via get_by_id.
    let got = repo.get_by_id(id).await.unwrap().expect("get_by_id");
    assert_eq!(got.id, id);
    assert_eq!(got.version, 1);

    drop_db(&db).await;
}

/// Owner-scoped reads: a foreign user never sees another user's file (single +
/// batch), while the owner does.
#[tokio::test]
async fn reads_are_owner_scoped() {
    let (pool, db) = fresh_db().await;
    let repo = FileRepository::new(pool);
    let owner = Uuid::new_v4();
    let stranger = Uuid::new_v4();

    let f = repo.create(file_data(owner, "a.txt", 10)).await.unwrap();

    assert!(
        repo.get_by_id_and_user(f.id, owner).await.unwrap().is_some(),
        "owner sees their file"
    );
    assert!(
        repo.get_by_id_and_user(f.id, stranger).await.unwrap().is_none(),
        "a stranger must NOT resolve the owner's file"
    );

    // Batch variants.
    let owned = repo.get_by_ids_and_user(&[f.id], owner).await.unwrap();
    assert_eq!(owned.len(), 1);
    let foreign = repo.get_by_ids_and_user(&[f.id], stranger).await.unwrap();
    assert!(foreign.is_empty(), "foreign batch fetch drops the id");

    // get_by_ids (no ownership filter) resolves regardless of user.
    let any = repo.get_by_ids(&[f.id]).await.unwrap();
    assert_eq!(any.len(), 1);

    drop_db(&db).await;
}

/// `list_by_user` paginates one row per file (newest first) and clamps
/// `per_page` into `[1, 100]` so neither `0` nor an enormous value escapes the
/// bound.
#[tokio::test]
async fn list_by_user_paginates_and_clamps_per_page() {
    let (pool, db) = fresh_db().await;
    let repo = FileRepository::new(pool);
    let user = Uuid::new_v4();

    for n in 0..3 {
        repo.create(file_data(user, &format!("f{n}.txt"), 10))
            .await
            .unwrap();
    }

    let (page1, total) = repo.list_by_user(user, 1, 2).await.unwrap();
    assert_eq!(total, 3, "total counts every file for the user");
    assert_eq!(page1.len(), 2, "page 1 with per_page=2 yields 2 rows");

    let (page2, _) = repo.list_by_user(user, 2, 2).await.unwrap();
    assert_eq!(page2.len(), 1, "page 2 holds the remaining row");

    // Upper clamp: an enormous per_page is bounded to <=100, never unbounded.
    let (huge, _) = repo.list_by_user(user, 1, 100_000_000).await.unwrap();
    assert_eq!(huge.len(), 3, "an over-large per_page returns all (clamped to 100)");

    // Lower clamp: per_page=0 is raised to 1 (one row), not zero.
    let (zero, _) = repo.list_by_user(user, 1, 0).await.unwrap();
    assert_eq!(zero.len(), 1, "per_page=0 is clamped up to 1");

    drop_db(&db).await;
}

/// `count_user_bytes` sums the head footprint per user (owner-scoped).
#[tokio::test]
async fn count_user_bytes_sums_owner_footprint() {
    let (pool, db) = fresh_db().await;
    let repo = FileRepository::new(pool);
    let user = Uuid::new_v4();
    let other = Uuid::new_v4();

    assert_eq!(repo.count_user_bytes(user).await.unwrap(), 0, "no files → 0");

    repo.create(file_data(user, "a", 100)).await.unwrap();
    repo.create(file_data(user, "b", 250)).await.unwrap();
    repo.create(file_data(other, "c", 999)).await.unwrap();

    assert_eq!(
        repo.count_user_bytes(user).await.unwrap(),
        350,
        "sums this user's bytes only, excluding the other user's file"
    );

    drop_db(&db).await;
}

/// `create_with_quota` inserts under the cap but rejects an upload that would
/// exceed the per-user quota with the stable `STORAGE_QUOTA_EXCEEDED` code
/// (cumulative across existing files).
#[tokio::test]
async fn create_with_quota_enforces_the_cap() {
    let (pool, db) = fresh_db().await;
    let repo = FileRepository::new(pool);
    let user = Uuid::new_v4();

    // Under the cap → succeeds.
    let ok = repo
        .create_with_quota(file_data(user, "first.txt", 100), 1_000)
        .await
        .expect("insert under quota");
    assert_eq!(ok.file_size, 100);

    // 100 already used + 100 incoming = 200 > 150 → rejected.
    let err = repo
        .create_with_quota(file_data(user, "second.txt", 100), 150)
        .await
        .expect_err("over-quota upload must be rejected");
    assert_eq!(err.error_code(), "STORAGE_QUOTA_EXCEEDED");
    assert_eq!(err.status_code(), 400);

    // The rejected upload left no row behind.
    assert_eq!(
        repo.count_user_bytes(user).await.unwrap(),
        100,
        "the over-quota insert must not have persisted"
    );

    drop_db(&db).await;
}

/// `append_version` advances the head: a new immutable v2 becomes head, the head
/// mirror re-points, and `list_versions`/`get_head`/`get_version` reflect it.
#[tokio::test]
async fn append_version_advances_head() {
    let (pool, db) = fresh_db().await;
    let repo = FileRepository::new(pool);
    let user = Uuid::new_v4();

    let f = repo.create(file_data(user, "doc.txt", 10)).await.unwrap();
    let new_vid = Uuid::new_v4();
    let v2 = repo
        .append_version(f.id, new_vid, version_data(20))
        .await
        .expect("append v2");

    assert_eq!(v2.version, 2, "the appended version increments to 2");
    assert!(v2.is_head, "the appended version is the new head");
    assert_eq!(v2.blob_version_id, new_vid, "an append blob is keyed by its own id");

    // Head mirror re-pointed on the parent.
    let head_view = repo.get_by_id(f.id).await.unwrap().unwrap();
    assert_eq!(head_view.version, 2);
    assert_eq!(head_view.current_version_id, new_vid);
    assert_eq!(head_view.file_size, 20, "the parent mirrors the new head size");

    // Version listing + lookups.
    let versions = repo.list_versions(f.id, user, 50, 0).await.unwrap();
    assert_eq!(versions.len(), 2, "both versions listed");
    assert_eq!(versions[0].version, 2, "newest first");

    let head = repo.get_head(f.id, user).await.unwrap().unwrap();
    assert_eq!(head.version, 2);

    let v1 = repo.get_version(f.id, 1, user).await.unwrap().unwrap();
    assert!(!v1.is_head, "the prior head was flipped to non-head");

    let by_id = repo.get_version_by_id(new_vid, user).await.unwrap().unwrap();
    assert_eq!(by_id.version, 2, "pin by version-id resolves v2 exactly");

    drop_db(&db).await;
}

/// `restore_version` appends a NEW head whose bytes are the target's — the blob
/// is NOT copied (`blob_version_id` points at the target's blob).
#[tokio::test]
async fn restore_version_points_new_head_at_target_blob() {
    let (pool, db) = fresh_db().await;
    let repo = FileRepository::new(pool);
    let user = Uuid::new_v4();

    let f = repo.create(file_data(user, "doc.txt", 10)).await.unwrap();
    // v1 blob id == file id.
    repo.append_version(f.id, Uuid::new_v4(), version_data(20))
        .await
        .unwrap();

    // Restore v1 → creates v3 sharing v1's blob (== file id).
    let v3 = repo
        .restore_version(f.id, 1, "user".to_string(), None)
        .await
        .expect("restore v1");

    assert_eq!(v3.version, 3, "restore appends a new version number");
    assert!(v3.is_head);
    assert_eq!(
        v3.blob_version_id, f.id,
        "the restored head reuses v1's blob (no copy)"
    );
    assert_ne!(v3.id, f.id, "the restored head is a distinct version row");

    let head = repo.get_head(f.id, user).await.unwrap().unwrap();
    assert_eq!(head.version, 3);
    assert_eq!(head.blob_version_id, f.id);

    drop_db(&db).await;
}

/// `delete` returns the DISTINCT blob ids to purge and removes the parent;
/// deleting a non-owned or absent file is a not-found.
#[tokio::test]
async fn delete_returns_blob_ids_and_is_owner_scoped() {
    let (pool, db) = fresh_db().await;
    let repo = FileRepository::new(pool);
    let user = Uuid::new_v4();
    let stranger = Uuid::new_v4();

    let f = repo.create(file_data(user, "doc.txt", 10)).await.unwrap();
    // Add a second version so there are two distinct blob ids.
    let vid2 = Uuid::new_v4();
    repo.append_version(f.id, vid2, version_data(20)).await.unwrap();

    // A stranger cannot delete it.
    let denied = repo.delete(f.id, stranger).await;
    assert!(denied.is_err(), "a non-owner delete must be rejected (not found)");
    assert!(repo.get_by_id(f.id).await.unwrap().is_some(), "file survives the denied delete");

    // Owner delete returns both distinct blob ids and removes the parent.
    let mut blobs = repo.delete(f.id, user).await.expect("owner delete");
    blobs.sort();
    let mut expected = vec![f.id, vid2];
    expected.sort();
    assert_eq!(blobs, expected, "delete returns the distinct blob ids to purge");
    assert!(
        repo.get_by_id(f.id).await.unwrap().is_none(),
        "the file's head view is gone after delete"
    );

    // A second delete of the now-absent file is not-found.
    let gone = repo.delete(f.id, user).await;
    assert!(gone.is_err(), "deleting an already-gone file is a not-found error");

    drop_db(&db).await;
}

/// `list_all_blob_ids_for_user` returns every distinct blob a user owns across
/// files + versions (drives the user-delete blob cleanup).
#[tokio::test]
async fn list_all_blob_ids_for_user_spans_files_and_versions() {
    let (pool, db) = fresh_db().await;
    let repo = FileRepository::new(pool);
    let user = Uuid::new_v4();

    let f1 = repo.create(file_data(user, "a.txt", 10)).await.unwrap();
    let vid = Uuid::new_v4();
    repo.append_version(f1.id, vid, version_data(20)).await.unwrap();
    let f2 = repo.create(file_data(user, "b.txt", 30)).await.unwrap();

    let mut blobs = repo.list_all_blob_ids_for_user(user).await.unwrap();
    blobs.sort();
    let mut expected = vec![f1.id, vid, f2.id]; // f1 v1 blob, f1 v2 blob, f2 v1 blob
    expected.sort();
    assert_eq!(blobs, expected, "every distinct owned blob id is returned");

    drop_db(&db).await;
}
