use chrono::Local;
use std::sync::Mutex;
use restlex_core::{
    reporter::{self, Report},
    restic::{BackupSummary, ResticRunner},
    telegram, Config, Profile,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use std::sync::atomic::{AtomicBool, Ordering};

/// Взводится, когда агент сам убивает restic при остановке службы (см. main.rs).
/// По коду выхода прерывание не опознать: `taskkill /F` — это TerminateProcess,
/// он отдаёт 1, а не 130, и restic запущен с CREATE_NO_WINDOW, т.е. настоящий
/// SIGINT/Ctrl+C до него не доходит в принципе. Без этого флага штатная
/// остановка службы улетала бы в Telegram как Backup_Failed.
pub static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

// ── Async jobs (REST API v1) ──────────────────────────────────────────────────

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus { Running, Done, Failed, Cancelled }

#[derive(Clone, serde::Serialize)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub status: JobStatus,
    pub result: Option<serde_json::Value>,
    /// Unix timestamp создания (seconds).
    pub created_at: u64,
    /// Unix timestamp завершения (Done/Failed). TTL считается от него.
    pub finished_at: Option<u64>,
}

// ─────────────────────────────────────────────────────────────────────────────

pub struct State {
    pub cfg: Config,
    pub update_trigger: Arc<tokio::sync::Notify>,
    pub shutdown_notify: Arc<tokio::sync::Notify>,
    pub last: Mutex<HashMap<String, chrono::DateTime<Local>>>,
    pub running: Mutex<HashMap<String, bool>>,
    pub last_log: Mutex<Vec<String>>,
    pub mount: Mutex<Option<crate::mount::MountSession>>,
    pub rest_server: std::sync::Arc<crate::rest_server::Session>,
    pub update_phase: Mutex<crate::updater::UpdatePhase>,
    pub update_latest_version: Mutex<String>,
    pub update_latest_gui_version: Mutex<String>,
    pub update_last_check: Mutex<Option<chrono::DateTime<Local>>>,
    pub force_apply: AtomicBool,
    /// Проверка обновлений инициирована вручную из GUI (кнопка «Проверить и обновить»).
    /// Ручная проверка сбрасывает circuit breaker и игнорирует активную блокировку —
    /// человек всегда может «разбанить» обновления одним нажатием.
    pub manual_check: AtomicBool,
    /// Сериализует VSS-бэкапы: только один профиль с use_vss может держать снепшот одновременно.
    pub vss_lock: Arc<tokio::sync::Mutex<()>>,
    /// Async jobs для REST API v1 (snapshots, restore и т.п.).
    pub jobs: Mutex<HashMap<String, Job>>,
    /// pid профиля -> PID процесса restic, чтобы `stop` прерывал адресно.
    pub restic_pids: Mutex<HashMap<String, u32>>,
}

impl State {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            update_trigger: Arc::new(tokio::sync::Notify::new()),
            shutdown_notify: Arc::new(tokio::sync::Notify::new()),
            last: Mutex::new(HashMap::new()),
            running: Mutex::new(HashMap::new()),
            last_log: Mutex::new(Vec::new()),
            mount: Mutex::new(None),
            rest_server: crate::rest_server::Session::new(),
            update_phase: Mutex::new(crate::updater::UpdatePhase::Idle),
            update_latest_version: Mutex::new(String::new()),
            update_latest_gui_version: Mutex::new(String::new()),
            update_last_check: Mutex::new(None),
            force_apply: AtomicBool::new(false),
            manual_check: AtomicBool::new(false),
            vss_lock: Arc::new(tokio::sync::Mutex::new(())),
            jobs: Mutex::new(HashMap::new()),
            restic_pids: Mutex::new(HashMap::new()),
        }
    }

    pub fn push_log(&self, line: String) {
        let mut g = self.last_log.lock().unwrap();
        g.push(line);
        if g.len() > 500 {
            let drop = g.len() - 500;
            g.drain(0..drop);
        }
    }
}

pub fn build_runner(cfg: &Config, profile: &Profile) -> ResticRunner {
    let globals = cfg.globals();
    let restic = if globals.restic.is_empty() {
        "restic".to_string()
    } else {
        globals.restic.clone()
    };

    let mut runner = ResticRunner::new(
        restic,
        profile.repo_path.clone(),
        profile.repo_password.clone(),
    )
    .with_language(&globals.language);

    if !globals.default_password_file.is_empty() {
        runner = runner.with_password_file(globals.default_password_file.clone());
    }
    if !globals.temp.is_empty() {
        runner = runner.with_temp(globals.temp.clone());
    }

    for (cfg_key, env_key) in [
        ("cred_s3_key", "AWS_ACCESS_KEY_ID"),
        ("cred_s3_secret", "AWS_SECRET_ACCESS_KEY"),
        ("cred_b2_account", "B2_ACCOUNT_ID"),
        ("cred_b2_key", "B2_ACCOUNT_KEY"),
        ("cred_az_account", "AZURE_ACCOUNT_NAME"),
        ("cred_az_key", "AZURE_ACCOUNT_KEY"),
        ("cred_gs_project", "GOOGLE_PROJECT_ID"),
        ("cred_gs_json", "GOOGLE_APPLICATION_CREDENTIALS"),
    ] {
        if let Some(value) = profile.extra.get(cfg_key).and_then(|v| v.as_str()) {
            runner = runner.with_env(env_key, value);
        }
    }

    runner
}

pub async fn run(state: Arc<State>) {
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let profiles = state.cfg.profiles();
        let now = Local::now();
        for (pid, prof) in profiles {
            // Пауза оператора (sched.enabled=false) — профиль не запускается по
            // расписанию, но остаётся видимым и доступным для ручного /backup.
            if !prof.sched.enabled {
                continue;
            }
            if !prof.sched.is_enabled() {
                continue;
            }
            let last = state.last.lock().unwrap().get(&pid).cloned().or(prof.last_run);
            if !prof.sched.should_fire(now, last) {
                continue;
            }
            if *state.running.lock().unwrap().get(&pid).unwrap_or(&false) {
                continue;
            }
            let st = state.clone();
            let pid_c = pid.clone();
            tokio::spawn(async move {
                let _ = run_backup(st, pid_c, prof).await;
            });
        }
    }
}

/// Выбор процесса, который надо убить по `stop` для профиля `pid`:
/// сначала бэкап (голый ключ), затем restore (`restore_key(pid)`).
///
/// Коллизии «убили не ту операцию» нет: бэкап (`run_backup`) и restore
/// (`v1_restore`) делят взаимный замок `State::running[pid]`, поэтому под этими
/// двумя ключами одновременно живёт максимум одна запись.
pub fn pick_stop_target(pids: &HashMap<String, u32>, pid: &str) -> Option<u32> {
    pids.get(pid).or_else(|| pids.get(&restore_key(pid))).copied()
}

/// Прервать идущую операцию профиля (бэкап ИЛИ restore): убиваем именно её
/// restic, не все подряд. Флаг «прервано намеренно» ставится до kill — иначе
/// результат уедет в Telegram как Backup_Failed (taskkill даёт код 1, а не
/// SIGINT-код), а restore пометился бы как failed.
pub fn stop_backup(state: &State, pid: &str) -> std::io::Result<u32> {
    // Запись живёт только пока restic реально бежит: run_backup / v1_restore
    // снимают её на выходе. Иначе мы бы стреляли по PID предыдущего прогона, а
    // ОС такие PID переиспользует — убили бы посторонний процесс.
    let os_pid = match pick_stop_target(&state.restic_pids.lock().unwrap(), pid) {
        Some(v) => v,
        None => return Ok(0),
    };
    let out = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &os_pid.to_string()])
        .output()?;
    if !out.status.success() {
        // Процесс мог завершиться сам между проверкой и kill — это не ошибка,
        // и помечать прогон отменённым в таком случае нельзя.
        tracing::warn!("stop_backup({pid}): taskkill rc={:?}", out.status.code());
        return Ok(0);
    }
    // Метку ставим только после фактического kill — и ПОД локом restic_pids,
    // из-под которого завершившийся прогон снимает свою запись. Иначе прогон,
    // успевший закончиться сам между taskkill и insert, уже убрал бы себя из
    // CANCELLED, а наша запись осталась бы в множестве навсегда (и «отменила»
    // бы следующий прогон с переиспользованным PID).
    {
        let pids = state.restic_pids.lock().unwrap();
        if pick_stop_target(&pids, pid) != Some(os_pid) {
            tracing::info!("stop_backup({pid}): процесс {os_pid} завершился сам, метку не ставим");
            return Ok(0);
        }
        CANCELLED.lock().unwrap().insert(os_pid);
    }
    tracing::info!("stop_backup({pid}): killed restic pid {os_pid}");
    Ok(os_pid)
}

/// Ключ в `State::restic_pids` для процесса restore этого профиля.
/// Отдельное пространство имён от бэкапа: обе операции регистрируют свой PID
/// в одной мапе, и общий ключ приводил к тому, что cleanup одной операции
/// стирал запись другой, а `stop` бил по чужому процессу. `stop` смотрит оба
/// ключа (см. `pick_stop_target`), так что restore прерывается тем же вызовом.
/// `pid` профиля валидируется `valid_pid` (только [A-Za-z0-9_-]), так что
/// префикс с ':' не может столкнуться с ключом бэкапа.
pub fn restore_key(pid: &str) -> String {
    format!("restore:{pid}")
}

/// PID-ы процессов restic, прерванных явно через API. Ключ — именно PID
/// процесса, а не профиля: запись, не подчистившаяся после одного прогона,
/// не должна «отменять» следующий.
pub static CANCELLED: Mutex<std::collections::BTreeSet<u32>> =
    Mutex::new(std::collections::BTreeSet::new());

const MIN_FREE_BYTES: u64 = 5 * 1024 * 1024 * 1024; // 5 ГБ

// ─── Crash recovery markers ──────────────────────────────────────────────────

fn running_marker_path(pid: &str) -> std::path::PathBuf {
    restlex_core::logs_dir().join(format!("{pid}.running"))
}

fn write_running_marker(pid: &str) {
    let _ = std::fs::write(running_marker_path(pid), "");
}

fn delete_running_marker(pid: &str) {
    let _ = std::fs::remove_file(running_marker_path(pid));
}

/// При старте агента: ищет {pid}.running маркеры от предыдущего крэша.
/// Для каждого — пишет "crashed" в last-run.json, чтобы GUI показал корректный статус.
pub async fn check_crash_markers(state: Arc<State>) {
    let log_dir = restlex_core::logs_dir();
    let entries = match std::fs::read_dir(&log_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let name = fname.to_string_lossy();
        if !name.ends_with(".running") {
            continue;
        }
        let pid = name.trim_end_matches(".running").to_string();
        tracing::warn!("crash_recovery: found stale marker for pid '{}' — marking as crashed", pid);

        // Ensure running state is false
        state.running.lock().unwrap().insert(pid.clone(), false);

        // Write crashed last-run.json
        let crashed = serde_json::json!({
            "pid": &pid,
            "profile": "",
            "timestamp": Local::now().to_rfc3339(),
            "status": "crashed",
            "duration_s": 0,
            "files_new": 0,
            "bytes_added": 0,
            "snapshot_id": null,
            "exit_code": -2,
            "error_count": 1,
            "errors": ["Agent restarted during backup — result unknown"],
        });
        let log_path = log_dir.join(format!("{pid}-last-run.json"));
        let _ = restlex_core::config::write_atomic(&log_path, &crashed);

        // Remove stale marker
        let _ = std::fs::remove_file(entry.path());
        tracing::info!("crash_recovery: cleaned up marker for pid '{}'", pid);
    }
}

/// Свободное место (байт) на диске, которому принадлежит path. None = не удалось определить.
async fn free_bytes_on_path(path: &str) -> Option<u64> {
    // Только по символам: `path[..1]` на пути вроде "Ж:\repo" резал строку не по
    // границе UTF-8 и валил задачу паникой — флаг running[pid] оставался true
    // навсегда, профиль больше не бэкапился, а POST /backup вечно отдавал 409.
    let drive = if path.chars().nth(1) == Some(':') {
        path.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "C".into())
    } else {
        "C".to_string()
    };
    let cmd = format!("(Get-PSDrive -Name '{}').Free", drive);
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &cmd])
            .output(),
    )
    .await
    .ok()?.ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().ok()
}

/// Сколько restic.exe процессов запущено на машине.
fn count_restic_processes() -> usize {
    match std::process::Command::new("tasklist")
        .args(["/fi", "imagename eq restic.exe", "/fo", "csv", "/nh"])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| l.to_lowercase().contains("restic.exe"))
            .count(),
        Err(_) => 1, // не можем проверить — считаем что мы одни
    }
}

/// Перезапустить службу VSS (net stop → пауза → net start → пауза).
async fn vss_reset() {
    let _ = tokio::process::Command::new("net")
        .args(["stop", "vss", "/y"])
        .output()
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let _ = tokio::process::Command::new("net")
        .args(["start", "vss"])
        .output()
        .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
}

/// Проверить VSS writer'ов: вернуть true если все в норме, false если есть [8] Неисправен.
async fn vss_writers_ok() -> bool {
    use tokio::time::timeout;
    let result = timeout(
        Duration::from_secs(8),
        tokio::process::Command::new("vssadmin")
            .args(["list", "writers"])
            .output(),
    )
    .await;
    match result {
        Ok(Ok(o)) => {
            let out = String::from_utf8_lossy(&o.stdout).to_lowercase();
            !out.contains("[8]") && !out.contains("failed")
        }
        _ => false, // timeout или ошибка → считаем сломанным
    }
}

/// Сброс VSS: стоп → старт → удалить несогласованные тени.
async fn vss_fix_writers() {
    let _ = tokio::process::Command::new("net")
        .args(["stop", "vss", "/y"])
        .output()
        .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let _ = tokio::process::Command::new("net")
        .args(["start", "vss"])
        .output()
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let _ = tokio::process::Command::new("vssadmin")
        .args(["delete", "shadows", "/all", "/quiet"])
        .output()
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// Запас поверх таймера shutdown: живы после него → выключение не состоялось.
const SHUTDOWN_CHECK_MARGIN_S: u64 = 15;

/// Magic packet по MAC (broadcast :9). Ошибки — только в форме Err, без паники.
fn wol_send(mac: &str, host: &str, en: bool) -> anyhow::Result<()> {
    let hex: String = mac.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 12 {
        if en {
            anyhow::bail!("invalid MAC: {mac}");
        }
        anyhow::bail!("некорректный MAC: {mac}");
    }
    let addr: Vec<u8> = (0..6)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16))
        .collect::<Result<_, _>>()?;
    let mut pkt = vec![0xFFu8; 6];
    for _ in 0..16 {
        pkt.extend_from_slice(&addr);
    }
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    sock.set_broadcast(true)?;
    let mut targets = vec!["255.255.255.255".to_string()];
    // host — IPv4: ещё и directed broadcast его /24 (уходит в нужный интерфейс)
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        let o = ip.octets();
        targets.push(format!("{}.{}.{}.255", o[0], o[1], o[2]));
    }
    // Ошибка одного адреса (нет маршрута до /24) не должна отменять остальные.
    let mut last_err = None;
    let mut sent = false;
    for t in &targets {
        for port in [9, 7] {
            match sock.send_to(&pkt, (t.as_str(), port)) {
                Ok(_) => sent = true,
                Err(e) => last_err = Some(e),
            }
        }
    }
    match (sent, last_err) {
        (false, Some(e)) => Err(e.into()),
        _ => Ok(()),
    }
}

/// Вывод консольных утилит (shutdown и т.п.) — в OEM-кодировке (cp866), не UTF-8.
fn oem_to_string(b: &[u8]) -> String {
    #[link(name = "kernel32")]
    extern "system" {
        fn MultiByteToWideChar(cp: u32, flags: u32, src: *const u8, n: i32, dst: *mut u16, cap: i32) -> i32;
    }
    const CP_OEMCP: u32 = 1;
    if b.is_empty() {
        return String::new();
    }
    let mut buf = vec![0u16; b.len()];
    // SAFETY: src/dst — валидные буферы указанной длины; OEM → UTF-16 не длиннее входа.
    let n = unsafe { MultiByteToWideChar(CP_OEMCP, 0, b.as_ptr(), b.len() as i32, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize]).trim().to_string()
}

/// MAC задом наперёд — адрес пакета Sleep-on-LAN.
fn reverse_mac(mac: &str) -> String {
    let hex = mac_hex(mac);
    if hex.len() != 12 {
        return mac.to_string();
    }
    (0..6).rev().map(|i| &hex[i * 2..i * 2 + 2]).collect()
}

/// IP по MAC из ARP-кэша (`arp -a`). MAC в любом формате.
async fn mac_to_ip(mac: &str) -> Option<String> {
    let hex: String = mac.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_lowercase();
    if hex.len() != 12 {
        return None;
    }
    let want: String = (0..6).map(|i| &hex[i * 2..i * 2 + 2]).collect::<Vec<_>>().join("-");
    let out = tokio::process::Command::new("arp").arg("-a")
        .creation_flags(0x08000000).output().await.ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (ip, m) = (it.next()?, it.next()?);
        (m.eq_ignore_ascii_case(&want)).then(|| ip.to_string())
    })
}

fn mac_hex(mac: &str) -> String {
    mac.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_lowercase()
}

/// Одна проверка IP — только наблюдение, не доказательство shutdown цели по MAC.
fn sleep_on_lan_probe_message(en: bool, profile: &str, mac: &str, ip: &str, reachable: bool) -> String {
    if en {
        let observed = if reachable { "responds to ping" } else { "does not respond to ping" };
        let category = if reachable { "Sleep-on-LAN warning" } else { "Sleep-on-LAN" };
        return format!("RestLex - Profile: {profile} | {category}: after 120 s IP {ip} {observed}. \
            State of the PC with MAC {mac} and packet delivery are not confirmed; ARP/IP may point to another PC");
    }
    let observed = if reachable { "отвечает на ping" } else { "не отвечает на ping" };
    let category = if reachable { "Предупреждение Sleep-on-LAN" } else { "Sleep-on-LAN" };
    format!("RestLex - Профиль: {profile} | {category}: через 120 с IP {ip} {observed}. \
        Состояние ПК с MAC {mac} и доставка пакета не подтверждены; ARP/IP может указывать на другой ПК")
}

async fn host_reachable(host: &str, timeout_s: u64) -> bool {
    let out = tokio::process::Command::new("ping")
        .args(["-n", "1", "-w", &(timeout_s * 1000).to_string(), host])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .await;
    matches!(out, Ok(o) if o.status.success())
}

/// Ждём отклика хоста, пингуя раз в `interval_s`, но не дольше `timeout_s`.
async fn wol_wait(host: &str, interval_s: u64, timeout_s: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_s);
    loop {
        let started = tokio::time::Instant::now();
        // таймаут одного ping не должен вылезать за общий дедлайн
        let left = deadline.saturating_duration_since(started).as_secs();
        if host_reachable(host, interval_s.min(left).max(1)).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        // ping мог вернуться мгновенно (unreachable) — добираем интервал
        let spent = started.elapsed();
        if spent < Duration::from_secs(interval_s) {
            tokio::time::sleep(Duration::from_secs(interval_s) - spent).await;
        }
    }
}

pub async fn run_backup(
    state: Arc<State>,
    pid: String,
    profile: Profile,
) -> anyhow::Result<BackupSummary> {
    {
        let mut r = state.running.lock().unwrap();
        if *r.get(&pid).unwrap_or(&false) {
            return Ok(BackupSummary::default());
        }
        r.insert(pid.clone(), true);
    }
    state.last.lock().unwrap().insert(pid.clone(), Local::now());
    write_running_marker(&pid);

    let started = Local::now();
    let globals = state.cfg.globals();
    let en = globals.en();
    let runner = build_runner(&state.cfg, &profile);

    // Проверка свободного места: temp-диск + локальный репозиторий
    {
        let mut check_paths: Vec<String> = Vec::new();
        if !globals.temp.is_empty() { check_paths.push(globals.temp.clone()); }
        // локальный репо: путь начинается с буквы диска
        if profile.repo_path.len() >= 2
            && profile.repo_path.chars().nth(1) == Some(':')
        {
            check_paths.push(profile.repo_path.clone());
        }
        if check_paths.is_empty() { check_paths.push("C:".to_string()); }
        check_paths.dedup_by(|a, b| {
            a.chars().next().map(|c| c.to_ascii_uppercase())
                == b.chars().next().map(|c| c.to_ascii_uppercase())
        });
        for path in &check_paths {
            if let Some(free) = free_bytes_on_path(path).await {
                if free < MIN_FREE_BYTES {
                    let gb = free / (1024 * 1024 * 1024);
                    // По символам, не по байтам: многобайтная буква диска
                    // (например "Ж:\") рвала байтовый срез → паника.
                    let disk = path.chars().take(3).collect::<String>();
                    let msg = if en {
                        format!("RestLex - Profile: {} | Error: Low disk space on {} ({} GB < 5 GB)",
                            profile.name, disk, gb)
                    } else {
                        format!("RestLex - Профиль: {} | Ошибка: Мало места на диске {} ({} ГБ < 5 ГБ)",
                            profile.name, disk, gb)
                    };
                    restlex_core::log_profile(&pid, "WARN", &msg);
                    state.push_log(format!("[{}] ⚠ {}", Local::now().format("%H:%M:%S"), msg));
                    if !globals.tg_token.is_empty() && !globals.tg_chat.is_empty() {
                        let tg_thread = if globals.tg_group.is_empty() { None } else { Some(globals.tg_group.as_str()) };
                        let _ = telegram::send_threaded(&globals.tg_token, &globals.tg_chat, &msg, tg_thread).await;
                    }
                    // Снять и running-флаг, и .running-маркер — иначе при следующем
                    // старте агента маркер примут за признак краша (ложное срабатывание).
                    delete_running_marker(&pid);
                    state.restic_pids.lock().unwrap().remove(&pid);
                    state.running.lock().unwrap().insert(pid.clone(), false);
                    return Ok(BackupSummary::default());
                }
            }
        }
    }

    tracing::info!("Starting backup for profile '{}' ({})", profile.name, pid);
    state.push_log(format!(
        "[{}] backup start: {}",
        started.format("%H:%M:%S"),
        profile.name
    ));
    restlex_core::log_profile(
        &pid,
        "INFO",
        &format!("=== scheduled backup start: {} ===", profile.name),
    );
    restlex_core::log_profile(
        &pid,
        "INFO",
        if profile.use_vss {
            "VSS shadow copy: ENABLED (--use-fs-snapshot)"
        } else {
            "VSS shadow copy: disabled"
        },
    );

    // Per-task overlay (prof.sched.params) → per-profile (profile.extra) → globals
    let task_params = profile.sched.params.clone();
    let task_bool = |key: &str| -> Option<bool> {
        task_params.get(key).and_then(|v| v.as_bool())
    };
    let task_u64 = |key: &str| -> Option<u64> {
        task_params.get(key).and_then(|v| v.as_u64())
    };
    let task_str_arr = |key: &str| -> Vec<String> {
        task_params.get(key)
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default()
    };
    let get_bool = |key: &str, default: bool| -> bool {
        task_bool(key)
            .or_else(|| profile.extra.get(key).and_then(|v| v.as_bool()))
            .unwrap_or(default)
    };
    let tg_fatal   = get_bool("tg_fatal", globals.tg_fatal);
    let tg_file    = get_bool("tg_file",  globals.tg_file);
    let tg_prune   = get_bool("tg_prune", globals.tg_prune);
    let pre_repair = get_bool("pre_repair_index", true);
    let no_scan    = task_bool("no_scan").unwrap_or(globals.global_no_scan);
    let vss_timeout_s = globals.global_vss_timeout_s;
    let use_vss    = task_bool("use_vss").unwrap_or(profile.use_vss);
    let extra_args = task_str_arr("extra_args");
    let extra_excludes = task_str_arr("exclude");
    let get_str = |key: &str| -> String {
        task_params.get(key).and_then(|v| v.as_str())
            .or_else(|| profile.extra.get(key).and_then(|v| v.as_str()))
            .unwrap_or("").trim().to_string()
    };
    let get_u64 = |key: &str, default: u64| -> u64 {
        task_u64(key)
            .or_else(|| profile.extra.get(key).and_then(|v| v.as_u64()))
            .unwrap_or(default)
    };
    let tg_configured = !globals.tg_token.is_empty() && !globals.tg_chat.is_empty();
    let tg_thread = if globals.tg_group.is_empty() { None } else { Some(globals.tg_group.clone()) };

    // ── Wake-on-LAN перед бэкапом (та же логика, что в GUI) ──────────────────
    if get_bool("wol_enabled", false) {
        let mac = get_str("wol_mac");
        let host = get_str("wol_host");
        if !mac.is_empty() {
            match wol_send(&mac, &host, en) {
                Ok(()) => restlex_core::log_profile(&pid, "INFO",
                    &format!("WOL: magic packet отправлен → {host} ({mac})")),
                Err(e) => restlex_core::log_profile(&pid, "ERROR",
                    &format!("WOL: не удалось отправить пакет: {e}")),
            }
            if !host.is_empty() && get_bool("wol_wait", true) {
                let interval = get_u64("wol_ping_interval", 2).clamp(1, 60);
                let wait_s = get_u64("wol_wait_timeout", 60).clamp(5, 3600);
                restlex_core::log_profile(&pid, "INFO",
                    &format!("WOL: жду {host} (пинг раз в {interval}с, не дольше {wait_s}с)"));
                if !wol_wait(&host, interval, wait_s).await {
                    let msg = if en {
                        format!("RestLex - Profile: {} | Error: WOL — {} does not respond",
                            profile.name, host)
                    } else {
                        format!("RestLex - Профиль: {} | Ошибка: WOL — {} не отвечает",
                            profile.name, host)
                    };
                    restlex_core::log_profile(&pid, "ERROR", &msg);
                    state.push_log(format!("[{}] ❌ {}", Local::now().format("%H:%M:%S"), msg));
                    if tg_configured && tg_fatal {
                        let _ = telegram::send_threaded(&globals.tg_token, &globals.tg_chat,
                            &msg, tg_thread.as_deref()).await;
                    }
                    delete_running_marker(&pid);
                    state.restic_pids.lock().unwrap().remove(&pid);
                    state.running.lock().unwrap().insert(pid.clone(), false);
                    return Ok(BackupSummary::default());
                }
                restlex_core::log_profile(&pid, "INFO", &format!("WOL: хост {host} доступен"));
            }
        }
    }

    // Сериализация VSS: ждём пока другой VSS-бэкап не освободит снепшот
    let _vss_guard = if profile.use_vss /* per-task overlay применим ниже к use_vss_eff */ {
        if state.vss_lock.try_lock().is_err() {
            restlex_core::log_profile(&pid, "INFO",
                "VSS занят другим бэкапом — ожидаю освобождения снепшота...");
        }
        Some(state.vss_lock.clone().lock_owned().await)
    } else {
        None
    };

    // VSS preflight: проверяем writer'ов до старта бэкапа
    if profile.use_vss {
        if !vss_writers_ok().await {
            restlex_core::log_profile(&pid, "WARN",
                "VSS writers неисправны (Несогласованная теневая копия) — сброс до бэкапа");
            vss_fix_writers().await;
            restlex_core::log_profile(&pid, "INFO", "VSS восстановлен, запускаю бэкап");
        }
    }

    // Pre-backup unlock: снимаем зависшие lock'и перед стартом
    match runner.unlock().await {
        Ok(out) if !out.is_empty() => restlex_core::log_profile(&pid, "INFO", &format!("unlock: {out}")),
        Ok(_) => {}
        Err(e) => restlex_core::log_profile(&pid, "WARN", &format!("unlock failed (продолжаем): {e}")),
    }

    // Pre-backup repair index
    if pre_repair {
        restlex_core::log_profile(&pid, "INFO", "repair index...");
        match runner.repair_index().await {
            Ok(out) if !out.is_empty() => restlex_core::log_profile(&pid, "INFO", &format!("repair index: {out}")),
            Ok(_) => restlex_core::log_profile(&pid, "INFO", "repair index done"),
            Err(e) => restlex_core::log_profile(&pid, "WARN", &format!("repair index failed (продолжаем): {e}")),
        }
    }

    let mut vss_retried = false;
    let (summary, status_str) = loop {
        let pid_log = pid.clone();
        let state_log = state.clone();
        let final_excludes: Vec<String> = profile.exclude
            .iter().cloned().chain(extra_excludes.iter().cloned()).collect();
        let result = runner
            .backup(
                &profile.sources,
                use_vss,
                &final_excludes,
                no_scan,
                vss_timeout_s,
                &extra_args,
                move |p| match &p {
                    restlex_core::restic::Progress::Started(os_pid) => {
                        state_log.restic_pids.lock().unwrap().insert(pid_log.clone(), *os_pid);
                    }
                    restlex_core::restic::Progress::Line(l) => {
                        if l.contains("\"message_type\":\"summary\"") {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(l) {
                                let fn_ = v["files_new"].as_u64().unwrap_or(0);
                                let fc = v["files_changed"].as_u64().unwrap_or(0);
                                let added = v["data_added"].as_u64().unwrap_or(0);
                                let dur = v["total_duration"].as_f64().unwrap_or(0.0);
                                restlex_core::log_profile(&pid_log, "INFO",
                                    &format!("Files: {fn_} new, {fc} changed | Added: {:.1} MiB | {dur:.0}s",
                                        added as f64 / 1048576.0));
                            }
                            return;
                        }
                        if l.contains("\"message_type\"") { return; }
                        restlex_core::log_profile(&pid_log, "INFO", l);
                        state_log.push_log(format!("[{}] {}", pid_log, l));
                    }
                    restlex_core::restic::Progress::Stats { percent, .. } => {
                        tracing::trace!("{} {:.0}%", pid_log, percent * 100.0);
                    }
                    restlex_core::restic::Progress::Done(_) => {}
                },
            )
            .await;

        // Считаем отменённость ДО match: побочный эффект в guard зависел бы от
        // порядка веток и от короткого замыкания `||`.
        let killed_pid = state.restic_pids.lock().unwrap().get(&pid).copied();
        let was_cancelled = killed_pid
            .map(|os_pid| CANCELLED.lock().unwrap().remove(&os_pid))
            .unwrap_or(false);
        let (s, st) = match result {
            Ok(s) if s.exit_code == 0 => break (s, "success".to_string()),
            // exit 3 = partial success (некоторые файлы нечитаемы) — как в GUI
            Ok(s) if s.exit_code == 3 => break (s, "warning".to_string()),
            // Прерывание — не сбой, Telegram-алерт слать не за что. Два источника:
            // наш собственный kill при остановке службы (флаг) и настоящий SIGINT,
            // на который restic >= 0.19.0 отдаёт 130 (раньше 1).
            Ok(s) if s.exit_code == 130
                || SHUTTING_DOWN.load(Ordering::SeqCst)
                || was_cancelled => {
                break (s, "cancelled".to_string())
            }
            Ok(s) => (s, "failed".to_string()),
            Err(e) => {
                let mut s = BackupSummary::default();
                s.exit_code = -1;
                s.errors.push(e.to_string());
                (s, "error".to_string())
            }
        };

        // VSS_E_SNAPSHOT_SET_IN_PROGRESS auto-fix (один раз)
        if !vss_retried && profile.use_vss {
            let is_vss_stuck = s.errors.iter().any(|e| {
                let l = e.to_lowercase();
                l.contains("snapshot_set_in_progress") || l.contains("snapshot set in progress")
            });
            if is_vss_stuck {
                let n = count_restic_processes();
                if n <= 1 {
                    vss_retried = true;
                    restlex_core::log_profile(&pid, "WARN",
                        "VSS_E_SNAPSHOT_SET_IN_PROGRESS — перезапуск VSS и повтор бэкапа");
                    vss_reset().await;
                    continue;
                } else {
                    restlex_core::log_profile(&pid, "WARN",
                        &format!("VSS_E_SNAPSHOT_SET_IN_PROGRESS — запущено {} restic, сброс VSS пропущен", n));
                }
            }
        }
        break (s, st);
    };

    tracing::info!(
        "Backup '{}' done: status={}, snapshot={:?}",
        profile.name,
        status_str,
        summary.snapshot_id
    );
    // Human-readable summary line in profile log
    restlex_core::log_profile(&pid, "INFO",
        &format!("Files: {} new, {} changed | Added: {:.1} MiB | {}s",
            summary.files_new,
            summary.files_changed,
            summary.bytes_added as f64 / 1_048_576.0,
            summary.duration_s));
    state.push_log(format!(
        "[{}] backup done: {} status={} dur={}s",
        Local::now().format("%H:%M:%S"),
        profile.name,
        status_str,
        summary.duration_s
    ));
    restlex_core::log_profile(
        &pid,
        match status_str.as_str() {
            "success" => "SUCCESS",
            "warning" => "SUCCESS",  // exit 3 — partial, но не фатальная ошибка
            "cancelled" => "CANCELLED",  // прерван нами при остановке службы — не сбой
            _ => "FAILED",
        },
        &format!(
            "backup done: exit={} files_new={} bytes_added={} duration={}s snapshot={:?}",
            summary.exit_code,
            summary.files_new,
            summary.bytes_added,
            summary.duration_s,
            summary.snapshot_id
        ),
    );
    for e in &summary.errors {
        restlex_core::log_profile(&pid, "ERROR", e);
    }
    let _ = state.cfg.update_last_run(&pid, &status_str);

    let tg_ok = !globals.tg_token.is_empty() && !globals.tg_chat.is_empty();
    let tg_thread_opt = if globals.tg_group.is_empty() { None } else { Some(globals.tg_group.clone()) };
    // Background network sends are detached so the per-profile `running` flag
    // (cleared at the end of this fn) is not held during retry waits. Each task
    // owns its inputs and retries on its own schedule.
    let net_retry_count = globals.net_retry_count;
    let net_retry_interval_s = globals.net_retry_interval_s;

    // tg_fatal: Telegram при фатальной ошибке (exit != 0 и != 3); обратная совместимость: sched.notify_tg
    if tg_ok && (tg_fatal || profile.sched.notify_tg) && (status_str == "failed" || status_str == "error") {
        let reason = if summary.errors.iter().any(|e| {
            let l = e.to_lowercase();
            l.contains("snapshot_set_in_progress") || l.contains("snapshot set in progress")
        }) {
            "VSS_Snapshot_In_Progress".to_string()
        } else {
            "Backup_Failed".to_string()
        };
        let msg = if en {
            format!("RestLex - Profile: {} | Error: {}", profile.name, reason)
        } else {
            format!("RestLex - Профиль: {} | Ошибка: {}", profile.name, reason)
        };
        let token = globals.tg_token.clone();
        let chat = globals.tg_chat.clone();
        let thread = tg_thread_opt.clone();
        tokio::spawn(async move {
            let _ = telegram::send_with_retry(
                &token, &chat, &msg, thread.as_deref(), net_retry_count, net_retry_interval_s,
            ).await;
        });
    }
    // tg_file: exit 3 — ошибки копирования файлов (как в GUI: code==3 → tg_file)
    if tg_ok && tg_file && status_str == "warning" {
        let msg = if en {
            format!("RestLex ⚠️ {} | File copy errors (exit 3)", profile.name)
        } else {
            format!("RestLex ⚠️ {} | Ошибки копирования файлов (exit 3)", profile.name)
        };
        let token = globals.tg_token.clone();
        let chat = globals.tg_chat.clone();
        let thread = tg_thread_opt.clone();
        tokio::spawn(async move {
            let _ = telegram::send_with_retry(
                &token, &chat, &msg, thread.as_deref(), net_retry_count, net_retry_interval_s,
            ).await;
        });
    }
    // tg_file: только ошибки файлов (exit 3) — на успех не уведомляем

    if !globals.report_url.is_empty() {
        let report = Report {
            profile: profile.name.clone(),
            pid: pid.clone(),
            status: status_str.clone(),
            snapshot_id: summary.snapshot_id.clone(),
            started_at: started,
            duration_s: summary.duration_s,
            files_new: summary.files_new,
            files_changed: summary.files_changed,
            bytes_added: summary.bytes_added,
            source_paths: profile.sources.clone(),
            dest_path: profile.repo_path.clone(),
            errors: summary.errors.clone(),
            host: reporter::hostname(),
            agent_version: {
                // Composite kept for backward-compat with old webserver builds; new
                // webserver reads launcher_version field below and ignores composite.
                let lv = restlex_core::read_launcher_version();
                let lv = if lv.is_empty() { "?".to_string() } else { lv };
                format!("{}/{}", lv, crate::agent_version())
            },
            launcher_version: restlex_core::read_launcher_version(),
        };
        let report_url = globals.report_url.clone();
        let report_api_key = globals.report_api_key.clone();
        tokio::spawn(async move {
            let _ = reporter::send_with_retry(
                &report_url, &report_api_key, &report, net_retry_count, net_retry_interval_s,
            ).await;
        });
    }

    // Retention if status ok (success или warning/exit-3) and forget enabled — как в GUI
    if (status_str == "success" || status_str == "warning") && profile.sched.forget {
        restlex_core::log_profile(&pid, "INFO", "=== forget/prune ===");
        // Применяем overlay из params.retention поверх profile.retention
        let mut eff_retention = profile.retention.clone();
        if let Some(serde_json::Value::Object(rmap)) = task_params.get("retention") {
            let pick = |k: &str| -> Option<u32> {
                rmap.get(k).or_else(|| rmap.get(&k.replace("-", "_")))
                    .and_then(|v| v.as_u64()).map(|n| n as u32)
            };
            if let Some(v) = pick("keep-last")    { eff_retention.keep_last = v; }
            if let Some(v) = pick("keep-hourly")  { eff_retention.keep_hourly = v; }
            if let Some(v) = pick("keep-daily")   { eff_retention.keep_daily = v; }
            if let Some(v) = pick("keep-weekly")  { eff_retention.keep_weekly = v; }
            if let Some(v) = pick("keep-monthly") { eff_retention.keep_monthly = v; }
            if let Some(v) = pick("keep-yearly")  { eff_retention.keep_yearly = v; }
        }
        match runner.forget(&eff_retention, true).await {
            Ok(_) => {
                restlex_core::log_profile(&pid, "INFO", "forget/prune done");
                // tg_prune: только ошибки (как в GUI — prune success не уведомляем)
            }
            Err(e) => {
                restlex_core::log_profile(&pid, "ERROR", &format!("forget failed: {e}"));
                tracing::warn!("forget failed: {e}");
                if tg_ok && tg_prune {
                    let msg = if en {
                        format!("RestLex - Profile: {} | Error: Prune_Failed", profile.name)
                    } else {
                        format!("RestLex - Профиль: {} | Ошибка: Prune_Failed", profile.name)
                    };
                    let token = globals.tg_token.clone();
                    let chat = globals.tg_chat.clone();
                    let thread = tg_thread_opt.clone();
                    tokio::spawn(async move {
                        let _ = telegram::send_with_retry(
                            &token, &chat, &msg, thread.as_deref(), net_retry_count, net_retry_interval_s,
                        ).await;
                    });
                }
            }
        }
    }

    // Save last-run summary
    {
        let error_count: usize = summary.errors.iter()
            .filter_map(|e| serde_json::from_str::<serde_json::Value>(e).ok())
            .filter_map(|v| v.get("error_count").and_then(|n| n.as_u64()))
            .map(|n| n as usize)
            .max()
            .unwrap_or(if summary.errors.is_empty() { 0 } else { summary.errors.len() });
        let last_run = serde_json::json!({
            "pid": &pid,
            "profile": &profile.name,
            "timestamp": Local::now().to_rfc3339(),
            "status": &status_str,
            "duration_s": summary.duration_s,
            "files_new": summary.files_new,
            "bytes_added": summary.bytes_added,
            "snapshot_id": summary.snapshot_id,
            "exit_code": summary.exit_code,
            "error_count": error_count,
            "errors": &summary.errors,
        });
        let log_path = restlex_core::logs_dir().join(format!("{pid}-last-run.json"));
        if let Some(d) = log_path.parent() { let _ = std::fs::create_dir_all(d); }
        // Атомарно: крэш посреди прямого write оставлял обрезанный JSON, и
        // last_run навсегда становился null (ошибка глотается), GUI показывал
        // «нет данных».
        if let Err(e) = restlex_core::config::write_atomic(&log_path, &last_run) {
            tracing::warn!("cannot write {}-last-run.json: {e}", pid);
        }
    }

    delete_running_marker(&pid);
    state.restic_pids.lock().unwrap().remove(&pid);
    state.running.lock().unwrap().insert(pid.clone(), false);

    // ── Выключение другого ПК по MAC: Sleep-on-LAN (magic packet с перевёрнутым MAC).
    // Ловит и выключает restlex-agent на том ПК (галка «Разрешить выключать по сети»).
    // До своего выключения — иначе служба умрёт раньше.
    if status_str == "success" && get_bool("shutdown_remote", false) {
        let mac = get_str("shutdown_remote_mac");
        let mut target = mac_to_ip(&mac).await;
        if target.is_none() && !mac_hex(&mac).is_empty() && mac_hex(&mac) == mac_hex(&get_str("wol_mac")) {
            target = Some(get_str("wol_host")).filter(|h| !h.is_empty());
        }
        match wol_send(&reverse_mac(&mac), target.as_deref().unwrap_or(""), en) {
            Ok(()) => {
                restlex_core::log_profile(&pid, "INFO", &format!(
                    "Sleep-on-LAN: UDP-пакет отправлен для MAC {mac}; доставка и shutdown не подтверждены"
                ));
                // Ping по адресу из ARP/wol_host не удостоверяет MAC цели:
                // наблюдение не должно утверждать, что именно ПК не выключился.
                if let Some(ip) = target.clone() {
                    let (token, chat, thread) = (globals.tg_token.clone(), globals.tg_chat.clone(), tg_thread.clone());
                    let (profile_name, pid_log, mac_log) = (profile.name.clone(), pid.clone(), mac.clone());
                    let telegram_enabled = tg_configured;
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(120)).await;
                        let reachable = host_reachable(&ip, 3).await;
                        let msg = sleep_on_lan_probe_message(en, &profile_name, &mac_log, &ip, reachable);
                        restlex_core::log_profile(&pid_log, "INFO", &msg);
                        if reachable && telegram_enabled {
                            let _ = telegram::send_threaded(&token, &chat, &msg, thread.as_deref()).await;
                        }
                    });
                } else {
                    restlex_core::log_profile(&pid, "INFO", &format!(
                        "Sleep-on-LAN: IP для MAC {mac} неизвестен; ping-проверка пропущена"
                    ));
                }
            }
            Err(e) => {
                let msg = if en {
                    format!("RestLex - Profile: {} | Error: shutdown packet for {} was not sent: {}",
                        profile.name, mac, e)
                } else {
                    format!("RestLex - Профиль: {} | Ошибка: пакет выключения {} не отправлен: {}",
                        profile.name, mac, e)
                };
                restlex_core::log_profile(&pid, "ERROR", &msg);
                if tg_configured {
                    let _ = telegram::send_threaded(&globals.tg_token, &globals.tg_chat,
                        &msg, tg_thread.as_deref()).await;
                }
            }
        }
    }

    // ── Выключение ПК после успешного бэкапа ─────────────────────────────────
    if status_str == "success" && get_bool("shutdown_after_backup", false) {
        let delay = get_u64("shutdown_delay", 60).min(3600);
        restlex_core::log_profile(&pid, "INFO",
            &format!("Выключение ПК через {delay}с (shutdown /s /t {delay})"));
        let out = tokio::process::Command::new("shutdown")
            .args(["/s", "/t", &delay.to_string()])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output()
            .await;
        let (rc, detail) = match &out {
            Ok(o) => (o.status.code().unwrap_or(-1),
                      oem_to_string(if o.stderr.is_empty() { &o.stdout } else { &o.stderr })),
            Err(e) => (-1, e.to_string()),
        };
        if rc != 0 {
            let detail = detail.chars().take(200).collect::<String>();
            let msg = if en {
                format!("RestLex - Profile: {} | Error: PC shutdown was not scheduled (code {}) {}",
                    profile.name, rc, detail)
            } else {
                format!("RestLex - Профиль: {} | Ошибка: выключение ПК не запланировано (код {}) {}",
                    profile.name, rc, detail)
            };
            restlex_core::log_profile(&pid, "ERROR", &msg);
            if tg_configured {
                let _ = telegram::send_threaded(&globals.tg_token, &globals.tg_chat,
                    &msg, tg_thread.as_deref()).await;
            }
        } else if tg_configured {
            // Watchdog: если через delay+запас служба ещё жива — ОС не выключилась
            // (shutdown /a, блокирующий диалог, Windows Update).
            let msg = if en {
                format!("RestLex - Profile: {} | Error: PC did not shut down \
                    ({}s passed since the command)", profile.name, delay + SHUTDOWN_CHECK_MARGIN_S)
            } else {
                format!("RestLex - Профиль: {} | Ошибка: выключение ПК не состоялось \
                    (прошло {}с после команды)", profile.name, delay + SHUTDOWN_CHECK_MARGIN_S)
            };
            let (token, chat, thread) = (globals.tg_token.clone(), globals.tg_chat.clone(), tg_thread.clone());
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(delay + SHUTDOWN_CHECK_MARGIN_S)).await;
                let _ = telegram::send_threaded(&token, &chat, &msg, thread.as_deref()).await;
            });
        }
    }
    Ok(summary)
}

/// Миграция с Windows schtasks на agent-scheduler.
/// Вызывается однажды при старте: удаляет все задачи Restlex\Backup_* из планировщика Windows.
/// Fire-and-forget (tokio::spawn). Ждёт lock-файлы (запущенные PS1) до 3 минут.
pub async fn migrate_from_schtasks(_state: Arc<State>) {
    use tokio::process::Command;

    tracing::info!("schtasks migration: scanning Restlex\\Backup_* tasks...");

    let out = match tokio::time::timeout(
        Duration::from_secs(15),
        Command::new("schtasks")
            .args(["/Query", "/TN", "\\Restlex\\", "/FO", "CSV", "/NH"])
            .output(),
    )
    .await
    {
        Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout).to_string(),
        Ok(Err(e)) => {
            tracing::warn!("schtasks migration: query failed: {e}");
            return;
        }
        Err(_) => {
            tracing::warn!("schtasks migration: query timed out");
            return;
        }
    };

    // CSV формат: "TaskName","NextRunTime","Status"
    let tasks: Vec<String> = out
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with("ERROR") || line.starts_with("INFO") {
                return None;
            }
            // Извлечь первую CSV-колонку
            let name = line.trim_start_matches('"');
            let name = if let Some(end) = name.find('"') {
                &name[..end]
            } else {
                name
            };
            // Только Restlex\Backup_* задачи
            if name.to_lowercase().contains("\\backup_") {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect();

    if tasks.is_empty() {
        tracing::info!("schtasks migration: no Restlex\\Backup_* tasks found — nothing to migrate");
        return;
    }

    tracing::info!("schtasks migration: found {} task(s) to delete", tasks.len());

    // Ждём lock-файлы (признак работающих PS1) до 3 минут
    let log_dir = restlex_core::logs_dir();
    let mut waited = false;
    for _ in 0..36 {
        let has_locks = std::fs::read_dir(&log_dir)
            .map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".lock")))
            .unwrap_or(false);
        if !has_locks {
            break;
        }
        if !waited {
            tracing::info!("schtasks migration: PS1 scripts still running (lock files found) — waiting up to 3 min...");
            waited = true;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    let mut deleted = 0usize;
    let mut failed = 0usize;
    for task in &tasks {
        let tn = if task.starts_with('\\') {
            task.clone()
        } else {
            format!("\\{}", task)
        };
        match tokio::time::timeout(
            Duration::from_secs(10),
            Command::new("schtasks").args(["/Delete", "/TN", &tn, "/F"]).output(),
        )
        .await
        {
            Ok(Ok(o)) if o.status.success() => {
                tracing::info!("schtasks migration: deleted '{}'", tn);
                deleted += 1;
            }
            Ok(Ok(o)) => {
                let s = String::from_utf8_lossy(&o.stderr);
                tracing::warn!("schtasks migration: failed to delete '{}': {}", tn, s.trim());
                failed += 1;
            }
            Ok(Err(e)) => {
                tracing::warn!("schtasks migration: error deleting '{}': {}", tn, e);
                failed += 1;
            }
            Err(_) => {
                tracing::warn!("schtasks migration: timeout deleting '{}'", tn);
                failed += 1;
            }
        }
    }

    tracing::info!("schtasks migration done: deleted={} failed={}", deleted, failed);
}

pub fn watch_config(cfg: Config, _state: Arc<State>) {
    use notify::{RecursiveMode, Watcher};
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = match notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    }) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("watcher init: {e}");
            return;
        }
    };
    let pd = restlex_core::program_data();
    if let Err(e) = watcher.watch(&pd, RecursiveMode::NonRecursive) {
        tracing::error!("watch: {e}");
        return;
    }
    while let Ok(_) = rx.recv() {
        std::thread::sleep(std::time::Duration::from_millis(300));
        while rx.try_recv().is_ok() {}
        if let Err(e) = cfg.reload() {
            tracing::warn!("reload: {e}");
        }
        tracing::debug!("Config reloaded from disk");
    }
}

#[cfg(test)]
mod tests {
    use super::{pick_stop_target, restore_key, sleep_on_lan_probe_message};
    use std::collections::HashMap;

    #[test]
    fn sleep_probe_does_not_claim_shutdown_failure_from_ping() {
        for reachable in [true, false] {
            let text = sleep_on_lan_probe_message(false, "test", "AA:BB:CC:DD:EE:FF", "192.0.2.5", reachable);
            assert!(text.contains("не подтверждены"));
            assert!(!text.contains("не выключился"));
            assert!(!text.contains("нет RestLex-агента"));
            assert!(text.contains(if reachable { "отвечает на ping" } else { "не отвечает на ping" }));

            let text = sleep_on_lan_probe_message(true, "test", "AA:BB:CC:DD:EE:FF", "192.0.2.5", reachable);
            assert!(text.starts_with("RestLex - Profile: test |"));
            assert!(text.contains("not confirmed"));
            assert!(!text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                "en-текст без кириллицы: {text}");
            assert!(text.contains(if reachable { " responds to ping" } else { "does not respond to ping" }));
        }
    }

    /// `stop` во время бэкапа бьёт по голому ключу — как и раньше.
    #[test]
    fn stop_target_prefers_backup() {
        let mut m = HashMap::new();
        m.insert("p1".to_string(), 111u32);
        assert_eq!(pick_stop_target(&m, "p1"), Some(111));
    }

    /// `stop` во время restore находит процесс под ключом `restore:<pid>`
    /// (регресс: раньше возвращалось 0 и restore было не прервать).
    #[test]
    fn stop_target_finds_restore() {
        let mut m = HashMap::new();
        m.insert(restore_key("p1"), 222u32);
        assert_eq!(pick_stop_target(&m, "p1"), Some(222));
    }

    /// Чужой профиль не задевается, и когда ничего не идёт — killed = 0.
    #[test]
    fn stop_target_isolated_per_profile() {
        let mut m = HashMap::new();
        m.insert("p2".to_string(), 333u32);
        m.insert(restore_key("p3"), 444u32);
        assert_eq!(pick_stop_target(&m, "p1"), None);
        assert_eq!(pick_stop_target(&m, "p2"), Some(333));
        assert_eq!(pick_stop_target(&m, "p3"), Some(444));
    }
}
