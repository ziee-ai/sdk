//! `ziee-sandbox-vm-launcher` — boots one libkrun microVM and *becomes* it.
//!
//! The server spawns this as a child process (one per warm flavor VM), passing
//! a JSON [`VmLaunchConfig`] file path as argv[1]. On macOS it configures a
//! libkrun context and calls `krun_start_enter`, which **never returns on
//! success** — the calling process turns into the VM and `exit()`s with the
//! guest's exit code. That's exactly why this lives in a separate process
//! rather than a `fork()` inside the server's multithreaded tokio runtime
//! (only async-signal-safe calls are legal post-fork; libkrun is far from it).
//!
//! The guest entrypoint is the `ziee-sandbox-agent` binary (see
//! `sandbox-guest-agent`), which mounts the squashfs + workspace and serves
//! exec requests over vsock. The server reaches the agent through the unix
//! socket libkrun bridges to the guest vsock port.

use serde::{Deserialize, Serialize};

/// Everything the launcher needs to stand up one VM. Written by the server as
/// a temp JSON file; path passed as argv[1].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmLaunchConfig {
    pub num_vcpus: u8,
    pub ram_mib: u32,
    /// Host directory used as the guest root (contains the agent + bwrap + the
    /// minimal userland). Shared via libkrun's root virtio-fs.
    pub root_path: String,
    /// Host path to the sandbox **squashfs** for this flavor; added as a
    /// read-only virtio-blk disk (the agent mounts it at `/sandbox-rootfs`).
    pub sandbox_disk_path: String,
    /// Host workspace root, shared into the guest via virtio-fs tag `workspace`.
    pub workspace_host_path: String,
    /// Unix socket libkrun bridges to the guest vsock port. The server connects
    /// here to talk to the agent.
    pub vsock_socket_path: String,
    /// Guest vsock port the agent listens on.
    pub vsock_port: u32,
    /// Absolute path to the agent binary *inside the guest root*.
    pub agent_exec_path: String,
    /// Extra host folders shared read-only into the guest (feature #3, host
    /// mounts). Ordered: index `i` is shared under virtio-fs tag
    /// `host-mount-<i>`; the agent mounts it at `/host-mounts/<i>` and the
    /// server's bwrap argv binds it to `/mnt/<full host path>`. Empty for
    /// every exec without host mounts (byte-identical boot to before).
    #[serde(default)]
    pub extra_mounts: Vec<String>,
}

/// Tag prefix for host-folder virtio-fs shares. MUST match the agent's
/// `EXTRA_MOUNT_TAG_PREFIX` and mac_vm.rs's `GUEST_EXTRA_MOUNT_TAG_PREFIX`.
/// Only referenced from the macOS libkrun path (its sole use-site is inside a
/// `#[cfg(target_os = "macos")]` block), so gate the const to match and avoid a
/// dead-code warning on Linux/Windows builds.
#[cfg(target_os = "macos")]
const EXTRA_MOUNT_TAG_PREFIX: &str = "host-mount-";

fn main() {
    let cfg_path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: ziee-sandbox-vm-launcher <config.json>");
        std::process::exit(2);
    });
    let cfg: VmLaunchConfig = {
        let bytes = std::fs::read(&cfg_path).unwrap_or_else(|e| {
            eprintln!("launcher: cannot read config {cfg_path}: {e}");
            std::process::exit(2);
        });
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            eprintln!("launcher: bad config json: {e}");
            std::process::exit(2);
        })
    };

    run(cfg);
}

#[cfg(target_os = "macos")]
fn run(cfg: VmLaunchConfig) -> ! {
    use std::ffi::CString;
    use std::os::raw::c_char;

    // libkrun FFI — signatures verified against containers/libkrun
    // include/libkrun.h. `krun_start_enter` only returns on a pre-boot error;
    // on success it exec/exits with the workload's code.
    #[link(name = "krun")]
    extern "C" {
        fn krun_create_ctx() -> i32;
        fn krun_set_vm_config(ctx_id: u32, num_vcpus: u8, ram_mib: u32) -> i32;
        // virtio-fs root with a read_only flag (gap #2): the guest root (agent +
        // bwrap) is mounted read-only so an escaped guest can't tamper it or
        // persist into the shared host dir for future VMs. Uses KRUN_FS_ROOT_TAG.
        fn krun_add_virtiofs3(
            ctx_id: u32,
            c_tag: *const c_char,
            c_path: *const c_char,
            shm_size: u64,
            read_only: bool,
        ) -> i32;
        fn krun_add_disk(
            ctx_id: u32,
            block_id: *const c_char,
            disk_path: *const c_char,
            read_only: bool,
        ) -> i32;
        fn krun_add_virtiofs(ctx_id: u32, c_tag: *const c_char, c_path: *const c_char) -> i32;
        fn krun_add_vsock_port2(
            ctx_id: u32,
            port: u32,
            c_filepath: *const c_char,
            listen: bool,
        ) -> i32;
        fn krun_set_exec(
            ctx_id: u32,
            exec_path: *const c_char,
            argv: *const *const c_char,
            envp: *const *const c_char,
        ) -> i32;
        fn krun_start_enter(ctx_id: u32) -> i32;
    }

    fn cstr(s: &str) -> CString {
        // A path/arg with an interior NUL is malformed and unrecoverable for a
        // VM launch; exit cleanly with a clear message rather than panicking.
        CString::new(s).unwrap_or_else(|_| {
            eprintln!("launcher: argument contains an interior NUL byte: {s:?}");
            std::process::exit(1)
        })
    }
    fn check(what: &str, rc: i32) {
        if rc < 0 {
            eprintln!("launcher: {what} failed (rc={rc})");
            std::process::exit(1);
        }
    }

    // libkrun's KRUN_FS_ROOT_TAG (the magic virtio-fs tag that becomes `/`).
    const KRUN_FS_ROOT_TAG: &str = "/dev/root";

    // Preload libkrunfw from the bundled lib/ next to this binary. libkrun
    // dlopens it with the bare leafname "libkrunfw.5.dylib", and macOS's
    // bare-leaf dlopen does NOT search @rpath — only DYLD_*_LIBRARY_PATH
    // and the system paths. In a self-contained bundle where libkrunfw
    // lives at <exe>/../lib/, libkrun's later dlopen would fail. Preloading
    // with an absolute path puts libkrunfw into the process address space;
    // libkrun's subsequent bare-leaf dlopen returns the already-loaded
    // handle. Skipped silently when running from a brew-installed libkrun
    // (system dlopen path still resolves the leaf).
    preload_libkrunfw();

    // Audit H-6: macOS has no PR_SET_PDEATHSIG, so we run a side thread
    // that watches the parent PID and `std::process::exit`s the launcher
    // (which tears down the VM) the moment the parent goes away.
    //
    // **NOTE — kqueue conflicts with libkrun's vsock proxy on macOS.**
    // The previous implementation used a `kqueue()` + `EVFILT_PROC` watch
    // in a side thread spawned BEFORE `krun_start_enter`. When that
    // thread was alive concurrently with libkrun, the host-side socket
    // path passed to `krun_add_vsock_port2(listen=true)` accepted
    // `connect(2)` from inside the launcher process but rejected
    // connects from any external process (including the server that
    // spawned the launcher) with ENOENT — libkrun's vsock-to-unix
    // bridge silently misroutes when another kqueue fd is open in the
    // same process. Reproduced with a minimal C program: with the
    // kqueue thread removed, `connect()` from `nc -U` succeeds in <1s.
    //
    // Until libkrun upstream is debugged for the kqueue interaction,
    // fall back to a tight 100 ms `getppid()` poll (the original
    // pre-kqueue defense). The latency window expands from
    // milliseconds → 100 ms, but the VM still tears down promptly when
    // the parent dies, and vsock works.
    {
        let initial_ppid = unsafe { libc::getppid() };
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let ppid = unsafe { libc::getppid() };
            if ppid != initial_ppid || ppid == 1 {
                eprintln!(
                    "launcher: parent (server, pid={initial_ppid}) exited or reparented to {ppid}; tearing down VM"
                );
                // Use `_exit`, NOT `std::process::exit`. We're on a side
                // thread while the MAIN thread is blocked inside libkrun's
                // `krun_start_enter` (the VM run loop). `std::process::exit`
                // runs the libc/Rust runtime shutdown (atexit handlers,
                // static destructors) — libkrun holds VM state + locks on the
                // main thread, so that orderly teardown DEADLOCKS in cleanup
                // and the process survives at PPID 1 with the in-process
                // Hypervisor.framework VM + multi-GB guest RAM still mapped
                // (the orphaned-VM RAM leak on killed/OOM'd parents).
                // `_exit` terminates immediately with no cleanup; the kernel
                // reclaims the whole task address space — including the VM —
                // so it can't hang. Workspace dirs are reclaimed by the
                // server's startup sweep on next boot.
                unsafe { libc::_exit(0) };
            }
        });
    }

    unsafe {
        let ctx = krun_create_ctx();
        check("krun_create_ctx", ctx);
        let ctx = ctx as u32;

        check("krun_set_vm_config", krun_set_vm_config(ctx, cfg.num_vcpus, cfg.ram_mib));

        // Read-only guest root via virtio-fs (gap #2). shm_size=0 disables the
        // DAX window (simpler; tune later if needed). The guest root image must
        // pre-create the mount points /proc, /sandbox-rootfs, /workspace since
        // the root is read-only.
        let root = cstr(&cfg.root_path);
        let root_tag = cstr(KRUN_FS_ROOT_TAG);
        check(
            "krun_add_virtiofs3(root, ro)",
            krun_add_virtiofs3(ctx, root_tag.as_ptr(), root.as_ptr(), 0, true),
        );

        // Sandbox squashfs as a read-only virtio-blk disk → /dev/vda in guest.
        let block_id = cstr("sandbox");
        let disk = cstr(&cfg.sandbox_disk_path);
        check("krun_add_disk", krun_add_disk(ctx, block_id.as_ptr(), disk.as_ptr(), true));

        // Workspace via virtio-fs (tag must match the agent's WORKSPACE_TAG).
        let tag = cstr("workspace");
        let ws = cstr(&cfg.workspace_host_path);
        check("krun_add_virtiofs", krun_add_virtiofs(ctx, tag.as_ptr(), ws.as_ptr()));

        // Extra host-folder shares (feature #3): each gets a read-only virtio-fs
        // device under tag `host-mount-<i>`. Keep the CStrings alive until
        // krun_start_enter. Empty for every exec without host mounts.
        let mut extra_cstrs: Vec<(std::ffi::CString, std::ffi::CString)> = Vec::new();
        for (i, host_path) in cfg.extra_mounts.iter().enumerate() {
            let mtag = cstr(&format!("{EXTRA_MOUNT_TAG_PREFIX}{i}"));
            let mpath = cstr(host_path);
            check(
                "krun_add_virtiofs3(host-mount, ro)",
                krun_add_virtiofs3(ctx, mtag.as_ptr(), mpath.as_ptr(), 0, true),
            );
            extra_cstrs.push((mtag, mpath));
        }

        // Bridge: libkrun listens on the host unix socket and forwards incoming
        // host connections to the guest vsock port (where the agent listens).
        // NOTE: confirm the `listen` semantics against the installed libkrun
        // version on first Mac run — flip if the server can't connect.
        let sock = cstr(&cfg.vsock_socket_path);
        check(
            "krun_add_vsock_port2",
            krun_add_vsock_port2(ctx, cfg.vsock_port, sock.as_ptr(), true),
        );

        // Run the agent as the guest entrypoint (PID 1).
        let exec = cstr(&cfg.agent_exec_path);
        let argv0 = cstr(&cfg.agent_exec_path);
        let argv: [*const c_char; 2] = [argv0.as_ptr(), std::ptr::null()];
        // Tell the agent how many host-folder shares to mount (host-mount-0..N-1).
        // Cleaner + safer than the agent probing tags until one fails.
        let env_extra = cstr(&format!("ZIEE_EXTRA_MOUNTS={}", cfg.extra_mounts.len()));
        let envp: [*const c_char; 2] = [env_extra.as_ptr(), std::ptr::null()];
        check(
            "krun_set_exec",
            krun_set_exec(ctx, exec.as_ptr(), argv.as_ptr(), envp.as_ptr()),
        );

        // Becomes the VM. Only returns on a pre-boot error.
        let rc = krun_start_enter(ctx);
        eprintln!("launcher: krun_start_enter returned unexpectedly (rc={rc})");
        std::process::exit(if rc < 0 { 1 } else { 0 });
    }
}

/// macOS: load `<exe-dir>/../lib/libkrunfw.5.dylib` with an absolute path
/// so libkrun's later bare-leaf `dlopen("libkrunfw.5.dylib")` finds it.
/// See the comment block at the call site for why this is necessary.
///
/// Best-effort by design: when running against a brew-installed libkrun
/// (dev path, no bundled lib/), there's nothing at `../lib/` and we just
/// let libkrun's dlopen find libkrunfw via the system search path.
#[cfg(target_os = "macos")]
fn preload_libkrunfw() {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};

    extern "C" {
        fn _NSGetExecutablePath(buf: *mut c_char, sz: *mut u32) -> c_int;
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        fn dlerror() -> *const c_char;
    }
    const RTLD_NOW: c_int = 0x2;
    const RTLD_GLOBAL: c_int = 0x8;

    let mut buf = vec![0u8; 4096];
    let mut sz = buf.len() as u32;
    let rc = unsafe { _NSGetExecutablePath(buf.as_mut_ptr() as *mut c_char, &mut sz) };
    if rc != 0 {
        return;
    }
    let nul = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let exe = std::path::PathBuf::from(std::str::from_utf8(&buf[..nul]).unwrap_or(""));
    let candidate = match exe.parent().and_then(|d| d.parent()) {
        Some(bundle_root) => bundle_root.join("lib").join("libkrunfw.5.dylib"),
        None => return,
    };
    if !candidate.exists() {
        // Not a bundled install — likely the brew/dev path. libkrun will
        // find libkrunfw via the system search path.
        return;
    }
    let c_path = match CString::new(candidate.as_os_str().as_encoded_bytes()) {
        Ok(s) => s,
        Err(_) => return,
    };
    let handle = unsafe { dlopen(c_path.as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
    if handle.is_null() {
        let err = unsafe {
            let p = dlerror();
            if p.is_null() { String::from("(no dlerror)") }
            else { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() }
        };
        eprintln!("launcher: preload of {} failed: {err}", candidate.display());
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn run(_cfg: VmLaunchConfig) -> ! {
    eprintln!("ziee-sandbox-vm-launcher is only supported on macOS (libkrun)");
    std::process::exit(1);
}
