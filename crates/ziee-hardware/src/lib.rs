//! `ziee-hardware` — DB-free system hardware detection + real-time monitoring
//! (Chunk `hardware`, build-DB-free).
//!
//! Moved verbatim from ziee's `modules/hardware`: the wire `types` (OS/CPU/mem/
//! GPU info + the `SSEHardwareUsageEvent` enum, via the shared
//! `ziee_core::sse_event_enum!` macro), the GPU/CPU/mem `detection` (trusted
//! vendor-binary resolution + parsing), the SSE `monitoring` broadcaster
//! (client-capped, atomic single-spawn), and the `permissions` keys
//! (`HardwareRead`/`HardwareMonitor`, implementing `ziee_identity::PermissionCheck`).
//!
//! Retained in ziee (bind ziee's concrete permission resolver + `module_api`):
//! the aide/axum `handlers` + `routes` (`RequirePermissions<…>` /
//! `with_permission::<…>` name `ZieeIdentityResolver`) and the
//! `AppModule`/`MODULE_ENTRIES` registration. ziee's `hardware/mod.rs`
//! re-exports `types`/`detection`/`monitoring`/`permissions` from here as
//! equivalence-preserving shims (decision N2), so every `super::…` path in the
//! retained handlers/routes + the `main.rs` shutdown call to
//! `monitoring::stop_hardware_monitoring()` resolve unchanged and the schemars
//! schema keys (short idents) are byte-identical in the emitted OpenAPI.

pub mod detection;
/// Pure parsers for GPU vendor version strings, shared with ziee's
/// `llm_local_runtime::utils::gpu_detect` so the two do not re-diverge.
pub mod gpu_version;
pub mod monitoring;
pub mod permissions;
pub mod types;
