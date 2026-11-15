use axum::{
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::{collections::HashSet, sync::{Arc, Mutex, OnceLock}};

use crate::{rest_server, scheduler, updater};

/// Не позволяем двум параллельным API-запросам принять один и тот же бэкап
/// до того, как запущенный tokio task успеет выставить State::running.
static API_BACKUP_STARTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

struct ApiBackupReservation(String);

impl ApiBackupReservation {
    fn acquire(pid: &str) -> Option<Self> {
        let mut active = API_BACKUP_STARTS.get_or_init(Default::default).lock().unwrap();
        if !active.insert(pid.to_string()) { return None; }
        Some(Self(pid.to_string()))
    }
}

impl Drop for ApiBackupReservation {
    fn drop(&mut self) {
        if let Some(active) = API_BACKUP_STARTS.get() {
            active.lock().unwrap().remove(&self.0);
        }
    }
}
use scheduler::{Job, JobStatus};

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

// ── Auth helpers ─────────────────────────────────────────────────────────────

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Токен считается ОТСУТСТВУЮЩИМ не только когда он пуст, но и когда остался
/// `enc:`-блобом: DPAPI не расшифровал его (конфиг перенесён на другую машину,
/// машина склонирована). Сравнение Bearer с шифроблобом давало 403 на всё
/// НАВСЕГДА — ни GUI, ни Recondeck, ни K-AutoAdmin не могли достучаться, и
/// удалённо это не чинилось. Считая такой токен отсутствующим, мы и
/// перегенерируем его в `ensure_api_token`, и уходим в fallback в middleware.
pub(crate) fn token_missing(t: &str) -> bool {
    t.is_empty() || restlex_core::crypto::is_undecrypted(t)
}

fn extract_bearer(headers: &HeaderMap) -> &str {
    headers.get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
}

async fn v1_auth_middleware(
    State(st): State<Arc<scheduler::State>>,
    req: Request,
    next: Next,
) -> Response {
    let g = st.cfg.globals();
    let en = g.en();
    // В норме сюда мы уже приходим с непустым api_token: `ensure_api_token`
    // генерирует его при старте. Fallback на update_api_key оставлен для
    // обратной совместимости (машины, где генерация не прошла — например
    // config.json не пишется) — ключ обновлений ОДИНАКОВ на всех машинах,
    // поэтому это именно запасной путь, а не штатный.
    // `enc:` = DPAPI не расшифровал токен (перенесённый/склонированный конфиг):
    // сравнивать Bearer с шифроблобом нельзя — это 403 на всё без шансов
    // починить удалённо. Считаем такой токен отсутствующим и уходим в fallback.
    let token = if token_missing(&g.api_token) {
        tracing::warn!(
            "api: api_token не задан (или не расшифрован) — авторизация идёт по \
             update_api_key (общий ключ парка!)"
        );
        g.update_api_key
    } else {
        g.api_token
    };
    if token_missing(&token) {
        let text = restlex_core::i18n::tr(
            en,
            "REST API отключено: api_token не задан",
            "REST API is disabled: api_token is not set",
        );
        return (StatusCode::FORBIDDEN, text).into_response();
    }
    let provided = extract_bearer(req.headers());
    if !constant_time_eq(provided.as_bytes(), token.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    next.run(req).await
}

// ── Job helpers ───────────────────────────────────────────────────────────────

fn new_job_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    format!("{:016x}{:016x}", rng.gen::<u64>(), rng.gen::<u64>())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// GC: удаляет завершённые (Done/Failed) jobs старше часа. Running не трогаем.
/// 10 минут было мало: внешний клиент (K-AutoAdmin) может опрашивать редко,
/// и результат restore/backup исчезал до того, как его успевали забрать.
pub async fn job_gc_loop(state: Arc<scheduler::State>) {
    const TTL_SECS: u64 = 3600;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        let now = now_unix();
        let mut jobs = state.jobs.lock().unwrap();
        jobs.retain(|_, j| {
            matches!(j.status, JobStatus::Running)
                || j.finished_at.map(|fa| now.saturating_sub(fa) < TTL_SECS).unwrap_or(true)
        });
    }
}

// ── Profile sanitization ─────────────────────────────────────────────────────

/// Ключи облачных/транспортных учёток в `extra`. Значения НИКОГДА не отдаём —
/// только факт заполнения (для UI: показать «ключ задан» без утечки).
const CRED_KEYS: &[&str] = &[
    "cred_s3_key", "cred_s3_secret", "cred_b2_account", "cred_b2_key",
    "cred_az_account", "cred_az_key", "cred_gs_project", "cred_gs_json",
    "cred_sftp_user", "cred_sftp_pass",
    "cred_rest_host", "cred_rest_path", "cred_rest_user", "cred_rest_pass",
];

/// Публичное (безопасное) представление профиля. Отдаёт несекретные поля,
/// нужные для редактирования из панели, но НЕ `repo_password` и НЕ значения
/// `cred_*` (только имена заполненных ключей в `creds_set` + `has_password`).
/// Это тот же контракт «секреты не покидают агент», что и раньше.
fn profile_public(pid: &str, p: &restlex_core::Profile) -> serde_json::Value {
    let creds_set: Vec<&str> = CRED_KEYS
        .iter()
        .copied()
        .filter(|k| {
            p.extra
                .get(*k)
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        })
        .collect();
    serde_json::json!({
        "pid":          pid,
        "name":         p.name,
        "sources":      p.sources,
        "exclude":      p.exclude,
        "repo_type":    format!("{:?}", p.repo_type).to_lowercase(),
        "repo_path":    p.repo_path,
        "use_vss":      p.use_vss,
        "retention":    p.retention,
        "sched":        p.sched,
        "enabled":      p.sched.enabled,
        "has_password": !p.repo_password.is_empty(),
        "creds_set":    creds_set,
        "last_run":     p.last_run.as_ref().map(|dt| dt.to_rfc3339()),
        "last_status":  p.last_status,
    })
}

// ── Write-path helpers (create/update/pause) ──────────────────────────────────

/// pid идёт в путь URL и имя файла лога — пропускаем только безопасные символы.
fn valid_pid(pid: &str) -> bool {
    !pid.is_empty()
        && pid.len() <= 64
        && pid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Уникальный 8-hex pid, не пересекающийся с существующими профилями.
fn new_pid(existing: &std::collections::HashMap<String, restlex_core::Profile>) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    loop {
        let pid = format!("{:08x}", rng.gen::<u32>());
        if !existing.contains_key(&pid) {
            return pid;
        }
    }
}

/// Верхние границы `extra`-мешка: authenticated-клиент не должен раздуть
/// profiles.json произвольными ключами/значениями.
const MAX_EXTRA_KEYS: usize = 64;
const MAX_EXTRA_BYTES: usize = 64 * 1024;

/// Валидация полей профиля на границе API. `repo_type` уже проверен enum'ом при
/// десериализации (неизвестный тип → ошибка разбора → 400 выше). Вызывается
/// ПОСЛЕ merge_secrets на update — капы считаются по итоговому профилю.
fn validate_profile(p: &restlex_core::Profile) -> std::result::Result<(), String> {
    if p.name.trim().is_empty() {
        return Err("name is required".into());
    }
    if p.name.chars().count() > 256 {
        return Err("name too long (max 256)".into());
    }
    if p.repo_path.trim().is_empty() {
        return Err("repo_path is required".into());
    }
    if p.repo_path.len() > 4096 {
        return Err("repo_path too long".into());
    }
    if p.sources.len() > 1000 {
        return Err("too many sources (max 1000)".into());
    }
    if p.exclude.len() > 1000 {
        return Err("too many exclude patterns (max 1000)".into());
    }
    if p.extra.len() > MAX_EXTRA_KEYS {
        return Err(format!("too many extra keys (max {MAX_EXTRA_KEYS})"));
    }
    let extra_bytes: usize = p.extra.iter().map(|(k, v)| k.len() + v.to_string().len()).sum();
    if extra_bytes > MAX_EXTRA_BYTES {
        return Err(format!("extra too large (max {} KB)", MAX_EXTRA_BYTES / 1024));
    }
    Ok(())
}

/// Мерж server-owned секретов из `existing` в `new` на update. Контракт PUT:
/// пустой/отсутствующий `repo_password` и `cred_*` (и любой другой `extra`-ключ)
/// СОХРАНЯЮТ прежнее значение, непустые — заменяют. Нужен потому, что GET маскирует
/// секреты (отдаёт has_password/creds_set, не значения) — без мержа штатный флоу
/// GET→правка→PUT затёр бы пароль репозитория и облачные креды пустыми.
fn merge_secrets(new: &mut restlex_core::Profile, existing: &restlex_core::Profile) {
    // repo_password: пустой в теле → берём прежний.
    if new.repo_password.is_empty() {
        new.repo_password = existing.repo_password.clone();
    }
    // extra (cred_* и прочее): стартуем от прежних, накатываем только НЕпустые
    // значения из тела. Пустая строка/отсутствие ключа = «оставить как было».
    let mut merged = existing.extra.clone();
    for (k, v) in new.extra.iter() {
        let is_empty_str = v.as_str().map(|s| s.is_empty()).unwrap_or(false);
        if !is_empty_str {
            merged.insert(k.clone(), v.clone());
        }
    }
    new.extra = merged;
}

/// Профиль пригоден к сохранению по паролю: либо задан repo_password, либо на
/// машине сконфигурирован глобальный password-file (restic возьмёт пароль оттуда).
/// Не даём создать/сохранить заведомо битый профиль с пустым паролем.
fn password_ok(p: &restlex_core::Profile, default_password_file: &str) -> bool {
    !p.repo_password.is_empty() || !default_password_file.is_empty()
}

// ── /status (internal) ───────────────────────────────────────────────────────

#[derive(Serialize)]
struct StatusResp {
    version: String,
    profiles: usize,
    running_now: Vec<String>,
    log_tail: Vec<String>,
}

/// Убивает процесс, удерживающий указанный порт в LISTEN, если это не текущий процесс.
async fn kill_port_holder(addr: &str) {
    if tokio::net::TcpListener::bind(addr).await.is_ok() {
        return;
    }
    let port = addr.rsplit(':').next().unwrap_or("");
    if port.is_empty() { return; }
    let out = match std::process::Command::new("netstat").args(["-ano"]).output() {
        Ok(o) => o,
        Err(_) => return,
    };
    let my_pid = std::process::id();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some(pid) = netstat_port_holder(line, port) else { continue };
        if pid == my_pid || pid == 0 {
            continue;
        }
        tracing::warn!("Port {port} held by zombie PID {pid}, killing it");
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
}

/// Разбор одной строки `netstat -ano`: "TCP  <local>  <remote>  <state>  <pid>".
/// Возвращает PID, если ЛОКАЛЬНЫЙ адрес (поле 2, а не вся строка — иначе под
/// матч попадал бы чужой ИСХОДЯЩИЙ коннект на удалённый :port) заканчивается
/// на `:{port}`.
///
/// Сверяем именно суффикс: зомби может слушать `0.0.0.0:17345`, `[::1]:17345`
/// или `192.168.1.5:17345` — точное равенство с `127.0.0.1:{port}` их не ловит,
/// а порт при этом занят и мы не поднимемся.
/// Слово состояния (поле 4) НЕ проверяем: на локализованной Windows netstat
/// печатает его на языке системы («ПРОСЛУШИВАНИЕ», «ABHÖREN»), и проверка на
/// "LISTEN" отбрасывала бы вообще всё. Вместо этого смотрим на УДАЛЁННЫЙ адрес:
/// у слушающего сокета он всегда `0.0.0.0:0` / `[::]:0`, и это признак
/// языконезависимый. Без него строка исходящего коннекта
/// `TCP 10.0.0.5:<port> 10.0.0.9:443 ESTABLISHED <pid>` (наш порт как локальный
/// эфемерный) матчилась бы по суффиксу, и taskkill /F прилетал бы чужому
/// процессу — для пользовательского `cfg.port` это вполне достижимо.
fn netstat_port_holder(line: &str, port: &str) -> Option<u32> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 5 || !parts[0].eq_ignore_ascii_case("TCP") {
        return None;
    }
    // Двоеточие в суффиксе само задаёт границу поля: "127.0.0.1:18797" не
    // оканчивается на ":8797" (перед "8797" стоит "1", а не ":").
    if !parts[1].ends_with(&format!(":{port}")) {
        return None;
    }
    if parts[2] != "0.0.0.0:0" && parts[2] != "[::]:0" && parts[2] != "*:*" {
        return None;
    }
    parts[4].parse::<u32>().ok()
}

/// Собирает роутер агента. Вынесено из `serve`, чтобы покрыть тестом факт,
/// что легаси-роуты закрыты авторизацией.
fn build_app(state: Arc<scheduler::State>) -> Router {
    let api_v1 = Router::new()
        .route("/profiles",                 get(v1_profiles).post(v1_create_profile))
        .route("/profiles/:pid",            get(v1_profile).put(v1_update_profile).delete(v1_delete_profile))
        .route("/profiles/:pid/backup",     post(v1_backup))
        .route("/profiles/:pid/pause",      post(v1_pause))
        .route("/profiles/:pid/resume",     post(v1_resume))
        .route("/profiles/:pid/status",     get(v1_status))
        .route("/profiles/:pid/stop",       post(v1_stop))
        .route("/profiles/:pid/test",       post(v1_test_connection))
        .route("/profiles/:pid/restore",    post(v1_restore))
        .route("/profiles/:pid/snapshots",  get(v1_snapshots))
        .route("/profiles/:pid/deleted",    post(v1_deleted))
        .route("/snapshots/:pid",           get(v1_snapshots))
        .route("/jobs/:job_id",             get(v1_job))
        .route("/settings",                 get(v1_settings).patch(v1_patch_settings))
        .route("/reload",                   post(v1_reload))
        .layer(axum::middleware::from_fn_with_state(state.clone(), v1_auth_middleware));

    // Легаси-роуты. Раньше висели голыми: любой непривилегированный процесс на
    // машине мог сделать POST /update/apply (служба под LocalSystem выкачивала и
    // подменяла свои бинарники — локальный EoP до SYSTEM), POST /mount/start/:pid
    // (публикация репозитория бэкапов) или GET /log. Теперь под тем же bearer'ом,
    // что и /api/v1. Открытым остаётся только GET /health (проверка живости).
    let legacy = Router::new()
        .route("/status", get(status))
        .route("/log", get(log))
        .route("/run/:pid", post(run_now))
        .route("/mount/status", get(mount_status))
        .route("/mount/start/:pid", post(mount_start))
        .route("/mount/stop", post(mount_stop))
        .route("/reload", post(reload))
        .route("/last-run/:pid", get(last_run))
        .route("/last-run", get(all_last_runs))
        .route("/rest-server/status", get(rs_status))
        .route("/rest-server/start", post(rs_start))
        .route("/rest-server/stop", post(rs_stop))
        .route("/update/status", get(update_status))
        .route("/update/check", post(update_check))
        .route("/update/apply", post(update_apply))
        .layer(axum::middleware::from_fn_with_state(state.clone(), v1_auth_middleware));

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(legacy)
        .nest("/api/v1", api_v1)
        .with_state(state)
}

/// Генерирует криптостойкий `api_token` и дописывает его в config.json, если он
/// пуст. Единый источник правды — файл: оттуда его читают и GUI, и CLI, и
/// K-AutoAdmin.
///
/// Без этого на свежей установке (и при headless-развёртывании) `api_token` и
/// `update_api_key` оба пусты, middleware отдаёт 403 на всё кроме /health — и
/// GUI полностью теряет управление агентом (Run now, mount, update, reload,
/// «Служба не запущена» при работающей службе).
///
/// Запись — через `patch_globals`: read-modify-write под `config_file_lock` с
/// `write_atomic`, чужие ключи не затираются. Сам токен в лог НЕ пишем.
pub fn ensure_api_token(cfg: &restlex_core::Config) {
    // Токен считаем отсутствующим и когда он остался `enc:`-блобом: DPAPI не
    // расшифровал (конфиг перенесён на другую машину / машина склонирована).
    // Без этого middleware сравнивал бы Bearer с шифроблобом — 403 на всё
    // НАВСЕГДА, без пути восстановления. Перегенерация возвращает управление.
    if !token_missing(&cfg.globals().api_token) {
        return;
    }
    // Если config.json не разбирается — НИЧЕГО не пишем: patch_globals в этом
    // случае унёс бы реальный конфиг в карантин, а на его место лёг бы файл с
    // дефолтами и токеном (потеря tg_token, report_url, restic, интервалов).
    let cfg_path = restlex_core::config_path();
    match std::fs::read_to_string(&cfg_path) {
        Ok(txt)
            if !txt.trim().is_empty()
                && serde_json::from_str::<serde_json::Value>(&txt).is_err() =>
        {
            tracing::error!(
                "config.json не разбирается — api_token НЕ генерируем и файл не трогаем; \
                 REST API останется на update_api_key до починки конфига"
            );
            return;
        }
        _ => {}
    }
    // rand::thread_rng — CSPRNG (ChaCha12), не быстрый LCG.
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let token: String = (0..32).map(|_| format!("{:02x}", rng.gen::<u8>())).collect();

    let mut patch = serde_json::Map::new();
    patch.insert("api_token".to_string(), serde_json::Value::String(token));
    match cfg.patch_globals(&patch) {
        Ok(_) => tracing::info!(
            "api_token не был задан — сгенерирован новый (32 байта) и сохранён в config.json"
        ),
        Err(e) => tracing::error!(
            "не удалось сохранить сгенерированный api_token ({e}) — \
             REST API останется на update_api_key"
        ),
    }
}

pub async fn serve(state: Arc<scheduler::State>) -> anyhow::Result<()> {
    // Один раз при старте (serve вызывается и службой, и CLI-режимом).
    ensure_api_token(&state.cfg);

    kill_port_holder(crate::API_ADDR).await;

    // Spawn job GC
    tokio::spawn(job_gc_loop(state.clone()));

    let app = build_app(state);

    let base_port: u16 = crate::API_ADDR.rsplit(':').next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(17345);

    let mut actual_port = base_port;
    let mut listener = None;
    for port in base_port..=(base_port + 9) {
        if let Ok(sock) = tokio::net::TcpSocket::new_v4() {
            let _ = sock.set_reuseaddr(true);
            let addr = format!("127.0.0.1:{port}");
            if let Ok(sa) = addr.parse() {
                if sock.bind(sa).is_ok() {
                    if let Ok(l) = sock.listen(1024) {
                        make_socket_non_inheritable(&l);
                        actual_port = port;
                        listener = Some(l);
                        break;
                    }
                }
            }
        }
    }
    let listener = listener
        .ok_or_else(|| anyhow::anyhow!("No free port in 127.0.0.1:{}..{}", base_port, base_port + 9))?;

    // Атомарно: крэш посреди прямого write оставлял обрезанный api.port и GUI
    // навсегда стучался не туда. write_atomic из core пишет JSON, а тут нужен
    // голый номер порта — делаем тот же tmp+rename вручную.
    let port_file = restlex_core::program_data().join("api.port");
    let tmp = port_file.with_extension(format!("tmp.{}", std::process::id()));
    match std::fs::write(&tmp, actual_port.to_string()) {
        Ok(()) => {
            if let Err(e) = std::fs::rename(&tmp, &port_file) {
                tracing::error!("cannot publish api.port ({e}) — GUI будет стучаться на старый порт");
                let _ = std::fs::remove_file(&tmp);
            }
        }
        Err(e) => {
            // Раньше молчали: GUI навсегда оставался на старом порту без следа в логе.
            tracing::error!("cannot write {} ({e}) — GUI будет стучаться на старый порт", tmp.display());
            let _ = std::fs::remove_file(&tmp);
        }
    }

    if actual_port != base_port {
        tracing::warn!("Port {} busy (zombie socket?), API using {}", base_port, actual_port);
    }
    tracing::info!("API listening on 127.0.0.1:{}", actual_port);
    axum::serve(listener, app).await?;
    Ok(())
}

// ── Internal handlers (existing) ─────────────────────────────────────────────

async fn status(State(st): State<Arc<scheduler::State>>) -> Json<StatusResp> {
    let profiles = st.cfg.profiles();
    let running = st.running.lock().unwrap()
        .iter().filter(|(_, v)| **v).map(|(k, _)| k.clone()).collect();
    let log = st.last_log.lock().unwrap()
        .iter().rev().take(50).cloned().collect::<Vec<_>>();
    Json(StatusResp {
        version: crate::agent_version().into(),
        profiles: profiles.len(),
        running_now: running,
        log_tail: log,
    })
}

async fn log(State(st): State<Arc<scheduler::State>>) -> Json<Vec<String>> {
    Json(st.last_log.lock().unwrap().clone())
}

async fn run_now(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
) -> Json<serde_json::Value> {
    match st.cfg.profile(&pid) {
        Some(p) => {
            let st2 = st.clone();
            let pid2 = pid.clone();
            tokio::spawn(async move {
                let _ = scheduler::run_backup(st2, pid2, p).await;
            });
            Json(serde_json::json!({"ok": true, "pid": pid}))
        }
        None => Json(serde_json::json!({"ok": false, "error": "profile not found"})),
    }
}

async fn reload(State(st): State<Arc<scheduler::State>>) -> Json<serde_json::Value> {
    match st.cfg.reload() {
        Ok(_) => Json(serde_json::json!({"ok": true})),
        Err(e) => Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    }
}

async fn mount_status(State(st): State<Arc<scheduler::State>>) -> Json<crate::mount::MountStatus> {
    Json(crate::mount::status(&st))
}

async fn mount_start(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
) -> Json<serde_json::Value> {
    match crate::mount::start(st, pid.clone()).await {
        Ok(info) => Json(serde_json::json!({"ok": true, "mount": info})),
        Err(e) => Json(serde_json::json!({"ok": false, "error": e.to_string(), "pid": pid})),
    }
}

async fn mount_stop(State(st): State<Arc<scheduler::State>>) -> Json<serde_json::Value> {
    match crate::mount::stop(st).await {
        Ok(info) => Json(serde_json::json!({"ok": true, "mount": info})),
        Err(e) => Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    }
}

async fn last_run(State(_st): State<Arc<scheduler::State>>, Path(pid): Path<String>) -> Json<serde_json::Value> {
    let path = restlex_core::logs_dir().join(format!("{pid}-last-run.json"));
    match std::fs::read_to_string(&path) {
        Ok(s) => Json(serde_json::from_str(&s).unwrap_or(serde_json::json!({"ok":false,"error":"parse error"}))),
        Err(_) => Json(serde_json::json!({"ok":false,"error":"no data"})),
    }
}

async fn all_last_runs(State(st): State<Arc<scheduler::State>>) -> Json<serde_json::Value> {
    let logs = restlex_core::logs_dir();
    let mut result = serde_json::Map::new();
    for (pid, _) in st.cfg.profiles() {
        let path = logs.join(format!("{pid}-last-run.json"));
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                result.insert(pid, v);
            }
        }
    }
    Json(serde_json::Value::Object(result))
}

async fn rs_status(State(st): State<Arc<scheduler::State>>) -> Json<rest_server::Status> {
    let session = st.rest_server.clone();
    let status = tokio::task::spawn_blocking(move || rest_server::status(&session))
        .await
        .unwrap_or(rest_server::Status { running: false, intentional_stop: true, port: "8000".to_string(), enabled: false });
    Json(status)
}

async fn rs_start(State(st): State<Arc<scheduler::State>>) -> Json<serde_json::Value> {
    let session = st.rest_server.clone();
    let cfg = rest_server::load_config();
    let en = st.cfg.globals().en();
    match tokio::task::spawn_blocking(move || rest_server::start(&session, &cfg, en)).await {
        Ok(Ok(())) => Json(serde_json::json!({"ok": true})),
        Ok(Err(e)) => Json(serde_json::json!({"ok": false, "error": e.to_string()})),
        Err(e)     => Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    }
}

async fn rs_stop(State(st): State<Arc<scheduler::State>>) -> Json<serde_json::Value> {
    let session = st.rest_server.clone();
    tokio::task::spawn_blocking(move || rest_server::stop(&session)).await.ok();
    Json(serde_json::json!({"ok": true}))
}

#[derive(serde::Serialize)]
struct UpdateStatusResp {
    current_version: String,
    latest_version: String,
    gui_version: String,
    gui_latest_version: String,
    last_check: Option<String>,
    phase: String,
    auto_update: bool,
}

async fn update_status(State(st): State<Arc<scheduler::State>>) -> Json<UpdateStatusResp> {
    let phase      = st.update_phase.lock().unwrap().to_string();
    let latest     = st.update_latest_version.lock().unwrap().clone();
    let gui_latest = st.update_latest_gui_version.lock().unwrap().clone();
    let last_check = st.update_last_check.lock().unwrap().map(|dt| dt.to_rfc3339());
    let auto_update = st.cfg.globals().auto_update;
    Json(UpdateStatusResp {
        current_version: crate::agent_version().to_string(),
        latest_version: latest,
        gui_version: {
            let v = restlex_core::read_launcher_version();
            if v.is_empty() { "?".into() } else { v }
        },
        gui_latest_version: gui_latest,
        last_check,
        phase,
        auto_update,
    })
}

async fn update_check(State(st): State<Arc<scheduler::State>>) -> Json<serde_json::Value> {
    updater::check_now(&st);
    Json(serde_json::json!({"ok": true}))
}

async fn update_apply(State(st): State<Arc<scheduler::State>>) -> Json<serde_json::Value> {
    updater::force_apply_now(&st);
    Json(serde_json::json!({"ok": true}))
}

// ── REST API v1 handlers ──────────────────────────────────────────────────────

/// GET /api/v1/profiles
async fn v1_profiles(State(st): State<Arc<scheduler::State>>) -> Json<serde_json::Value> {
    let list: Vec<_> = st.cfg.profiles()
        .iter()
        .map(|(pid, p)| profile_public(pid, p))
        .collect();
    Json(serde_json::json!({"ok": true, "profiles": list}))
}

/// GET /api/v1/profiles/:pid
async fn v1_profile(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match st.cfg.profile(&pid) {
        Some(p) => (StatusCode::OK,        Json(serde_json::json!({"ok": true, "profile": profile_public(&pid, &p)}))),
        None    => (StatusCode::NOT_FOUND,  Json(serde_json::json!({"ok": false, "error": "profile not found"}))),
    }
}

/// POST /api/v1/profiles/:pid/backup
/// Запускает бэкап. Возвращает 409 Conflict если бэкап уже идёт.
async fn v1_backup(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let profile = match st.cfg.profile(&pid) {
        Some(p) => p,
        None    => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": "profile not found"}))),
    };
    // Атомарная бронь для конкурентных HTTP-запросов; guard живёт до конца
    // run_backup и снимается даже при panic/отмене задачи (Drop).
    let Some(reservation) = ApiBackupReservation::acquire(&pid) else {
        return (StatusCode::CONFLICT, Json(serde_json::json!({"ok": false, "error": "backup already starting"})));
    };
    let already_running = *st.running.lock().unwrap().get(&pid).unwrap_or(&false);
    if already_running {
        return (StatusCode::CONFLICT, Json(serde_json::json!({"ok": false, "error": "backup already running"})));
    }
    let st2  = st.clone();
    let pid2 = pid.clone();
    tokio::spawn(async move {
        let _reservation = reservation;
        let _ = scheduler::run_backup(st2, pid2, profile).await;
    });
    (StatusCode::OK, Json(serde_json::json!({"ok": true, "pid": pid})))
}

/// POST /api/v1/profiles — создать профиль. Тело = поля профиля (name, sources,
/// exclude, repo_type, repo_path, repo_password, use_vss, retention, sched,
/// cred_* …). Возвращает 201 + сгенерированный pid. История прогонов
/// (last_run/last_status) и created/updated — server-owned, из тела игнорируются.
async fn v1_create_profile(
    State(st): State<Arc<scheduler::State>>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut profile: restlex_core::Profile = match serde_json::from_value(body) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": format!("invalid profile: {e}")}))),
    };
    // На create прежних секретов нет — пустой repo_password без глобального
    // password-file = заведомо битый профиль, отклоняем.
    if !password_ok(&profile, &st.cfg.globals().default_password_file) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "repo_password is required (no default password file configured)"})));
    }
    if let Err(e) = validate_profile(&profile) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": e})));
    }
    // Клиент не задаёт историю прогонов и метки времени — их владеет агент.
    profile.last_run = None;
    profile.last_status = None;
    profile.created = None;
    profile.updated = None;
    let pid = new_pid(&st.cfg.profiles());
    match st.cfg.save_profile(&pid, profile.clone()) {
        Ok(()) => {
            // В лог — только имя и pid, НИКОГДА пароль/креды.
            tracing::info!("api: profile created '{}' ({})", profile.name, pid);
            let saved = st.cfg.profile(&pid).unwrap_or(profile);
            (StatusCode::CREATED, Json(serde_json::json!({"ok": true, "pid": pid, "profile": profile_public(&pid, &saved)})))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"ok": false, "error": e.to_string()}))),
    }
}

/// PUT /api/v1/profiles/:pid — обновить профиль. Тело = те же поля, что при
/// создании. created и историю прогонов сохраняем со стороны сервера (клиент их
/// перезаписать не может). 404 если профиля нет.
async fn v1_update_profile(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !valid_pid(&pid) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "bad pid"})));
    }
    let existing = match st.cfg.profile(&pid) {
        Some(p) => p,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": "profile not found"}))),
    };
    let mut profile: restlex_core::Profile = match serde_json::from_value(body) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": format!("invalid profile: {e}")}))),
    };
    // P0-фикс: смержить секреты из existing ДО валидации — GET маскирует пароль и
    // cred_*, поэтому штатное тело PUT приходит с пустыми секретами и без мержа
    // затёрло бы их (все будущие бэкапы сломались бы молча).
    merge_secrets(&mut profile, &existing);
    // Сохраняем server-owned поля от текущей версии (клиент не стирает историю).
    profile.last_run = existing.last_run;
    profile.last_status = existing.last_status;
    profile.created = existing.created;
    profile.updated = None; // save_profile проставит now
    // Пароль проверяем ПОСЛЕ мержа: если и в теле, и в existing пусто (и нет
    // password-file) — профиль битый, отклоняем.
    if !password_ok(&profile, &st.cfg.globals().default_password_file) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "repo_password is required (no default password file configured)"})));
    }
    if let Err(e) = validate_profile(&profile) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": e})));
    }
    match st.cfg.save_profile(&pid, profile) {
        Ok(()) => {
            tracing::info!("api: profile updated ({})", pid);
            let saved = st.cfg.profile(&pid);
            match saved {
                Some(p) => (StatusCode::OK, Json(serde_json::json!({"ok": true, "pid": pid, "profile": profile_public(&pid, &p)}))),
                None => (StatusCode::OK, Json(serde_json::json!({"ok": true, "pid": pid}))),
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"ok": false, "error": e.to_string()}))),
    }
}

/// DELETE /api/v1/profiles/:pid — удалить профиль. 404 если его нет.
async fn v1_delete_profile(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !valid_pid(&pid) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "bad pid"})));
    }
    if st.cfg.profile(&pid).is_none() {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": "profile not found"})));
    }
    match st.cfg.del_profile(&pid) {
        Ok(()) => {
            tracing::info!("api: profile deleted ({})", pid);
            (StatusCode::OK, Json(serde_json::json!({"ok": true, "pid": pid})))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"ok": false, "error": e.to_string()}))),
    }
}

/// Общая реализация pause/resume: выставляет sched.enabled и сохраняет профиль.
async fn set_enabled(
    st: Arc<scheduler::State>,
    pid: String,
    enabled: bool,
) -> (StatusCode, Json<serde_json::Value>) {
    if !valid_pid(&pid) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "bad pid"})));
    }
    let mut profile = match st.cfg.profile(&pid) {
        Some(p) => p,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": "profile not found"}))),
    };
    profile.sched.enabled = enabled;
    match st.cfg.save_profile(&pid, profile) {
        Ok(()) => {
            tracing::info!("api: profile {} ({})", if enabled { "resumed" } else { "paused" }, pid);
            (StatusCode::OK, Json(serde_json::json!({"ok": true, "pid": pid, "enabled": enabled})))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"ok": false, "error": e.to_string()}))),
    }
}

/// POST /api/v1/profiles/:pid/pause — снять профиль с расписания (ручной backup
/// по-прежнему возможен).
async fn v1_pause(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    set_enabled(st, pid, false).await
}

/// POST /api/v1/profiles/:pid/resume — вернуть профиль на расписание.
async fn v1_resume(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    set_enabled(st, pid, true).await
}

/// GET /api/v1/profiles/:pid/status
async fn v1_status(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if st.cfg.profile(&pid).is_none() {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": "profile not found"})));
    }
    let running = *st.running.lock().unwrap().get(&pid).unwrap_or(&false);
    let last_run_path = restlex_core::logs_dir().join(format!("{pid}-last-run.json"));
    let last_run: Option<serde_json::Value> = std::fs::read_to_string(&last_run_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    (StatusCode::OK, Json(serde_json::json!({"ok": true, "running": running, "last_run": last_run})))
}

/// Ключи глобальных настроек, которые никогда не отдаются наружу.
const GLOBAL_SECRET_KEYS: &[&str] = &["tg_token", "report_api_key", "update_api_key", "api_token"];

/// Белый список ключей, которые разрешено менять через PATCH /api/v1/settings.
///
/// Именно белый, а не чёрный: агент работает под LocalSystem, и запись
/// произвольного поля `Globals` по сети — это выполнение произвольного кода.
/// Прежде всего `restic` (путь к бинарнику: подменил — и твой exe запускается
/// как SYSTEM с RESTIC_REPOSITORY/RESTIC_PASSWORD в окружении), а также
/// `default_password_file` и `encrypt_secrets`. Такие вещи меняются только
/// локально — в GUI или через CLI на самой машине.
const PATCHABLE_SETTINGS: &[&str] = &[
    "tg_chat", "tg_group", "tg_fatal", "tg_file", "tg_prune",
    "global_no_scan", "global_vss_timeout_s", "global_pre_backup_cmd",
    "global_backup_timeout_hours", "global_post_backup_check",
    "auto_update", "update_check_interval_m",
    "net_retry_count", "net_retry_interval_s",
    "temp", "cache_cleanup",
    "sleep_on_lan_listen",
];

/// GET /api/v1/settings — глобальные настройки без секретов.
/// Вместо значения секрета отдаём `<key>_set: bool`, как это сделано для
/// профилей (`has_password` / `creds_set`).
async fn v1_settings(State(st): State<Arc<scheduler::State>>) -> (StatusCode, Json<serde_json::Value>) {
    let g = st.cfg.globals();
    let mut v = match serde_json::to_value(&g) {
        Ok(v) => v,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
                          Json(serde_json::json!({"ok": false, "error": e.to_string()}))),
    };
    if let Some(obj) = v.as_object_mut() {
        for k in GLOBAL_SECRET_KEYS {
            let set = obj.get(*k).and_then(|x| x.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
            obj.remove(*k);
            obj.insert(format!("{k}_set"), serde_json::Value::Bool(set));
        }
    }
    (StatusCode::OK, Json(serde_json::json!({"ok": true, "settings": v})))
}

/// PATCH /api/v1/settings — наложить частичный патч на глобальные настройки.
/// Принимает только те ключи, которые прислали; остальное на диске не трогает.
async fn v1_patch_settings(
    State(st): State<Arc<scheduler::State>>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let patch = match body.as_object() {
        Some(o) if !o.is_empty() => o.clone(),
        Some(_) => return (StatusCode::BAD_REQUEST,
                           Json(serde_json::json!({"ok": false, "error": "empty patch"}))),
        None => return (StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"ok": false, "error": "body must be a JSON object"}))),
    };
    // `<key>_set` — вывод GET-а, а не поле конфига; молча приняв его, мы бы
    // записали в config.json мусорный ключ.
    for k in patch.keys() {
        if k.ends_with("_set") {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "ok": false,
                "error": format!("'{k}' is read-only (output of GET); send '{}' instead",
                                 k.trim_end_matches("_set"))})));
        }
        if !PATCHABLE_SETTINGS.contains(&k.as_str()) {
            return (StatusCode::FORBIDDEN, Json(serde_json::json!({
                "ok": false,
                "error": format!("'{k}' is not patchable over the API (change it locally)"),
                "patchable": PATCHABLE_SETTINGS})));
        }
    }
    match st.cfg.patch_globals(&patch) {
        Ok(_) => {
            let keys: Vec<&String> = patch.keys().collect();
            tracing::info!("api: settings patched ({:?})", keys);
            v1_settings(State(st)).await
        }
        // Невалидный тип значения — ошибка клиента, а не сервера.
        Err(restlex_core::Error::BadPatch(msg)) =>
            (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": msg}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR,
                   Json(serde_json::json!({"ok": false, "error": e.to_string()}))),
    }
}

/// POST /api/v1/reload — перечитать конфиг с диска.
/// Дубль неаутентифицируемого `/reload`: интеграциям предписано ходить только
/// в `/api/v1/*`.
async fn v1_reload(State(st): State<Arc<scheduler::State>>) -> (StatusCode, Json<serde_json::Value>) {
    match st.cfg.reload() {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR,
                   Json(serde_json::json!({"ok": false, "error": e.to_string()}))),
    }
}

/// POST /api/v1/profiles/:pid/stop — прервать идущую операцию профиля:
/// и бэкап, и restore (см. `scheduler::pick_stop_target`). Одновременно у
/// профиля идёт только одна из них — обе держат `running[pid]`.
async fn v1_stop(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !valid_pid(&pid) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "bad pid"})));
    }
    if st.cfg.profile(&pid).is_none() {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": "profile not found"})));
    }
    if !*st.running.lock().unwrap().get(&pid).unwrap_or(&false) {
        return (StatusCode::CONFLICT, Json(serde_json::json!({"ok": false, "error": "backup is not running"})));
    }
    match scheduler::stop_backup(&st, &pid) {
        Ok(n) => (StatusCode::OK, Json(serde_json::json!({"ok": true, "killed": n}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR,
                   Json(serde_json::json!({"ok": false, "error": e.to_string()}))),
    }
}

/// POST /api/v1/profiles/:pid/test — проверить доступность репозитория.
/// Async job: `restic cat config` — дешевле, чем snapshots, и проверяет
/// и сеть, и пароль сразу.
async fn v1_test_connection(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !valid_pid(&pid) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "bad pid"})));
    }
    let profile = match st.cfg.profile(&pid) {
        Some(p) => p,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": "profile not found"}))),
    };
    let job_id = new_job_id();
    st.jobs.lock().unwrap().insert(job_id.clone(), Job {
        id: job_id.clone(),
        kind: "test".into(),
        status: JobStatus::Running,
        result: None,
        created_at: now_unix(),
        finished_at: None,
    });
    let st2     = st.clone();
    let jid     = job_id.clone();
    let cfg_ref = st.cfg.clone();
    tokio::spawn(async move {
        let runner = scheduler::build_runner(&cfg_ref, &profile);
        // Недоступный репозиторий — это валидный ответ («не достучались»),
        // а не сбой самой проверки: job завершается Done в обоих случаях.
        let result = match runner.check_access().await {
            Ok(()) => serde_json::json!({"reachable": true}),
            Err(e) => serde_json::json!({"reachable": false, "error": e.to_string()}),
        };
        let mut jobs = st2.jobs.lock().unwrap();
        if let Some(j) = jobs.get_mut(&jid) {
            j.status = JobStatus::Done;
            j.result = Some(result);
            j.finished_at = Some(now_unix());
        }
    });
    (StatusCode::ACCEPTED, Json(serde_json::json!({"ok": true, "job_id": job_id})))
}

/// GET /api/v1/snapshots/:pid
/// Запускает async job: restic snapshots --json.
async fn v1_snapshots(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let profile = match st.cfg.profile(&pid) {
        Some(p) => p,
        None    => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": "profile not found"}))),
    };
    let job_id = new_job_id();
    let job = Job {
        id: job_id.clone(),
        kind: "snapshots".into(),
        status: JobStatus::Running,
        result: None,
        created_at: now_unix(),
        finished_at: None,
    };
    st.jobs.lock().unwrap().insert(job_id.clone(), job);

    let st2     = st.clone();
    let jid     = job_id.clone();
    let cfg_ref = st.cfg.clone();
    tokio::spawn(async move {
        let runner = scheduler::build_runner(&cfg_ref, &profile);
        let (status, result) = match runner.snapshots_json().await {
            Ok(json_str) => {
                let parsed: serde_json::Value = serde_json::from_str(&json_str)
                    .unwrap_or(serde_json::Value::String(json_str));
                (JobStatus::Done, serde_json::json!({"snapshots": parsed}))
            }
            Err(e) => (JobStatus::Failed, serde_json::json!({"error": e.to_string()})),
        };
        let mut jobs = st2.jobs.lock().unwrap();
        if let Some(j) = jobs.get_mut(&jid) {
            j.status = status;
            j.result = Some(result);
            j.finished_at = Some(now_unix());
        }
    });

    (StatusCode::ACCEPTED, Json(serde_json::json!({"ok": true, "job_id": job_id})))
}

// -- Deleted (снимок vs диск) -------------------------------------------------

/// Путь снимка (`/C/Users/x`) -> локальный (`C:\Users\x`). Первый компонент --
/// буква диска (`C` или `C:`); иначе путь не наш и сравнивать нечего.
fn snap_to_local(snap_path: &str) -> Option<std::path::PathBuf> {
    let mut parts = snap_path.split('/').filter(|p| !p.is_empty());
    let first = parts.next()?;
    let b = first.as_bytes();
    let drive_ok = (b.len() == 1 && b[0].is_ascii_alphabetic())
        || (b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':');
    if !drive_ok {
        return None;
    }
    let mut p = std::path::PathBuf::from(format!("{}:\\", (b[0] as char).to_ascii_uppercase()));
    for c in parts {
        p.push(c);
    }
    Some(p)
}

/// Разбор вывода `restic ls --json` и сравнение с диском. `exists` вынесен в
/// параметр, чтобы логика проверялась тестом без файловой системы.
/// Каталог, которого нет на диске, запоминается: всё внутри него удалено,
/// и по файлам его содержимое не проверяется.
fn compute_deleted(
    ndjson: &str,
    folder: &str,
    recursive: bool,
    exists: impl Fn(&std::path::Path) -> bool,
) -> serde_json::Value {
    const MAX_ITEMS: usize = 50_000;
    let folder = folder.trim_end_matches('/');
    let mut missing: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut items = Vec::new();
    let (mut checked, mut count) = (0usize, 0usize);
    for line in ndjson.lines() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("struct_type").and_then(|x| x.as_str()) != Some("node") {
            continue;
        }
        let path = match v.get("path").and_then(|x| x.as_str()) {
            Some(p) if !p.is_empty() && p != "/" => p.trim_end_matches('/').to_string(),
            _ => continue,
        };
        let parent = path.rsplit_once('/').map(|(h, _)| h).unwrap_or("");
        if !recursive && parent != folder {
            continue;
        }
        let is_dir = v.get("type").and_then(|x| x.as_str()) == Some("dir");
        let gone = if missing.contains(parent) {
            true
        } else {
            match snap_to_local(&path) {
                Some(local) => !exists(&local),
                None => continue,
            }
        };
        if is_dir {
            if gone {
                missing.insert(path.clone());
                // В рекурсивном режиме папку отдельно не выдаём -- её файлы
                // попадут в список сами; в режиме «папка» это и есть результат.
                if recursive {
                    continue;
                }
            } else {
                continue;
            }
        } else {
            checked += 1;
            if !gone {
                continue;
            }
        }
        count += 1;
        if items.len() < MAX_ITEMS {
            items.push(serde_json::json!({
                "path": path,
                "type": if is_dir { "dir" } else { "file" },
                "size": v.get("size").cloned().unwrap_or(serde_json::Value::Null),
                "mtime": v.get("mtime").cloned().unwrap_or(serde_json::Value::Null),
            }));
        }
    }
    serde_json::json!({
        "folder": if folder.is_empty() { "/" } else { folder },
        "recursive": recursive,
        "count": count,
        "checked_files": checked,
        "truncated": count > items.len(),
        "deleted": items,
    })
}

#[derive(serde::Deserialize)]
struct DeletedReq {
    #[serde(default = "default_snapshot")]
    snapshot_id: String,
    /// Папка в снимке (`/C/Users/x`). Пусто/`/` + `recursive` -- весь снимок.
    #[serde(default)]
    path: String,
    /// true (по умолчанию) -- папка и всё поддерево; false -- только содержимое самой папки.
    #[serde(default = "default_true")]
    recursive: bool,
}

/// POST /api/v1/profiles/:pid/deleted
/// Тело: {"snapshot_id": "latest"|"<id>", "path": "/C/Users/x", "recursive": true}
/// Сравнивает снимок с локальным диском ЭТОЙ машины и возвращает объекты, которых
/// на диске нет. Async job (202 + job_id), результат -- GET /api/v1/jobs/:job_id:
/// {"folder", "recursive", "count", "checked_files", "truncated", "deleted": [{path,type,size,mtime}]}.
/// По умолчанию (recursive=true) проверяется папка и всё поддерево -- по сети может занять минуты;
/// recursive=false читает только содержимое самой папки и быстр даже по сети.
async fn v1_deleted(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
    Json(req): Json<DeletedReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    let profile = match st.cfg.profile(&pid) {
        Some(p) => p,
        None    => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": "profile not found"}))),
    };
    if !valid_snapshot_id(&req.snapshot_id) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "invalid snapshot_id"})));
    }
    let path = req.path.trim().to_string();
    if path.starts_with('-') || path.contains('\0') || (!path.is_empty() && !path.starts_with('/')) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "path must be a snapshot path like /C/Users/x"})));
    }
    let whole = path.is_empty() || path == "/";
    if whole && !req.recursive {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "specify a folder path, or recursive=true for the whole snapshot"})));
    }
    let ls_path = if whole { String::new() } else { path.clone() };

    let job_id = new_job_id();
    st.jobs.lock().unwrap().insert(job_id.clone(), Job {
        id: job_id.clone(),
        kind: "deleted".into(),
        status: JobStatus::Running,
        result: None,
        created_at: now_unix(),
        finished_at: None,
    });

    let st2     = st.clone();
    let jid     = job_id.clone();
    let cfg_ref = st.cfg.clone();
    let recursive = req.recursive;
    let snap = req.snapshot_id.clone();
    tokio::spawn(async move {
        let runner = scheduler::build_runner(&cfg_ref, &profile);
        let (status, result) = match runner.ls_json_ex(&snap, &ls_path, recursive).await {
            Ok(out) => {
                let folder = if whole { "/".to_string() } else { path };
                // Файловые проверки блокирующие -- не держим ими async-воркер.
                match tokio::task::spawn_blocking(move || {
                    compute_deleted(&out, &folder, recursive, |p| std::fs::symlink_metadata(p).is_ok())
                }).await {
                    Ok(v)  => (JobStatus::Done, v),
                    Err(e) => (JobStatus::Failed, serde_json::json!({"error": e.to_string()})),
                }
            }
            Err(e) => (JobStatus::Failed, serde_json::json!({"error": e.to_string()})),
        };
        let mut jobs = st2.jobs.lock().unwrap();
        if let Some(j) = jobs.get_mut(&jid) {
            j.status = status;
            j.result = Some(result);
            j.finished_at = Some(now_unix());
        }
    });

    (StatusCode::ACCEPTED, Json(serde_json::json!({"ok": true, "job_id": job_id})))
}

// -- Restore -----------------------------------------------------------------

/// Идентификатор снапшота на границе доверия: только `latest` или голый
/// hex/alnum-id. Всё остальное отбрасываем -- снапшот уезжает в командную
/// строку restic, и значение вида `--option` там стало бы лишним аргументом.
fn valid_snapshot_id(s: &str) -> bool {
    if s == "latest" {
        return true;
    }
    (4..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Каталог назначения: только абсолютный локальный путь (`X:\...`) или UNC
/// (`\\server\share\...`). Относительные пути, пустая строка, `..` и `-`
/// в начале запрещены: первое сложилось бы относительно рабочего каталога
/// службы (SYSTEM), последнее -- превратилось бы в флаг restic.
fn validate_restore_target(t: &str) -> std::result::Result<(), String> {
    let t = t.trim();
    if t.is_empty() {
        return Err("target is empty".into());
    }
    if t.starts_with('-') {
        return Err("target must not start with '-'".into());
    }
    if t.contains('\0') {
        return Err("target contains NUL".into());
    }
    let b = t.as_bytes();
    let drive = b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/');
    let unc = t.starts_with("\\\\") && t.len() > 2;
    if !(drive || unc) {
        return Err("target must be an absolute path".into());
    }
    // Обход вверх: сравниваем по компонентам, чтобы не ловить ложное
    // срабатывание на легальном имени вроде `C:\arch..2024`.
    if t.split(['\\', '/']).any(|c| c == "..") {
        return Err("target must not contain '..'".into());
    }
    Ok(())
}

/// `--include`-маски внутри снапшота. Пустые пропускаем (restic их и так
/// игнорирует), ведущий `-` запрещаем, количество ограничиваем.
fn validate_restore_includes(v: &[String]) -> std::result::Result<Vec<String>, String> {
    if v.len() > 64 {
        return Err("too many include paths (max 64)".into());
    }
    let mut out = Vec::new();
    for i in v {
        let i = i.trim();
        if i.is_empty() {
            continue;
        }
        if i.starts_with('-') {
            return Err("include path must not start with '-'".into());
        }
        if i.contains('\0') {
            return Err("include path contains NUL".into());
        }
        out.push(i.to_string());
    }
    Ok(out)
}

#[derive(serde::Deserialize)]
struct RestoreReq {
    #[serde(default = "default_snapshot")]
    snapshot_id: String,
    #[serde(default)]
    target: String,
    /// Синонимы: `include` (основной) и `paths` -- панель шлёт то одно, то другое.
    #[serde(default, alias = "paths")]
    include: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_snapshot() -> String {
    "latest".to_string()
}

/// POST /api/v1/profiles/:pid/restore
/// Тело: {"snapshot_id": "latest"|"<id>", "target": "D:\\restore", "include": [...]}
/// Возвращает 202 + job_id; прогресс и результат -- через GET /api/v1/jobs/:job_id.
/// Профиль на время restore помечается running, и прерывает его тот же
/// POST /api/v1/profiles/:pid/stop, что и бэкап: stop смотрит и ключ бэкапа,
/// и `restore_key(pid)`. Прерванный job получает статус `cancelled`.
async fn v1_restore(
    State(st): State<Arc<scheduler::State>>,
    Path(pid): Path<String>,
    Json(req): Json<RestoreReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !valid_pid(&pid) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "bad pid"})));
    }
    let profile = match st.cfg.profile(&pid) {
        Some(p) => p,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": "profile not found"}))),
    };
    if !valid_snapshot_id(&req.snapshot_id) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": "bad snapshot_id"})));
    }
    if let Err(e) = validate_restore_target(&req.target) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": e})));
    }
    let includes = match validate_restore_includes(&req.include) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false, "error": e}))),
    };
    let target = req.target.trim().to_string();

    // Один профиль -- одна операция: restore и backup делят флаг running,
    // иначе они писали бы в один репозиторий/цель одновременно.
    {
        let mut running = st.running.lock().unwrap();
        if *running.get(&pid).unwrap_or(&false) {
            return (StatusCode::CONFLICT, Json(serde_json::json!({"ok": false, "error": "profile is busy"})));
        }
        running.insert(pid.clone(), true);
    }

    let job_id = new_job_id();
    st.jobs.lock().unwrap().insert(job_id.clone(), Job {
        id: job_id.clone(),
        kind: "restore".into(),
        status: JobStatus::Running,
        result: Some(serde_json::json!({"percent": 0.0, "target": target})),
        created_at: now_unix(),
        finished_at: None,
    });

    let st2     = st.clone();
    let jid     = job_id.clone();
    let pid2    = pid.clone();
    let snap    = req.snapshot_id.clone();
    let cfg_ref = st.cfg.clone();
    tokio::spawn(async move {
        let runner = scheduler::build_runner(&cfg_ref, &profile);
        let st_p   = st2.clone();
        let jid_p  = jid.clone();
        let rkey_p = scheduler::restore_key(&pid2);
        let res = runner
            .restore_with_progress(&snap, &target, &includes, move |ev| {
                use restlex_core::restic::RestoreProgress as RP;
                match ev {
                    RP::Started(os_pid) => {
                        // Ключ ОТДЕЛЬНЫЙ от бэкапа ("restore:<pid>"): под общим
                        // ключом завершившийся restore снимал регистрацию
                        // идущего бэкапа того же профиля, а stop бэкапа убивал
                        // процесс restore.
                        st_p.restic_pids.lock().unwrap().insert(rkey_p.clone(), os_pid);
                    }
                    RP::Status { percent, files_restored, total_files, bytes_restored, total_bytes } => {
                        let mut jobs = st_p.jobs.lock().unwrap();
                        if let Some(j) = jobs.get_mut(&jid_p) {
                            j.result = Some(serde_json::json!({
                                "percent": percent,
                                "files_restored": files_restored,
                                "total_files": total_files,
                                "bytes_restored": bytes_restored,
                                "total_bytes": total_bytes,
                            }));
                        }
                    }
                    RP::Line(_) | RP::Done(_) => {}
                }
            })
            .await;

        // Снимаем регистрацию по СВОЕМУ ключу и забираем метку «прервано вручную»:
        // её ставит stop_backup перед kill. Метку обязательно СНИМАЕМ — раньше
        // os_pid оставался в CANCELLED навсегда, и после переиспользования PID
        // системой реально упавший бэкап помечался cancelled, Telegram-алерт о
        // сбое не уходил.
        let os_pid = st2.restic_pids.lock().unwrap().remove(&scheduler::restore_key(&pid2));
        let was_cancelled = os_pid
            .map(|p| scheduler::CANCELLED.lock().unwrap().remove(&p))
            .unwrap_or(false);

        // Прерванный restore — не сбой: отдельный статус `cancelled`, чтобы клиент
        // отличил его и от успеха (файлы восстановлены ЧАСТИЧНО), и от настоящей
        // ошибки. Текст от убитого restic кладём в `error` — он бывает полезен,
        // но статус по нему не судят.
        let (status, result) = if was_cancelled {
            (
                JobStatus::Cancelled,
                serde_json::json!({
                    "cancelled": true,
                    "partial": true,
                    "target": target,
                    "error": res.err()
                        .map(|e| restlex_core::restic::mask_repo_creds(&e.to_string())),
                }),
            )
        } else { match res {
            Ok(sum) => (JobStatus::Done, serde_json::json!({"summary": sum, "target": target})),
            Err(e) => (
                JobStatus::Failed,
                serde_json::json!({"error": restlex_core::restic::mask_repo_creds(&e.to_string())}),
            ),
        }};
        st2.running.lock().unwrap().insert(pid2.clone(), false);
        let mut jobs = st2.jobs.lock().unwrap();
        if let Some(j) = jobs.get_mut(&jid) {
            j.status = status;
            j.result = Some(result);
            j.finished_at = Some(now_unix());
        }
    });

    (StatusCode::ACCEPTED, Json(serde_json::json!({"ok": true, "job_id": job_id})))
}

/// GET /api/v1/jobs/:job_id
async fn v1_job(
    State(st): State<Arc<scheduler::State>>,
    Path(job_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let jobs = st.jobs.lock().unwrap();
    match jobs.get(&job_id) {
        Some(j) => (StatusCode::OK,       Json(serde_json::json!({"ok": true, "job": j}))),
        None    => (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok": false, "error": "job not found"}))),
    }
}

#[cfg(test)]
mod tests {
    use super::{compute_deleted, merge_secrets, netstat_port_holder, password_ok, snap_to_local,
               token_missing, valid_pid, valid_snapshot_id, validate_restore_target,
               validate_restore_includes};
    use restlex_core::Profile;

    #[test]
    fn snap_to_local_maps_drive_paths() {
        assert_eq!(snap_to_local("/C/Users/Lex").unwrap(), std::path::PathBuf::from("C:\\Users\\Lex"));
        assert_eq!(snap_to_local("/d:/x").unwrap(), std::path::PathBuf::from("D:\\x"));
        assert!(snap_to_local("/Users/Lex").is_none());
        assert!(snap_to_local("/").is_none());
    }

    #[test]
    fn compute_deleted_folder_and_recursive() {
        let n = |p: &str, t: &str| format!(r#"{{"struct_type":"node","path":"{p}","type":"{t}","size":1,"mtime":"2026-01-01T00:00:00Z"}}"#);
        let out = [
            r#"{"struct_type":"snapshot","id":"abc"}"#.to_string(),
            n("/C/data", "dir"),
            n("/C/data/keep.txt", "file"),
            n("/C/data/gone.txt", "file"),
            n("/C/data/gonedir", "dir"),
            n("/C/data/gonedir/a.txt", "file"),
        ].join("\n");
        // на «диске» есть только data и keep.txt
        let exists = |p: &std::path::Path| {
            let s = p.to_string_lossy().to_string();
            s == "C:\\data" || s == "C:\\data\\keep.txt"
        };
        // режим «папка»: только прямые потомки -- файл и каталог, внутрь каталога не заходим
        let v = compute_deleted(&out, "/C/data", false, exists);
        let paths: Vec<_> = v["deleted"].as_array().unwrap().iter().map(|x| x["path"].as_str().unwrap().to_string()).collect();
        assert_eq!(paths, vec!["/C/data/gone.txt", "/C/data/gonedir"]);
        assert_eq!(v["count"], 2);
        // рекурсивно: только файлы (каталог gonedir целиком не существует -> a.txt удалён)
        let v = compute_deleted(&out, "/C/data", true, exists);
        let paths: Vec<_> = v["deleted"].as_array().unwrap().iter().map(|x| x["path"].as_str().unwrap().to_string()).collect();
        assert_eq!(paths, vec!["/C/data/gone.txt", "/C/data/gonedir/a.txt"]);
        assert_eq!(v["checked_files"], 3);
    }

    /// `ProgramData` — переменная окружения ВСЕГО процесса, а тесты в одном
    /// бинарнике идут параллельно. Загрузку конфига делаем строго под этим
    /// локом, в СВОЁМ каталоге, и сразу возвращаем переменную как было —
    /// соседний тест не увидит чужой (тем более удалённый) ProgramData.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Каталог-песочница + загруженный из неё Config. `Config` дальше держит
    /// абсолютные пути внутри себя, поэтому env после загрузки не нужен.
    fn isolated_config(tag: &str, config_json: &str) -> (std::path::PathBuf, restlex_core::Config) {
        let dir = std::env::temp_dir()
            .join(format!("restlex-api-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("restlex")).unwrap();
        if !config_json.is_empty() {
            std::fs::write(dir.join("restlex").join("config.json"), config_json).unwrap();
        }
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ProgramData");
        std::env::set_var("ProgramData", &dir);
        let cfg = restlex_core::Config::load().unwrap();
        match prev {
            Some(p) => std::env::set_var("ProgramData", p),
            None => std::env::remove_var("ProgramData"),
        }
        (dir, cfg)
    }

    fn prof(pw: &str, creds: &[(&str, &str)]) -> Profile {
        let mut p = Profile::default();
        p.repo_password = pw.into();
        for (k, v) in creds {
            p.extra.insert((*k).into(), serde_json::Value::String((*v).into()));
        }
        p
    }

    #[test]
    fn put_keeps_existing_password_when_body_empty() {
        // Штатный флоу GET(маскирует)→PUT: тело без пароля не должно затереть его.
        let existing = prof("secret-pw", &[("cred_s3_key", "AKIA123")]);
        let mut new = prof("", &[]);
        merge_secrets(&mut new, &existing);
        assert_eq!(new.repo_password, "secret-pw");
        assert_eq!(new.extra.get("cred_s3_key").and_then(|v| v.as_str()), Some("AKIA123"));
    }

    #[test]
    fn put_replaces_password_when_body_nonempty() {
        let existing = prof("old-pw", &[]);
        let mut new = prof("new-pw", &[]);
        merge_secrets(&mut new, &existing);
        assert_eq!(new.repo_password, "new-pw");
    }

    #[test]
    fn put_cred_empty_keeps_existing_nonempty_overrides() {
        let existing = prof("x", &[("cred_s3_key", "OLD"), ("cred_s3_secret", "KEEP")]);
        // Тело: s3_key меняем, s3_secret прислали пустым (=оставить).
        let mut new = prof("x", &[("cred_s3_key", "NEW"), ("cred_s3_secret", "")]);
        merge_secrets(&mut new, &existing);
        assert_eq!(new.extra.get("cred_s3_key").and_then(|v| v.as_str()), Some("NEW"));
        assert_eq!(new.extra.get("cred_s3_secret").and_then(|v| v.as_str()), Some("KEEP"));
    }

    #[test]
    fn password_ok_requires_pw_or_password_file() {
        assert!(password_ok(&prof("pw", &[]), ""));
        assert!(password_ok(&prof("", &[]), "C:\\secret.txt")); // password-file спасает
        assert!(!password_ok(&prof("", &[]), "")); // ни того, ни другого → битый
    }

    #[test]
    fn valid_pid_rejects_traversal() {
        assert!(valid_pid("3fa8c210"));
        assert!(valid_pid("my-profile_1"));
        assert!(!valid_pid("../etc"));
        assert!(!valid_pid("a/b"));
        assert!(!valid_pid(""));
    }

    #[test]
    fn snapshot_id_accepts_latest_and_hex_only() {
        assert!(valid_snapshot_id("latest"));
        assert!(valid_snapshot_id("a1b2c3d4"));
        assert!(valid_snapshot_id(&"0".repeat(64)));
        // Инъекция аргументов и мусор -- отказ.
        assert!(!valid_snapshot_id(""));
        assert!(!valid_snapshot_id("abc"));
        assert!(!valid_snapshot_id(&"0".repeat(65)));
        assert!(!valid_snapshot_id("--target"));
        assert!(!valid_snapshot_id("a1b2 --target C:\\"));
        assert!(!valid_snapshot_id("latest;calc"));
        assert!(!valid_snapshot_id("../latest"));
    }

    #[test]
    fn restore_target_must_be_absolute_without_traversal() {
        assert!(validate_restore_target("D:\\restore").is_ok());
        assert!(validate_restore_target("d:/restore/sub").is_ok());
        assert!(validate_restore_target("\\\\srv\\share\\out").is_ok());
        // Легальное имя с точками не должно ловиться проверкой на «..».
        assert!(validate_restore_target("D:\\arch..2024").is_ok());

        assert!(validate_restore_target("").is_err());
        assert!(validate_restore_target("   ").is_err());
        assert!(validate_restore_target("restore").is_err());
        assert!(validate_restore_target(".\\restore").is_err());
        assert!(validate_restore_target("\\restore").is_err());
        assert!(validate_restore_target("D:\\a\\..\\..\\Windows").is_err());
        assert!(validate_restore_target("D:/a/../Windows").is_err());
        assert!(validate_restore_target("--target").is_err());
        assert!(validate_restore_target("D:\\a\0b").is_err());
    }

    /// Легаси-роуты должны быть закрыты тем же bearer'ом, что и /api/v1:
    /// без заголовка POST /update/apply не должен доходить до хендлера
    /// (иначе — локальный EoP до SYSTEM). GET /health остаётся открытым.
    #[tokio::test]
    async fn legacy_routes_require_bearer() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let (tmp, cfg) = isolated_config("legacy-bearer", r#"{"api_token":"secret-token"}"#);
        let state = std::sync::Arc::new(crate::scheduler::State::new(cfg));
        let app = super::build_app(state);

        // /health — открыт.
        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Легаси-роуты без Bearer — 401.
        for (method, uri) in [
            ("POST", "/update/apply"),
            ("POST", "/mount/start/abc"),
            ("GET", "/log"),
            ("GET", "/status"),
            ("POST", "/rest-server/start"),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
        }

        // С правильным токеном — пропускает.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/status")
                    .header("Authorization", "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn restore_includes_drop_empty_reject_flags() {
        let v = vec!["/C/data".to_string(), "  ".to_string(), "/C/x".to_string()];
        assert_eq!(validate_restore_includes(&v).unwrap(), vec!["/C/data", "/C/x"]);
        assert!(validate_restore_includes(&["--exclude".to_string()]).is_err());
        assert!(validate_restore_includes(&vec!["/a".to_string(); 65]).is_err());
    }

    /// На свежей установке api_token и update_api_key пусты — без генерации
    /// middleware отдавал бы 403 на всё, и GUI полностью терял управление.
    #[test]
    fn api_token_generated_once_and_persisted() {
        let (tmp, cfg) = isolated_config("token-gen", "{}");
        assert_eq!(cfg.globals().api_token, "");

        super::ensure_api_token(&cfg);
        let token = cfg.globals().api_token;
        assert_eq!(token.len(), 64, "32 байта в hex");
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));

        // Токен реально лёг в config.json (единый источник правды для GUI/CLI).
        let on_disk = std::fs::read_to_string(tmp.join("restlex").join("config.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        let disk_token = v["api_token"].as_str().unwrap_or("");
        assert!(!disk_token.is_empty(), "api_token должен быть записан в файл");

        // Повторный старт НЕ перегенерирует: иначе GUI/K-AutoAdmin отваливались
        // бы после каждого рестарта службы.
        super::ensure_api_token(&cfg);
        assert_eq!(cfg.globals().api_token, token);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Зомби на порту может слушать не только 127.0.0.1, и состояние netstat
    /// печатает на языке системы — по нему фильтровать нельзя.
    #[test]
    fn netstat_matches_any_local_bind_of_the_port() {
        let p = "17345";
        assert_eq!(
            netstat_port_holder("  TCP    127.0.0.1:17345    0.0.0.0:0    LISTENING    4242", p),
            Some(4242)
        );
        assert_eq!(
            netstat_port_holder("  TCP    0.0.0.0:17345    0.0.0.0:0    LISTENING    777", p),
            Some(777)
        );
        assert_eq!(
            netstat_port_holder("  TCP    [::1]:17345    [::]:0    LISTENING    888", p),
            Some(888)
        );
        // Локализованное состояние («ПРОСЛУШИВАНИЕ») — тоже ловим.
        assert_eq!(
            netstat_port_holder("  TCP    0.0.0.0:17345    0.0.0.0:0    ПРОСЛУШИВАНИЕ    99", p),
            Some(99)
        );

        // Исходящий коннект НА удалённый :17345 — не наш случай (сверяем поле
        // локального адреса, а не всю строку).
        assert_eq!(
            netstat_port_holder("  TCP    10.0.0.5:52341    10.0.0.9:17345    ESTABLISHED    31", p),
            None
        );
        // Порт-подстрока: ":8797" не должен совпасть с ":18797" и наоборот.
        assert_eq!(
            netstat_port_holder("  TCP    127.0.0.1:18797    0.0.0.0:0    LISTENING    5", "8797"),
            None
        );
        assert_eq!(
            netstat_port_holder("  TCP    127.0.0.1:8797    0.0.0.0:0    LISTENING    5", "8797"),
            Some(5)
        );
        // UDP и мусор — мимо.
        assert_eq!(
            netstat_port_holder("  UDP    0.0.0.0:17345    *:*    1234", p),
            None
        );
        assert_eq!(netstat_port_holder("Активные подключения", p), None);
        // Наш порт как ЛОКАЛЬНЫЙ эфемерный у чужого исходящего коннекта:
        // удалённый адрес не 0.0.0.0:0 — значит не LISTENING, taskkill не наш.
        assert_eq!(
            netstat_port_holder("  TCP    10.0.0.5:17345    10.0.0.9:443    ESTABLISHED    31", p),
            None
        );
        assert_eq!(
            netstat_port_holder("  TCP    127.0.0.1:17345    127.0.0.1:52000    CLOSE_WAIT    7", p),
            None
        );
    }

    #[test]
    fn concurrent_api_backup_requests_reserve_pid_until_finished() {
        let first = super::ApiBackupReservation::acquire("reservation-test").unwrap();
        assert!(super::ApiBackupReservation::acquire("reservation-test").is_none());
        // Другой профиль можно запускать независимо.
        let different = super::ApiBackupReservation::acquire("reservation-other").unwrap();
        drop(different);
        drop(first);
        assert!(super::ApiBackupReservation::acquire("reservation-test").is_some());
    }

    /// Нерасшифрованный (`enc:`) api_token обязан считаться отсутствующим:
    /// иначе `ensure_api_token` его не перегенерирует, а middleware сравнит
    /// Bearer с шифроблобом — 403 навсегда, без пути восстановления.
    #[test]
    fn undecrypted_token_counts_as_missing() {
        assert!(token_missing(""));
        assert!(token_missing("enc:AQAAANCMnd8BFdERjHoAwE/Cl+s="));
        assert!(!token_missing("2f6c9a1b3d4e5f60718293a4b5c6d7e8"));
    }
}
