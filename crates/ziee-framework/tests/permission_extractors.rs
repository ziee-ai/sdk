//! Crate-scoped integration test for `ziee-framework`'s permission-enforcement
//! extractors (`RequirePermissions` / `RequireAdmin`).
//!
//! The in-source unit tests cover the `PermissionList`/`with_permission` OpenAPI
//! plumbing and the sync registry's audience routing, but NOTHING exercises the
//! actual FromRequestParts ENFORCEMENT ALGORITHM: JWT → user → admin
//! short-circuit → group union (ALL-of AND) → 403 formatting, plus the
//! missing-resolver 500 and auth 401. This mounts the real extractors on an axum
//! router behind a fake `IdentityResolver` and drives each branch in-process via
//! `tower::oneshot` — the byte-for-byte behavior every ziee gate depends on. The
//! concrete `ZieeIdentityResolver` wiring stays in ziee's server/tests.

use std::sync::Arc;

use axum::{
    Extension, Router,
    body::Body,
    http::{Request, StatusCode, request::Parts},
    routing::get,
};
use tower::ServiceExt; // oneshot

use ziee_core::AppError;
use ziee_framework::permissions::{IdentityResolver, RequireAdmin, RequirePermissions};
use ziee_identity::{PermissionCheck, Principal};

// ---- Fake identity types --------------------------------------------------

#[derive(Clone)]
struct TestGroup {
    perms: Vec<String>,
    active: bool,
}

#[derive(Clone)]
struct TestUser {
    admin: bool,
    direct: Vec<String>,
    groups: Vec<TestGroup>,
}

impl Principal for TestUser {
    fn is_admin(&self) -> bool {
        self.admin
    }
    fn direct_permissions(&self) -> &[String] {
        &self.direct
    }
}

/// Resolver whose `authenticate` maps a bearer token to a canned identity, so a
/// test drives a given branch purely via the request's `Authorization` header.
struct TestResolver;

#[async_trait::async_trait]
impl IdentityResolver for TestResolver {
    type User = TestUser;
    type Group = TestGroup;

    async fn authenticate(&self, parts: &mut Parts) -> Result<TestUser, (StatusCode, AppError)> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|h| h.to_str().ok());
        let user = match token {
            None => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    AppError::unauthorized("NO_TOKEN", "missing bearer token"),
                ));
            }
            Some("Bearer admin") => TestUser { admin: true, direct: vec![], groups: vec![] },
            Some("Bearer direct") => TestUser {
                admin: false,
                direct: vec!["users::read".to_string()],
                groups: vec![],
            },
            Some("Bearer group") => TestUser {
                admin: false,
                direct: vec![],
                groups: vec![TestGroup {
                    perms: vec!["users::read".to_string()],
                    active: true,
                }],
            },
            Some("Bearer inactivegroup") => TestUser {
                admin: false,
                direct: vec![],
                groups: vec![TestGroup {
                    perms: vec!["users::read".to_string()],
                    active: false,
                }],
            },
            Some("Bearer both") => TestUser {
                admin: false,
                direct: vec!["users::read".to_string(), "users::edit".to_string()],
                groups: vec![],
            },
            Some("Bearer none") => TestUser { admin: false, direct: vec![], groups: vec![] },
            Some(_) => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    AppError::unauthorized("BAD_TOKEN", "unrecognized token"),
                ));
            }
        };
        Ok(user)
    }

    async fn load_groups(&self, user: &TestUser) -> Result<Vec<TestGroup>, (StatusCode, AppError)> {
        Ok(user.groups.clone())
    }

    fn active_group_permissions(group: &TestGroup) -> Option<&[String]> {
        if group.active {
            Some(&group.perms)
        } else {
            None
        }
    }
}

// ---- Test permissions -----------------------------------------------------

struct UsersRead;
impl PermissionCheck for UsersRead {
    const NAME: &'static str = "UsersRead";
    const PERMISSION: &'static str = "users::read";
    const DESCRIPTION: &'static str = "Read users";
    const MODULE: &'static str = "users";
}

struct UsersEdit;
impl PermissionCheck for UsersEdit {
    const NAME: &'static str = "UsersEdit";
    const PERMISSION: &'static str = "users::edit";
    const DESCRIPTION: &'static str = "Edit users";
    const MODULE: &'static str = "users";
}

// ---- Router ---------------------------------------------------------------

/// Router with the resolver installed. `/one` needs `users::read`; `/two` needs
/// BOTH `users::read` AND `users::edit`; `/admin` needs root admin.
fn app() -> Router {
    Router::new()
        .route(
            "/one",
            get(|_: RequirePermissions<TestResolver, (UsersRead,)>| async { StatusCode::OK }),
        )
        .route(
            "/two",
            get(
                |_: RequirePermissions<TestResolver, (UsersRead, UsersEdit)>| async {
                    StatusCode::OK
                },
            ),
        )
        .route(
            "/admin",
            get(|_: RequireAdmin<TestResolver>| async { StatusCode::OK }),
        )
        .layer(Extension(Arc::new(TestResolver)))
}

/// Router WITHOUT the resolver installed — exercises the misconfiguration 500.
fn app_without_resolver() -> Router {
    Router::new().route(
        "/one",
        get(|_: RequirePermissions<TestResolver, (UsersRead,)>| async { StatusCode::OK }),
    )
}

async fn get_with(app: Router, path: &str, bearer: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::builder().uri(path);
    if let Some(b) = bearer {
        req = req.header("authorization", b);
    }
    let res = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn missing_token_is_401() {
    let (status, _) = get_with(app(), "/one", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no token → the resolver's 401");
}

#[tokio::test]
async fn admin_bypasses_permission_check() {
    // Admin has NO direct perms + no groups, yet the is_admin short-circuit
    // grants access before any group load.
    let (status, _) = get_with(app(), "/one", Some("Bearer admin")).await;
    assert_eq!(status, StatusCode::OK, "root admin bypasses the permission gate");
}

#[tokio::test]
async fn direct_permission_holder_is_allowed() {
    let (status, _) = get_with(app(), "/one", Some("Bearer direct")).await;
    assert_eq!(status, StatusCode::OK, "a direct users::read holder passes /one");
}

#[tokio::test]
async fn group_derived_permission_is_allowed() {
    // The perm comes ONLY from an active group — exercises the union's group leg.
    let (status, _) = get_with(app(), "/one", Some("Bearer group")).await;
    assert_eq!(status, StatusCode::OK, "an active-group users::read holder passes");
}

#[tokio::test]
async fn inactive_group_permission_is_denied() {
    // Same perm, but the group is inactive → active_group_permissions returns
    // None → the union excludes it → 403.
    let (status, body) = get_with(app(), "/one", Some("Bearer inactivegroup")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an INACTIVE group grants nothing");
    assert!(body.contains("INSUFFICIENT_PERMISSIONS"), "403 body: {body}");
}

#[tokio::test]
async fn missing_permission_is_403_and_names_it() {
    let (status, body) = get_with(app(), "/one", Some("Bearer none")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("INSUFFICIENT_PERMISSIONS"), "403 error_code: {body}");
    assert!(
        body.contains("users::read"),
        "the 403 message must name the missing permission: {body}"
    );
}

#[tokio::test]
async fn multi_permission_requires_all_and_lists_the_missing_one() {
    // `direct` holds ONLY users::read; /two also requires users::edit → 403
    // naming the missing edit perm (the AND semantics).
    let (status, body) = get_with(app(), "/two", Some("Bearer direct")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "holding 1 of 2 required perms is denied");
    assert!(
        body.contains("users::edit"),
        "the 403 must name the missing users::edit: {body}"
    );
    assert!(
        !body.contains("Missing required permissions: users::read"),
        "the held perm must not be listed as missing: {body}"
    );
}

#[tokio::test]
async fn holding_all_permissions_passes_the_multi_gate() {
    let (status, _) = get_with(app(), "/two", Some("Bearer both")).await;
    assert_eq!(status, StatusCode::OK, "holding BOTH required perms passes /two");
}

#[tokio::test]
async fn missing_resolver_is_500() {
    let (status, _) = get_with(app_without_resolver(), "/one", Some("Bearer admin")).await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a request with no resolver installed is a server misconfiguration → 500"
    );
}

#[tokio::test]
async fn require_admin_allows_admin_and_denies_others() {
    let (ok, _) = get_with(app(), "/admin", Some("Bearer admin")).await;
    assert_eq!(ok, StatusCode::OK, "RequireAdmin admits the root admin");

    let (denied, body) = get_with(app(), "/admin", Some("Bearer direct")).await;
    assert_eq!(denied, StatusCode::FORBIDDEN, "RequireAdmin rejects a non-admin");
    assert!(body.contains("ADMIN_REQUIRED"), "admin-only 403 body: {body}");

    let (unauth, _) = get_with(app(), "/admin", None).await;
    assert_eq!(unauth, StatusCode::UNAUTHORIZED, "RequireAdmin still 401s a tokenless request");
}
