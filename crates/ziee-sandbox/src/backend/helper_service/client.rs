//! Client side: the unprivileged `ziee` server dials the helper pipe.
//!
//! Synchronous + blocking — these calls are rare (once per WSL session for
//! VmId) and the callers in `wsl2.rs` are already on a `spawn_blocking`-ish
//! path. Keeping it blocking avoids dragging an async named-pipe client in.

use std::fs::OpenOptions;
use std::io;

use windows_sys::core::GUID;

use super::hvsocket;
use super::protocol::{read_frame, write_frame, Request, Response};
use super::{PIPE_NAME, SERVICE_NAME, VSOCK_PORT_BASE, VSOCK_PORT_COUNT};
use ziee_core::AppError;

/// One request/response round-trip over the helper pipe. Opens the pipe as a
/// regular file (named pipes support `CreateFile` semantics), writes the
/// request frame, reads the response frame.
fn call(req: &Request) -> io::Result<Response> {
    // A named pipe is opened like a file. If the service isn't running the
    // pipe doesn't exist → NotFound, which the callers map to the
    // "service not installed" hard-fail.
    let mut pipe = OpenOptions::new().read(true).write(true).open(PIPE_NAME)?;
    write_frame(&mut pipe, req)?;
    read_frame(&mut pipe)
}

/// Map a pipe-open/IO failure to a clear, actionable AppError. The whole
/// point of "require the service" is that this message tells the operator
/// exactly what to do.
fn not_installed_err(detail: impl std::fmt::Display) -> AppError {
    AppError::internal_error(format!(
        "code sandbox requires the '{SERVICE_NAME}' helper service, which is \
         not reachable ({detail}). Install it once as Administrator:\n  \
         ziee --install-sandbox-helper\n(The Ziee desktop installer normally \
         does this for you.)"
    ))
}

/// Resolve the WSL utility VM's VmId via the helper. Hard-fails with an
/// install instruction when the service pipe is unreachable.
pub fn resolve_vm_id() -> Result<GUID, AppError> {
    match call(&Request::ResolveVmId) {
        Ok(Response::VmId(s)) => hvsocket::parse_guid(&s).map_err(|e| {
            AppError::internal_error(format!("helper returned malformed VmId {s:?}: {e}"))
        }),
        Ok(Response::Error(msg)) => Err(AppError::internal_error(format!(
            "sandbox helper failed to resolve VmId: {msg}"
        ))),
        Ok(other) => Err(AppError::internal_error(format!(
            "sandbox helper returned unexpected response to ResolveVmId: {other:?}"
        ))),
        Err(e) => Err(not_installed_err(e)),
    }
}

/// Ask the helper to ensure the default vsock port range is registered.
/// Returns the count newly added (0 = already present). Primarily used by the
/// install path; exposed here for a runtime self-check if ever needed.
pub fn ensure_registered() -> Result<u32, AppError> {
    match call(&Request::EnsureRegistered {
        port_start: VSOCK_PORT_BASE,
        count: VSOCK_PORT_COUNT,
    }) {
        Ok(Response::Registered { added }) => Ok(added),
        Ok(Response::Error(msg)) => Err(AppError::internal_error(format!(
            "sandbox helper failed to register vsock ports: {msg}"
        ))),
        Ok(other) => Err(AppError::internal_error(format!(
            "sandbox helper returned unexpected response to EnsureRegistered: {other:?}"
        ))),
        Err(e) => Err(not_installed_err(e)),
    }
}

/// Liveness probe — true iff the service answers `Ping` with `Pong`.
pub fn is_running() -> bool {
    matches!(call(&Request::Ping), Ok(Response::Pong))
}
