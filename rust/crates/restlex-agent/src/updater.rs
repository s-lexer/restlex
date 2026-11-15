//! Self-update при старте + фоновый клиент обновлений от restlex-update-server.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::{info, warn};

// ── Startup apply ────────────────────────────────────────────────────────────

pub fn pending_update_path() -> PathBuf {
    restlex_core::program_data().join("agent_update.exe")
}

/// Маркер версии агента, которая не смогла стартовать после self-update.
/// self_update.bat записывает сюда версию при откате на .bak; update_loop читает
/// его, чтобы не переустанавливать заведомо битый билд каждый цикл.
pub fn failed_version_path() -> PathBuf {
    restlex_core::program_data().join("agent_update_failed.txt")
}

/// Версия, для которой маркер неудачного обновления УЖЕ снимали вручную.
/// Не даёт `forced` стирать anti-ping-pong маркер бесконечно.
pub fn cleared_version_path() -> PathBuf {
    restlex_core::program_data().join("agent_update_cleared.txt")
}

/// Как часто «Проверить и обновить» может снимать маркер неудачи для ОДНОЙ и
/// той же версии. Не «один раз навсегда»: единственная неудача (агент не успел
/// подняться за 20 с на нагруженном сервере) иначе блокировала версию насмерть.
const CLEAR_COOLDOWN: Duration = Duration::from_secs(6 * 60 * 60);

/// Содержимое маркера: `<версия> <unix-время снятия>`. Старый формат (одна
/// версия без времени) читается как «снято очень давно» — cooldown уже истёк.
fn write_cleared_marker(ver: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{ver} {now}")
}

/// Возвращает (версия, сколько прошло с момента снятия). None — маркера нет.
fn read_cleared_marker(path: &Path) -> Option<(String, Duration)> {
    let txt = std::fs::read_to_string(path).ok()?;
    let mut it = txt.split_whitespace();
    let ver = it.next()?.to_string();
    let at = it.next().and_then(|s| s.parse::<u64>().ok());
    let age = match at {
        Some(at) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            // now < at — часы перевели назад. saturating_sub давал 0, т.е.
            // маркер вечно «свежий», и «Проверить и обновить» не снимало
            // failmark, пока часы не догонят. Считаем cooldown истёкшим.
            match now.checked_sub(at) {
                Some(d) => Duration::from_secs(d),
                None => CLEAR_COOLDOWN,
            }
        }
        // Старый формат без метки времени: cooldown считаем истёкшим.
        None => CLEAR_COOLDOWN,
    };
    Some((ver, age))
}

pub fn current_exe() -> anyhow::Result<PathBuf> {
    Ok(std::env::current_exe()?)
}

/// SERIALNUMBER'ы в Subject наших EV-сертификатов (ООО СофтИнноватиксПлюс).
/// СПИСОК, а не константа: перевыпуск сертификата с другим рег. номером иначе
/// молча остановил бы обновления на всём парке — а починить это было бы нечем,
/// обновление-то как раз и не приедет. Новый серийник добавляется сюда ЗАРАНЕЕ,
/// до перехода на новый сертификат.
#[cfg(windows)]
const ALLOWED_SIGNER_SERIALS: &[&str] = &["SERIALNUMBER=193808466"];

/// Холодный старт PS 2.0 и проверка отзыва на старых Windows могут быть долгими.
/// Таймаут блокирует только данную попытку обновления.
#[cfg(windows)]
const SIGCHECK_TIMEOUT: Duration = Duration::from_secs(45);

/// Результат проверки подписи. Важно различать «подпись плохая» и «проверить не
/// смогли»: в обоих случаях запрещаем запуск непроверенного кода под SYSTEM.
#[derive(Debug, PartialEq, Eq)]
enum SigCheck {
    Ok,
    Bad(String),
    Unknown(String),
}

#[cfg(windows)]
fn authenticode_check(path: &Path) -> SigCheck {
    // ВНИМАНИЕ: скрипт обязан работать в PowerShell 2.0 (штатный на Win7/2008R2).
    // Нельзя: -LiteralPath у Get-AuthenticodeSignature (появился в PS 3.0),
    // -EA/-ErrorAction в сокращённой форме на этом командлете, ??, ?:, $PSItem,
    // ConvertFrom-Json. -FilePath есть с PS 1.0.
    let ps = format!(
        "$ErrorActionPreference='SilentlyContinue'; \
         $s=Get-AuthenticodeSignature -FilePath '{}'; \
         if($s -eq $null){{'ERR'}}else{{'ST:'+$s.Status+'|SUBJ:'+$s.SignerCertificate.Subject}}",
        path.display().to_string().replace('\'', "''")
    );

    // Таймаут вручную: std::process::Command::output() ждёт вечно, а вызывается
    // это в том числе из run_service до рапорта SCM. Поток с output() + канал с
    // recv_timeout — powershell, не уложившийся в срок, дожуёт в фоне сам.
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let res = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
            .output();
        let _ = tx.send(res);
    });
    let out = match rx.recv_timeout(SIGCHECK_TIMEOUT) {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return SigCheck::Unknown(format!("powershell не запустился: {e}")),
        Err(_) => {
            return SigCheck::Unknown(format!(
                "powershell не ответил за {} с",
                SIGCHECK_TIMEOUT.as_secs()
            ))
        }
    };
    if !out.status.success() {
        return SigCheck::Unknown(format!(
            "powershell завершился с rc={:?}: {}",
            out.status.code(), String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if stdout.is_empty() || stdout == "ERR" {
        return SigCheck::Unknown(format!(
            "powershell не вернул статус (rc={:?}, stderr={})",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let status = stdout
        .strip_prefix("ST:")
        .and_then(|s| s.split('|').next())
        .unwrap_or("")
        .trim()
        .to_string();
    let subject = stdout.split("|SUBJ:").nth(1).unwrap_or("").trim().to_string();
    if status.is_empty() {
        return SigCheck::Unknown(format!("непонятный ответ powershell: {stdout}"));
    }
    // Дальше powershell ЯВНО ответил — это уже суждение о подписи, fail-closed.
    if status != "Valid" {
        return SigCheck::Bad(format!("статус подписи {status}"));
    }
    // Издатель: EV-сертификат «ООО СофтИнноватиксПлюс». Сверяем по
    // SERIALNUMBER (регистрационный номер организации) — он в подписи ASCII и
    // не зависит от кодовой страницы консоли, в отличие от кириллического CN.
    if !ALLOWED_SIGNER_SERIALS.iter().any(|s| subject.contains(s)) {
        return SigCheck::Bad(format!("чужой подписант: {subject}"));
    }
    SigCheck::Ok
}

/// Authenticode-проверка бинарника перед подменой службы.
/// `C:\ProgramData\RestLex\agent_update.exe` лежит в каталоге, где дефолтные ACL
/// позволяют обычному пользователю СОЗДАВАТЬ файлы: без проверки подписи любой
/// непривилегированный юзер подкладывал свой exe и получал его запуск как
/// LocalSystem при следующем старте службы.
///
/// Любой результат кроме Ok останавливает обновление.
fn signature_allows_update(check: &SigCheck) -> bool {
    matches!(check, SigCheck::Ok)
}

#[cfg(windows)]
fn authenticode_valid(path: &Path) -> bool {
    let check = authenticode_check(path);
    match &check {
        SigCheck::Ok => {
            info!("Authenticode OK: {:?}", path);
        }
        SigCheck::Bad(why) => {
            warn!("Authenticode: отказ для {:?} — {why}", path);
        }
        SigCheck::Unknown(why) => {
            tracing::error!(
                "Authenticode: ПРОВЕРИТЬ НЕ УДАЛОСЬ для {:?} — {why}; обновление отложено",
                path
            );
        }
    }
    signature_allows_update(&check)
}

#[cfg(not(windows))]
fn authenticode_valid(_path: &Path) -> bool {
    true
}

/// Удаляет устаревшие agent_old_*.exe рядом с текущим бинарником.
fn cleanup_old_agents() {
    let exe = match current_exe() {
        Ok(e) => e,
        Err(_) => return,
    };
    let dir = match exe.parent() {
        Some(d) => d.to_path_buf(),
        None => return,
    };
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if s.starts_with("agent_old_") && s.ends_with(".exe") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Новый каталог для кандидата, доступный только SYSTEM и Администраторам.
///
/// `agent_update.exe` лежит в `C:\ProgramData\RestLex`, где дефолтные ACL
/// позволяют обычному пользователю СОЗДАВАТЬ файлы. Проверка подписи и подмена —
/// это три отдельных открытия одного и того же пути (`fs::read`,
/// `Get-AuthenticodeSignature`, `fs::rename`), между которыми файл можно
/// подменить: проверили подписанный, запустили под LocalSystem чужой.
/// DACL устанавливается атомарно при создании: последовательность create +
/// icacls оставляет окно для открытия каталога с правом записи. Случайное имя
/// и CreateDirectoryW (без открытия существующего каталога) исключают подмену
/// ранее созданным stage или reparse point.
#[cfg(windows)]
fn secure_stage_dir() -> std::io::Result<PathBuf> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        descriptor: *mut c_void,
        inherit_handle: i32,
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl: *const u16, revision: u32, descriptor: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateDirectoryW(path: *const u16, attrs: *const SecurityAttributes) -> i32;
        fn LocalFree(ptr: *mut c_void) -> *mut c_void;
    }

    // Protected DACL: no inherited ACEs, only LocalSystem and Administrators.
    // SDDL API и CreateDirectoryW доступны начиная с Windows XP.
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"
        .encode_utf16().chain(Some(0)).collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl.as_ptr(), 1, &mut descriptor, std::ptr::null_mut(),
    ) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let attrs = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        descriptor,
        inherit_handle: 0,
    };
    let exe = std::env::current_exe()?;
    let parent = exe.parent().ok_or_else(|| std::io::Error::other("exe без каталога"))?;
    let dir = parent.join(format!("stage-{:032x}", rand::random::<u128>()));
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let created = unsafe { CreateDirectoryW(wide.as_ptr(), &attrs) };
    let error = if created == 0 { Some(std::io::Error::last_os_error()) } else { None };
    unsafe { LocalFree(descriptor); }
    match error { Some(e) => Err(e), None => Ok(dir) }
}

#[cfg(not(windows))]
fn secure_stage_dir() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let parent = exe.parent().ok_or_else(|| std::io::Error::other("exe без каталога"))?;
    let dir = parent.join(format!("stage-{:032x}", rand::random::<u128>()));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Копирует staged-бинарник в защищённый каталог и удаляет оригинал.
/// Возвращает путь к копии — дальше работаем ТОЛЬКО с ней.
/// fail-CLOSED: не смогли обеспечить закрытый каталог — обновление не
/// применяем (иначе проверка подписи ничего не гарантирует).
fn stage_secure_copy(upd: &Path) -> std::io::Result<PathBuf> {
    let dir = secure_stage_dir()?;
    let dst = dir.join("agent_update.exe");
    let copied = (|| {
        let mut source = std::fs::File::open(upd)?;
        let mut target = std::fs::OpenOptions::new().write(true).create_new(true).open(&dst)?;
        std::io::copy(&mut source, &mut target)?;
        target.sync_all()
    })();
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&dst);
        let _ = std::fs::remove_dir(&dir);
        return Err(e);
    }
    // Оригинал в общедоступном каталоге больше не нужен: иначе битый/чужой
    // файл заставлял бы повторять всю процедуру каждый старт.
    let _ = std::fs::remove_file(upd);
    Ok(dst)
}

fn remove_secure_copy(path: &Path) {
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir(dir);
    }
}

/// Вызывается при старте службы. Возвращает true если обновление применено
/// (SCM перезапустит нас с новым бинарником).
pub fn apply_pending_update() -> bool {
    let upd = pending_update_path();
    if !upd.exists() {
        cleanup_old_agents();
        if let Ok(exe) = current_exe() {
            if let Some(dir) = exe.parent() {
                cleanup_old_gui(dir);
            }
        }
        return false;
    }

    let exe = match current_exe() {
        Ok(e) => e,
        Err(_) => return false,
    };

    // ДО любых проверок уводим кандидата из общедоступного ProgramData в
    // каталог, куда может писать только SYSTEM: иначе между проверкой подписи и
    // rename файл подменяется, и под LocalSystem стартует чужой бинарник.
    let upd = match stage_secure_copy(&upd) {
        Ok(p) => p,
        Err(e) => {
            warn!("не удалось положить кандидата в защищённый каталог — обновление не применяю: {e}");
            return false;
        }
    };

    // Validate staged binary before touching anything
    match std::fs::read(&upd) {
        Ok(data) => {
            if data.len() < 500_000 || data.get(0..2) != Some(b"MZ") {
                warn!("Staged agent_update.exe is invalid (size={} no MZ), removing", data.len());
                remove_secure_copy(&upd);
                return false;
            }
        }
        Err(e) => {
            warn!("Cannot read agent_update.exe: {e}");
            remove_secure_copy(&upd);
            return false;
        }
    }

    // MZ+размер ничего не доказывают: ProgramData позволяет обычному юзеру
    // создать файл. Требуем валидную подпись нашего EV-сертификата.
    if !authenticode_valid(&upd) {
        warn!("Staged agent_update.exe не подписан нашим сертификатом, удаляю");
        remove_secure_copy(&upd);
        return false;
    }

    // Unique name to avoid AV/ERS handle conflicts
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let old = exe.with_file_name(format!("agent_old_{nanos}.exe"));

    // Step 1: rename current → old
    if let Err(e) = std::fs::rename(&exe, &old) {
        warn!("Cannot rename current exe to old: {e}");
        remove_secure_copy(&upd);
        return false;
    }

    // Step 2: rename update → current (may fail cross-drive → fallback copy)
    let applied = if std::fs::rename(&upd, &exe).is_ok() {
        true
    } else {
        // Cross-drive or AV hold — try copy
        match std::fs::copy(&upd, &exe) {
            Ok(_) => {
                remove_secure_copy(&upd);
                true
            }
            Err(e) => {
                warn!("Cannot copy agent_update.exe to exe path: {e}");
                false
            }
        }
    };

    if !applied {
        // fs::copy может оставить частичный файл, мешающий rollback на Windows.
        remove_secure_copy(&upd);
        rollback_agent(&exe, &old);
        return false;
    }

    // Step 3: validate applied binary
    match std::fs::metadata(&exe) {
        Ok(m) if m.len() >= 500_000 => {}
        _ => {
            warn!("Applied binary is too small, rolling back");
            rollback_agent(&exe, &old);
            return false;
        }
    }

    // rename уносит файл из stage, copy оставляет его до очистки выше.
    if let Some(dir) = upd.parent() { let _ = std::fs::remove_dir(dir); }

    info!("Self-update applied (old saved as {:?}), restart required", old.file_name());
    true
}

fn rollback_agent(exe: &Path, old: &Path) {
    if let Err(e) = std::fs::remove_file(exe) {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn!("Rollback: не удалить частичный exe {:?}: {e}", exe);
        }
    }
    if let Err(e) = std::fs::rename(old, exe) {
        tracing::error!("Rollback: не восстановить {:?} из {:?}: {e}", exe, old);
    }
}

// ── UpdatePhase ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    Downloading,
    WaitingBackup,
    Applying,
    UpToDate,
    Error(String),
}

impl std::fmt::Display for UpdatePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Checking => write!(f, "checking"),
            Self::Downloading => write!(f, "downloading"),
            Self::WaitingBackup => write!(f, "waiting_backup"),
            Self::Applying => write!(f, "applying"),
            Self::UpToDate => write!(f, "up_to_date"),
            Self::Error(e) => write!(f, "error: {e}"),
        }
    }
}

// ── Manifest types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Manifest {
    /// Agent version — triggers agent self-update when newer than AGENT_VERSION.
    #[serde(alias = "version", default)]
    agent_version: String,
    /// GUI version — informational; GUI update is driven by SHA-256 diff.
    #[serde(default)]
    gui_version: String,
    files: Vec<FileEntry>,
}

#[derive(Debug, Deserialize)]
struct FileEntry {
    name: String,
    size: u64,
    sha256: String,
}

/// Имя файла из манифеста уезжает в `upd_dir.join(name)` и в URL закачки.
/// Подменённый манифест с `"..\\..\\Windows\\System32\\evil.dll"` дал бы запись
/// произвольного файла под LocalSystem — пропускаем только плоское имя.
fn valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains(':')
        && !name.contains('\0')
        && !name.contains("..")
        && std::path::Path::new(name).file_name().map(|f| f == name).unwrap_or(false)
}

/// Версия агента подставляется в текст .bat, который выполняется под SYSTEM
/// (`(echo {ver})>"{failmark}"`). `parse_ver` мусор пропускает — фильтрует
/// нечисловые части, — поэтому `9.9 & powershell -enc ...` доезжал бы до cmd.
fn valid_version(ver: &str) -> bool {
    !ver.is_empty()
        && ver.len() <= 16
        && ver.chars().all(|c| c.is_ascii_digit() || c == '.')
}

// ── Circuit breaker ───────────────────────────────────────────────────────────
// Мягкие пороги: служба и так проверяет обновления раз в update_check_interval_m
// (деф. 30 мин) и спит между циклами, поэтому долгий агрессивный блок не нужен —
// он лишь мешает легитимным ручным проверкам. Взводим редко и ненадолго.
const CB_FAIL_THRESHOLD: u32 = 10;   // неудач ПОДРЯД до взвода (счётчик обнуляется при любом успехе)
const CB_BLOCK_SECS: u64    = 300;   // блок на 5 мин (было 10 мин; ручная проверка снимает мгновенно)

#[derive(Debug, Default, Serialize, Deserialize)]
struct FailRecord {
    version: String,
    download_fails: u32,
    apply_fails: u32,
    blocked_until: Option<u64>, // unix secs
}

fn fails_path(upd_dir: &Path) -> PathBuf {
    upd_dir.join("update_fails.json")
}

fn load_fails(upd_dir: &Path) -> FailRecord {
    let p = fails_path(upd_dir);
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_fails(upd_dir: &Path, rec: &FailRecord) {
    if let Ok(s) = serde_json::to_string_pretty(rec) {
        let _ = std::fs::write(fails_path(upd_dir), s);
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn is_blocked(rec: &FailRecord) -> bool {
    rec.blocked_until.map(|t| now_secs() < t).unwrap_or(false)
}

// ── Version comparison ────────────────────────────────────────────────────────

fn parse_ver(s: &str) -> (u32, u32, u32) {
    let parts: Vec<u32> = s.trim().split('.').filter_map(|p| p.parse().ok()).collect();
    (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    )
}

fn is_newer(remote: &str, local: &str) -> bool {
    parse_ver(remote) > parse_ver(local)
}

// ── SHA256 verify ─────────────────────────────────────────────────────────────

fn sha256_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

async fn fetch_manifest(url: &str, api_key: &str) -> anyhow::Result<Manifest> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let resp = client
        .get(format!("{}/api/manifest", url.trim_end_matches('/')))
        .header("X-Api-Key", api_key)
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("manifest HTTP {}", resp.status());
    }
    Ok(resp.json::<Manifest>().await?)
}

async fn fetch_file(url: &str, api_key: &str, name: &str) -> anyhow::Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;
    let resp = client
        .get(format!(
            "{}/api/download?file={}",
            url.trim_end_matches('/'),
            name
        ))
        .header("X-Api-Key", api_key)
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("download HTTP {} for {name}", resp.status());
    }
    Ok(resp.bytes().await?.to_vec())
}

// ── Download phase ────────────────────────────────────────────────────────────

async fn download_files(
    manifest: &Manifest,
    upd_dir: &Path,
    url: &str,
    api_key: &str,
    en: bool,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(upd_dir)?;
    for f in &manifest.files {
        if !valid_file_name(&f.name) {
            if en {
                anyhow::bail!("manifest: invalid file name {:?}", f.name);
            }
            anyhow::bail!("manifest: недопустимое имя файла {:?}", f.name);
        }
        let staged = upd_dir.join(&f.name);
        // skip if already staged with correct hash
        if staged.exists() {
            if let Ok(data) = std::fs::read(&staged) {
                if data.len() as u64 == f.size && sha256_bytes(&data) == f.sha256 {
                    info!("Skip download {}: already staged", f.name);
                    continue;
                }
            }
        }
        info!("Downloading {}", f.name);
        let data = fetch_file(url, api_key, &f.name).await?;
        if data.len() as u64 != f.size {
            anyhow::bail!("size mismatch for {}: got {} expected {}", f.name, data.len(), f.size);
        }
        let hash = sha256_bytes(&data);
        if hash != f.sha256 {
            anyhow::bail!("sha256 mismatch for {}", f.name);
        }
        let tmp = staged.with_extension("tmp");
        std::fs::write(&tmp, &data)?;
        std::fs::rename(&tmp, &staged)?;
        info!("Staged {}", f.name);
    }
    Ok(())
}

// ── GUI update check by SHA-256 ───────────────────────────────────────────────

fn gui_update_needed(files: &[FileEntry], install_dir: &Path) -> bool {
    let entry = match files.iter().find(|f| f.name.eq_ignore_ascii_case("restlex.exe")) {
        Some(e) => e,
        None => return false,
    };
    let installed = install_dir.join("restlex.exe");
    match std::fs::read(&installed) {
        Ok(data) => sha256_bytes(&data) != entry.sha256,
        Err(_) => false, // not installed — don't force-install GUI automatically
    }
}

// ── Cleanup old GUI binaries ──────────────────────────────────────────────────

fn cleanup_old_gui(install_dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(install_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if s.starts_with("restlex_old_") && s.ends_with(".exe") {
                if let Err(e) = std::fs::remove_file(entry.path()) {
                    tracing::warn!("cleanup_old_gui: cannot remove {:?}: {e}", entry.path());
                }
            }
        }
    }
}

// ── Apply: restlex.exe (GUI) ──────────────────────────────────────────────────

fn apply_gui_update(upd_dir: &Path, install_dir: &Path, gui_version: &str, en: bool) -> anyhow::Result<()> {
    let src = upd_dir.join("restlex.exe");
    if !src.exists() {
        return Ok(());
    }
    let dst = install_dir.join("restlex.exe");
    // GUI запускают администраторы — без проверки издателя подменённый по HTTP
    // manifest+exe (sha256 берётся из того же manifest) ставился бы как есть.
    if !authenticode_valid(&src) {
        if en {
            anyhow::bail!("restlex.exe: signature check failed — GUI not updated");
        }
        anyhow::bail!("restlex.exe: подпись не прошла проверку — GUI не обновлён");
    }

    // Remove any leftover _old_ files from previous updates before starting
    cleanup_old_gui(install_dir);

    // check if GUI was running
    let was_running = is_process_running("restlex.exe");
    if was_running {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", "restlex.exe"])
            .output();
        std::thread::sleep(Duration::from_secs(2));
    }

    let nanos = now_secs();
    let old = install_dir.join(format!("restlex_old_{nanos}.exe"));
    if dst.exists() {
        std::fs::rename(&dst, &old)?;
    }
    std::fs::copy(&src, &dst)?;

    let meta = std::fs::metadata(&dst)?;
    if meta.len() < 500_000 {
        // rollback
        if old.exists() { let _ = std::fs::rename(&old, &dst); }
        anyhow::bail!("restlex.exe too small after copy, rollback");
    }

    // Retry removing old binary — Windows may hold handle briefly after taskkill
    for i in 0..5u32 {
        if std::fs::remove_file(&old).is_ok() {
            break;
        }
        if i < 4 {
            std::thread::sleep(Duration::from_millis(600));
        }
    }
    let _ = std::fs::remove_file(&src);
    info!("restlex.exe updated");

    // Sync launcher.version so reports/header reflect the actually installed GUI
    // without requiring user to launch restlex.exe manually.
    if !gui_version.is_empty() {
        let lv = restlex_core::program_data().join("launcher.version");
        if let Some(parent) = lv.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&lv, gui_version) {
            tracing::warn!("cannot update launcher.version: {e}");
        }
    }

    if was_running {
        relaunch_gui_in_user_session(&dst);
        info!("restlex.exe relaunched in user session");
    }
    Ok(())
}

fn relaunch_gui_in_user_session(exe: &Path) {
    // Get interactive user by finding who owns explorer.exe — reliable from SYSTEM context.
    let username = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command",
               "try{$p=Get-Process explorer -EA Stop|Select-Object -First 1;\
               (Get-WmiObject Win32_Process -Filter \"ProcessId=$($p.Id)\").GetOwner().User}catch{''}"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let tn = format!("RestlexLaunchGUI_{}", now_secs());
    let exe_str = exe.to_string_lossy().to_string();

    let mut schtasks_args: Vec<String> = vec![
        "/create".into(), "/f".into(),
        "/tn".into(), tn.clone(),
        "/sc".into(), "once".into(), "/st".into(), "23:59".into(),
        "/tr".into(), exe_str,
        "/rl".into(), "LIMITED".into(),
    ];
    if !username.is_empty() {
        schtasks_args.extend_from_slice(&["/ru".into(), username, "/it".into()]);
    } else {
        tracing::warn!("relaunch_gui: no interactive user found, task will run as SYSTEM");
    }

    let create_ok = std::process::Command::new("schtasks")
        .args(&schtasks_args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !create_ok {
        tracing::warn!("relaunch_gui: schtasks /create failed, spawning directly");
        let _ = std::process::Command::new(exe).spawn();
        return;
    }

    // Sleep before /run to ensure task is registered
    std::thread::sleep(Duration::from_secs(2));
    let _ = std::process::Command::new("schtasks")
        .args(["/run", "/tn", &tn])
        .output();
    std::thread::sleep(Duration::from_secs(3));
    let _ = std::process::Command::new("schtasks")
        .args(["/delete", "/f", "/tn", &tn])
        .output();
}

// ── Apply: restlex-agent.exe (self-update via SCM restart) ───────────────────

/// Подготавливает self-update через внешний batch-скрипт.
/// Запускает detached cmd.exe, который:
/// 1. Ждёт 3 сек (даёт службе остановиться)
/// 2. sc stop RestlexAgent + ждёт STOPPED
/// 3. ren current → .bak, copy staged → current
/// 4. sc start RestlexAgent
/// 5. Удаляет себя
/// После Ok caller должен вызвать shutdown_notify.
fn stage_agent_self_update(upd_dir: &Path, ver: &str, en: bool) -> anyhow::Result<()> {
    // Версия попадает в тело .bat, исполняемого под SYSTEM — только цифры и точки.
    if !valid_version(ver) {
        if en {
            anyhow::bail!("update refused: invalid version in manifest {ver:?}");
        }
        anyhow::bail!("отказ от обновления: недопустимая версия в манифесте {ver:?}");
    }
    let src = upd_dir.join("restlex-agent.exe");
    if !src.exists() {
        anyhow::bail!("staged restlex-agent.exe not found in {:?}", upd_dir);
    }

    // Копируем до проверки из доступного ProgramData в каталог с закрытой DACL.
    // Batch позже читает именно эту неизменяемую для обычного пользователя копию.
    let src = stage_secure_copy(&src)?;
    let data = match std::fs::read(&src) {
        Ok(data) => data,
        Err(e) => {
            remove_secure_copy(&src);
            return Err(e.into());
        }
    };
    if data.len() < 500_000 || data.get(0..2) != Some(b"MZ") {
        remove_secure_copy(&src);
        anyhow::bail!("staged restlex-agent.exe is invalid (size={} no MZ)", data.len());
    }
    if !authenticode_valid(&src) {
        remove_secure_copy(&src);
        if en {
            anyhow::bail!("staged restlex-agent.exe is not signed with our certificate");
        }
        anyhow::bail!("staged restlex-agent.exe не подписан нашим сертификатом");
    }

    let exe = current_exe()?;
    let install_dir = exe.parent().ok_or_else(|| anyhow::anyhow!("no parent"))?;
    let bak = install_dir.join("restlex-agent.exe.bak");
    // Batch исполняется под SYSTEM: если положить его в общий корень
    // ProgramData, пользователь может подменить команды между write и spawn.
    // Держим .bat в том же stage с защищённой DACL, что и проверенный exe.
    let bat = src.parent().expect("secure candidate has a parent").join("self_update.bat");
    // Маркер «эта версия агента не стартовала после self-update». Лежит в корне
    // ProgramData (НЕ в upd\, который может быть очищен apply-circuit-breaker'ом),
    // чтобы пережить переустановку и заблокировать повторное применение битого билда.
    let failmark = failed_version_path();
    // Свежая попытка применения — снимаем возможный устаревший маркер.
    let _ = std::fs::remove_file(&failmark);
    let tn = format!("RestlexSelfUpdate_{}", now_secs());

    let bat_content = format!(
r#"@echo off
setlocal enableextensions
set LOG="{install_dir}\self_update.log"
echo [%date% %time%] self_update start >> %LOG%
ping -n 4 127.0.0.1 >nul

sc stop {svc} >nul 2>&1
set /a TRIES=0
:wait_stop
sc query {svc} | find "STOPPED" >nul 2>&1 && goto do_swap
set /a TRIES+=1
if %TRIES% GEQ 30 (
  echo [%date% %time%] FAIL stop timeout >> %LOG%
  sc start {svc}
  goto cleanup
)
ping -n 2 127.0.0.1 >nul
goto wait_stop

:do_swap
echo [%date% %time%] stopped, swapping >> %LOG%
if exist "{bak}" del /f /q "{bak}" 2>nul
move /Y "{exe}" "{bak}" >nul 2>&1
if errorlevel 1 (
  echo [%date% %time%] FAIL move current to bak >> %LOG%
  sc start {svc}
  goto cleanup
)
copy /Y "{src}" "{exe}" >nul 2>&1
if errorlevel 1 (
  echo [%date% %time%] FAIL copy new exe — rollback >> %LOG%
  del /f /q "{exe}" 2>nul
  move /Y "{bak}" "{exe}" >nul 2>&1
  if errorlevel 1 (
    echo [%date% %time%] ERROR rollback failed; service left stopped >> %LOG%
    goto cleanup
  )
  sc start {svc}
  goto cleanup
)

echo [%date% %time%] swap done, starting service >> %LOG%
sc start {svc} >nul 2>&1
set /a RTRIES=0
:wait_run
sc query {svc} | find "RUNNING" >nul 2>&1 && goto run_ok
set /a RTRIES+=1
if %RTRIES% GEQ 10 goto run_fail
ping -n 2 127.0.0.1 >nul
goto wait_run

:run_ok
echo [%date% %time%] OK service RUNNING >> %LOG%
rem ponytail: .bak НЕ удаляем. 20 секунд аптайма — не доказательство здоровья:
rem на одном из серверов новый агент падал через ~30 с (битый парсер конфига), а .bak уже
rem был стёрт — откатываться стало нечем, SCM крутил рестарты битого билда.
rem .bak живёт до следующего успешного self-update (его перезатирает :do_swap).
rem Подтверждение по первому успешному heartbeat — кода heartbeat нет в updater.rs
rem (он в main.rs, шлётся раз в 6 ч), поэтому маркер «версия подтверждена» не заводим.
if exist "{failmark}" del /f /q "{failmark}" 2>nul
goto cleanup

:run_fail
echo [%date% %time%] WARN new exe not RUNNING — rolling back to .bak >> %LOG%
(echo {ver})>"{failmark}"
sc stop {svc} >nul 2>&1
set /a STRIES=0
:wait_stop2
sc query {svc} | find "STOPPED" >nul 2>&1 && goto do_rollback
set /a STRIES+=1
if %STRIES% GEQ 30 goto do_rollback
ping -n 2 127.0.0.1 >nul
goto wait_stop2
:do_rollback
if exist "{bak}" (
  del /f /q "{exe}" 2>nul
  move /Y "{bak}" "{exe}" >nul 2>&1
  if errorlevel 1 (
    echo [%date% %time%] ERROR rollback failed; service left stopped >> %LOG%
    goto cleanup
  )
  sc start {svc} >nul 2>&1
  echo [%date% %time%] rolled back to previous agent >> %LOG%
) else (
  echo [%date% %time%] ERROR no .bak to roll back; service left stopped >> %LOG%
)
goto cleanup

:cleanup
echo [%date% %time%] cleanup >> %LOG%
del /f /q "{src}" 2>nul
schtasks /delete /f /tn "{tn}" >nul 2>&1
rem Batch находится в защищённой stage: сначала удалить себя, затем каталог.
(goto) 2>nul & del /f /q "%~f0" & rmdir "{stage_dir}" >nul 2>&1
"#,
        tn = tn,
        install_dir = install_dir.display(),
        svc = "RestlexAgent",
        bak = bak.display(),
        exe = exe.display(),
        src = src.display(),
        stage_dir = src.parent().expect("stage path has parent").display(),
        failmark = failmark.display(),
        ver = ver,
    );

    std::fs::write(&bat, bat_content)?;
    info!("Self-update batch written to {:?}", bat);

    let bat_str = bat.to_string_lossy().to_string();

    // Primary: WMI Win32_Process.Create via wmic.exe — the new process is
    // hosted by WmiPrvSE, completely outside the service's job tree.
    // This is the only reliable escape on machines where EDR wraps the
    // service in a nested job that ignores JOB_OBJECT_LIMIT_BREAKAWAY_OK.
    match spawn_via_wmi(&bat_str) {
        Ok(()) => {
            info!("Self-update batch spawned via WMI");
            return Ok(());
        }
        Err(e) => {
            warn!("WMI spawn failed: {e} — falling back to schtasks");
        }
    }

    // Fallback 1: schtasks one-time task.
    let now_t = chrono::Local::now() + chrono::Duration::minutes(1);
    let st = now_t.format("%H:%M").to_string();
    let create_out = std::process::Command::new("schtasks")
        .args(["/create", "/f", "/tn", &tn, "/sc", "once", "/st", &st,
               "/tr", &format!(r#"cmd /c "{}""#, bat_str),
               "/ru", "SYSTEM", "/rl", "HIGHEST"])
        .output()?;
    if !create_out.status.success() {
        let stderr = String::from_utf8_lossy(&create_out.stderr);
        let stdout = String::from_utf8_lossy(&create_out.stdout);
        warn!(
            "schtasks create failed (code={:?}) stderr={} stdout={} — falling back to direct breakaway",
            create_out.status.code(), stderr.trim(), stdout.trim()
        );
        // Fallback 2: last-resort direct breakaway spawn.
        spawn_detached_breakaway("cmd", &["/c", &bat_str])?;
        info!("Self-update batch spawned via direct breakaway (last-resort fallback)");
        return Ok(());
    }
    let _ = std::process::Command::new("schtasks")
        .args(["/run", "/tn", &tn]).output();
    info!("Self-update scheduled task {} created and triggered (schtasks fallback)", tn);
    Ok(())
}

#[cfg(windows)]
fn spawn_via_wmi(bat_path: &str) -> anyhow::Result<()> {
    // wmic process call create "cmd /c <bat>"
    // wmic talks to WMI service via RPC; WMI service spawns the new process
    // hosted by WmiPrvSE.exe — separate process tree, separate job.
    let cmdline = format!(r#"cmd /c "{}""#, bat_path);
    let out = std::process::Command::new("wmic")
        .args(["process", "call", "create", &cmdline])
        .output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        anyhow::bail!(
            "wmic process create failed (code={:?}) stderr={} stdout={}",
            out.status.code(), stderr.trim(), stdout.trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // wmic prints ReturnValue = N; 0 means success.
    if stdout.contains("ReturnValue = 0;") || stdout.contains("ReturnValue = 0\n") {
        return Ok(());
    }
    anyhow::bail!("wmic process create non-zero ReturnValue: {}", stdout.trim());
}

#[cfg(not(windows))]
fn spawn_via_wmi(_bat_path: &str) -> anyhow::Result<()> {
    anyhow::bail!("WMI spawn is Windows-only")
}

#[cfg(windows)]
fn spawn_detached_breakaway(program: &str, args: &[&str]) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    // Windows process creation flags
    const DETACHED_PROCESS: u32        = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
    let child = cmd.spawn()?;
    // Don't wait — that's the point. Drop the handle.
    drop(child);
    Ok(())
}

#[cfg(not(windows))]
fn spawn_detached_breakaway(_program: &str, _args: &[&str]) -> anyhow::Result<()> {
    anyhow::bail!("breakaway spawn is Windows-only")
}

// ── Process check helper ──────────────────────────────────────────────────────

fn is_process_running(name: &str) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {name}"), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(name))
        .unwrap_or(false)
}

// ── Main update loop ──────────────────────────────────────────────────────────

pub async fn update_loop(state: Arc<crate::scheduler::State>) {
    // Even when auto_update=false we run the loop to populate latest_version for display
    let upd_dir = restlex_core::program_data().join("upd");
    let install_dir = match current_exe() {
        Ok(e) => e.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
        Err(_) => return,
    };
    let trigger = state.update_trigger.clone();

    loop {
        // Конфиг перечитывается на КАЖДОЙ итерации (config-watcher держит его
        // свежим на диске), поэтому смена URL/API-ключа сервера обновлений и
        // отчётов применяется БЕЗ перезапуска службы — на следующем цикле
        // проверки или сразу при нажатии «Проверить обновления» в GUI.
        let globals = state.cfg.globals();
        // Язык текстов error в отчётах на веб-сервер (логи — как были).
        let en = globals.en();
        // ponytail: захардкоженный дефолт — headless-хосты (агент без единого
        // запуска GUI) не должны вечно висеть без проверки обновлений и
        // самолечения launcher.version. GUI пишет тот же URL в config.json,
        // если пользователь его откроет — тогда это просто переопределится.
        let url = if globals.update_server_url.is_empty() {
            "http://1221.by:3266".to_string()
        } else {
            globals.update_server_url.clone()
        };
        // Ключа в исходниках нет: сервер обновлений работает в режиме public
        // (программы без ключа). Сборка может вшить ключ через RESTLEX_UPDATE_KEY
        // для приватного сервера. ponytail: канал всё ещё plain HTTP — защищает
        // только Authenticode-проверка; HTTPS — отдельная задача.
        let api_key = if globals.update_api_key.is_empty() {
            option_env!("RESTLEX_UPDATE_KEY").unwrap_or("").to_string()
        } else {
            globals.update_api_key.clone()
        };
        let report_url = globals.report_url.clone();
        let report_api_key = globals.report_api_key.clone();
        let interval_m = globals.update_check_interval_m.max(1);

        // Сервер обновлений ещё не задан — простаиваем и перепроверяем конфиг
        // позже (НЕ делаем постоянный return: URL могут вписать на лету).
        if url.is_empty() {
            set_phase(&state, UpdatePhase::Idle);
            tokio::select! {
                _ = sleep_minutes(interval_m) => {}
                _ = trigger.notified() => {}
            }
            continue;
        }

        // Short initial delay so GUI gets latest_version quickly after restart
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = trigger.notified() => {}
        }

        set_phase(&state, UpdatePhase::Checking);
        *state.update_last_check.lock().unwrap() = Some(chrono::Local::now());

        let mut rec = load_fails(&upd_dir);

        // Ручная проверка из GUI («Проверить и обновить») ВСЕГДА снимает блокировку
        // и обнуляет счётчик неудач: человек явно просит проверить — не мешаем ему.
        // Это локальный «разбан» — не нужно идти в админку веб-сервера.
        if state.manual_check.swap(false, std::sync::atomic::Ordering::Relaxed)
            && (is_blocked(&rec) || rec.download_fails > 0 || rec.apply_fails > 0)
        {
            let _ = std::fs::remove_file(fails_path(&upd_dir));
            rec = FailRecord::default();
            info!("Ручная проверка обновлений — circuit breaker сброшен локально");
        }

        macro_rules! sleep_or_trigger {
            () => {
                tokio::select! {
                    _ = sleep_minutes(interval_m) => {}
                    _ = trigger.notified() => {}
                }
            };
        }

        // Pull-команда из админки сервера отчётов: сбросить circuit breaker.
        // (сервер не может достучаться до агента за NAT — агент сам тянет команду)
        if let Ok(cmd) = restlex_core::reporter::fetch_commands(
            &report_url, &report_api_key, &restlex_core::reporter::hostname(),
        ).await {
            if cmd.reset_circuit_breaker {
                let _ = std::fs::remove_file(fails_path(&upd_dir));
                rec = FailRecord::default();
                info!("Circuit breaker сброшен командой из админки");
            }
        }

        // Сообщаем серверу текущее состояние circuit breaker (для вкладки «Обновления»).
        {
            let blocked = is_blocked(&rec);
            let _ = restlex_core::reporter::send_cb_status(
                &report_url, &report_api_key,
                &restlex_core::reporter::CbStatus {
                    host: restlex_core::reporter::hostname(),
                    blocked_until: if blocked { rec.blocked_until.unwrap_or(0) as i64 } else { 0 },
                    fail_count: rec.download_fails + rec.apply_fails,
                },
            ).await;
        }

        if is_blocked(&rec) {
            warn!("Update blocked by circuit breaker until {}", rec.blocked_until.unwrap_or(0));
            set_phase(&state, UpdatePhase::Idle);
            sleep_or_trigger!();
            continue;
        }

        let manifest = match fetch_manifest(&url, &api_key).await {
            Ok(m) => {
                // Успех — сбрасываем накопленные неудачи загрузки, чтобы порог
                // взводился только на ПОДРЯД идущих сбоях, а не на редких за сутки.
                if rec.download_fails > 0 {
                    rec.download_fails = 0;
                    save_fails(&upd_dir, &rec);
                }
                m
            }
            Err(e) => {
                warn!("Manifest fetch failed: {e}");
                rec.download_fails += 1;
                if rec.download_fails >= CB_FAIL_THRESHOLD {
                    rec.blocked_until = Some(now_secs() + CB_BLOCK_SECS);
                    warn!("Download circuit breaker tripped for {} min", CB_BLOCK_SECS / 60);
                }
                save_fails(&upd_dir, &rec);
                set_phase(&state, UpdatePhase::Error(e.to_string()));
                sleep_or_trigger!();
                continue;
            }
        };

        let remote_agent_ver = manifest.agent_version.clone();
        let remote_gui_ver = manifest.gui_version.clone();
        *state.update_latest_version.lock().unwrap() = remote_agent_ver.clone();
        *state.update_latest_gui_version.lock().unwrap() = remote_gui_ver.clone();

        let auto_update = state.cfg.globals().auto_update;
        let agent_needs_update = is_newer(&remote_agent_ver, crate::agent_version());
        let gui_needs_update = gui_update_needed(&manifest.files, &install_dir);

        // Self-heal: if installed restlex.exe is already up to date but launcher.version
        // is stale or missing (e.g. user never launched GUI after agent-side update),
        // sync it now so reports carry the correct launcher version.
        if !gui_needs_update && !remote_gui_ver.is_empty() && install_dir.join("restlex.exe").exists() {
            let lv = restlex_core::program_data().join("launcher.version");
            let cur = std::fs::read_to_string(&lv).ok().map(|s| s.trim().to_string()).unwrap_or_default();
            if cur != remote_gui_ver {
                if let Some(parent) = lv.parent() { let _ = std::fs::create_dir_all(parent); }
                if let Err(e) = std::fs::write(&lv, &remote_gui_ver) {
                    tracing::warn!("cannot self-heal launcher.version: {e}");
                } else {
                    info!("launcher.version self-healed: {} -> {}", cur, remote_gui_ver);
                }
            }
        }

        // Manual force (GUI button) bypasses auto_update flag
        let forced = state.force_apply.swap(false, std::sync::atomic::Ordering::Relaxed);

        // Known-bad guard: если предыдущий self-update именно ЭТОЙ версии не смог
        // стартовать, self_update.bat откатился на .bak и оставил маркер. Не
        // переустанавливаем заведомо битый билд каждый цикл (иначе ping-pong:
        // подмена → краш → откат → снова подмена). Снимается сменой версии в
        // манифесте или ручным «Проверить и обновить» (forced).
        let failmark = failed_version_path();
        if forced {
            // Маркер снимаем НЕ ЧАЩЕ РАЗА В CLEAR_COOLDOWN на версию. Без
            // ограничения любой цикл, дёргающий /update/apply, стирал бы его
            // каждый раз — бесконечный ping-pong «подмена → краш → откат».
            // Но и «один раз на версию» нельзя: одна неудача на нагруженном
            // сервере (не поднялся за 20 с) блокировала версию НАВСЕГДА, и
            // кнопка «Проверить и обновить» переставала работать — лечилось
            // только ручным удалением файла.
            let cleared_path = cleared_version_path();
            match read_cleared_marker(&cleared_path) {
                Some((ver, age)) if ver == remote_agent_ver && age < CLEAR_COOLDOWN => {
                    warn!(
                        "forced: маркер неудачного обновления для {} снимали {} мин назад — \
                         повторно можно через {} мин (защита от ping-pong)",
                        remote_agent_ver,
                        age.as_secs() / 60,
                        (CLEAR_COOLDOWN.saturating_sub(age)).as_secs() / 60
                    );
                }
                _ => {
                    let _ = std::fs::remove_file(&failmark);
                    let _ = std::fs::write(&cleared_path, write_cleared_marker(&remote_agent_ver));
                }
            }
        }
        let last_failed_ver = std::fs::read_to_string(&failmark)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let agent_blocked_bad = agent_needs_update
            && !forced
            && last_failed_ver.as_deref() == Some(remote_agent_ver.as_str());
        let agent_needs_update = agent_needs_update && !agent_blocked_bad;
        if agent_blocked_bad {
            warn!(
                "Agent {} ранее не стартовал после self-update (откат на .bak). \
                 Пропускаю повторную установку до смены версии или ручной проверки.",
                remote_agent_ver
            );
            let _ = restlex_core::reporter::send_update_report(
                &report_url,
                &report_api_key,
                &restlex_core::reporter::UpdateReport {
                    component: "agent".to_string(),
                    from_version: crate::agent_version().to_string(),
                    to_version: remote_agent_ver.clone(),
                    host: restlex_core::reporter::hostname(),
                    success: false,
                    error: Some(
                        restlex_core::i18n::tr(
                            en,
                            "Новый агент не стартовал — выполнен откат",
                            "New agent failed to start — rolled back",
                        )
                        .to_string(),
                    ),
                },
            )
            .await;
        }

        // If auto_update is disabled and not forced — just show available versions, don't apply
        if !auto_update && !forced {
            if agent_needs_update || gui_needs_update {
                info!("Update available but auto_update=false (agent={} gui={})", remote_agent_ver, remote_gui_ver);
                set_phase(&state, UpdatePhase::Idle);
            } else {
                set_phase(&state, UpdatePhase::UpToDate);
            }
            sleep_or_trigger!();
            continue;
        }

        if !agent_needs_update && !gui_needs_update {
            info!("Already up to date (agent={} gui={})", crate::agent_version(), remote_agent_ver);
            set_phase(&state, UpdatePhase::UpToDate);
            sleep_or_trigger!();
            continue;
        }

        if agent_needs_update {
            info!("Agent update available: {} → {}", crate::agent_version(), remote_agent_ver);
        }
        if gui_needs_update {
            info!("GUI update available (gui_version={})", remote_gui_ver);
        }

        // reset fail counter if agent version changed
        if rec.version != remote_agent_ver {
            rec = FailRecord { version: remote_agent_ver.clone(), ..Default::default() };
        }
        if rec.apply_fails >= 3 {
            warn!("Apply circuit breaker: clearing staged files for re-download");
            let _ = std::fs::remove_dir_all(&upd_dir);
            rec.apply_fails = 0;
        }
        save_fails(&upd_dir, &rec);

        set_phase(&state, UpdatePhase::Downloading);
        if let Err(e) = download_files(&manifest, &upd_dir, &url, &api_key, en).await {
            warn!("Download failed: {e}");
            rec.download_fails += 1;
            if rec.download_fails >= CB_FAIL_THRESHOLD {
                rec.blocked_until = Some(now_secs() + CB_BLOCK_SECS);
            }
            save_fails(&upd_dir, &rec);
            set_phase(&state, UpdatePhase::Error(e.to_string()));
            let _ = restlex_core::reporter::send_update_report(&report_url, &report_api_key, &restlex_core::reporter::UpdateReport {
                component: "download".to_string(),
                from_version: crate::agent_version().to_string(),
                to_version: remote_agent_ver.clone(),
                host: restlex_core::reporter::hostname(),
                success: false,
                error: Some(if en { format!("Download: {e}") } else { format!("Скачивание: {e}") }),
            }).await;
            sleep_or_trigger!();
            continue;
        }
        rec.download_fails = 0;
        save_fails(&upd_dir, &rec);

        // wait for backup to finish
        // Ждём завершения бэкапа, но НЕ бесконечно: зависший restic иначе
        // навсегда оставлял update_phase=waiting_backup и блокировал обновления.
        set_phase(&state, UpdatePhase::WaitingBackup);
        const WAIT_BACKUP_MAX_SECS: u64 = 2 * 60 * 60;
        let wait_started = tokio::time::Instant::now();
        let mut wait_timed_out = false;
        loop {
            let busy = state.running.lock().unwrap().values().any(|&v| v);
            if !busy { break; }
            if wait_started.elapsed() >= Duration::from_secs(WAIT_BACKUP_MAX_SECS) {
                wait_timed_out = true;
                break;
            }
            info!("Waiting for backup to finish before applying update...");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        if wait_timed_out {
            warn!("Бэкап не завершился за 2 ч — откладываю применение обновления до следующего цикла");
            set_phase(&state, UpdatePhase::Idle);
            sleep_or_trigger!();
            continue;
        }

        set_phase(&state, UpdatePhase::Applying);
        let mut apply_ok = true;

        if gui_needs_update {
            if let Err(e) = apply_gui_update(&upd_dir, &install_dir, &remote_gui_ver, en) {
                warn!("GUI update failed: {e}");
                apply_ok = false;
                let _ = restlex_core::reporter::send_update_report(&report_url, &report_api_key, &restlex_core::reporter::UpdateReport {
                    component: "gui".to_string(),
                    from_version: String::new(),
                    to_version: remote_gui_ver.clone(),
                    host: restlex_core::reporter::hostname(),
                    success: false,
                    error: Some(format!("Apply GUI: {e}")),
                }).await;
            }
        }
        if agent_needs_update {
            match stage_agent_self_update(&upd_dir, &remote_agent_ver, en) {
                Ok(()) => {
                    let _ = restlex_core::reporter::send_update_report(&report_url, &report_api_key, &restlex_core::reporter::UpdateReport {
                        component: "agent".to_string(),
                        from_version: crate::agent_version().to_string(),
                        to_version: remote_agent_ver.clone(),
                        host: restlex_core::reporter::hostname(),
                        success: true,
                        error: None,
                    }).await;
                    info!("Agent staged; signaling service restart");
                    set_phase(&state, UpdatePhase::Applying);
                    state.shutdown_notify.notify_one();
                    // Выходим из цикла совсем. Раньше тут был sleep(60): если
                    // рантайм не умирал, цикл продолжался и стейджил self-update
                    // повторно — второй bat и вторая задача schtasks.
                    return;
                }
                Err(e) => {
                    warn!("Agent self-update staging failed: {e}");
                    apply_ok = false;
                    let _ = restlex_core::reporter::send_update_report(&report_url, &report_api_key, &restlex_core::reporter::UpdateReport {
                        component: "agent".to_string(),
                        from_version: crate::agent_version().to_string(),
                        to_version: remote_agent_ver.clone(),
                        host: restlex_core::reporter::hostname(),
                        success: false,
                        error: Some(format!("Staging: {e}")),
                    }).await;
                }
            }
        }

        if apply_ok && !agent_needs_update {
            info!("Update to gui={remote_gui_ver} applied successfully");
            let _ = restlex_core::reporter::send_update_report(&report_url, &report_api_key, &restlex_core::reporter::UpdateReport {
                component: "gui".to_string(),
                from_version: String::new(),
                to_version: remote_gui_ver.clone(),
                host: restlex_core::reporter::hostname(),
                success: true,
                error: None,
            }).await;
            set_phase(&state, UpdatePhase::UpToDate);
        } else if !apply_ok {
            let mut rec = load_fails(&upd_dir);
            rec.apply_fails += 1;
            save_fails(&upd_dir, &rec);
            set_phase(&state, UpdatePhase::Error("apply failed".into()));
        }

        tokio::select! {
            _ = sleep_minutes(interval_m) => {}
            _ = trigger.notified() => {}
        }
    }
}

fn set_phase(state: &crate::scheduler::State, phase: UpdatePhase) {
    *state.update_phase.lock().unwrap() = phase;
}

async fn sleep_minutes(m: u64) {
    tokio::time::sleep(Duration::from_secs(m * 60)).await;
}

/// Разбудить update_loop для проверки (авто-режим: применение только если auto_update=true).
/// Это ручное действие из GUI — снимаем circuit breaker, чтобы пользователь
/// мог проверить обновления даже после серии неудач.
pub fn check_now(state: &crate::scheduler::State) {
    use std::sync::atomic::Ordering;
    state.manual_check.store(true, Ordering::Relaxed);
    state.update_trigger.notify_one();
}

/// Принудительно запустить проверку И применение независимо от auto_update (кнопка GUI).
pub fn force_apply_now(state: &crate::scheduler::State) {
    use std::sync::atomic::Ordering;
    state.force_apply.store(true, Ordering::Relaxed);
    state.manual_check.store(true, Ordering::Relaxed);
    state.update_trigger.notify_one();
}

#[cfg(test)]
mod tests {
    use super::{signature_allows_update, valid_file_name, valid_version, SigCheck};

    #[test]
    fn signature_policy_fails_closed() {
        assert!(signature_allows_update(&SigCheck::Ok));
        for check in [
            SigCheck::Bad("NotSigned".into()),
            SigCheck::Bad("чужой подписант".into()),
            SigCheck::Unknown("powershell не запустился".into()),
            SigCheck::Unknown("таймаут".into()),
            SigCheck::Unknown("пустой или повреждённый ответ".into()),
        ] {
            assert!(!signature_allows_update(&check), "{check:?}");
        }
    }

    #[test]
    fn file_name_rejects_traversal() {
        assert!(valid_file_name("restlex-agent.exe"));
        assert!(valid_file_name("restlex.exe"));
        // Path traversal — запись произвольного файла под LocalSystem.
        assert!(!valid_file_name(r"..\..\..\Windows\System32\evil.dll"));
        assert!(!valid_file_name("../evil.dll"));
        assert!(!valid_file_name(".."));
        assert!(!valid_file_name("."));
        assert!(!valid_file_name(r"C:\Windows\evil.dll"));
        assert!(!valid_file_name("sub/dir.exe"));
        assert!(!valid_file_name(r"sub\dir.exe"));
        assert!(!valid_file_name(""));
        assert!(!valid_file_name("a\0b"));
        assert!(!valid_file_name(&"a".repeat(129)));
    }

    #[test]
    fn version_rejects_command_injection() {
        assert!(valid_version("1.8.7"));
        assert!(valid_version("9.9"));
        assert!(valid_version("10"));
        // Версия подставляется в .bat, исполняемый под SYSTEM.
        assert!(!valid_version("9.9 & calc"));
        assert!(!valid_version("9.9 & powershell -enc AAAA"));
        assert!(!valid_version("9.9\"&calc&\""));
        assert!(!valid_version("9.9\r\ncalc"));
        assert!(!valid_version(""));
        assert!(!valid_version("1.2.3.4.5.6.7.8.9.0"));
    }
}
