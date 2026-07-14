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
