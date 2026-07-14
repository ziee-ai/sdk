//! Permission key for the server_update module.
//!
//! Admin-only — Administrators have `server_update::read` implicitly via the
//! `*` wildcard, so no grant migration is needed (same as the code_sandbox
//! resource-limits permissions). The `<module>::<action>` prefix matches every
//! peer module (code_sandbox::, hardware::, …).

use ziee_identity::PermissionCheck;

/// Read the cached server update-availability status.
pub struct ServerUpdateRead;

impl PermissionCheck for ServerUpdateRead {
    const NAME: &'static str = "ServerUpdateRead";
    const PERMISSION: &'static str = "server_update::read";
    const DESCRIPTION: &'static str = "View the cached server update-availability status.";
    const MODULE: &'static str = "server_update";
}

#[cfg(test)]
mod tests {
    use super::ServerUpdateRead;
    use ziee_identity::{PermissionCheck, PermissionList};

    /// `get_update_status_docs` calls `with_permission::<(ServerUpdateRead,)>`,
    /// which builds the OpenAPI 403 example from the tuple's
    /// names()/permissions()/descriptions() (permissions/openapi.rs:53-86). The
    /// UI `Permissions` enum is scraped from THAT example, so this pins the data
    /// feeding it (gap 1268a9a). A drift here silently drops `ServerUpdateRead`
    /// from the generated enum.
    #[test]
    fn server_update_403_example_carries_the_read_permission() {
        assert_eq!(
            <(ServerUpdateRead,) as PermissionList>::permissions(),
            vec!["server_update::read"],
            "403 example must embed the server_update::read value"
        );
        assert_eq!(
            <(ServerUpdateRead,) as PermissionList>::names(),
            vec!["ServerUpdateRead"],
            "403 example must embed the ServerUpdateRead name (UI enum variant)"
        );
        let descs = <(ServerUpdateRead,) as PermissionList>::descriptions();
        assert_eq!(descs.len(), 1);
        assert_eq!(descs[0], ServerUpdateRead::DESCRIPTION);
    }
}
