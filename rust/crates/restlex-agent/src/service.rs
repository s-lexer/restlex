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
        account_name: None, // LocalSystem
        account_password: None,
    };

    let service = match manager.open_service(crate::SERVICE_NAME, ServiceAccess::all()) {
        Ok(s) => s,
        Err(_) => manager.create_service(&service_info, ServiceAccess::all())?,
    };
    service.set_description(crate::SERVICE_DESC)?;

    // Failure actions: restart after 5s, 30s, 60s
    let actions = vec![
        ServiceAction {
            action_type: ServiceActionType::Restart,
            delay: Duration::from_secs(5),
        },
        ServiceAction {
            action_type: ServiceActionType::Restart,
            delay: Duration::from_secs(30),
        },
        ServiceAction {
            action_type: ServiceActionType::Restart,
            delay: Duration::from_secs(60),
        },
    ];
    let failure = ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86400)),
        reboot_msg: None,
        command: None,
        actions: Some(actions),
    };
    service.update_failure_actions(failure)?;
    service.set_failure_actions_on_non_crash_failures(true)?;

    // Create watchdog scheduled task (runs every 6 hours, starts service if stopped)
    let _ = create_watchdog();

    println!("Служба установлена: {}", crate::SERVICE_NAME);
    Ok(())
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
            if service.query_status()?.current_state == ServiceState::Stopped {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    service.delete()?;
    let _ = delete_watchdog();
    println!("Служба удалена: {}", crate::SERVICE_NAME);
    Ok(())
}

pub fn start_svc() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(
        crate::SERVICE_NAME,
        ServiceAccess::START | ServiceAccess::QUERY_STATUS,
    )?;
    service.start::<&str>(&[])?;
    println!("Служба запущена");
    Ok(())
}

pub fn stop_svc() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(
        crate::SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
    )?;
    service.stop()?;
    println!("Служба остановлена");
    Ok(())
}

pub fn print_status() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    match manager.open_service(crate::SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
        Ok(s) => {
            let st = s.query_status()?;
            println!("Состояние: {:?}", st.current_state);
        }
        Err(_) => println!("Служба не установлена"),
    }
    Ok(())
}

fn create_watchdog() -> anyhow::Result<()> {
    let cmd = format!(
        r#"sc query {} | find "RUNNING" > nul || sc start {}"#,
        crate::SERVICE_NAME,
        crate::SERVICE_NAME
    );
    // Delete old legacy task and current task before recreating
    let _ = std::process::Command::new("schtasks")
        .args(&["/delete", "/f", "/tn", "RestlexWatchdog"]).output();
    let _ = std::process::Command::new("schtasks")
        .args(&["/delete", "/f", "/tn", r"\Restlex\RestlexWatchdog"]).output();
    let out = std::process::Command::new("schtasks")
        .args(&[
            "/create", "/f",
            "/tn", r"\Restlex\RestlexWatchdog",
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

fn delete_watchdog() -> anyhow::Result<()> {
    let _ = std::process::Command::new("schtasks")
        .args(&["/delete", "/f", "/tn", r"\Restlex\RestlexWatchdog"])
        .output();
    let _ = std::process::Command::new("schtasks")
        .args(&["/delete", "/f", "/tn", "RestlexWatchdog"])
        .output();
    Ok(())
}

define_windows_service!(ffi_service_main, my_service_main);

pub fn dispatch() -> anyhow::Result<()> {
    service_dispatcher::start(crate::SERVICE_NAME, ffi_service_main)?;
    Ok(())
}

fn my_service_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!("service run error: {e}");
    }
}

fn run_service() -> anyhow::Result<()> {
    crate::init_logging(true)?;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = std::sync::Mutex::new(Some(shutdown_tx));

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                if let Some(tx) = shutdown_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
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

    // ТОЛЬКО ПОСЛЕ рапорта SCM: apply_pending_update() дёргает powershell
    // (проверка Authenticode), а холодный старт PS 2.0 на нагруженном 2008R2
    // легко перекрывал 30-секундное окно старта — служба помечалась как не
    // стартовавшая ровно в окне деплоя. Сам powershell тоже под таймаутом.
    if crate::updater::apply_pending_update() {
        tracing::info!("Exiting to let SCM restart us on new binary");
        // НЕ рапортуем Stopped: рапорт с ненулевым exit_code считается
        // «non-crash failure», и failure actions к нему применяются ТОЛЬКО при
        // set_failure_actions_on_non_crash_failures(true). Этот флаг ставит
        // install(), т.е. на уже установленных машинах его нет — служба лежала
        // бы до schtasks-watchdog (до 5 часов). Вместо рапорта просто умираем:
        // процесс службы, исчезнувший без рапорта Stopped, для SCM — крэш, и
        // recovery-действия (Restart через 5 с) применяются всегда, независимо
        // от конфигурации службы. Ровно так вело себя обновление до правки.
        std::process::exit(1);
    }

    // Ensure watchdog task exists (recreate on every start so existing installs get it too)
    let _ = create_watchdog();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let res = rt.block_on(crate::async_main(shutdown_rx));
    if let Err(e) = &res {
        tracing::error!("async_main: {e}");
    }

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
