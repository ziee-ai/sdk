use ziee_identity::PermissionCheck;

// =====================================================
// Hardware Module Permissions
// =====================================================

/// Permission to view hardware information
pub struct HardwareRead;
impl PermissionCheck for HardwareRead {
    const NAME: &'static str = "HardwareRead";
    const PERMISSION: &'static str = "hardware::read";
    const DESCRIPTION: &'static str = "View hardware information";
    const MODULE: &'static str = "hardware";
}

/// Permission to monitor real-time hardware usage
pub struct HardwareMonitor;
impl PermissionCheck for HardwareMonitor {
    const NAME: &'static str = "HardwareMonitor";
    const PERMISSION: &'static str = "hardware::monitor";
    const DESCRIPTION: &'static str = "Monitor real-time hardware usage";
    const MODULE: &'static str = "hardware";
}

// =====================================================
// Helper Function to Collect All Permissions
// =====================================================

#[cfg(test)]
mod tests {
    use super::{HardwareMonitor, HardwareRead};
    use ziee_identity::{PermissionCheck, PermissionList};

    /// The two hardware permission keys feed the OpenAPI 403 example (which the
    /// UI `Permissions` enum is scraped from) via `RequirePermissions`/
    /// `with_permission`. Pin the string + name + module + derived resource/
    /// action so a drift can't silently drop them from the generated enum.
    #[test]
    fn hardware_permission_keys_are_stable() {
        assert_eq!(HardwareRead::PERMISSION, "hardware::read");
        assert_eq!(HardwareRead::NAME, "HardwareRead");
        assert_eq!(HardwareRead::MODULE, "hardware");
        assert_eq!(HardwareRead::resource(), "hardware");
        assert_eq!(HardwareRead::action(), "read");

        assert_eq!(HardwareMonitor::PERMISSION, "hardware::monitor");
        assert_eq!(HardwareMonitor::NAME, "HardwareMonitor");
        assert_eq!(HardwareMonitor::action(), "monitor");
    }

    /// A 2-tuple of the hardware keys surfaces the complete, ordered permission
    /// set the extractor AND-checks.
    #[test]
    fn permission_list_tuple_collects_both() {
        type Both = (HardwareRead, HardwareMonitor);
        assert_eq!(
            <Both as PermissionList>::permissions(),
            vec!["hardware::read", "hardware::monitor"]
        );
        assert_eq!(
            <Both as PermissionList>::names(),
            vec!["HardwareRead", "HardwareMonitor"]
        );
    }
}
