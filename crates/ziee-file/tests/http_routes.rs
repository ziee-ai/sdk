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
use ziee_file::seams::{DenyAllFileAccess, FileAccess, FileAccessPolicy, FileEvents, OwnerOnlyFileAccess};
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

/// Build the crate's real router behind the stub resolver + a `FileContext`
/// carrying an explicit access policy.
fn app_with(pool: PgPool, user: User, access: Arc<dyn FileAccessPolicy>) -> axum::Router {
    let ctx = FileContext {
        files: Arc::new(FileRepository::new(pool)),
        events: Arc::new(NoopEvents),
        download_token: DownloadTokenSigner {
            issuer: "ziee".to_string(),
            secret: "test-secret".to_string(),
        },
        access,
    };
    let mut openapi = aide::openapi::OpenApi::default();
    file_routes::<StubResolver>()
        .finish_api(&mut openapi)
        .layer(Extension(ctx))
        .layer(Extension(Arc::new(StubResolver { user })))
}

/// The route TEMPLATES the bundle actually registers, straight from the router's
/// own emitted OpenAPI — the ground truth the hand-written table is checked
/// against.
fn mounted_route_templates() -> Vec<String> {
    let mut openapi = aide::openapi::OpenApi::default();
    let _ = file_routes::<StubResolver>().finish_api(&mut openapi);
    let mut pairs: Vec<String> = openapi
        .paths
        .map(|p| {
            p.paths
                .into_iter()
                .flat_map(|(path, item)| {
                    // PANIC rather than silently skip: returning an empty Vec for
                    // a path item this guard cannot read would drop that path from
                    // `mounted`, so it would never be required to appear in the
                    // swept table — a green guard over an unswept route, which is
                    // the exact failure this test exists to prevent.
                    let item = match item {
                        aide::openapi::ReferenceOr::Item(i) => i,
                        aide::openapi::ReferenceOr::Reference { .. } => panic!(
                            "path {path} is a $ref; this guard cannot verify it is \
                             swept — teach it to resolve refs rather than letting \
                             the route go unchecked"
                        ),
                    };
                    // (METHOD, path) pairs, not bare path templates. Several
                    // paths already carry two methods (`/files/{file_id}` is GET
                    // and DELETE), and the bundle is explicitly designed to have
                    // a host MERGE further methods onto the same paths. Comparing
                    // templates alone would let a new `PATCH /files/{file_id}` —
                    // or any host-merged method — mount ungated while this guard
                    // stayed green, which is exactly the blind spot it exists to
                    // remove.
                    [
                        ("GET", &item.get),
                        ("POST", &item.post),
                        ("PUT", &item.put),
                        ("PATCH", &item.patch),
                        ("DELETE", &item.delete),
                        // EVERY method `PathItem` can carry. The first cut omitted
                        // these three, which silently dropped any route mounted
                        // under them — the same green-guard/unswept-route result as
                        // the $ref case above.
                        ("HEAD", &item.head),
                        ("OPTIONS", &item.options),
                        ("TRACE", &item.trace),
                    ]
                    .into_iter()
                    .filter(|(_, op)| op.is_some())
                    .map(|(m, _)| format!("{m} {path}"))
                    .collect::<Vec<_>>()
                })
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

/// The pre-seam behaviour: ownership is the whole answer. Used by the original
/// owner-scope test so its assertions keep meaning exactly what they meant.
fn app(pool: PgPool, user: User) -> axum::Router {
    app_with(pool, user, Arc::new(OwnerOnlyFileAccess))
}

async fn get(app: &axum::Router, method: &str, uri: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    send(app, method, uri, None).await
}

/// Drive one request, optionally with a JSON body.
///
/// `restore` takes `Json<RestoreVersionRequest>` as its LAST extractor, so a
/// body-less POST is rejected at extraction (415/400) and never reaches the
/// handler. A deny test that sent no body would assert `404` against a status the
/// authorization never produced — it would be green while proving nothing about
/// the guard. Hence the body.
async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    json: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match json {
        Some(j) => {
            req = req.header(axum::http::header::CONTENT_TYPE, "application/json");
            Body::from(j.to_string())
        }
        None => Body::empty(),
    };
    let res = app
        .clone()
        .oneshot(req.body(body).unwrap())
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

// ===========================================================================
// The injected authorization seam (`FileAccessPolicy`)
// ===========================================================================

/// Every route `file_routes()` mounts for one file id.
///
/// Enumerated by hand so the tests can drive concrete URIs, and then asserted
/// EQUAL to the router's own emitted path set by
/// `the_route_table_covers_every_mounted_route`. That assertion is what makes
/// this list load-bearing: adding a route to the bundle without adding it here
/// fails that test, so an ungated new surface cannot slip past the sweeps below
/// simply by being absent from a hand-written `Vec`.
fn every_route(file_id: Uuid) -> Vec<(&'static str, String, Option<&'static str>)> {
    let mut all = read_routes(file_id);
    all.extend(destructive_routes(file_id));
    all
}

/// The non-destructive surfaces — safe to sweep repeatedly against one fixture.
fn read_routes(file_id: Uuid) -> Vec<(&'static str, String, Option<&'static str>)> {
    vec![
        ("GET", format!("/files/{file_id}"), None),
        ("GET", format!("/files/{file_id}/download"), None),
        ("GET", format!("/files/{file_id}/raw"), None),
        ("GET", format!("/files/{file_id}/preview"), None),
        ("GET", format!("/files/{file_id}/thumbnail"), None),
        ("GET", format!("/files/{file_id}/text"), None),
        ("GET", format!("/files/{file_id}/text-rects?page=1&start=0&end=1"), None),
        ("GET", format!("/files/{file_id}/versions"), None),
        ("GET", format!("/files/{file_id}/head"), None),
        ("GET", format!("/files/{file_id}/versions/1"), None),
        ("GET", format!("/files/{file_id}/versions/1/download"), None),
        ("GET", format!("/files/{file_id}/versions/1/preview"), None),
        ("GET", format!("/files/{file_id}/versions/1/text"), None),
        ("POST", format!("/files/{file_id}/download-token"), None),
    ]
}

/// The surfaces that MUTATE. Kept separate because sweeping them against a
/// fixture destroys it — the positive-control pass has to run them last, and on
/// their own file, or every later assertion reads a file the sweep deleted.
fn destructive_routes(file_id: Uuid) -> Vec<(&'static str, String, Option<&'static str>)> {
    vec![
        ("POST", format!("/files/{file_id}/restore"), Some(r#"{"version":1}"#)),
        ("DELETE", format!("/files/{file_id}"), None),
    ]
}

/// A host policy that admits an ARBITRARY, explicitly-listed set of file ids —
/// a rule the store knows nothing whatsoever about.
///
/// This is the point of the seam: the store must not have an opinion. If the
/// crate had kept any authorization rule of its own, the routes could not track
/// a policy whose logic is "this id is in a list I made up".
struct HostAcl {
    readable: Vec<Uuid>,
}

#[async_trait::async_trait]
impl FileAccessPolicy for HostAcl {
    async fn can_access(
        &self,
        _principal: Uuid,
        file_id: Uuid,
        _access: FileAccess,
    ) -> Result<bool, AppError> {
        Ok(self.readable.contains(&file_id))
    }

    async fn filter(
        &self,
        _principal: Uuid,
        candidates: &[Uuid],
        _access: FileAccess,
    ) -> Result<Vec<Uuid>, AppError> {
        Ok(candidates
            .iter()
            .copied()
            .filter(|id| self.readable.contains(id))
            .collect())
    }
}

/// TEST-8 [acceptance] [invariant: INV-4] — **bare `files.user_id` is no longer
/// sufficient by itself.**
///
/// The file's own OWNER is refused on every mounted route when the injected
/// policy says no. That is the whole claim of the fail-closed design: before the
/// seam, ownership WAS the authorization, so an owner could never be refused.
///
/// The positive control is what makes it mean something — the identical fixture,
/// the identical routes, the identical owner, admitted under a permissive policy.
/// Without it a green deny would be indistinguishable from a router that never
/// mounted, a resolver that rejected the user, or a blob that was never written.
#[tokio::test]
async fn deny_all_policy_refuses_even_the_files_owner_on_every_route() {
    let (pool, db) = fresh_db().await;
    let tmp = tempfile::tempdir().unwrap();
    init_file_storage(tmp.path());

    let owner = Uuid::new_v4();
    let repo = FileRepository::new(pool.clone());
    let bytes = b"owned bytes";
    let file_id = seed_file(&repo, owner, "owned.txt", bytes).await;

    // POSITIVE CONTROL FIRST: under a permissive policy the owner reaches
    // everything, so every 404 below is attributable to the policy alone.
    let permissive = app_with(pool.clone(), fixed_user(owner), Arc::new(OwnerOnlyFileAccess));
    for (method, uri, body) in read_routes(file_id) {
        let (status, _h, _b) = send(&permissive, method, &uri, body).await;
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "positive control: {method} {uri} is MOUNTED and does not refuse the owner \
             under a permissive policy (a non-404 — some derivative routes 500 here \
             because the fixture writes only the original blob; what matters is that \
             the 404 below is the guard's doing and not a missing route)"
        );
    }
    // …and really gets the bytes, so the fixture is genuinely complete.
    let (status, _h, dl) = get(&permissive, "GET", &format!("/files/{file_id}/download")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(dl, bytes, "positive control: the real bytes are on disk");
    let (_s, _h, body) = get(&permissive, "GET", "/files").await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"], 1, "positive control: the owner enumerates it");

    // The MUTATING routes get their own throwaway file — sweeping them against
    // the fixture above would delete it out from under the assertions.
    let doomed = seed_file(&repo, owner, "doomed.txt", b"doomed").await;
    for (method, uri, body) in destructive_routes(doomed) {
        let (status, _h, _b) = send(&permissive, method, &uri, body).await;
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "positive control: {method} {uri} is MOUNTED and does not refuse the owner \
             under a permissive policy (a non-404 — some derivative routes 500 here \
             because the fixture writes only the original blob; what matters is that \
             the 404 below is the guard's doing and not a missing route)"
        );
    }
    assert!(
        repo.get_by_id(doomed).await.unwrap().is_none(),
        "positive control: the permissive DELETE really destroyed the file — so a \
         404 below is a refusal, not a no-op route"
    );

    // THE INVARIANT: a deny-all policy refuses the OWNER everywhere.
    let denied = app_with(pool.clone(), fixed_user(owner), Arc::new(DenyAllFileAccess));
    for (method, uri, body) in every_route(file_id) {
        let (status, _h, _out) = send(&denied, method, &uri, body).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "deny-all must refuse the file's OWNER on {method} {uri} — ownership \
             alone is not authorization"
        );
    }

    // Enumeration is gated too: page AND total.
    let (status, _h, body) = get(&denied, "GET", "/files").await;
    assert_eq!(status, StatusCode::OK, "the list route still answers");
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"], 0, "deny-all lists nothing, and counts nothing");
    assert!(json["files"].as_array().unwrap().is_empty());

    // The row survived every refusal — including the DELETE.
    assert!(
        repo.get_by_id(file_id).await.unwrap().is_some(),
        "a refused DELETE must not have destroyed the file"
    );

    drop_db(&db).await;
}

/// TEST-10 [acceptance] [invariant: INV-5] — **the store stays generic.**
///
/// Every route's outcome tracks a host rule the crate cannot possibly know
/// ("this id is in a list I made up"), which is only possible if the crate holds
/// no authorization opinion of its own. Paired with the fact that this entire
/// suite builds and runs standalone — no host crate, and a build DB carrying
/// only `files`/`file_versions`, with no tenancy tables of any kind for the crate
/// to have joined against.
#[tokio::test]
async fn the_store_defers_entirely_to_a_host_rule_it_cannot_know() {
    let (pool, db) = fresh_db().await;
    let tmp = tempfile::tempdir().unwrap();
    init_file_storage(tmp.path());

    let owner = Uuid::new_v4();
    let repo = FileRepository::new(pool.clone());
    let reachable = seed_file(&repo, owner, "reachable.txt", b"yes").await;
    let withheld = seed_file(&repo, owner, "withheld.txt", b"no").await;

    // Same owner, same store, same routes — only the host's arbitrary list differs.
    let app = app_with(
        pool.clone(),
        fixed_user(owner),
        Arc::new(HostAcl { readable: vec![reachable] }),
    );

    for (method, uri, body) in read_routes(reachable) {
        let (status, _h, _b) = send(&app, method, &uri, body).await;
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "the host admitted this id, so {method} {uri} must be reachable"
        );
    }
    // The withheld file gets the FULL sweep, mutating routes included: if the
    // store had any rule of its own that overrode the host, a DELETE is where it
    // would show.
    for (method, uri, body) in every_route(withheld) {
        let (status, _h, _b) = send(&app, method, &uri, body).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "the host withheld this id, so {method} {uri} must 404 — the store \
             owns no rule that could override it"
        );
    }

    // Both files are the SAME owner's, so owner scope cannot explain the split.
    assert_eq!(
        repo.get_by_id(withheld).await.unwrap().unwrap().user_id,
        owner,
        "the withheld file is the caller's OWN — the refusal is the policy's, \
         not the store's owner scope"
    );

    drop_db(&db).await;
}

/// TEST-14 — enumeration arithmetic. The filtered list must be paged and counted
/// over the FILTERED set, in the candidate order, with no short pages and no
/// phantom total.
#[tokio::test]
async fn filtered_list_pages_and_counts_over_the_filtered_set() {
    let (pool, db) = fresh_db().await;
    let tmp = tempfile::tempdir().unwrap();
    init_file_storage(tmp.path());

    let owner = Uuid::new_v4();
    let repo = FileRepository::new(pool.clone());

    // Six files; the host withholds the middle two, so a naive
    // "page first, then filter" would return short pages and a total of 6.
    let mut ids = Vec::new();
    for i in 0..6 {
        ids.push(seed_file(&repo, owner, &format!("f{i}.txt"), b"x").await);
    }
    let readable: Vec<Uuid> = ids
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 2 && *i != 3)
        .map(|(_, id)| *id)
        .collect();

    let app = app_with(
        pool.clone(),
        fixed_user(owner),
        Arc::new(HostAcl { readable: readable.clone() }),
    );

    let (status, _h, body) = get(&app, "GET", "/files?page=1&per_page=2").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["total"], 4,
        "total counts the FILTERED set (4 of 6), never the withheld rows"
    );
    assert_eq!(
        json["files"].as_array().unwrap().len(),
        2,
        "a full page — filtering happens before paging, so no short pages"
    );

    // Walk every page and confirm the union is exactly the readable set.
    let mut seen: Vec<Uuid> = Vec::new();
    for page in 1..=2 {
        let (_s, _h, body) = get(&app, "GET", &format!("/files?page={page}&per_page=2")).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        for f in json["files"].as_array().unwrap() {
            seen.push(Uuid::parse_str(f["id"].as_str().unwrap()).unwrap());
        }
    }
    seen.sort();
    let mut expected = readable.clone();
    expected.sort();
    assert_eq!(
        seen, expected,
        "paging the filtered list yields exactly the readable files — no \
         withheld id appears on any page, and none of the readable ones is lost"
    );

    // A policy that admits nothing yields an empty, zero-total list — a
    // principal with no readable files is indistinguishable from one with none.
    let none = app_with(pool.clone(), fixed_user(owner), Arc::new(DenyAllFileAccess));
    let (_s, _h, body) = get(&none, "GET", "/files").await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"], 0);
    assert!(json["files"].as_array().unwrap().is_empty());

    drop_db(&db).await;
}

/// TEST-16 — the hand-written route table must cover EVERY route the bundle mounts.
///
/// Without this, `read_routes`/`destructive_routes` are just a list someone
/// remembered to update: a new route added to `file_routes()` and forgotten here
/// would be swept by nothing, and the deny tests would still pass while leaving
/// it ungated. That is precisely the failure this whole branch exists to fix, so
/// the enumeration is pinned to the router rather than trusted.
#[test]
fn the_route_table_covers_every_mounted_route() {
    let nil = Uuid::nil();
    let mut enumerated: Vec<String> = every_route(nil)
        .into_iter()
        .map(|(m, uri, _b)| {
            // Strip the query string, then put the path parameters back into
            // their `{...}` template form so the two sets are comparable.
            let path = uri.split('?').next().unwrap().to_string();
            let path = path
                .replace(&nil.to_string(), "{file_id}")
                .replace("/versions/1", "/versions/{version}");
            format!("{m} {path}")
        })
        .collect();
    enumerated.push("GET /files".to_string()); // the list route, swept separately
    enumerated.sort();
    enumerated.dedup();

    let mounted = mounted_route_templates();
    assert_eq!(
        enumerated, mounted,
        "the hand-written route table and the router's mounted paths must agree.\n\
         Missing from the table (mounted but never swept, i.e. possibly UNGATED): {:?}\n\
         Stale in the table (listed but not mounted): {:?}",
        mounted.iter().filter(|m| !enumerated.contains(m)).collect::<Vec<_>>(),
        enumerated.iter().filter(|e| !mounted.contains(e)).collect::<Vec<_>>(),
    );
}

/// TEST-13 — **the emitted contract did not move.**
///
/// The seam rewired every handler in the bundle. None of that should be visible
/// to a consumer: `FileContext` is an `Extension`, invisible to `aide`, and no
/// path, method or `operationId` changed. This pins that at the SDK level, so a
/// contract drift is caught in the crate's own suite rather than only as a
/// surprise diff when a host regenerates its client.
///
/// The app-side half — `just openapi-regen` producing an empty diff in both UI
/// workspaces — is recorded as a gate line in TEST_RESULTS.md.
#[test]
fn file_routes_openapi_surface_is_unchanged() {
    let mut openapi = aide::openapi::OpenApi::default();
    let _ = file_routes::<StubResolver>().finish_api(&mut openapi);

    let mut ids: Vec<String> = openapi
        .paths
        .map(|p| {
            p.paths
                .into_iter()
                .flat_map(|(_path, item)| {
                    // Same rule as the route guard: a path item this cannot read
                    // must FAIL, not vanish from the set being compared.
                    let item = match item {
                        aide::openapi::ReferenceOr::Item(i) => i,
                        aide::openapi::ReferenceOr::Reference { .. } => {
                            panic!("path {_path} is a $ref; this pin cannot read it")
                        }
                    };
                    [
                        &item.get,
                        &item.post,
                        &item.delete,
                        &item.put,
                        &item.patch,
                        &item.head,
                        &item.options,
                        &item.trace,
                    ]
                        .into_iter()
                        .flatten()
                        .filter_map(|op| op.operation_id.clone())
                        .collect::<Vec<_>>()
                })
                .collect()
        })
        .unwrap_or_default();
    ids.sort();

    let expected = vec![
        "File.delete",
        "File.download",
        "File.downloadVersion",
        "File.generateDownloadToken",
        "File.get",
        "File.getHeadVersion",
        "File.getPreview",
        "File.getRaw",
        "File.getTextContent",
        "File.getTextRects",
        "File.getThumbnail",
        "File.getVersion",
        "File.list",
        "File.listVersions",
        "File.previewVersion",
        "File.restore",
        "File.textVersion",
    ];

    assert_eq!(
        ids, expected,
        "the file route bundle's operationId set must be byte-identical across \
         the authorization change — a consumer's generated client must not move"
    );
}
