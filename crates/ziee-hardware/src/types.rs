use schemars::JsonSchema;
use serde::Serialize;

// =====================================================
// Hardware Information Structures
// =====================================================

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct OperatingSystemInfo {
    pub name: String,
    pub version: String,
    pub kernel_version: Option<String>,
    pub architecture: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CPUInfo {
    pub model: String,
    pub architecture: String,
    pub cores: usize,
    pub threads: Option<usize>,
    pub base_frequency: Option<u64>,
    pub max_frequency: Option<u64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MemoryInfo {
    pub total_ram: u64,
    pub total_swap: Option<u64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GPUComputeCapabilities {
    pub cuda_support: bool,
    pub cuda_version: Option<String>,
    pub metal_support: bool,
    pub opencl_support: bool,
    pub vulkan_support: Option<bool>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GPUDevice {
    pub device_id: String, // e.g., "cuda:0", "metal:0", "opencl:0"
    pub name: String,
    pub vendor: String,
    pub memory: Option<u64>,
    pub driver_version: Option<String>,
    pub compute_capabilities: GPUComputeCapabilities,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HardwareInfo {
    pub operating_system: OperatingSystemInfo,
    pub cpu: CPUInfo,
    pub memory: MemoryInfo,
    pub gpu_devices: Vec<GPUDevice>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HardwareInfoResponse {
    pub hardware: HardwareInfo,
}

// =====================================================
// Real-time Usage Structures
// =====================================================

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CPUUsage {
    pub usage_percentage: f32,
    pub temperature: Option<f32>,
    pub frequency: Option<u64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MemoryUsage {
    pub used_ram: u64,
    pub available_ram: u64,
    pub used_swap: Option<u64>,
    pub available_swap: Option<u64>,
    pub usage_percentage: f32,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GPUUsage {
    pub device_id: String,
    pub device_name: String,
    pub utilization_percentage: Option<f32>,
    pub memory_used: Option<u64>,
    pub memory_total: Option<u64>,
    pub memory_usage_percentage: Option<f32>,
    pub temperature: Option<f32>,
    pub power_usage: Option<f32>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HardwareUsageUpdate {
    pub timestamp: String,
    pub cpu: CPUUsage,
    pub memory: MemoryUsage,
    pub gpu_devices: Vec<GPUUsage>,
}

// =====================================================
// SSE Event Structures
// =====================================================

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SSEHardwareUsageConnectedData {
    pub message: String,
}

// SSE event types for hardware usage monitoring
ziee_core::sse_event_enum! {
    #[derive(Debug, Clone, Serialize, JsonSchema)]
    pub enum SSEHardwareUsageEvent {
        Connected(SSEHardwareUsageConnectedData),
        Update(HardwareUsageUpdate),
    }
}
