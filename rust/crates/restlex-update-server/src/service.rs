use std::{ffi::OsString, time::Duration};
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
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )?;
    let exe = std::env::current_exe()?;
    let info = ServiceInfo {
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
    let svc = match manager.open_service(crate::SERVICE_NAME, ServiceAccess::all()) {
        Ok(s) => s,
        Err(_) => manager.create_service(&info, ServiceAccess::all())?,
    };
    svc.set_description(crate::SERVICE_DESC)?;
    let actions = vec![
        ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(5) },
        ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(30) },
        ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(60) },
    ];
    svc.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86400)),
        reboot_msg: None,
        command: None,
        actions: Some(actions),
    })?;
    svc.set_failure_actions_on_non_crash_failures(true)?;
    println!("Служба установлена: {}", crate::SERVICE_NAME);
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let svc = manager.open_service(
        crate::SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
    )?;
    if svc.query_status()?.current_state != ServiceState::Stopped {
        let _ = svc.stop();
        for _ in 0..30 {
            if svc.query_status()?.current_state == ServiceState::Stopped { break; }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    svc.delete()?;
    println!("Служба удалена: {}", crate::SERVICE_NAME);
    Ok(())
}

pub fn start_svc() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    manager.open_service(crate::SERVICE_NAME, ServiceAccess::START)?.start::<&str>(&[])?;
    println!("Служба запущена");
    Ok(())
}

pub fn stop_svc() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    manager.open_service(crate::SERVICE_NAME, ServiceAccess::STOP)?.stop()?;
    println!("Служба остановлена");
    Ok(())
}

define_windows_service!(ffi_service_main, svc_main);

pub fn dispatch() -> anyhow::Result<()> {
    service_dispatcher::start(crate::SERVICE_NAME, ffi_service_main)?;
    Ok(())
}

fn svc_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!("service error: {e}");
    }
}

fn run_service() -> anyhow::Result<()> {
    let cfg = crate::Config::load()?;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = std::sync::Mutex::new(Some(shutdown_tx));

    let handler = move |ctrl| -> ServiceControlHandlerResult {
        match ctrl {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                if let Some(tx) = shutdown_tx.lock().unwrap().take() { let _ = tx.send(()); }
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(crate::SERVICE_NAME, handler)?;
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
    let res = rt.block_on(async move {
        tokio::select! {
            r = crate::run_server(cfg) => r,
            _ = async move { shutdown_rx.await.ok(); } => Ok(()),
        }
    });

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
