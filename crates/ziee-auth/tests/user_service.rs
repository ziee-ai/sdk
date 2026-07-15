//! Crate-scoped DB integration test for `ziee-auth`'s `UserService`.
//!
//! `UserService::get_effective_permissions` (user/service.rs) computes a user's
//! effective permission set as the UNION of their direct permissions + the
//! permissions of every ACTIVE group they belong to. It was called by no test.
//! This drives it against a fresh throwaway DB migrated with the crate's OWN
//! `AUTH_MIGRATOR`, complementing the framework's extractor test (which exercises
//! the same union at the request-extraction layer) at the repository layer.

mod common;

use common::{drop_db, fresh_db};
use ziee_auth::user::{GroupRepository, UserRepository, UserService};

#[tokio::test]
async fn effective_permissions_union_direct_and_active_groups() {
    let (pool, db) = fresh_db().await;
    let user_repo = UserRepository::new(pool.clone());
    let group_repo = GroupRepository::new(pool.clone());

    // A user with ONE direct permission and NO default group (plain `create`
    // does not assign the Users group, so the union is unpolluted).
    let user = user_repo
        .create(
            "perm-user",
            "perm-user@corp.com",
            None,
            None,
            Some(vec!["users::read".to_string()]),
        )
        .await
        .expect("create user");

    // An ACTIVE group contributing two permissions.
    let active = group_repo
        .create(
            "active-grp",
            None,
            vec!["projects::read".to_string(), "projects::edit".to_string()],
        )
        .await
        .expect("create active group");
    user_repo
        .assign_to_group(user.id, active.id, None)
        .await
        .expect("assign active group");

    // An INACTIVE group whose permission MUST be excluded from the union.
    let inactive = group_repo
        .create("inactive-grp", None, vec!["secret::admin".to_string()])
        .await
        .expect("create inactive group");
    group_repo
        .update(inactive.id, None, None, None, Some(false))
        .await
        .expect("deactivate group");
    user_repo
        .assign_to_group(user.id, inactive.id, None)
        .await
        .expect("assign inactive group");

    let service = UserService::new(user_repo);
    let perms = service
        .get_effective_permissions(user.id)
        .await
        .expect("effective permissions");

    // Union = direct + active-group perms.
    assert!(perms.contains(&"users::read".to_string()), "direct perm present: {perms:?}");
    assert!(
        perms.contains(&"projects::read".to_string()),
        "active-group perm present: {perms:?}"
    );
    assert!(
        perms.contains(&"projects::edit".to_string()),
        "active-group perm present: {perms:?}"
    );

    // The INACTIVE group's permission is EXCLUDED.
    assert!(
        !perms.contains(&"secret::admin".to_string()),
        "an inactive group's permissions must NOT be in the effective set: {perms:?}"
    );

    // Exactly the three expected perms, deduped + sorted (the service sorts).
    assert_eq!(
        perms,
        vec![
            "projects::edit".to_string(),
            "projects::read".to_string(),
            "users::read".to_string(),
        ],
        "effective set is the sorted union of direct + active-group perms"
    );

    drop_db(&db).await;
}
