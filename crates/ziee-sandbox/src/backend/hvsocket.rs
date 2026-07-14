//! AF_HYPERV (Hyper-V vsock) transport between the Windows host and the WSL2
//! utility VM (Plan 1 §3 HIGH-1 fix; grounded in `.sec-audits/wsl2-source-deep-read-2026-05-22.md`).
//!
//! ## Why vsock, not `127.0.0.1:<port>`
//!
//! WSL2 distros share a single network namespace inside the utility VM
//! ([microsoft/WSL#4304], verified in source at
//! `src/linux/init/main.cpp:2283` — the clone flags are
//! `CLONE_NEWNS | CLONE_NEWPID | CLONE_NEWUTS`, no `CLONE_NEWNET`). Any other
//! distro the user has installed can therefore reach our agent on
//! `127.0.0.1:<port>` and submit an arbitrary bwrap argv. AF_VSOCK is
//! **point-to-point** (host ⟷ this-guest), so cross-distro reachability
//! becomes *structurally* impossible — the OS does the auth.
//!
//! The Windows-side hvsocket is additionally per-user-DACL'd by HCS to
//! `D:P(A;;FA;;;SY)(A;;FA;;;<user-sid>)`
//! (`src/windows/service/exe/HcsVirtualMachine.cpp:245-254`), so other Windows
//! users on the same machine can't connect either.
//!
//! ## Address resolution
//!
//! WSL2 runs a **single shared utility VM** for every distro per session, so
//! all flavors share one VmId. We resolve it once via `hcsdiag list` (a system
//! component shipped with Hyper-V) and cache it on the [`Wsl2Backend`]. The env
//! override `ZIEE_WSL_VM_ID=<guid>` short-circuits the probe for dev /
//! diagnostic use.
//!
//! ## Service-ID registration prereq (HOST → guest)
//!
//! Despite Microsoft's docs suggesting `HV_GUID_VSOCK_TEMPLATE` is auto-routable,
//! in practice the Windows host's connect to a port-templated GUID times out
//! with **WSA 10060** unless that specific GUID is registered under
//! `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Virtualization\GuestCommunicationServices\<GUID>`.
//! The vmcompute service caches the registration list at VM-start time, so a
//! fresh registration also requires `wsl --shutdown` before the next VM boot
//! sees it. The bundled `scripts/register-sandbox-vsock-ports.ps1` does both
//! in one admin invocation (registers 10001..10100 + restarts WSL).
//!
//! Symptom of skipped setup: `connect AF_HYPERV port=<N> ... failed: WSA error 10060`.
//! The connect-wait loops in [`super::wsl2::Wsl2Backend::ensure_distro`] and
//! `ensure_test_distro` surface the script path in their timeout error.
//!
//! ## tokio wrapping
//!
//! We open the AF_HYPERV socket synchronously through `windows-sys`, set it
//! nonblocking, then hand the resulting `SOCKET` to
//! `tokio::net::TcpStream::from_std`. tokio's Windows IOCP registration is
//! address-family-agnostic at the WSARecv/WSASend level, so the async path
//! works on a non-TCP socket. TcpStream's address-specific methods
//! (`peer_addr`, `set_nodelay`) would error at runtime, but we only use
//! `AsyncRead` / `AsyncWrite` which delegate to WSARecv/WSASend.

#![cfg(target_os = "windows")]

use std::os::windows::io::FromRawSocket;
use std::ptr;

use windows_sys::core::GUID;
use windows_sys::Win32::Networking::WinSock as ws;

use ziee_core::AppError;

/// `AF_HYPERV` is missing from `windows-sys` 0.59. Microsoft documents it as
/// constant **34**; defined in `<hvsocket.h>`.
const AF_HYPERV: i32 = 34;
/// `HV_PROTOCOL_RAW`, the only Hyper-V socket protocol exposed to user mode.
const HV_PROTOCOL_RAW: i32 = 1;

/// Hyper-V vsock template GUID (`00000000-facb-11e6-bd58-64006a7986d3`). Set
/// `data1` to the desired vsock port to address a specific listener. Per
/// `src/windows/common/hvsocket.cpp:22-29` in microsoft/WSL.
const HV_GUID_VSOCK_TEMPLATE: GUID = GUID {
    data1: 0x00000000,
    data2: 0xfacb,
    data3: 0x11e6,
    data4: [0xbd, 0x58, 0x64, 0x00, 0x6a, 0x79, 0x86, 0xd3],
};

/// `SOCKADDR_HV` — the AF_HYPERV bind/connect address. Not exposed by
/// `windows-sys` 0.59; reproduced exactly from `<hvsocket.h>`.
#[repr(C)]
struct SockaddrHv {
    family: u16,
    reserved: u16,
    vm_id: GUID,
    service_id: GUID,
}

const _: () = {
    // Compile-time sanity that the layout matches the Win32 size (36 bytes:
    // 2 + 2 + 16 + 16).
    assert!(std::mem::size_of::<SockaddrHv>() == 36);
};

/// Connect to the WSL2 utility VM's vsock listener on `port`. Synchronous
/// (the caller wraps with `tokio::net::TcpStream::from_std` to go async).
fn connect_blocking(vm_id: GUID, port: u32) -> Result<i64, AppError> {
    unsafe {
        // WSAStartup is process-wide and idempotent — multiple calls just bump
        // a refcount. Safe to call here even if the rest of the server already
        // initialized winsock (e.g. via reqwest).
        let mut wsa: ws::WSADATA = std::mem::zeroed();
        let r = ws::WSAStartup(0x0202, &mut wsa);
        if r != 0 {
            return Err(AppError::internal_error(format!(
                "WSAStartup failed (Hyper-V): {r}"
            )));
        }

        let sock = ws::WSASocketW(
            AF_HYPERV,
            ws::SOCK_STREAM as i32,
            HV_PROTOCOL_RAW,
            ptr::null_mut(),
            0,
            0,
        );
        if sock == ws::INVALID_SOCKET {
            let err = ws::WSAGetLastError();
            ws::WSACleanup();
            return Err(AppError::internal_error(format!(
                "WSASocket(AF_HYPERV) failed: WSA error {err}"
            )));
        }

        let mut service_id = HV_GUID_VSOCK_TEMPLATE;
        service_id.data1 = port;
        let addr = SockaddrHv {
            family: AF_HYPERV as u16,
            reserved: 0,
            vm_id,
            service_id,
        };

        let r = ws::connect(
            sock,
            &addr as *const _ as *const ws::SOCKADDR,
            std::mem::size_of::<SockaddrHv>() as i32,
        );
        if r == ws::SOCKET_ERROR {
            let err = ws::WSAGetLastError();
            ws::closesocket(sock);
            return Err(AppError::internal_error(format!(
                "connect AF_HYPERV port={port} vm_id={} failed: WSA error {err}",
                fmt_guid(&vm_id)
            )));
        }

        // Switch to non-blocking so tokio can register the SOCKET with IOCP
        // when we hand it to `tokio::net::TcpStream::from_std`.
        let mut nonblocking: u32 = 1;
        let r = ws::ioctlsocket(sock, ws::FIONBIO, &mut nonblocking);
        if r == ws::SOCKET_ERROR {
            let err = ws::WSAGetLastError();
            ws::closesocket(sock);
            return Err(AppError::internal_error(format!(
                "ioctlsocket(FIONBIO) failed: WSA error {err}"
            )));
        }

        Ok(sock as i64)
    }
}

/// Public entry point: dial the WSL2 agent on `vm_id:port` and hand back an
/// async tokio stream. Wraps the AF_HYPERV `SOCKET` via
/// `tokio::net::TcpStream::from_std` — IOCP doesn't care about address family,
/// so the read/write path JustWorks; address-specific TcpStream methods would
/// fail at runtime but we never call them.
pub async fn connect(vm_id: GUID, port: u32) -> Result<tokio::net::TcpStream, AppError> {
    let sock = tokio::task::spawn_blocking(move || connect_blocking(vm_id, port))
        .await
        .map_err(|e| AppError::internal_error(format!("spawn_blocking: {e}")))??;
    // SAFETY: we own `sock` (returned from WSASocketW); the std wrapper takes
    // ownership and closes it on drop, just like a regular TCP socket.
    let std_stream = unsafe { std::net::TcpStream::from_raw_socket(sock as u64) };
    tokio::net::TcpStream::from_std(std_stream)
        .map_err(|e| AppError::internal_error(format!("tokio::TcpStream::from_std (hvsocket): {e}")))
}

/// Resolve the per-session VmId of the WSL2 utility VM. Cached by the caller.
///
/// Strategy:
///   1. `ZIEE_WSL_VM_ID=<guid>` env var (dev / diagnostic override).
///   2. Parse `hcsdiag list` output for a `Wsl` / `LCOWv2` entry.
///
/// Both `hcsdiag` and the HCS API (`HcsEnumerateComputeSystems`) are
/// Hyper-V/Windows components shipped with the OS; `hcsdiag` is the simplest
/// to consume from a child-process and avoids a sync-over-async HCS FFI.
pub fn wsl_utility_vm_id() -> Result<GUID, AppError> {
    if let Ok(s) = std::env::var("ZIEE_WSL_VM_ID") {
        return parse_guid(&s).map_err(|e| {
            AppError::internal_error(format!("ZIEE_WSL_VM_ID malformed: {e}"))
        });
    }

    let out = std::process::Command::new("hcsdiag")
        .arg("list")
        .output()
        .map_err(|e| {
            AppError::internal_error(format!(
                "`hcsdiag list` not runnable ({e}); set ZIEE_WSL_VM_ID=<guid> as a workaround"
            ))
        })?;
    if !out.status.success() {
        return Err(AppError::internal_error(format!(
            "hcsdiag list failed (exit {:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    let text = String::from_utf8_lossy(&out.stdout);
    if let Some(g) = parse_wsl_vm_id_from_hcsdiag(&text) {
        return Ok(g);
    }

    Err(AppError::internal_error(format!(
        "WSL utility VM not present in `hcsdiag list` output. Is WSL2 running? \
         Try `wsl --status` first. Workaround: set ZIEE_WSL_VM_ID=<guid>. \
         Raw output:\n{text}"
    )))
}

/// Extract the WSL utility VM's GUID from `hcsdiag list` output.
///
/// Returns `None` when no block in the output mentions WSL/LCOWv2 or
/// no GUID-shaped token is found in the WSL block. Layout varies by
/// Windows version:
///   - Win11 24H2+ tends to emit one row per VM:
///     `<GUID>, Running, WSL`
///   - Older `hcsdiag` builds split a VM across two rows:
///     `<GUID>\n    VM, Running, <GUID>, WSL`
///
/// Both layouts use blank lines as block separators when multiple VMs
/// are listed. Block-scoping the GUID search keeps us from grabbing
/// the GUID of a *different* Hyper-V VM (e.g. Docker Desktop's
/// data-plane VM, a developer's local guest) when WSL is listed
/// alongside it.
pub(crate) fn parse_wsl_vm_id_from_hcsdiag(text: &str) -> Option<GUID> {
    let normalized = text.replace("\r\n", "\n");
    for block in normalized.split("\n\n") {
        let block_lower = block.to_ascii_lowercase();
        if !(block_lower.contains("wsl") || block_lower.contains("lcowv2")) {
            continue;
        }
        for token in block.split(|c: char| c.is_whitespace() || c == ',') {
            let tok = token.trim();
            if tok.is_empty() {
                continue;
            }
            if let Ok(g) = parse_guid(tok) {
                return Some(g);
            }
        }
    }
    None
}

/// Parse a GUID from either `aabbccdd-eeff-…` or `{aabbccdd-eeff-…}` form.
pub(crate) fn parse_guid(s: &str) -> Result<GUID, String> {
    let s = s.trim().trim_start_matches('{').trim_end_matches('}');
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return Err(format!("expected 5 dash-separated parts, got {}", parts.len()));
    }
    if parts[0].len() != 8 || parts[1].len() != 4 || parts[2].len() != 4
        || parts[3].len() != 4 || parts[4].len() != 12
    {
        return Err(format!(
            "wrong segment lengths in {s:?}; want 8-4-4-4-12 hex characters"
        ));
    }
    let data1 = u32::from_str_radix(parts[0], 16).map_err(|e| e.to_string())?;
    let data2 = u16::from_str_radix(parts[1], 16).map_err(|e| e.to_string())?;
    let data3 = u16::from_str_radix(parts[2], 16).map_err(|e| e.to_string())?;
    let p3 = parts[3];
    let p4 = parts[4];
    let mut data4 = [0u8; 8];
    data4[0] = u8::from_str_radix(&p3[0..2], 16).map_err(|e| e.to_string())?;
    data4[1] = u8::from_str_radix(&p3[2..4], 16).map_err(|e| e.to_string())?;
    for i in 0..6 {
        data4[2 + i] =
            u8::from_str_radix(&p4[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(GUID { data1, data2, data3, data4 })
}

pub(crate) fn fmt_guid(g: &GUID) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_guid_accepts_canonical_and_braced_forms() {
        let canonical = "12345678-9abc-def0-1234-56789abcdef0";
        let braced = "{12345678-9abc-def0-1234-56789abcdef0}";
        let g1 = parse_guid(canonical).unwrap();
        let g2 = parse_guid(braced).unwrap();
        assert_eq!(g1.data1, 0x12345678);
        assert_eq!(g1.data2, 0x9abc);
        assert_eq!(g1.data3, 0xdef0);
        assert_eq!(g1.data4, [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0]);
        assert_eq!(g1.data1, g2.data1);
        assert_eq!(g1.data4, g2.data4);
    }

    #[test]
    fn parse_guid_rejects_malformed_input() {
        assert!(parse_guid("not-a-guid").is_err());
        assert!(parse_guid("12345678-9abc-def0-1234").is_err()); // too few parts
        assert!(parse_guid("12345678-9abc-def0-1234-56789abcdef").is_err()); // wrong length
        assert!(parse_guid("zzzzzzzz-9abc-def0-1234-56789abcdef0").is_err()); // non-hex
    }

    #[test]
    fn fmt_guid_roundtrips_against_parse() {
        let g = parse_guid("a1b2c3d4-e5f6-0789-abcd-ef0123456789").unwrap();
        let s = fmt_guid(&g);
        let g2 = parse_guid(&s).unwrap();
        assert_eq!(g.data1, g2.data1);
        assert_eq!(g.data2, g2.data2);
        assert_eq!(g.data3, g2.data3);
        assert_eq!(g.data4, g2.data4);
    }

    #[test]
    fn vsock_template_data1_is_the_port() {
        // Smoke-check that we're setting the right field. WSL source
        // (hvsocket.cpp:22-29) explicitly assigns the port to ServiceId.data1.
        let mut svc = HV_GUID_VSOCK_TEMPLATE;
        svc.data1 = 1024;
        assert_eq!(svc.data1, 1024);
        assert_eq!(svc.data2, 0xfacb);
        assert_eq!(svc.data3, 0x11e6);
    }

    // ─── hcsdiag output parser regression tests ────────────────────────

    #[test]
    fn parse_wsl_vm_id_single_row_format() {
        // Newer Win11 hcsdiag emits one row per VM.
        let text = "2C340624-1A5B-43CB-BD13-CF3831D8C760, Running, WSL\r\n";
        let g = parse_wsl_vm_id_from_hcsdiag(text).expect("found GUID");
        assert_eq!(g.data1, 0x2C340624);
    }

    #[test]
    fn parse_wsl_vm_id_two_row_format() {
        // Older hcsdiag (observed on this Win11 host): GUID on line 1,
        // metadata on line 2 with leading whitespace.
        let text = "2C340624-1A5B-43CB-BD13-CF3831D8C760\r\n    VM,                       \tRunning, 2C340624-1A5B-43CB-BD13-CF3831D8C760, WSL\r\n";
        let g = parse_wsl_vm_id_from_hcsdiag(text).expect("found GUID");
        assert_eq!(g.data1, 0x2C340624);
    }

    #[test]
    fn parse_wsl_vm_id_picks_wsl_block_over_other_vms() {
        // Multiple VMs: the WSL block's GUID must be picked, NOT the
        // first GUID in the output. Closes the "wrong VM selected"
        // class of bug if the host is also running a Docker Desktop
        // data-plane VM or a Hyper-V dev guest.
        let text = "11111111-aaaa-bbbb-cccc-111111111111, Running, OtherType\n\n\
                    22222222-aaaa-bbbb-cccc-222222222222, Running, WSL\n";
        let g = parse_wsl_vm_id_from_hcsdiag(text).expect("found GUID");
        assert_eq!(g.data1, 0x22222222);
    }

    #[test]
    fn parse_wsl_vm_id_matches_lcowv2_block() {
        // hcsdiag on some Windows versions calls the WSL2 utility VM
        // "LCOWv2" (Linux Containers On Windows v2). Accept either
        // marker so this parser remains robust against the naming
        // change Microsoft has cycled through.
        let text = "3a3a3a3a-bbbb-cccc-dddd-3a3a3a3a3a3a, Running, LCOWv2\r\n";
        let g = parse_wsl_vm_id_from_hcsdiag(text).expect("found GUID");
        assert_eq!(g.data1, 0x3a3a3a3a);
    }

    #[test]
    fn parse_wsl_vm_id_returns_none_without_wsl_marker() {
        // Output that has GUIDs but none in a WSL/LCOWv2 block: no match.
        let text = "11111111-aaaa-bbbb-cccc-111111111111, Running, Docker\n";
        assert!(parse_wsl_vm_id_from_hcsdiag(text).is_none());
    }

    #[test]
    fn parse_wsl_vm_id_returns_none_for_empty_output() {
        assert!(parse_wsl_vm_id_from_hcsdiag("").is_none());
    }

    #[test]
    fn parse_wsl_vm_id_returns_none_when_block_lacks_guid() {
        // Pathological: a block mentions WSL but has no valid GUID.
        // Don't return a garbage-extracted value.
        let text = "    Some status line, WSL\n";
        assert!(parse_wsl_vm_id_from_hcsdiag(text).is_none());
    }
}
