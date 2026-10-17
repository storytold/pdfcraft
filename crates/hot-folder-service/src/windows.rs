use crate::{SERVICE_NAME, Status, Worker};
use std::path::{Path, PathBuf};
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use windows_service::{
    service::*,
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
    service_manager::*,
};

fn service(access: ServiceAccess) -> Result<Service, windows_service::Error> {
    ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?.open_service(SERVICE_NAME, access)
}

pub fn status(executable: &Path, config: &Path) -> Result<Status, String> {
    let handle = match service(ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG) {
        Ok(handle) => handle,
        Err(windows_service::Error::Winapi(error)) if error.raw_os_error() == Some(1060) => return Ok(Status::absent()),
        Err(error) => return Err(format!("Could not inspect the Windows service: {error}")),
    };
    let registered = handle.query_config().map_err(|e| e.to_string())?;
    if !crate::definitions::windows_command_matches(
        &registered.executable_path.to_string_lossy(),
        crate::path_text(executable)?,
        crate::path_text(config)?,
    )? {
        return Err("The installed Hot Folder service uses a different executable or configuration; open its configured task file".into());
    }
    let state = handle.query_status().map_err(|e| e.to_string())?;
    Ok(Status {
        installed: true,
        enabled: registered.start_type == ServiceStartType::AutoStart,
        running: matches!(state.current_state, ServiceState::Running | ServiceState::StartPending),
        account: registered.account_name.map(|value| value.to_string_lossy().into_owned()).unwrap_or_else(|| "LocalSystem".into()),
    })
}

pub fn set_enabled(enabled: bool, executable: &Path, config: &Path) -> Result<(), String> {
    let handle = service(ServiceAccess::QUERY_CONFIG | ServiceAccess::CHANGE_CONFIG)
        .map_err(|e| format!("Administrator permission is required to change service startup: {e}"))?;
    let current = handle.query_config().map_err(|e| e.to_string())?;
    handle
        .change_config(&ServiceInfo {
            name: SERVICE_NAME.into(),
            display_name: current.display_name,
            service_type: current.service_type,
            start_type: if enabled { ServiceStartType::AutoStart } else { ServiceStartType::OnDemand },
            error_control: current.error_control,
            executable_path: executable.into(),
            launch_arguments: vec!["--service".into(), "--config".into(), config.as_os_str().into()],
            dependencies: current.dependencies,
            // ChangeServiceConfigW treats a null account/password as "do not change".
            account_name: None,
            account_password: None,
        })
        .map_err(|e| e.to_string())
}

pub fn start() -> Result<(), String> {
    service(ServiceAccess::START)
        .and_then(|s| s.start::<&str>(&[]))
        .map_err(|e| format!("Could not start the service (administrator permission may be required): {e}"))
}
pub fn stop() -> Result<(), String> {
    service(ServiceAccess::STOP)
        .and_then(|s| s.stop())
        .map(|_| ())
        .map_err(|e| format!("Could not stop the service (administrator permission may be required): {e}"))
}
pub fn install(_: &Path, _: &Path, _: &str) -> Result<(), String> {
    Err("Install the optional Hot Folder component with the PdfCraft MSI. It registers a LocalService account and a protected task directory.".into())
}
pub fn uninstall() -> Result<(), String> {
    Err("Remove the optional Hot Folder component with the PdfCraft installer, which also removes its Windows service.".into())
}

pub fn prepare_account() -> Result<(), String> {
    let handle = service(ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .map_err(|e| format!("Administrator permission is required to prepare the Windows service: {e}"))?;
    // A stable service SID keeps installer-protected data accessible when an
    // administrator changes Log On to a domain account or gMSA in Windows SCM.
    handle.set_config_service_sid_info(ServiceSidType::Unrestricted).map_err(|e| e.to_string())?;
    handle
        .update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86400)),
            reboot_msg: Some("".into()),
            command: Some("".into()),
            // SCM repeats the final action for subsequent failures. A delay avoids
            // a tight crash loop while retaining recovery after prolonged outages.
            actions: Some(vec![ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(60) }]),
        })
        .map_err(|e| e.to_string())?;
    handle.set_failure_actions_on_non_crash_failures(true).map_err(|e| e.to_string())
}

static JOB: OnceLock<(PathBuf, Worker)> = OnceLock::new();
static OUTCOME: OnceLock<Result<(), String>> = OnceLock::new();
windows_service::define_windows_service!(service_entry, service_main);

pub fn run(config: &Path, worker: Worker) -> Result<(), String> {
    JOB.set((config.to_owned(), worker)).map_err(|_| "The Windows service host can only start once per process")?;
    service_dispatcher::start(SERVICE_NAME, service_entry).map_err(|e| format!("Start this mode through Windows Services: {e}"))?;
    OUTCOME.get().cloned().unwrap_or_else(|| Err("The service did not report its completion".into()))
}

fn service_main(_: Vec<std::ffi::OsString>) {
    let result = std::panic::catch_unwind(host).unwrap_or_else(|_| Err("The service worker failed safely".into()));
    let _ = OUTCOME.set(result);
}

fn status_value(state: ServiceState, failed: bool) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: if failed { ServiceExitCode::ServiceSpecific(1) } else { ServiceExitCode::Win32(0) },
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    }
}

fn host() -> Result<(), String> {
    let stopping = std::sync::Arc::new(AtomicBool::new(false));
    let signal = stopping.clone();
    let handle = service_control_handler::register(SERVICE_NAME, move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            signal.store(true, Ordering::SeqCst);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })
    .map_err(|e| e.to_string())?;
    handle.set_service_status(status_value(ServiceState::Running, false)).map_err(|e| e.to_string())?;
    let finished = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        // The supervisor terminates its task child within a bounded grace period.
        // Keep SCM informed while that child releases its journal lock.
        let monitor = std::thread::Builder::new()
            .name("service-stop-status".into())
            .spawn_scoped(scope, || {
                let mut checkpoint = 0u32;
                while !finished.load(Ordering::SeqCst) {
                    if stopping.load(Ordering::SeqCst) {
                        checkpoint = checkpoint.saturating_add(1);
                        let mut state = status_value(ServiceState::StopPending, false);
                        state.checkpoint = checkpoint;
                        state.wait_hint = Duration::from_secs(15);
                        let _ = handle.set_service_status(state);
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .map_err(|e| format!("Could not start the service status monitor: {e}"))?;
        let result = match JOB.get() {
            Some((config, worker)) => {
                std::panic::catch_unwind(|| worker(config, &stopping)).unwrap_or_else(|_| Err("The service worker failed safely".into()))
            }
            None => Err("Service configuration is unavailable".into()),
        };
        finished.store(true, Ordering::SeqCst);
        monitor.join().map_err(|_| "Service status reporting failed safely")?;
        result
    });
    handle.set_service_status(status_value(ServiceState::Stopped, result.is_err())).map_err(|e| e.to_string())?;
    result
}
