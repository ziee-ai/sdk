//! Linux backend: bwrap runs directly on the host (today's behavior).
//!
//! Thin delegation to the existing functions — `sandbox::run_in_sandbox`,
//! `runtime_mount::{ensure_rootfs_ready, shutdown, evict_flavor}` — so the
//! audited Linux path is byte-identical; the seam only changes *who calls it*.

use std::path::Path;

use async_trait::async_trait;

use super::SandboxBackend;
use ziee_core::AppError;
use crate::sandbox_config::CodeSandboxConfig;
use crate::probes;
use crate::provider::EnsureOutcome;
use crate::sandbox::{self, SandboxRunResult};
use crate::types::{CodeSandboxState, HostCapabilities, SandboxContext};

pub struct LinuxBwrapBackend;

impl LinuxBwrapBackend {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SandboxBackend for LinuxBwrapBackend {
    fn probe_host(&self, cfg: &CodeSandboxConfig) -> Option<HostCapabilities> {
        // Today's behavior: bwrap on PATH + cgroup probe + seccomp-filter compile.
        probes::probe_host_only(cfg)
    }

    async fn ensure_rootfs_ready(
        &self,
        state: &CodeSandboxState,
        flavor: &str,
    ) -> Result<EnsureOutcome, AppError> {
        state.rootfs.ensure_rootfs_ready(flavor).await
    }

    async fn run(
        &self,
        state: &CodeSandboxState,
        ctx: &SandboxContext,
        command: &str,
        timeout_secs: Option<u64>,
        flavor: &str,
    ) -> Result<SandboxRunResult, AppError> {
        sandbox::run_in_sandbox(state, ctx, command, timeout_secs, flavor).await
    }

    async fn run_with_mounts(
        &self,
        state: &CodeSandboxState,
        ctx: &SandboxContext,
        command: &str,
        timeout_secs: Option<u64>,
        flavor: &str,
        extra_mounts: &[crate::workflow_staging::StagedMount],
        progress_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    ) -> Result<SandboxRunResult, AppError> {
        // bwrap runs directly on the host, so the host owns the progress FIFO
        // (created + read inside `run_in_sandbox_with_mounts`). Thread the sink
        // straight through.
        sandbox::run_in_sandbox_with_mounts(
            state,
            ctx,
            command,
            timeout_secs,
            flavor,
            extra_mounts,
            progress_tx,
        )
        .await
    }

    fn supports_extra_mounts(&self) -> bool {
        // bwrap binds host paths directly on the host — host folders and
        // workflow staged dirs both work.
        true
    }

    async fn shutdown(&self) {
        // The squashfuse mount teardown owns the ziee-side `runtime_mount`
        // mount statics, so it stays in the server crate behind the injected
        // `RootfsProvider`; reach it through the engine-owned global state.
        if let Some(state) = crate::config::get_state() {
            state.rootfs.shutdown().await;
        }
    }

    async fn exec_raw_argv(
        &self,
        argv: Vec<String>,
        _rootfs_squashfs: &Path,
        timeout: std::time::Duration,
    ) -> Result<super::RawExecResult, AppError> {
        let output = tokio::time::timeout(
            timeout,
            tokio::process::Command::new("bwrap").args(&argv).output(),
        )
        .await;

        match output {
            Ok(Ok(out)) => Ok(super::RawExecResult {
                exit_code: out.status.code().unwrap_or(-1),
                stdout: out.stdout,
                stderr: out.stderr,
                timed_out: false,
            }),
            Ok(Err(e)) => Err(AppError::internal_error(format!("bwrap spawn failed: {e}"))),
            Err(_) => Ok(super::RawExecResult {
                exit_code: -1,
                stdout: Vec::new(),
                stderr: format!("bwrap timed out after {timeout:?}").into_bytes(),
                timed_out: true,
            }),
        }
    }
}
