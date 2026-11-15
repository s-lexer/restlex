mod api;
mod mount;
mod rest_server;
mod scheduler;
#[cfg(windows)]
mod service;
mod sleep_on_lan;
mod updater;

use std::env;

pub const SERVICE_NAME: &str = "RestlexAgent";
pub const SERVICE_DISPLAY: &str = "RestLex Backup Agent";
pub const SERVICE_DESC: &str = "Управляет расписаниями restic-бэкапов профилей RestLex";
pub const API_ADDR: &str = "127.0.0.1:17345";
pub const AGENT_VERSION_RAW: &str = env!("CARGO_PKG_VERSION");

/// Cargo требует semver X.Y.Z, но в UI/отчётах/сравнениях мы используем X.Y.
/// Срезаем хвостовой ".0" при первом запросе и кешируем.
pub fn agent_version() -> &'static str {
    use std::sync::OnceLock;
    static V: OnceLock<&'static str> = OnceLock::new();
    V.get_or_init(|| AGENT_VERSION_RAW.strip_suffix(".0").unwrap_or(AGENT_VERSION_RAW))
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("run");

    match cmd {
        "install" => {
            #[cfg(windows)]
            return service::install();
            #[allow(unreachable_code)]
            Ok(())
        }
        "uninstall" => {
            #[cfg(windows)]
            return service::uninstall();
            #[allow(unreachable_code)]
            Ok(())
        }
        "start" => {
            #[cfg(windows)]
            return service::start_svc();
            #[allow(unreachable_code)]
            Ok(())
        }
        "stop" => {
            #[cfg(windows)]
            return service::stop_svc();
            #[allow(unreachable_code)]
            Ok(())
        }
        "status" => {
            #[cfg(windows)]
            return service::print_status();
            #[allow(unreachable_code)]
            Ok(())
        }
        "service" => {
            #[cfg(windows)]
            return service::dispatch();
            #[allow(unreachable_code)]
            Ok(())
        }
        "run" | "foreground" => run_foreground(),
        _ => {
            eprintln!(
                "Использование: restlex-agent [install|uninstall|start|stop|status|service|run]"
            );
            Ok(())
        }
    }
}

fn run_foreground() -> anyhow::Result<()> {
    init_logging(false)?;
    tracing::info!("RestLex agent v{} — foreground", agent_version());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async_main(tokio::sync::oneshot::channel::<()>().1))
}

pub fn init_logging(service: bool) -> anyhow::Result<()> {
    let logs = restlex_core::logs_dir();
    if service {
        let _ = std::fs::create_dir_all(&logs);
        // Single agent.log file with 10 MB size limit.
        // On startup: if current log > 10 MB → rotate to agent.log.old (overwrites prev .old).
        let log_path = logs.join("agent.log");
        if let Ok(meta) = std::fs::metadata(&log_path) {
            if meta.len() > 10 * 1024 * 1024 {
                let _ = std::fs::remove_file(logs.join("agent.log.old"));
                let _ = std::fs::rename(&log_path, logs.join("agent.log.old"));
            }
        }
        // Cleanup old daily-rotated files (agent.log.YYYY-MM-DD)
        if let Ok(rd) = std::fs::read_dir(&logs) {
            for ent in rd.flatten() {
                let n = ent.file_name();
                let s = n.to_string_lossy();
                if s.starts_with("agent.log.") && s != "agent.log.old"
                    && s.chars().nth(10) == Some('2') /* date pattern */ {
                    let _ = std::fs::remove_file(ent.path());
                }
            }
        }
        if let Ok(appender) = std::panic::catch_unwind(|| tracing_appender::rolling::never(&logs, "agent.log")) {
            tracing_subscriber::fmt()
                .with_writer(appender)
                .with_ansi(false)
                .with_target(false)
                .with_level(true)
                .init();
        } else {
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_target(false)
                .with_level(true)
                .with_writer(std::io::sink)
                .init();
        }
    } else {
        tracing_subscriber::fmt()
            .with_target(false)
            .with_level(true)
            .with_writer(std::io::stdout)
            .init();
    }
    Ok(())
}

pub async fn async_main(mut shutdown: tokio::sync::oneshot::Receiver<()>) -> anyhow::Result<()> {
    restlex_core::enable_long_paths();

    let cfg = restlex_core::Config::load()?;

    // Write agent.version for PS scheduled tasks to read
    {
        let p = restlex_core::program_data().join("agent.version");
        if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
        let _ = std::fs::write(&p, agent_version());
    }

    // launcher.version пишется при первом запуске GUI, а на серверах его не
    // открывают — такой хост навсегда остаётся с версией "?" в веб-отчётах.
    // Восстанавливаем файл из самого restlex.exe, спросив у него версию.
    {
        let lv_path = restlex_core::program_data().join("launcher.version");
        let exe = restlex_core::program_data().join("restlex.exe");
        if !lv_path.exists() && exe.exists() {
            match std::process::Command::new(&exe).arg("--version").output() {
                // "RestLex 2.7" -> "2.7"
                Ok(o) if o.status.success() => {
                    let v = String::from_utf8_lossy(&o.stdout)
                        .split_whitespace().last().unwrap_or("").to_string();
                    if !v.is_empty() {
                        if let Err(e) = std::fs::write(&lv_path, &v) {
                            tracing::warn!("cannot restore launcher.version: {e}");
                        } else {
                            tracing::info!("launcher.version restored: {v}");
                        }
                    }
                }
                Ok(o) => tracing::warn!("restlex.exe --version rc={:?}", o.status.code()),
                Err(e) => tracing::warn!("cannot run restlex.exe --version: {e}"),
            }
        }
    }

    // Boot heartbeat: tell webserver our current launcher+agent versions
    // immediately on startup (without waiting for next backup or self-update).
    {
        let globals = cfg.globals();
        let report_url = globals.report_url.clone();
        let report_api_key = globals.report_api_key.clone();
        if !report_url.is_empty() {
            let lv = std::fs::read_to_string(restlex_core::program_data().join("launcher.version"))
                .ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
                .unwrap_or_else(|| "?".into());
            let av = agent_version().to_string();
            tokio::spawn(async move {
                let _ = restlex_core::reporter::send_update_report(
                    &report_url, &report_api_key,
                    &restlex_core::reporter::UpdateReport {
                        component: "boot".to_string(),
                        from_version: lv,
                        to_version: av,
                        host: restlex_core::reporter::hostname(),
                        success: true,
                        error: None,
                    },
                ).await;
            });
        }
    }

    // Prune logs older than the retention window once at startup.
    restlex_core::cleanup_old_logs();

    // Surface restic compatibility at startup.
    {
        let g0 = cfg.globals();
        let bin = if g0.restic.is_empty() {
            "restic".to_string()
        } else {
            g0.restic.clone()
        };
        let rr = restlex_core::restic::ResticRunner::new(bin, "", "");
        match rr.version_info().await {
            Ok((raw, _, true)) => tracing::info!("restic OK: {raw}"),
            Ok((raw, parsed, false)) => tracing::warn!(
                "restic версия несовместима (нужна >= {}.{}.{}): '{raw}' parsed={parsed:?}",
                restlex_core::restic::MIN_RESTIC_VERSION.0,
                restlex_core::restic::MIN_RESTIC_VERSION.1,
                restlex_core::restic::MIN_RESTIC_VERSION.2,
            ),
            Err(e) => tracing::warn!("restic недоступен: {e}"),
        }
    }

    // Periodic heartbeat: announce the current agent version to the report server
    // every 6 hours so a fresh upgrade shows up in the web UI without waiting for
    // the next scheduled backup; also prune old logs each cycle.
    let hb_cfg = cfg.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(6 * 60 * 60));
        loop {
            tick.tick().await;
            let g = hb_cfg.globals();
            if let Err(e) = restlex_core::reporter::send_heartbeat(
                &g.report_url,
                &g.report_api_key,
                agent_version(),
            )
            .await
            {
                tracing::debug!("heartbeat failed: {e}");
            }
            restlex_core::cleanup_old_logs();
        }
    });

    let state = std::sync::Arc::new(scheduler::State::new(cfg.clone()));

    // File watcher for hot-reload
    let state_w = state.clone();
    let cfg_w = cfg.clone();
    tokio::task::spawn_blocking(move || scheduler::watch_config(cfg_w, state_w));

    // HTTP API
    let api_state = state.clone();
    let api_handle = tokio::spawn(async move {
        if let Err(e) = api::serve(api_state).await {
            tracing::error!("API: {e}");
        }
    });

    // Phase 1: crash recovery — маркеры от предыдущего крэша
    scheduler::check_crash_markers(state.clone()).await;

    // Phase 1: однократная миграция с schtasks на agent-scheduler при старте
    tokio::spawn(scheduler::migrate_from_schtasks(state.clone()));

    // Scheduler loop
    let sched_handle = tokio::spawn(scheduler::run(state.clone()));

    // rest-server watchdog
    {
        let rs_cfg = rest_server::load_config();
        if rs_cfg.enabled {
            let session = state.rest_server.clone();
            // Два уровня Result: JoinError снаружи и anyhow изнутри start().
            // Раньше внутренний просто отбрасывался — ошибка «нет rest-server.exe»
            // не попадала в лог вообще.
            match tokio::task::spawn_blocking(move || rest_server::start(&session, &rs_cfg, false)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::error!("rest-server auto-start failed: {e}"),
                Err(e) => tracing::error!("rest-server auto-start task panicked: {e}"),
            }
        }
        let wdog_session = state.rest_server.clone();
        tokio::spawn(rest_server::watchdog_loop(wdog_session, state.clone()));
    }

    // Sleep-on-LAN: выключение этого ПК по сетевому пакету (если разрешено в настройках)
    tokio::spawn(sleep_on_lan::listen_loop(state.clone()));

    // Update client
    tokio::spawn(updater::update_loop(state.clone()));

    let self_update_exit = state.shutdown_notify.notified();
    tokio::select! {
        _ = &mut shutdown => {
            tracing::info!("Shutdown signal received");
        }
        _ = sched_handle => { tracing::warn!("Scheduler exited"); }
        _ = api_handle   => { tracing::warn!("API exited"); }
        _ = self_update_exit => {
            tracing::info!("Self-update staged, exiting for SCM restart");
        }
    }

    // Kill any restic processes that are mid-backup
    {
        let running_count = state.running.lock().unwrap().values().filter(|&&v| v).count();
        if running_count > 0 {
            tracing::info!("Shutdown: {} backup(s) still running — killing restic.exe", running_count);
            // до kill: иначе бэкап успеет классифицироваться как failed и уйдёт в Telegram
            scheduler::SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/IM", "restic.exe"])
                .output();
        }
    }

    tracing::info!("Agent stopped");
    Ok(())
}
