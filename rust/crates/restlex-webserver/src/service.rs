use std::ffi::OsString;
use std::time::Duration;
use windows_service::{
    define_windows_service,
    service::{
        ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
        ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
    service_manager::{ServiceManager, ServiceManagerAccess},
};

const SVC_TYPE: ServiceType = ServiceType::OWN_PROCESS;

pub fn install() -> anyhow::Result<()> {
    let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
    let manager = ServiceManager::local_computer(None::<&str>, manager_access)?;
    let exe = std::env::current_exe()?;

    let service_info = ServiceInfo {
        name: OsString::from(crate::SERVICE_NAME),
        display_name: OsString::from(crate::SERVICE_DISPLAY),
        service_type: SVC_TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("service")],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };

    let service = match manager.open_service(crate::SERVICE_NAME, ServiceAccess::all()) {
        Ok(s) => s,
        Err(_) => manager.create_service(&service_info, ServiceAccess::all())?,
    };
    service.set_description(crate::SERVICE_DESC)?;

    let actions = vec![
        ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(5) },
        ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(30) },
        ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(60) },
    ];
    let failure = ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86400)),
        reboot_msg: None,
        command: None,
        actions: Some(actions),
    };
    service.update_failure_actions(failure)?;
    service.set_failure_actions_on_non_crash_failures(true)?;

    let _ = service.start::<&str>(&[]);
    let _ = create_watchdog();
    println!("Служба установлена и запущена: {}", crate::SERVICE_NAME);
    Ok(())
}

fn create_watchdog() -> anyhow::Result<()> {
    let cmd = format!(
        r#"sc query {} | find "RUNNING" > nul || sc start {}"#,
        crate::SERVICE_NAME, crate::SERVICE_NAME
    );
    let out = std::process::Command::new("schtasks")
        .args(&[
            "/create", "/f",
            "/tn", r"\Restlex\RestlexWebServerWatchdog",
            "/sc", "hourly", "/mo", "5",
            "/ru", "SYSTEM", "/rl", "HIGHEST",
            "/tr", &format!("cmd.exe /c {}", cmd),
        ])
        .output()?;
    if !out.status.success() {
        tracing::warn!("watchdog create: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

fn delete_watchdog() {
    let _ = std::process::Command::new("schtasks")
        .args(&["/delete", "/f", "/tn", r"\Restlex\RestlexWebServerWatchdog"])
        .output();
}

pub fn uninstall() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(
        crate::SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
    )?;
    let status = service.query_status()?;
    if status.current_state != ServiceState::Stopped {
        let _ = service.stop();
        for _ in 0..30 {
            if service.query_status()?.current_state == ServiceState::Stopped { break; }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    service.delete()?;
    delete_watchdog();
    println!("Служба удалена: {}", crate::SERVICE_NAME);
    Ok(())
}

pub fn start_svc() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(crate::SERVICE_NAME, ServiceAccess::START | ServiceAccess::QUERY_STATUS)?;
    service.start::<&str>(&[])?;
    println!("Служба запущена");
    Ok(())
}

pub fn stop_svc() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(crate::SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)?;
    service.stop()?;
    println!("Служба остановлена");
    Ok(())
}

define_windows_service!(ffi_service_main, my_service_main);

pub fn dispatch() -> anyhow::Result<()> {
    service_dispatcher::start(crate::SERVICE_NAME, ffi_service_main)?;
    Ok(())
}

fn my_service_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service() { tracing::error!("service run error: {e}"); }
}

fn run_service() -> anyhow::Result<()> {
    crate::init_logging(true)?;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = std::sync::Mutex::new(Some(shutdown_tx));

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                if let Some(tx) = shutdown_tx.lock().unwrap().take() { let _ = tx.send(()); }
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(crate::SERVICE_NAME, event_handler)?;
    status_handle.set_service_status(ServiceStatus {
        service_type: SVC_TYPE,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let res = rt.block_on(crate::async_main(shutdown_rx));

    status_handle.set_service_status(ServiceStatus {
        service_type: SVC_TYPE,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    res
}
