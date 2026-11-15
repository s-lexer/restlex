mod i18n;
mod web_server;
#[cfg(windows)]
mod service;

use clap::Parser;
use std::env;

pub const SERVICE_NAME: &str = "RestlexWebServer";
pub const SERVICE_DISPLAY: &str = "RestLex Web Server";
pub const SERVICE_DESC: &str = "Сервер отчётов о бэкапах RestLex";
pub const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "restlex-webserver", disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Установить и запустить службу Windows.
    Install {
        #[arg(long, default_value = "8798")]
        port: u16,
        /// Порт приёма отчётов от агентов
        #[arg(long, default_value = "8797")]
        report_port: u16,
        /// Адрес привязки
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        /// API ключ для приёма отчётов от агентов (X-Api-Key)
        #[arg(long)]
        api_key: String,
        /// Логин администратора (миграция в users при первом старте)
        #[arg(long, default_value = "admin")]
        admin_user: String,
        /// Пароль администратора (миграция в users при первом старте)
        #[arg(long)]
        admin_password: String,
        /// Пароль для просмотра без логина (опционально)
        #[arg(long, default_value = "")]
        viewer_password: String,
        /// Хранить N последних отчётов на профиль
        #[arg(long, default_value = "10")]
        history: usize,
        /// Порог устаревания (часов, 0 = выкл.)
        #[arg(long, default_value = "0")]
        stale_hours: f64,
        /// Telegram Bot Token
        #[arg(long, default_value = "")]
        tg_token: String,
        /// Telegram Chat ID
        #[arg(long, default_value = "")]
        tg_chat: String,
        /// Telegram Topic/Thread ID (для форумных групп)
        #[arg(long, default_value = "")]
        tg_group: String,
    },
    /// Задать один секрет заново, не трогая остальной конфиг.
    ///
    /// Нужно, когда DPAPI перестал расшифровывать `enc:`-значения: конфиг
    /// перенесён на другую машину (восстановление из бэкапа, переустановка ОС).
    /// Раньше единственным выходом был повторный `install`, затирающий конфиг.
    ///
    ///   restlex-webserver.exe set-secret --name api_key --value МОЙКЛЮЧ
    ///   echo МОЙКЛЮЧ | restlex-webserver.exe set-secret --name api_key --stdin
    SetSecret {
        /// admin_password | viewer_password | api_key | tg_token
        #[arg(long)]
        name: String,
        /// Новое значение (пустое = очистить поле). Виден в списке процессов —
        /// для чувствительных значений используйте --stdin (или --value -).
        #[arg(long, default_value = "")]
        value: String,
        /// Прочитать значение из stdin, а не из аргумента командной строки.
        #[arg(long)]
        stdin: bool,
    },
    /// Удалить службу Windows
    Uninstall,
    /// Запустить службу
    Start,
    /// Остановить службу
    Stop,
    /// Запустить в консоли (читает существующий конфиг)
    Run,
    /// Внутренняя команда для SCM
    #[cfg(windows)]
    Service,
}

fn print_help_and_exit() {
    println!("RestLex Web Server — сервер отчётов о бэкапах");
    println!("Конфиг: C:\\ProgramData\\RestLex\\webserver-config.json");
    println!();
    println!("Команды:");
    println!("  install   Установить службу Windows и сохранить конфиг");
    println!("  uninstall Удалить службу");
    println!("  start     Запустить службу");
    println!("  stop      Остановить службу");
    println!("  run       Запустить в консоли (без службы)");
    println!("  set-secret Задать один секрет заново (api_key/admin_password/...),");
    println!("            не затирая остальной конфиг — после переноса на другую машину");
    println!();
    println!("Пример первого запуска:");
    println!("  restlex-webserver.exe install --api-key МОЙКЛЮЧ --admin-password ПАРОЛЬ");
    println!();
    println!("Полная справка:");
    println!("  restlex-webserver.exe install --help");
}

fn main() -> anyhow::Result<()> {
    // Custom no-args handling
    if env::args().len() == 1 {
        print_help_and_exit();
        return Ok(());
    }
    let cli = Cli::parse();
    match cli.cmd {
        None => { print_help_and_exit(); Ok(()) }
        Some(Cmd::Install { port, report_port, host, api_key, admin_user, admin_password,
                            viewer_password, history, stale_hours,
                            tg_token, tg_chat, tg_group }) => {
            let cfg = web_server::Config {
                enabled: true,
                host: host.clone(),
                port,
                report_port,
                admin_user: admin_user.clone(),
                admin_password,
                viewer_password,
                api_key,
                history_count: history,
                auto_group: true,
                tg_token,
                tg_chat,
                tg_group,
                stale_hours,
                secret_key: String::new(),
                lockout_max_attempts: 5,
                lockout_minutes: 15,
                behind_tls: false,
            };
            web_server::save_config(&cfg)?;
            println!("Конфиг сохранён: C:\\ProgramData\\RestLex\\webserver-config.json");
            #[cfg(windows)]
            service::install()?;
            println!("Веб-интерфейс: http://localhost:{port}  (порт {port})");
            println!("Отчёты агентов: POST http://localhost:{report_port}/report  (X-Api-Key: <ключ>)");
            Ok(())
        }
        Some(Cmd::SetSecret { name, value, stdin }) => {
            // Секрет из stdin: аргумент командной строки виден в списке процессов.
            let value = if stdin || value == "-" {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf.trim_end_matches(['\r', '\n']).to_string()
            } else { value };
            // read-modify-write: остальные поля сохраняются как есть.
            // Строго: на неразобранном конфиге НЕЛЬЗЯ писать дефолты — иначе
            // порт, tg_chat, lockout-настройки и прочие секреты обнулятся.
            let mut cfg = match web_server::load_config_strict() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Конфиг повреждён, запись отменена: {e}");
                    eprintln!(r"Почините C:\ProgramData\RestLex\webserver-config.json (или удалите его и выполните install), затем повторите команду.");
                    std::process::exit(2);
                }
            };
            match name.as_str() {
                "admin_password"  => cfg.admin_password  = value,
                "viewer_password" => cfg.viewer_password = value,
                "api_key"         => cfg.api_key         = value,
                "tg_token"        => cfg.tg_token        = value,
                other => anyhow::bail!(
                    "неизвестное поле '{other}'; допустимо: admin_password, viewer_password, api_key, tg_token"
                ),
            }
            web_server::save_config(&cfg)?;
            println!("Секрет '{name}' записан.");
            // Служба держит конфиг в памяти и при следующем сохранении настроек
            // из веб-админки перезаписывает файл целиком — свежий секрет с диска
            // будет молча откачен. Поэтому останавливать службу нужно ДО правки.
            println!("ВНИМАНИЕ: если служба сейчас запущена, она держит старый конфиг в памяти и");
            println!("перезапишет файл при первом же сохранении настроек в веб-админке.");
            println!("Правильный порядок: restlex-webserver.exe stop -> set-secret -> start");
            Ok(())
        }
        Some(Cmd::Uninstall) => {
            #[cfg(windows)]
            service::uninstall()?;
            Ok(())
        }
        Some(Cmd::Start) => {
            #[cfg(windows)]
            service::start_svc()?;
            Ok(())
        }
        Some(Cmd::Stop) => {
            #[cfg(windows)]
            service::stop_svc()?;
            Ok(())
        }
        Some(Cmd::Run) => run_foreground(),
        #[cfg(windows)]
        Some(Cmd::Service) => service::dispatch(),
    }
}

fn run_foreground() -> anyhow::Result<()> {
    init_logging(false)?;
    tracing::info!("RestLex Web Server v{} — foreground", AGENT_VERSION);
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async_main(tokio::sync::oneshot::channel::<()>().1))
}

pub fn init_logging(service: bool) -> anyhow::Result<()> {
    let logs = restlex_core::logs_dir();
    if service {
        let _ = std::fs::create_dir_all(&logs);
        let log_path = logs.join("webserver.log");
        if let Ok(meta) = std::fs::metadata(&log_path) {
            if meta.len() > 10 * 1024 * 1024 {
                let _ = std::fs::remove_file(logs.join("webserver.log.old"));
                let _ = std::fs::rename(&log_path, logs.join("webserver.log.old"));
            }
        }
        if let Ok(rd) = std::fs::read_dir(&logs) {
            for ent in rd.flatten() {
                let n = ent.file_name();
                let s = n.to_string_lossy();
                if s.starts_with("webserver.log.") && s != "webserver.log.old"
                    && s.chars().nth(14) == Some('2') {
                    let _ = std::fs::remove_file(ent.path());
                }
            }
        }
        if let Ok(appender) = std::panic::catch_unwind(|| tracing_appender::rolling::never(&logs, "webserver.log")) {
            tracing_subscriber::fmt()
                .with_writer(appender).with_ansi(false).with_target(false).with_level(true).init();
        } else {
            tracing_subscriber::fmt().with_ansi(false).with_target(false).with_level(true)
                .with_writer(std::io::sink).init();
        }
    } else {
        tracing_subscriber::fmt().with_target(false).with_level(true)
            .with_writer(std::io::stdout).init();
    }
    Ok(())
}

pub async fn async_main(mut shutdown: tokio::sync::oneshot::Receiver<()>) -> anyhow::Result<()> {
    let cfg = web_server::load_config();
    let session = web_server::Session::new();
    if let Err(e) = web_server::start(session.clone(), cfg) {
        tracing::error!("webserver start: {e}");
        return Err(e);
    }
    let _ = (&mut shutdown).await;
    web_server::stop(&session);
    tracing::info!("Web server stopped");
    Ok(())
}
