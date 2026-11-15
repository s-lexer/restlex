use std::sync::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub data_dir: String,
    #[serde(default = "default_port")]
    pub port: String,
    #[serde(default)]
    pub username: String,
    #[serde(default = "default_interval")]
    pub watchdog_interval_s: u64,
    #[serde(default)]
    pub tg_notify: bool,
    /// Явное разрешение поднять rest-server БЕЗ аутентификации.
    /// Без этого флага отсутствие `.htpasswd` — ошибка, а не повод стартовать
    /// с `--no-auth`: раньше очистка каталога данных открывала сетевой доступ
    /// на чтение И ЗАПИСЬ ко всем репозиториям.
    #[serde(default)]
    pub allow_no_auth: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            data_dir: String::new(),
            port: default_port(),
            username: "admin".to_string(),
            watchdog_interval_s: default_interval(),
            tg_notify: false,
            allow_no_auth: false,
        }
    }
}

fn default_port() -> String {
    "8000".to_string()
}
fn default_interval() -> u64 {
    10
}

pub struct Session {
    pub child: Mutex<Option<Child>>,
    pub intentional_stop: Mutex<bool>,
}

impl Session {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            child: Mutex::new(None),
            intentional_stop: Mutex::new(false),
        })
    }
}

pub fn config_path() -> PathBuf {
    restlex_core::program_data().join("rest-server-config.json")
}

pub fn exe_path() -> PathBuf {
    restlex_core::program_data().join("bin").join("rest-server.exe")
}

/// Последний УСПЕШНО разобранный конфиг. GUI переписывает файл, watchdog может
/// прочитать его ровно в момент записи — раньше это молча давало
/// `Config::default()` с `enabled:false`, и watchdog переставал поднимать
/// упавший rest-server (без единой строчки в логе).
static LAST_GOOD: Mutex<Option<Config>> = Mutex::new(None);

pub fn load_config() -> Config {
    let p = config_path();
    let fallback = |reason: String| -> Config {
        match LAST_GOOD.lock().unwrap().clone() {
            Some(cfg) => {
                tracing::warn!("rest-server-config.json: {reason} — использую предыдущий валидный конфиг");
                cfg
            }
            None => {
                tracing::warn!("rest-server-config.json: {reason} — использую значения по умолчанию");
                Config::default()
            }
        }
    };
    let s = match std::fs::read_to_string(&p) {
        Ok(s) => s,
        // Файла нет — это штатное состояние (rest-server не настраивали).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
        Err(e) => return fallback(format!("не читается ({e})")),
    };
    match serde_json::from_str::<Config>(&s) {
        Ok(cfg) => {
            *LAST_GOOD.lock().unwrap() = Some(cfg.clone());
            cfg
        }
        Err(e) => fallback(format!("ошибка разбора ({e})")),
    }
}

/// `en` — язык текста ошибки (уходит в ответ REST API); логи не локализуются.
pub fn start(session: &Arc<Session>, cfg: &Config, en: bool) -> anyhow::Result<()> {
    let exe = exe_path();
    if !exe.exists() {
        if en {
            anyhow::bail!("rest-server.exe not found: {}", exe.display());
        }
        anyhow::bail!("rest-server.exe не найден: {}", exe.display());
    }
    if cfg.data_dir.is_empty() {
        if en {
            anyhow::bail!("data_dir is not set in the rest-server configuration");
        }
        anyhow::bail!("data_dir не задан в конфигурации rest-server");
    }
    std::fs::create_dir_all(&cfg.data_dir)?;

    // Лок child держим от проверки «уже запущен» до записи нового Child:
    // гонка watchdog ↔ POST /rest-server/start иначе спавнит два процесса,
    // а перезаписанный Child-handle становится сиротой и держит порт.
    let mut child_guard = session.child.lock().unwrap();
    if let Some(ref mut c) = *child_guard {
        if matches!(c.try_wait(), Ok(None)) {
            if en {
                anyhow::bail!("rest-server is already running");
            }
            anyhow::bail!("rest-server уже запущен");
        }
    }

    *session.intentional_stop.lock().unwrap() = false;

    let htpasswd = PathBuf::from(&cfg.data_dir).join(".htpasswd");
    let mut cmd = Command::new(&exe);
    cmd.arg("--path")
        .arg(&cfg.data_dir)
        .arg("--listen")
        .arg(format!(":{}", cfg.port));

    if htpasswd.exists() {
        cmd.arg("--htpasswd-file").arg(&htpasswd);
    } else if cfg.allow_no_auth {
        tracing::warn!(
            "rest-server стартует БЕЗ аутентификации (allow_no_auth=true, .htpasswd отсутствует): {}",
            htpasswd.display()
        );
        cmd.arg("--no-auth");
    } else {
        // Молчаливый --no-auth открывал сетевой доступ на чтение и запись ко
        // всем репозиториям, если каталог данных пересоздали/очистили.
        tracing::error!("rest-server НЕ запущен: нет {}", htpasswd.display());
        if en {
            anyhow::bail!(
                "{} not found — rest-server was not started (without .htpasswd the repositories \
                 would be open to the whole network; set allow_no_auth=true in \
                 rest-server-config.json if this is intentional)",
                htpasswd.display()
            );
        }
        anyhow::bail!(
            "не найден {} — rest-server не запущен (без .htpasswd репозитории были бы \
             открыты всей сети; задайте allow_no_auth=true в rest-server-config.json, \
             если это осознанное решение)",
            htpasswd.display()
        );
    }

    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let child = cmd.spawn()?;
    *child_guard = Some(child);
    tracing::info!("rest-server запущен (порт {})", cfg.port);
    Ok(())
}

pub fn stop(session: &Arc<Session>) {
    *session.intentional_stop.lock().unwrap() = true;
    let mut guard = session.child.lock().unwrap();
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        tracing::info!("rest-server остановлен");
    }
}

pub fn is_alive(session: &Arc<Session>) -> bool {
    let mut guard = session.child.lock().unwrap();
    if let Some(ref mut child) = *guard {
        match child.try_wait() {
            Ok(None) => return true,
            _ => {}
        }
        *guard = None;
    }
    false
}

#[derive(Serialize)]
pub struct Status {
    pub running: bool,
    pub intentional_stop: bool,
    pub port: String,
    pub enabled: bool,
}

pub fn status(session: &Arc<Session>) -> Status {
    let cfg = load_config();
    Status {
        running: is_alive(session),
        intentional_stop: *session.intentional_stop.lock().unwrap(),
        port: cfg.port,
        enabled: cfg.enabled,
    }
}

/// `state` нужен, чтобы токен/чат Telegram читались из АКТУАЛЬНОГО конфига на
/// каждой итерации. Раньше они снимались один раз при старте службы в
/// `Arc<Mutex<String>>`: после смены токена бота уведомления о падении
/// rest-server уходили со старым токеном и терялись.
pub async fn watchdog_loop(session: Arc<Session>, state: Arc<crate::scheduler::State>) {
    // Backoff. Без него получался цикл «упал → WARN+Telegram → неудачный старт»
    // каждые 5-10 секунд: до 6 сообщений в минуту (флуд/бан бота) и ~8600
    // ERROR-строк в сутки. Ретраим старт и уведомляем не чаще раза в 15 минут,
    // а повторную ТУ ЖЕ ошибку не дублируем.
    const RETRY_EVERY: Duration = Duration::from_secs(15 * 60);
    // Сколько итераций ПОДРЯД процесс должен быть жив, чтобы считать старт
    // удавшимся и снять backoff. Раньше backoff снимался по факту спавна и по
    // первой же живой итерации — процесс, падающий через 3 с после старта,
    // каждый раз выглядел «первым падением», и WARN + Telegram шли каждые
    // watchdog_interval_s. Backoff душил только серию неудачных СТАРТОВ.
    const HEALTHY_ITERS: u32 = 3;
    let mut last_attempt: Option<tokio::time::Instant> = None;
    let mut last_error: Option<String> = None;
    let mut healthy: u32 = 0;

    loop {
        let cfg = load_config();
        let interval = Duration::from_secs(cfg.watchdog_interval_s.max(5));
        tokio::time::sleep(interval).await;

        if !cfg.enabled || *session.intentional_stop.lock().unwrap() {
            continue;
        }

        let session_c = session.clone();
        let alive = tokio::task::spawn_blocking(move || is_alive(&session_c))
            .await
            .unwrap_or(false);

        if alive || *session.intentional_stop.lock().unwrap() {
            // Backoff снимаем только после HEALTHY_ITERS подряд живых проверок:
            // «поднялся и сразу упал» не должен считаться успехом.
            if alive {
                healthy = healthy.saturating_add(1);
                if healthy >= HEALTHY_ITERS {
                    last_attempt = None;
                    last_error = None;
                }
            }
            continue;
        }
        healthy = 0;

        // Ещё не отстояли период с прошлой неудачной попытки — молча ждём.
        if last_attempt.is_some_and(|t| t.elapsed() < RETRY_EVERY) {
            continue;
        }
        let first_try = last_attempt.is_none();
        last_attempt = Some(tokio::time::Instant::now());

        tracing::warn!("rest-server не запущен, пробую поднять...");

        // Telegram — только на первом падении и потом не чаще RETRY_EVERY.
        if cfg.tg_notify {
            let g = state.cfg.globals();
            let en = g.en();
            let (token, chat) = (g.tg_token, g.tg_chat);
            if !token.is_empty() && !chat.is_empty() {
                let msg = if en {
                    format!("[RestLex] rest-server crashed (port {}), restarting...", cfg.port)
                } else {
                    format!("[RestLex] rest-server упал (порт {}), перезапуск...", cfg.port)
                };
                let _ = restlex_core::telegram::send(&token, &chat, &msg).await;
            }
        }

        let session_s = session.clone();
        let port = cfg.port.clone();
        let res = tokio::task::spawn_blocking(move || start(&session_s, &cfg, false)).await;
        let err = match res {
            Ok(Ok(())) => {
                // last_attempt НЕ сбрасываем: успех — это не факт спавна, а
                // HEALTHY_ITERS живых итераций подряд (см. выше). Иначе
                // crash-loop давал WARN + Telegram каждые watchdog_interval_s.
                tracing::info!("rest-server поднят watchdog'ом (порт {port})");
                last_error = None;
                continue;
            }
            Ok(Err(e)) => e.to_string(),
            Err(e) => format!("задача старта паникнула: {e}"),
        };
        // Та же ошибка, что и в прошлый раз, — не дублируем текст, только факт.
        if !first_try && last_error.as_deref() == Some(err.as_str()) {
            tracing::warn!(
                "rest-server всё ещё не поднимается (та же ошибка), следующая попытка через {} мин",
                RETRY_EVERY.as_secs() / 60
            );
        } else {
            tracing::error!(
                "rest-server не запустился: {err}; следующая попытка через {} мин",
                RETRY_EVERY.as_secs() / 60
            );
        }
        last_error = Some(err);
    }
}
