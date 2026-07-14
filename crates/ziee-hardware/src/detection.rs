use super::types::{GPUComputeCapabilities, GPUDevice, GPUUsage};

/// Resolve a vendor binary by name to its absolute path within trusted
/// system directories. Mirrors the helper in
/// `llm_local_runtime/utils/gpu_detect.rs`. Closes 12-hardware F-06 (Low):
/// every `Command::new("nvidia-smi")` etc inherits the server's PATH —
/// a malicious prefix dir shadows the real binary. We now refuse to
/// spawn from PATH and only try the well-known absolute locations.
fn resolve_system_binary(name: &str) -> Option<std::path::PathBuf> {
    const TRUSTED_DIRS: &[&str] = &[
        "/usr/bin",
        "/usr/sbin",
        "/usr/local/bin",
        "/usr/local/cuda/bin",
        "/opt/rocm/bin",
        "/opt/homebrew/bin",
        "/sbin",
    ];
    for dir in TRUSTED_DIRS {
        let candidate = std::path::PathBuf::from(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Build a `std::process::Command` for the absolute path of a trusted
/// vendor binary, or None when the binary isn't installed under any
/// known location. Callers that previously did
/// `Command::new("nvidia-smi")` should switch to
/// `trusted_command("nvidia-smi")` so PATH isn't consulted.
fn trusted_command(name: &str) -> Option<std::process::Command> {
    resolve_system_binary(name).map(std::process::Command::new)
}

// =====================================================
// GPU Detection
// =====================================================

/// Detect GPU devices and their compute capabilities
pub fn detect_gpu_devices() -> Vec<GPUDevice> {
    let mut gpu_devices = Vec::new();

    // Try to detect GPUs using different methods

    // 1. Try NVIDIA GPUs using NVML
    #[cfg(feature = "gpu-detect")]
    {
        if let Ok(nvidia_gpus) = detect_nvidia_gpus() {
            gpu_devices.extend(nvidia_gpus);
        }
    }

    // 2. Platform-specific fallbacks if no GPUs detected
    if gpu_devices.is_empty() {
        #[cfg(target_os = "macos")]
        {
            gpu_devices.push(GPUDevice {
                device_id: "metal:0".to_string(),
                name: get_apple_chip_name() + " GPU",
                vendor: "Apple".to_string(),
                memory: get_system_total_memory(),
                driver_version: None,
                compute_capabilities: GPUComputeCapabilities {
                    cuda_support: false,
                    cuda_version: None,
                    metal_support: true,
                    opencl_support: check_opencl_support(),
                    vulkan_support: Some(check_vulkan_support()),
                },
            });
        }

        #[cfg(not(target_os = "macos"))]
        {
            gpu_devices.push(GPUDevice {
                device_id: "gpu:0".to_string(),
                name: "GPU Device".to_string(),
                vendor: "Unknown".to_string(),
                memory: None,
                driver_version: None,
                compute_capabilities: GPUComputeCapabilities {
                    cuda_support: check_cuda_support(),
                    cuda_version: get_cuda_version(),
                    metal_support: false,
                    opencl_support: check_opencl_support(),
                    vulkan_support: Some(check_vulkan_support()),
                },
            });
        }
    }

    gpu_devices
}

/// Get GPU usage data using various methods
pub fn get_gpu_usage_data() -> Vec<GPUUsage> {
    let mut gpu_usage = Vec::new();

    // Try NVIDIA GPUs first using NVML
    #[cfg(feature = "gpu-detect")]
    {
        if let Ok(nvidia_usage) = get_nvidia_gpu_usage() {
            gpu_usage.extend(nvidia_usage);
        }
    }

    // Add AMD GPU usage detection
    #[cfg(all(feature = "gpu-detect", target_os = "linux"))]
    {
        if let Ok(amd_usage) = get_amd_gpu_usage() {
            gpu_usage.extend(amd_usage);
        }
    }

    // Add Intel GPU usage detection
    #[cfg(feature = "gpu-detect")]
    {
        if let Ok(intel_usage) = get_intel_gpu_usage() {
            gpu_usage.extend(intel_usage);
        }
    }

    // Add Apple GPU usage detection
    #[cfg(all(feature = "gpu-detect", target_os = "macos"))]
    {
        if let Ok(apple_usage) = get_apple_gpu_usage() {
            gpu_usage.extend(apple_usage);
        }
    }

    gpu_usage
}

// =====================================================
// NVIDIA GPU Detection
// =====================================================

// NVIDIA GPU detection using NVML with nvidia-smi fallback
#[cfg(feature = "gpu-detect")]
fn detect_nvidia_gpus() -> Result<Vec<GPUDevice>, Box<dyn std::error::Error>> {
    let mut gpu_devices = Vec::new();

    // Try NVML first
    match nvml_wrapper::Nvml::init() {
        Ok(nvml) => {
            if let Ok(device_count) = nvml.device_count() {
                for i in 0..device_count {
                    if let Ok(device) = nvml.device_by_index(i) {
                        let name = device.name().unwrap_or_else(|_| "NVIDIA GPU".to_string());
                        let memory = device.memory_info().ok().map(|mem| mem.total);
                        let driver_version = nvml.sys_driver_version().ok();

                        let cuda_version = device
                            .cuda_compute_capability()
                            .ok()
                            .map(|cap| format!("{}.{}", cap.major, cap.minor));

                        gpu_devices.push(GPUDevice {
                            device_id: format!("cuda:{}", i),
                            name,
                            vendor: "NVIDIA".to_string(),
                            memory,
                            driver_version,
                            compute_capabilities: GPUComputeCapabilities {
                                cuda_support: true,
                                cuda_version,
                                metal_support: false,
                                opencl_support: true,
                                vulkan_support: Some(true),
                            },
                        });
                    }
                }
            }
        }
        Err(_) => {
            // NVML failed, try nvidia-smi fallback
            if let Ok(nvidia_gpus) = detect_nvidia_gpus_nvidia_smi() {
                gpu_devices.extend(nvidia_gpus);
            }
        }
    }

    Ok(gpu_devices)
}

// Fallback NVIDIA GPU detection using nvidia-smi
#[cfg(feature = "gpu-detect")]
fn detect_nvidia_gpus_nvidia_smi() -> Result<Vec<GPUDevice>, Box<dyn std::error::Error>> {
    let mut gpu_devices = Vec::new();

    // First, get CUDA version from nvidia-smi header
    let mut cuda_version = None;
    if let Some(mut cmd) = trusted_command("nvidia-smi")
    && let Ok(output) = cmd.output()
        && output.status.success() {
            let output_str = String::from_utf8_lossy(&output.stdout);
            for line in output_str.lines() {
                if line.contains("CUDA Version:")
                    && let Some(version_part) = line.split("CUDA Version:").nth(1) {
                        cuda_version = version_part
                            .split_whitespace()
                            .next()
                            .map(|v| v.to_string());
                        break;
                    }
            }
        }

    // Query GPU information
    if let Some(mut cmd) = trusted_command("nvidia-smi")
    && let Ok(output) = cmd
        .args([
            "--query-gpu=index,name,memory.total,driver_version",
            "--format=csv,noheader,nounits",
        ])
        .output()
        && output.status.success() {
            let output_str = String::from_utf8_lossy(&output.stdout);
            for line in output_str.lines() {
                let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                if parts.len() >= 4 {
                    let index = parts[0].parse::<u32>().unwrap_or(gpu_devices.len() as u32);
                    let name = parts[1].to_string();
                    let memory = parts[2].parse::<u64>().ok().map(|mb| mb * 1024 * 1024);
                    let driver_version = Some(parts[3].to_string());

                    gpu_devices.push(GPUDevice {
                        device_id: format!("cuda:{}", index),
                        name,
                        vendor: "NVIDIA".to_string(),
                        memory,
                        driver_version,
                        compute_capabilities: GPUComputeCapabilities {
                            cuda_support: true,
                            cuda_version: cuda_version.clone(),
                            metal_support: false,
                            opencl_support: true,
                            vulkan_support: Some(true),
                        },
                    });
                }
            }
        }

    Ok(gpu_devices)
}

// Get NVIDIA GPU usage data using NVML
#[cfg(feature = "gpu-detect")]
fn get_nvidia_gpu_usage() -> Result<Vec<GPUUsage>, Box<dyn std::error::Error>> {
    let mut gpu_usage = Vec::new();

    match nvml_wrapper::Nvml::init() {
        Ok(nvml) => {
            let device_count = nvml.device_count()?;

            for i in 0..device_count {
                if let Ok(device) = nvml.device_by_index(i) {
                    let device_name = device.name().unwrap_or_else(|_| "NVIDIA GPU".to_string());

                    let utilization = device.utilization_rates().ok();
                    let memory_info = device.memory_info().ok();
                    let temperature = device
                        .temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu)
                        .ok();
                    let power_usage = device.power_usage().ok().map(|p| p as f32 / 1000.0); // Convert mW to W

                    let (memory_usage_percentage, memory_used, memory_total) =
                        if let Some(mem) = memory_info {
                            let percentage = (mem.used as f32 / mem.total as f32) * 100.0;
                            (Some(percentage), Some(mem.used), Some(mem.total))
                        } else {
                            (None, None, None)
                        };

                    gpu_usage.push(GPUUsage {
                        device_id: format!("cuda:{}", i),
                        device_name,
                        utilization_percentage: utilization.map(|u| u.gpu as f32),
                        memory_used,
                        memory_total,
                        memory_usage_percentage,
                        temperature: temperature.map(|t| t as f32),
                        power_usage,
                    });
                }
            }
        }
        Err(_) => {
            // NVML not available
        }
    }

    Ok(gpu_usage)
}

// =====================================================
// AMD GPU Detection (Linux Only)
// =====================================================

// AMD GPU usage detection (Linux only)
#[cfg(all(feature = "gpu-detect", target_os = "linux"))]
fn get_amd_gpu_usage() -> Result<Vec<GPUUsage>, Box<dyn std::error::Error>> {
    // Method 1: Try rocm-smi (ROCm System Management Interface)
    if let Ok(amd_usage) = get_amd_gpu_usage_rocm_smi()
        && !amd_usage.is_empty() {
            return Ok(amd_usage);
        }

    // Method 2: Fallback to sysfs parsing
    get_amd_gpu_usage_sysfs()
}

// AMD GPU usage detection using rocm-smi
#[cfg(all(feature = "gpu-detect", target_os = "linux"))]
fn get_amd_gpu_usage_rocm_smi() -> Result<Vec<GPUUsage>, Box<dyn std::error::Error>> {
    let mut gpu_usage = Vec::new();

    let Some(mut cmd) = trusted_command("rocm-smi") else {
        return Ok(gpu_usage); // rocm-smi not installed
    };
    let output = cmd
        .args([
            "--showuse",
            "--showmeminfo",
            "--showtemp",
            "--showpower",
            "--csv",
        ])
        .output()?;

    if !output.status.success() {
        return Ok(gpu_usage);
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    for (i, line) in output_str.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue; // Skip header and empty lines
        }

        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() >= 6 {
            let device_name = format!("AMD GPU {}", i);
            let utilization = parts[1].trim_end_matches('%').parse::<f32>().ok();
            let memory_used = parts[2].parse::<u64>().ok().map(|mb| mb * 1024 * 1024);
            let memory_total = parts[3].parse::<u64>().ok().map(|mb| mb * 1024 * 1024);
            let temperature = parts[4].parse::<f32>().ok();
            let power_usage = parts[5].parse::<f32>().ok();

            let memory_usage_percentage =
                if let (Some(used), Some(total)) = (memory_used, memory_total) {
                    Some((used as f32 / total as f32) * 100.0)
                } else {
                    None
                };

            gpu_usage.push(GPUUsage {
                device_id: format!("amd:{}", i - 1), // i-1 because we skip header line
                device_name,
                utilization_percentage: utilization,
                memory_used,
                memory_total,
                memory_usage_percentage,
                temperature,
                power_usage,
            });
        }
    }

    Ok(gpu_usage)
}

// AMD GPU usage detection using sysfs
#[cfg(all(feature = "gpu-detect", target_os = "linux"))]
fn get_amd_gpu_usage_sysfs() -> Result<Vec<GPUUsage>, Box<dyn std::error::Error>> {
    let mut gpu_usage = Vec::new();

    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && name.starts_with("card") && !name.contains("-") {
                    let device_path = format!("/sys/class/drm/{}/device", name);

                    // Check if it's AMD (vendor ID 0x1002)
                    if let Ok(vendor) = std::fs::read_to_string(format!("{}/vendor", device_path))
                        && vendor.trim() == "0x1002" {
                            let device_name =
                                std::fs::read_to_string(format!("{}/device", device_path))
                                    .ok()
                                    .map(|d| format!("AMD GPU {}", d.trim()))
                                    .unwrap_or_else(|| "AMD GPU".to_string());

                            let utilization = std::fs::read_to_string(format!(
                                "{}/gpu_busy_percent",
                                device_path
                            ))
                            .ok()
                            .and_then(|s| s.trim().parse::<f32>().ok());

                            let memory_used = std::fs::read_to_string(format!(
                                "{}/mem_info_vram_used",
                                device_path
                            ))
                            .ok()
                            .and_then(|s| s.trim().parse::<u64>().ok());

                            let memory_total = std::fs::read_to_string(format!(
                                "{}/mem_info_vram_total",
                                device_path
                            ))
                            .ok()
                            .and_then(|s| s.trim().parse::<u64>().ok());

                            let memory_usage_percentage =
                                if let (Some(used), Some(total)) = (memory_used, memory_total) {
                                    Some((used as f32 / total as f32) * 100.0)
                                } else {
                                    None
                                };

                            gpu_usage.push(GPUUsage {
                                device_id: format!("amd:{}", gpu_usage.len()),
                                device_name,
                                utilization_percentage: utilization,
                                memory_used,
                                memory_total,
                                memory_usage_percentage,
                                temperature: None,
                                power_usage: None,
                            });
                        }
                }
        }
    }

    Ok(gpu_usage)
}

// =====================================================
// Intel GPU Detection
// =====================================================

// Intel GPU usage detection (Linux and Windows)
#[cfg(feature = "gpu-detect")]
fn get_intel_gpu_usage() -> Result<Vec<GPUUsage>, Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    let mut gpu_usage: Vec<GPUUsage> = Vec::new();

    #[cfg(not(target_os = "linux"))]
    let gpu_usage: Vec<GPUUsage> = Vec::new();

    #[cfg(target_os = "linux")]
    {
        // Try using intel_gpu_top for Intel GPU monitoring on Linux
        let Some(mut cmd) = trusted_command("intel_gpu_top") else {
            return Ok(gpu_usage);
        };
        match cmd
            .arg("-J") // JSON output
            .arg("-s") // Single sample
            .arg("1000") // 1 second sample
            .output()
        {
            Ok(output) => {
                if output.status.success() {
                    let json_str = String::from_utf8_lossy(&output.stdout);
                    if let Ok(json_data) = serde_json::from_str::<serde_json::Value>(&json_str) {
                        // Parse Intel GPU usage data from JSON
                        let device_name = "Intel GPU".to_string();

                        let utilization = json_data
                            .get("render/3d")
                            .and_then(|v| v.get("busy"))
                            .and_then(|v| v.as_f64())
                            .map(|v| v as f32);

                        gpu_usage.push(GPUUsage {
                            device_id: "intel:0".to_string(),
                            device_name,
                            utilization_percentage: utilization,
                            memory_used: None,
                            memory_total: None,
                            memory_usage_percentage: None,
                            temperature: None,
                            power_usage: None,
                        });
                    }
                }
            }
            Err(_) => {
                // intel_gpu_top not available, try reading from sysfs or other sources
                if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Some(name) = path.file_name().and_then(|n| n.to_str())
                            && name.starts_with("card") && !name.contains("-") {
                                let device_path = format!("/sys/class/drm/{}/device", name);

                                // Check if this is an Intel GPU
                                if let Ok(vendor) =
                                    std::fs::read_to_string(format!("{}/vendor", device_path))
                                    && vendor.trim() == "0x8086" {
                                        // Intel vendor ID
                                        let device_name = "Intel GPU".to_string();

                                        // Intel GPUs don't typically expose detailed usage via sysfs
                                        // This would require more complex integration with Intel's tools
                                        gpu_usage.push(GPUUsage {
                                            device_id: format!("intel:{}", gpu_usage.len()),
                                            device_name,
                                            utilization_percentage: None,
                                            memory_used: None,
                                            memory_total: None,
                                            memory_usage_percentage: None,
                                            temperature: None,
                                            power_usage: None,
                                        });
                                    }
                            }
                    }
                }
            }
        }
    }

    // On Windows, Intel GPU monitoring would require WMI or Performance
    // Counters, which are not yet implemented. Report no device rather than a
    // fabricated all-`None` entry (which the UI would show as a real GPU).

    Ok(gpu_usage)
}

// =====================================================
// Apple GPU Detection (macOS Only)
// =====================================================

// Apple GPU usage detection (macOS only)
#[cfg(all(feature = "gpu-detect", target_os = "macos"))]
fn get_apple_gpu_usage() -> Result<Vec<GPUUsage>, Box<dyn std::error::Error>> {
    let chip_name = get_apple_chip_name();
    let device_name = format!("{} GPU", chip_name);

    // Try activity monitor approach (iokit/IOReport)
    if let Ok(metrics) = get_apple_gpu_usage_iokit() {
        // Calculate memory usage percentage if both values are available
        let memory_usage_percentage =
            if let (Some(used), Some(total)) = (metrics.memory_used, metrics.total_system_memory) {
                if total > 0 {
                    Some((used as f32 / total as f32 * 100.0).min(100.0))
                } else {
                    None
                }
            } else {
                None
            };

        return Ok(vec![GPUUsage {
            device_id: "metal:0".to_string(),
            device_name,
            utilization_percentage: metrics.utilization,
            memory_used: metrics.memory_used,
            memory_total: metrics.total_system_memory,
            memory_usage_percentage,
            temperature: None,
            power_usage: None,
        }]);
    }

    // Fallback: Return basic GPU info without metrics
    Ok(vec![GPUUsage {
        device_id: "metal:0".to_string(),
        device_name,
        utilization_percentage: None,
        memory_used: None,
        memory_total: None,
        memory_usage_percentage: None,
        temperature: None,
        power_usage: None,
    }])
}

// Use iokit to read GPU usage directly
#[cfg(all(feature = "gpu-detect", target_os = "macos"))]
fn get_apple_gpu_usage_iokit() -> Result<AppleGpuMetrics, Box<dyn std::error::Error>> {
    // Try using ioreg to get GPU usage from IOKit (absolute path).
    let mut cmd = trusted_command("ioreg")
        .ok_or_else(|| "ioreg not found in trusted system directories".to_string())?;
    let output = cmd
        .args(&["-c", "AGXAccelerator", "-r", "-d1"])
        .output()?;

    if !output.status.success() {
        return Err("ioreg AGXAccelerator failed".into());
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    Ok(parse_apple_gpu_metrics_iokit(&output_str))
}

// Parse ioreg output for GPU performance statistics
#[cfg(all(feature = "gpu-detect", target_os = "macos"))]
fn parse_apple_gpu_metrics_iokit(output: &str) -> AppleGpuMetrics {
    let mut utilization = None;
    let mut memory_used = None;

    // Get total system memory
    let total_system_memory = get_system_total_memory();

    // Look for PerformanceStatistics in the ioreg output
    for line in output.lines() {
        let line = line.trim();

        // Look for the PerformanceStatistics line which contains the GPU utilization and memory data
        if line.contains("PerformanceStatistics") {
            // Extract Device Utilization %
            if line.contains("Device Utilization %") {
                if let Some(start) = line.find("\"Device Utilization %\"=") {
                    let after_equals = &line[start + 23..];
                    let mut end_pos = 0;
                    for (i, ch) in after_equals.char_indices() {
                        if ch == ',' || ch == '}' || ch.is_whitespace() {
                            end_pos = i;
                            break;
                        }
                    }
                    if end_pos > 0 {
                        let util_str = &after_equals[..end_pos];
                        utilization = util_str.parse::<f32>().ok();
                    }
                }
            }

            // Extract In use system memory (not the driver version)
            // We want the second occurrence, the one without "(driver)" suffix
            if line.contains("\"In use system memory\"=") {
                // Find the last occurrence to get the non-driver version
                if let Some(start) = line.rfind("\"In use system memory\"=") {
                    let after_equals = &line[start + 23..];
                    let mut end_pos = 0;
                    for (i, ch) in after_equals.char_indices() {
                        if ch == ',' || ch == '}' || ch.is_whitespace() {
                            end_pos = i;
                            break;
                        }
                    }
                    if end_pos > 0 {
                        let mem_str = &after_equals[..end_pos];
                        memory_used = mem_str.parse::<u64>().ok();
                    }
                }
            }

            // Extract Alloc system memory
        }
        // Fallback: Look for standalone device utilization lines (older formats)
        else if line.contains("Device Utilization %") && line.contains("=") {
            if let Some(util_str) = line.split('=').nth(1) {
                let cleaned = util_str.trim().trim_end_matches(',').trim_end_matches('}');
                utilization = cleaned.parse::<f32>().ok();
            }
        }
        // Additional fallback for activity percentages
        else if line.contains("Activity") && line.contains('%') {
            if let Some(percent_pos) = line.find('%') {
                let before_percent = &line[..percent_pos];
                if let Some(last_space) = before_percent.rfind(' ') {
                    let util_str = &before_percent[last_space + 1..];
                    if utilization.is_none() {
                        // Only use as fallback
                        utilization = util_str.parse::<f32>().ok();
                    }
                }
            }
        }
    }

    AppleGpuMetrics {
        utilization,
        memory_used,
        total_system_memory,
    }
}

#[cfg(all(feature = "gpu-detect", target_os = "macos"))]
struct AppleGpuMetrics {
    utilization: Option<f32>,
    memory_used: Option<u64>,
    total_system_memory: Option<u64>,
}

// Get Apple chip name using sysctl (faster than system_profiler)
#[cfg(all(feature = "gpu-detect", target_os = "macos"))]
fn get_apple_chip_name() -> String {
    if let Some(mut cmd) = trusted_command("sysctl") {
    if let Ok(output) = cmd
        .args(&["-n", "machdep.cpu.brand_string"])
        .output()
    {
        let cpu_brand = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if cpu_brand.contains("Apple M") {
            for part in cpu_brand.split_whitespace() {
                if part.starts_with("M") && part.chars().nth(1).map_or(false, |c| c.is_numeric()) {
                    let parts: Vec<&str> = cpu_brand.split_whitespace().collect();
                    if let Some(pos) = parts.iter().position(|&x| x == part) {
                        if pos + 1 < parts.len() {
                            let suffix = parts[pos + 1];
                            if suffix == "Pro" || suffix == "Max" || suffix == "Ultra" {
                                return format!("Apple {} {}", part, suffix);
                            }
                        }
                    }
                    return format!("Apple {}", part);
                }
            }
        }
    }
    }
    "Apple Silicon".to_string()
}

// =====================================================
// Helper functions for GPU capability detection
// =====================================================

#[cfg(not(target_os = "macos"))]
fn check_cuda_support() -> bool {
    #[cfg(feature = "gpu-detect")]
    {
        // Check if NVML can initialize (indicates NVIDIA driver presence)
        if let Ok(_nvml) = nvml_wrapper::Nvml::init() {
            return true;
        }
    }

    // Fallback: check for CUDA libraries in system paths
    #[cfg(target_os = "windows")]
    {
        std::path::Path::new("C:\\Program Files\\NVIDIA GPU Computing Toolkit\\CUDA").exists()
    }
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/usr/local/cuda").exists()
            || std::path::Path::new("/opt/cuda").exists()
    }
    #[cfg(target_os = "macos")]
    {
        false // CUDA not supported on modern macOS
    }
}

#[cfg(not(target_os = "macos"))]
fn get_cuda_version() -> Option<String> {
    #[cfg(feature = "gpu-detect")]
    {
        if let Ok(nvml) = nvml_wrapper::Nvml::init()
            && let Ok(version) = nvml.sys_cuda_driver_version() {
                let major = version / 1000;
                let minor = (version % 1000) / 10;
                return Some(format!("{}.{}", major, minor));
            }
    }
    None
}

fn check_opencl_support() -> bool {
    #[cfg(feature = "gpu-detect")]
    {
        use opencl3::platform::get_platforms;

        // Simple check - if we can get platforms, OpenCL is available
        if let Ok(platforms) = get_platforms() {
            return !platforms.is_empty();
        }
    }
    false
}

fn check_vulkan_support() -> bool {
    #[cfg(feature = "gpu-detect")]
    {
        use ash::vk;

        // Try to create a Vulkan instance
        let entry = match unsafe { ash::Entry::load() } {
            Ok(entry) => entry,
            Err(_) => return false,
        };

        let app_info = vk::ApplicationInfo::default()
            .application_name(c"GPU Detection")
            .api_version(vk::make_api_version(0, 1, 0, 0));

        let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);

        match unsafe { entry.create_instance(&create_info, None) } {
            Ok(instance) => {
                // Check for physical devices
                let devices = match unsafe { instance.enumerate_physical_devices() } {
                    Ok(devices) => devices,
                    Err(_) => {
                        unsafe { instance.destroy_instance(None) };
                        return false;
                    }
                };

                let has_gpu = !devices.is_empty();
                unsafe { instance.destroy_instance(None) };
                has_gpu
            }
            Err(_) => false,
        }
    }

    #[cfg(not(feature = "gpu-detect"))]
    false
}

// Get total system memory on macOS
#[cfg(target_os = "macos")]
fn get_system_total_memory() -> Option<u64> {
    let mut cmd = trusted_command("sysctl")?;
    let output = cmd
        .arg("-n")
        .arg("hw.memsize")
        .output()
        .ok()?;

    if output.status.success() {
        let mem_str = String::from_utf8_lossy(&output.stdout);
        mem_str.trim().parse::<u64>().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercise the GPU *detection logic* directly (not just the HTTP wrapper):
    /// `detect_gpu_devices` must never panic and must return well-formed rows —
    /// every device carries a non-empty `device_id` + `name`, and `memory`,
    /// when present, is positive. On a machine with no GPU it returns an empty
    /// Vec, which is a valid (and asserted) outcome.
    #[test]
    fn detect_gpu_devices_returns_well_formed_rows() {
        let devices = detect_gpu_devices();
        for d in &devices {
            assert!(!d.device_id.trim().is_empty(), "device_id non-empty: {d:?}");
            assert!(!d.name.trim().is_empty(), "name non-empty: {d:?}");
            if let Some(mem) = d.memory {
                assert!(mem > 0, "reported memory must be positive: {d:?}");
            }
        }
    }

    /// `get_gpu_usage_data` must likewise be panic-free and well-formed: each
    /// usage row has a non-empty `device_id`, and any present utilization /
    /// memory-usage percentage is within [0, 100].
    #[test]
    fn get_gpu_usage_data_percentages_are_in_range() {
        let usages = get_gpu_usage_data();
        for u in &usages {
            assert!(!u.device_id.trim().is_empty(), "usage device_id non-empty: {u:?}");
            if let Some(p) = u.utilization_percentage {
                assert!((0.0..=100.0).contains(&p), "utilization in range: {u:?}");
            }
            if let Some(p) = u.memory_usage_percentage {
                assert!((0.0..=100.0).contains(&p), "memory % in range: {u:?}");
            }
            if let (Some(used), Some(total)) = (u.memory_used, u.memory_total) {
                assert!(used <= total, "memory_used must not exceed memory_total: {u:?}");
            }
        }
    }
}
