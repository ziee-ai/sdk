//! Permission key for the notification inbox.

use ziee_identity::PermissionCheck;

/// Read + manage YOUR OWN notifications (list / unread-count / mark-read /
/// read-all / delete). Granted to the default Users group by migration 142.
///
/// The inbox is strictly per-user (every query is `WHERE user_id = $1`), so the
/// same permission covers the reads and the per-user mutations — there is no
/// cross-tenant exposure to gate separately (mirrors the citations use/manage
/// rationale).
pub struct NotificationsRead;
impl PermissionCheck for NotificationsRead {
    const NAME: &'static str = "NotificationsRead";
    const PERMISSION: &'static str = "notifications::read";
    const DESCRIPTION: &'static str = "Read and manage your own notifications.";
    const MODULE: &'static str = "notification";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_constants_are_stable() {
        // The PERMISSION string is a wire contract: the migration-142 grant, the
        // handler `RequirePermissions<(NotificationsRead,)>`, and the FE refetch
        // self-gate must all agree on `notifications::read`.
        assert_eq!(NotificationsRead::NAME, "NotificationsRead");
        assert_eq!(NotificationsRead::PERMISSION, "notifications::read");
        assert_eq!(NotificationsRead::MODULE, "notification");
        assert!(!NotificationsRead::DESCRIPTION.is_empty());
    }
}
