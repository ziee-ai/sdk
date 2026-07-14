//! Windows SCM integration: the dispatcher + control handler that the
//! `--run-sandbox-helper-service` entry point drives. Launched by the Service
//! Control Manager, never by a user directly.

use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use windows_service::define_windows_service;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;

use super::server;
use super::SERVICE_NAME;

define_windows_service!(ffi_service_main, service_main);

/// Entry from `main.rs` when the SCM starts us. Hands control to the SCM
/// dispatcher, which calls [`service_main`]; blocks until the service stops.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
    Ok(())
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        eprintln!("ziee-sandbox-helper: service error: {e}");
    }
}

fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_handler = stop.clone();

    let status_handle = service_control_handler::register(SERVICE_NAME, move |control| match control
    {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            stop_for_handler.store(true, Ordering::Relaxed);
            // The serve loop is parked in a blocking ConnectNamedPipe, so a
            // flag alone won't wake it promptly. Exiting the process is the
            // reliable stop — SCM treats clean exit as Stopped.
            std::process::exit(0);
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    let result = server::serve(stop);

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(if result.is_ok() { 0 } else { 1 }),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    result.map_err(Into::into)
}
