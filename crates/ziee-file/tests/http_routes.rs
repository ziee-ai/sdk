//! Crate-scoped HTTP-boundary integration test for `ziee-file`'s mountable
//! `file_routes::<R>()` bundle.
//!
//! Every handler under `src/http/handlers/` is otherwise only exercised via
//! ziee's assembled server. This mounts the crate's OWN router standalone —
//! generic over a minimal stub `IdentityResolver` (returns one fixed
//! `files::*`-holding user) with a temp file store + a fresh throwaway DB — and
//! drives the request/response boundary in-process via `tower::oneshot`. It
//! gives the crate a self-contained regression guard on route wiring, owner
//! scoping (foreign / absent file → 404), the download byte + Content-Disposition
//! path, and delete. Mirrors `ziee-health`'s `finish_api → oneshot` pattern and
//! `ziee-framework`'s stub-resolver pattern.
#![cfg(feature = "routes")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{request::Parts, Request, StatusCode};
use axum::Extension;
use sqlx::{postgres::PgPoolOptions, Connection, Executor, PgConnection, PgPool};
use tower::ServiceExt; // oneshot
use uuid::Uuid;

use ziee_auth::{Group, User};
use ziee_core::AppError;
use ziee_file::http::context::{DownloadTokenSigner, FileContext};
use ziee_file::http::routes::file_routes;
use ziee_file::models::FileCreateData;
use ziee_file::repository::FileRepository;
use ziee_file::seams::FileEvents;
use ziee_file::{get_file_storage, init_file_storage};
use ziee_framework::permissions::IdentityResolver;

// ---- fresh-DB harness (mirrors tests/repository.rs) -----------------------

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:password@127.0.0.1:54321/postgres".to_string())
}

async fn fresh_db() -> (PgPool, String) {
    let admin = admin_url();
    let base_full = admin.split('?').next().unwrap();
    let (base, _) = base_full.rsplit_once('/').expect("admin url has a /db suffix");
    let dbname = format!("ziee_file_http_test_{}", Uuid::new_v4().simple());

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

    pool.execute(include_str!("../migrations/202607140125_file_schema.sql"))
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

// ---- stub identity + events -----------------------------------------------

/// Build a fixed non-admin `User` holding the `files::*` permissions the routes
/// gate on. Non-admin (not the `is_admin` short-circuit) so the real
/// direct-permission union path runs.
fn fixed_user(id: Uuid) -> User {
    let now = chrono::Utc::now();
    User {
        id,
        username: "tester".to_string(),
        email: "tester@example.com".to_string(),
        email_verified: true,
        password_hash: None,
        display_name: None,
        avatar_url: None,
        is_active: true,
        is_admin: false,
        permissions: vec![
            "files::read".to_string(),
            "files::upload".to_string(),
            "files::download".to_string(),
            "files::delete".to_string(),
            "files::preview".to_string(),
            "files::generate_token".to_string(),
        ],
        created_at: now,
        updated_at: now,
        last_login_at: None,
        password_changed_at: None,
    }
}

/// Resolver that authenticates every request as one fixed user — the routes'
/// owner scoping (`user.id`) then does the real work.
struct StubResolver {
    user: User,
}

#[async_trait::async_trait]
impl IdentityResolver for StubResolver {
    type User = User;
    type Group = Group;

    async fn authenticate(&self, _parts: &mut Parts) -> Result<User, (StatusCode, AppError)> {
        Ok(self.user.clone())
    }

    async fn load_groups(&self, _user: &User) -> Result<Vec<Group>, (StatusCode, AppError)> {
        Ok(vec![])
    }

    fn active_group_permissions(group: &Group) -> Option<&[String]> {
        if group.is_active {
            Some(&group.permissions)
        } else {
            None
        }
    }
}

struct NoopEvents;
impl FileEvents for NoopEvents {
    fn on_file_changed(&self, _user_id: Uuid, _file_id: Uuid, _origin: Option<Uuid>) {}
    fn on_file_deleted(&self, _user_id: Uuid, _file_id: Uuid, _origin: Option<Uuid>) {}
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

/// Build the crate's real router behind the stub resolver + a `FileContext`.
fn app(pool: PgPool, user: User) -> axum::Router {
    let ctx = FileContext {
        files: Arc::new(FileRepository::new(pool)),
        events: Arc::new(NoopEvents),
        download_token: DownloadTokenSigner {
            issuer: "ziee".to_string(),
            secret: "test-secret".to_string(),
        },
    };
    let mut openapi = aide::openapi::OpenApi::default();
    file_routes::<StubResolver>()
        .finish_api(&mut openapi)
        .layer(Extension(ctx))
        .layer(Extension(Arc::new(StubResolver { user })))
}

async fn get(app: &axum::Router, method: &str, uri: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

/// Seed a real file row + its head blob on disk for `owner`, returning the id.
async fn seed_file(repo: &FileRepository, owner: Uuid, filename: &str, bytes: &[u8]) -> Uuid {
    let file = repo
        .create(file_data(owner, filename, bytes.len() as i64))
        .await
        .expect("create file row");
    let ext = filename.rsplit('.').next().unwrap_or("bin").to_lowercase();
    // v1 blob is keyed by the file id (== blob_version_id).
    get_file_storage()
        .save_original(owner, file.blob_version_id, &ext, bytes)
        .await
        .expect("save original blob");
    file.id
}

#[tokio::test]
async fn file_routes_list_get_download_delete_and_owner_scope() {
    let (pool, db) = fresh_db().await;
    let tmp = tempfile::tempdir().unwrap();
    init_file_storage(tmp.path());

    let owner = Uuid::new_v4();
    let user = fixed_user(owner);
    let repo = FileRepository::new(pool.clone());

    let bytes = b"the original file bytes";
    let file_id = seed_file(&repo, owner, "notes.txt", bytes).await;

    // A file owned by ANOTHER user — must stay invisible to `owner` (owner scope).
    let stranger = Uuid::new_v4();
    let foreign_id = seed_file(&repo, stranger, "secret.txt", b"not yours").await;

    let app = app(pool.clone(), user);

    // GET /files → 200, lists exactly the owner's one file.
    let (status, _h, body) = get(&app, "GET", "/files").await;
    assert_eq!(status, StatusCode::OK, "list must be 200");
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"], 1, "owner sees exactly their own file: {json}");
    assert_eq!(json["files"].as_array().unwrap().len(), 1);

    // GET /files/{id} → 200 metadata.
    let (status, _h, body) = get(&app, "GET", &format!("/files/{file_id}")).await;
    assert_eq!(status, StatusCode::OK, "get own file → 200");
    let meta: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(meta["id"], file_id.to_string());
    assert_eq!(meta["filename"], "notes.txt");

    // GET a FOREIGN file → 404 (owner-scoped, never leaks another user's file).
    let (status, _h, _b) = get(&app, "GET", &format!("/files/{foreign_id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a foreign file must 404 for owner scope");

    // GET an ABSENT file → 404.
    let (status, _h, _b) = get(&app, "GET", &format!("/files/{}", Uuid::new_v4())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "an absent file → 404");

    // GET /files/{id}/download → 200 with the exact bytes + attachment disposition.
    let (status, headers, dl) = get(&app, "GET", &format!("/files/{file_id}/download")).await;
    assert_eq!(status, StatusCode::OK, "download own file → 200");
    assert_eq!(dl, bytes, "download returns the exact stored bytes");
    let disp = headers
        .get(axum::http::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(disp.contains("attachment"), "download is an attachment: {disp:?}");
    assert!(disp.contains("notes.txt"), "disposition names the file: {disp:?}");

    // Downloading a FOREIGN file → 404 (owner scope on the binary path too).
    let (status, _h, _b) = get(&app, "GET", &format!("/files/{foreign_id}/download")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "foreign download must 404");

    // DELETE /files/{id} → 204, then the file is gone (subsequent GET → 404).
    let (status, _h, _b) = get(&app, "DELETE", &format!("/files/{file_id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "delete own file → 204");

    let (status, _h, _b) = get(&app, "GET", &format!("/files/{file_id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the file is gone after delete");

    // Deleting a FOREIGN file → 404 (repo delete is owner-scoped).
    let (status, _h, _b) = get(&app, "DELETE", &format!("/files/{foreign_id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "foreign delete must 404");

    drop_db(&db).await;
}
