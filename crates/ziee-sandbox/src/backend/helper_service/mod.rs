//! LocalSystem **sandbox-helper service** (Windows only).
//!
//! ## Why this exists
//!
//! The WSL2 code-sandbox backend needs two privileged operations:
//!
//!   1. **Resolve the WSL utility VM's VmId** (`hcsdiag list` / HCS) — needs
//!      Hyper-V Administrators membership, which only attaches to a token at
//!      login (forcing a log-out/in for the interactive user).
//!   2. **Register the `GuestCommunicationServices` vsock GUIDs** under HKLM —
//!      needs Administrator (a privileged registry write) and a one-time
//!      `wsl --shutdown` so vmcompute re-reads the list.
//!
//! Everything else (launching the agent via `wsl.exe`, dialing the AF_HYPERV
//! socket, bwrap, the protocol) runs fine unprivileged — the Windows-side
//! hvsocket is already per-user-DACL'd to the interactive user (see
//! `hvsocket.rs`). So rather than escalate the *whole* server, or push the
//! user through the Hyper-V Admin group + a log-out/in, we broker exactly
//! those two operations through a small **LocalSystem Windows service** —
//! the same architectural move Docker Desktop makes with `com.docker.service`.
//!
//! Result: no runtime UAC, no manual `register-sandbox-vsock-ports.ps1`, and
//! crucially **no log-out/in** — the user's own token never needs a new
//! privileged group, because the privileged work happens in SYSTEM's token.
//!
//! ## Shape
//!
//!   - The service is a hidden subcommand of the `ziee` binary itself
//!     (`ziee --run-sandbox-helper-service`), launched by the SCM. One binary
//!     to ship + sign; reuses `hvsocket`'s VmId resolution directly.
//!   - It exposes a **named pipe** (`\\.\pipe\ziee-sandbox-helper`) whose DACL
//!     grants only SYSTEM + interactive logon users — so a background service
//!     account or a remote session can't poke it.
//!   - Two RPCs only ([`protocol::Request`]): `ResolveVmId`, `EnsureRegistered`.
//!     No arbitrary command execution, no untrusted input beyond a bounded
//!     port range — keeps the SYSTEM attack surface minimal.
//!   - The unprivileged server calls [`client::resolve_vm_id`] from
//!     `wsl2::Wsl2Backend::vm_id`; if the pipe is unreachable it hard-fails
//!     with an install instruction (the sandbox requires the service).
//!
//! ## Install
//!
//!   - `ziee --install-sandbox-helper` (elevated, once): registers the SCM
//!     service as LocalSystem, writes the vsock GUIDs, runs `wsl --shutdown`.
//!     The Tauri Windows installer invokes this at setup; standalone server
//!     deployments run it by hand.
//!   - `ziee --uninstall-sandbox-helper` (elevated): stops + deletes the
//!     service. GUID registrations are left in place (harmless, idempotent).

#![cfg(target_os = "windows")]

pub mod client;
pub mod install;
pub mod ops;
pub mod protocol;
pub mod server;
pub mod service;

// Private alias to the sibling `hvsocket` module (VmId resolution + GUID
// helpers) so the helper's `ops`/`client` can reach it as `super::hvsocket`
// without `super::super` path noise. Must be a private `use` (not
// `pub(crate)`): `hvsocket` is a private `mod` of `backend`, so re-exporting
// it at wider visibility is rejected (E0365).
use super::hvsocket;

/// Windows named pipe the helper listens on. `\\.\pipe\` namespace is
/// machine-local; the per-pipe DACL (set in `server`) is the access gate.
pub const PIPE_NAME: &str = r"\\.\pipe\ziee-sandbox-helper";

/// SCM service name (internal id) and human-facing display name.
pub const SERVICE_NAME: &str = "ZieeSandboxHelper";
pub const SERVICE_DISPLAY_NAME: &str = "Ziee Sandbox Helper";

/// vsock port range the WSL2 backend allocates from. MUST stay in sync with
/// `VSOCK_PORT_BASE` / `VSOCK_PORT_COUNT` in `wsl2.rs` and the doc in
/// `scripts/register-sandbox-vsock-ports.ps1`.
pub const VSOCK_PORT_BASE: u32 = 10001;
pub const VSOCK_PORT_COUNT: u32 = 100;
