//! Native Windows service entrypoint for the admin server.

use std::ffi::OsString;
use std::time::Duration;

use clap::Parser;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;

pub const SERVICE_NAME: &str = "fleet-admin";

windows_service::define_windows_service!(ffi_service_main, service_main);

/// Register with the service control manager and block until it stops us.
pub fn run() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(err) = run_service() {
        tracing::error!(error = %err, "fleet admin Windows service failed");
    }
}

fn run_service() -> windows_service::Result<()> {
    let event_handler = move |control| -> ServiceControlHandlerResult {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                fleet_core_common::shutdown::request();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;
    status_handle.set_service_status(status(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        0,
    ))?;

    let result = run_server();
    let exit_code = if result.is_ok() { 0 } else { 1 };
    let _ = status_handle.set_service_status(status(
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        exit_code,
    ));
    if let Err(err) = result {
        tracing::error!(error = %err, "fleet admin server stopped with an error");
    }
    Ok(())
}

fn run_server() -> anyhow::Result<()> {
    let cli = crate::Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(crate::run(cli))
}

fn status(
    current_state: ServiceState,
    controls_accepted: ServiceControlAccept,
    exit_code: u32,
) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state,
        controls_accepted,
        exit_code: ServiceExitCode::ServiceSpecific(exit_code),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    }
}
