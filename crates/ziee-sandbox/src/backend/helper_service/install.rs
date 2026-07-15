//! Install / uninstall the LocalSystem helper as an SCM service.
//!
//! [`install`] is designed to be **safe to call on every app launch**: it
//! does an unprivileged "already registered?" check first and returns
//! silently if so — no UAC. Only when the service is genuinely missing does
//! it self-elevate (one UAC prompt) to do the privileged work. That lets the
//! desktop app run `ziee --install-sandbox-helper` at startup without
//! prompting users repeatedly, and without baking it into an installer step.

use std::ffi::{OsStr, OsString};
use std::process::Command;
use std::time::Duration;

use windows_service::service::{
    ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::ops;
use super::{SERVICE_DISPLAY_NAME, SERVICE_NAME, VSOCK_PORT_BASE, VSOCK_PORT_COUNT};

/// SCM error: the named service is not installed.
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
/// SCM error: create_service when it already exists.
const ERROR_SERVICE_EXISTS: i32 = 1073;
/// SCM error: start on an already-running service.
const ERROR_SERVICE_ALREADY_RUNNING: i32 = 1056;

/// Idempotent, UAC-free-when-installed entry point.
///
/// `no_reelevate` is set on the elevated child we spawn for the actual
/// install, so it never tries to elevate again (no UAC loop).
///
///   1. Registered already? → silent `Ok` (the every-launch fast path).
///   2. Missing + we're elevated → do the privileged install.
///   3. Missing + not elevated → relaunch self elevated (one UAC), wait.
pub fn install(no_reelevate: bool) -> Result<(), Box<dyn std::error::Error>> {
    if is_registered() {
        // Already installed — nothing to do, no privilege needed. Stay quiet
        // so an every-launch caller produces no console noise.
        return Ok(());
    }

    if is_elevated() {
        return do_install();
    }

    if no_reelevate {
        return Err(format!(
            "'{SERVICE_DISPLAY_NAME}' is not installed and this process is not \
             elevated. Run `ziee --install-sandbox-helper` as Administrator."
        )
        .into());
    }

    relaunch_elevated()
}

/// Unprivileged check: is the service registered with the SCM? `CONNECT` +
/// `QUERY_STATUS` are available to standard users, so this never triggers UAC.
pub fn is_registered() -> bool {
    let Ok(mgr) =
        ServiceManager::local_computer(None::<&OsStr>, ServiceManagerAccess::CONNECT)
    else {
        return false;
    };
    match mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
        Ok(_) => true,
        Err(windows_service::Error::Winapi(e))
            if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST) =>
        {
            false
        }
        // Any other error (rare) → treat as "not confidently registered" so we
        // fall through to the install attempt rather than silently skipping.
        Err(_) => false,
    }
}

/// True iff the current process can open the SCM with `CREATE_SERVICE` rights,
/// which requires Administrator. Avoids a separate token-elevation FFI dance.
fn is_elevated() -> bool {
    ServiceManager::local_computer(
        None::<&OsStr>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .is_ok()
}

/// The actual privileged install. Only call when elevated.
fn do_install() -> Result<(), Box<dyn std::error::Error>> {
    // Register the vsock GUIDs + restart WSL so vmcompute re-reads them. Doing
    // it here means the first sandbox call works without waiting on a restart.
    let added = ops::ensure_registered(VSOCK_PORT_BASE, VSOCK_PORT_COUNT)?;
    println!(
        "Registered {added} new vsock-port GUID(s) (range {}..{}).",
        VSOCK_PORT_BASE,
        VSOCK_PORT_BASE + VSOCK_PORT_COUNT - 1
    );
    if added > 0 {
        println!("Restarting WSL so vmcompute picks up the new registrations...");
        ops::wsl_shutdown()?;
    }

    let manager = ServiceManager::local_computer(
        None::<&OsStr>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )?;

    let exe = std::env::current_exe()?;
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("--run-sandbox-helper-service")],
        dependencies: vec![],
        account_name: None, // None ⇒ LocalSystem
        account_password: None,
    };

    let access =
        ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS;

    let service = match manager.create_service(&info, access) {
        Ok(s) => s,
        Err(windows_service::Error::Winapi(e))
            if e.raw_os_error() == Some(ERROR_SERVICE_EXISTS) =>
        {
            manager.open_service(SERVICE_NAME, access)?
        }
        Err(e) => return Err(e.into()),
    };

    let _ = service.set_description(
        "Brokers privileged WSL operations (utility-VM id resolution + vsock \
         port registration) for the Ziee code sandbox, so the Ziee server runs \
         unprivileged.",
    );

    match service.start(&[] as &[&OsStr]) {
        Ok(()) => {}
        Err(windows_service::Error::Winapi(e))
            if e.raw_os_error() == Some(ERROR_SERVICE_ALREADY_RUNNING) => {}
        Err(e) => return Err(e.into()),
    }

    println!("'{SERVICE_DISPLAY_NAME}' installed and started (LocalSystem, auto-start).");
    Ok(())
}

/// Relaunch this exe elevated to perform the install (one UAC prompt), via
/// PowerShell's `Start-Process -Verb RunAs`. The `--sandbox-helper-elevated`
/// flag tells the child not to elevate again. `-PassThru -Wait` propagates the
/// child's exit code; a declined UAC makes Start-Process throw → non-zero.
fn relaunch_elevated() -> Result<(), Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?;
    let exe_ps = exe.display().to_string().replace('\'', "''");
    let script = format!(
        "$p = Start-Process -FilePath '{exe_ps}' \
         -ArgumentList '--install-sandbox-helper','--sandbox-helper-elevated' \
         -Verb RunAs -Wait -PassThru; exit $p.ExitCode"
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()?;
    if !status.success() {
        return Err(format!(
            "elevated install did not complete successfully ({status:?}). \
             Approve the UAC prompt, or run `ziee --install-sandbox-helper` \
             from an Administrator terminal."
        )
        .into());
    }
    Ok(())
}

pub fn uninstall() -> Result<(), Box<dyn std::error::Error>> {
    let manager =
        ServiceManager::local_computer(None::<&OsStr>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
    )?;

    if let Ok(status) = service.query_status() {
        if status.current_state != ServiceState::Stopped {
            let _ = service.stop();
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    service.delete()?;
    println!(
        "'{SERVICE_DISPLAY_NAME}' uninstalled. (vsock GUID registrations left in \
         place — harmless and idempotent.)"
    );
    Ok(())
}
