//! Service side: create the DACL-restricted named pipe and serve requests.
//!
//! Runs inside the LocalSystem service process. One client at a time (calls
//! are rare), each connection handled then closed.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::FromRawHandle;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW};

use super::ops;
use super::protocol::{read_frame, write_frame, Request, Response};
use super::PIPE_NAME;

// Inlined Win32 constants (avoids feature/path churn across windows-sys
// minor versions; values are stable ABI from <winbase.h> / <sddl.h>).
const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
const PIPE_WAIT: u32 = 0x0000_0000;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
const ERROR_PIPE_CONNECTED: u32 = 535;
const SDDL_REVISION_1: u32 = 1;
const PIPE_BUF: u32 = 64 * 1024;

/// DACL: protected (`P`, no inheritance), grant Generic All to SYSTEM (`SY`)
/// and to Interactive logon users (`IU`). A service account or a network
/// logon therefore cannot open the pipe — only the locally-logged-in user
/// (who is who the desktop `ziee` server runs as) and SYSTEM itself.
const PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;IU)";

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Build a `SECURITY_ATTRIBUTES` from [`PIPE_SDDL`]. The returned descriptor
/// pointer is allocated by the OS (LocalAlloc); we intentionally leak it —
/// it's built once for the lifetime of the service.
fn build_security_attributes() -> io::Result<SECURITY_ATTRIBUTES> {
    let sddl_w = to_wide(PIPE_SDDL);
    let mut psd: *mut c_void = ptr::null_mut();
    // SAFETY: sddl_w is a valid NUL-terminated UTF-16 buffer; psd receives an
    // OS-allocated security descriptor on success.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_w.as_ptr(),
            SDDL_REVISION_1,
            &mut psd,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::from_raw_os_error(unsafe { GetLastError() } as i32));
    }
    Ok(SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: psd,
        bInheritHandle: 0,
    })
}

/// Serve forever. Returns only on a fatal pipe-creation error; the SCM
/// control handler stops the process on Stop. `stop` lets a future graceful
/// path break the loop between connections.
pub fn serve(stop: Arc<AtomicBool>) -> io::Result<()> {
    let name_w = to_wide(PIPE_NAME);
    let mut sa = build_security_attributes()?;

    while !stop.load(Ordering::Relaxed) {
        // SAFETY: name_w is NUL-terminated; sa holds a valid security
        // descriptor for the pipe's lifetime.
        let handle = unsafe {
            CreateNamedPipeW(
                name_w.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUF,
                PIPE_BUF,
                0,
                &mut sa,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::from_raw_os_error(unsafe { GetLastError() } as i32));
        }

        // Block until a client connects. ERROR_PIPE_CONNECTED means a client
        // raced in between Create and Connect — also a success.
        let connected = unsafe { ConnectNamedPipe(handle, ptr::null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if !connected {
            unsafe { CloseHandle(handle) };
            continue;
        }

        // Wrap the handle as a File for framed read/write. On drop the handle
        // is closed, which ends the connection (client sees EOF) — so we do
        // NOT also CloseHandle here.
        // SAFETY: handle is a valid, connected pipe instance we own.
        let mut conn = unsafe { File::from_raw_handle(handle as _) };
        if let Err(e) = handle_one(&mut conn) {
            // Best-effort: log to stderr (captured by the service host). A
            // single bad client must not take the service down.
            eprintln!("ziee-sandbox-helper: connection error: {e}");
        }
        // conn drops here → handle closed.
    }
    Ok(())
}

/// Read one request, dispatch to the privileged op, write one response.
fn handle_one(conn: &mut File) -> io::Result<()> {
    let req: Request = read_frame(conn)?;
    let resp = match req {
        Request::Ping => Response::Pong,
        Request::ResolveVmId => match ops::resolve_vm_id() {
            Ok(s) => Response::VmId(s),
            Err(e) => Response::Error(e),
        },
        Request::EnsureRegistered { port_start, count } => {
            match ops::ensure_registered(port_start, count) {
                Ok(added) => Response::Registered { added },
                Err(e) => Response::Error(e),
            }
        }
    };
    write_frame(conn, &resp)
}
