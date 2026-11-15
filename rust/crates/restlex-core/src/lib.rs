pub mod config;
pub mod crypto;
pub mod error;
pub mod i18n;
pub mod reporter;
pub mod restic;
pub mod schedule;
pub mod telegram;

pub use config::{Config, Globals, Profile, RepoType, Retention};
pub use error::Error;
pub use schedule::{Frequency, Schedule};

use std::path::PathBuf;

pub fn program_data() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("restlex")
}

pub fn logs_dir() -> PathBuf {
    program_data().join("logs")
}
pub fn profiles_path() -> PathBuf {
    program_data().join("profiles.json")
}
pub fn config_path() -> PathBuf {
    program_data().join("config.json")
}

/// Read launcher.version (GUI version installed on this host).
/// Returns empty string if file is missing/unreadable. Trim whitespace.
pub fn read_launcher_version() -> String {
    std::fs::read_to_string(program_data().join("launcher.version"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Maximum size of a single log file before it is trimmed to its tail.
pub const MAX_LOG_BYTES: u64 = 50 * 1024 * 1024;
/// Delete log files not modified within this many days.
pub const LOG_RETENTION_DAYS: u64 = 30;

/// Serializes log writes (and size-trimming) across threads so a concurrent
/// trim can't race an append on the same file.
static LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Append a timestamped line to a per-profile log at ProgramData\restlex\logs\{pid}.log
pub fn log_profile(pid: &str, level: &str, msg: &str) {
    use std::io::Write;
    if pid.is_empty() {
        return;
    }
    let dir = logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{}.log", pid));
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let _guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    trim_log_if_oversized(&path);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[{}] [{}] {}", ts, level, msg);
    }
}

/// If a log file exceeds [`MAX_LOG_BYTES`], rewrite it keeping only the most
/// recent half (aligned to a line boundary). Best-effort; must be called while
/// holding `LOG_LOCK`.
fn trim_log_if_oversized(path: &PathBuf) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= MAX_LOG_BYTES {
        return;
    }
    let keep = (MAX_LOG_BYTES / 2) as usize;
    if let Ok(data) = std::fs::read(path) {
        if data.len() <= keep {
            return;
        }
        let start = data.len() - keep;
        // Advance to the start of the next full line so we don't keep a fragment.
        let nl = data[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| start + i + 1)
            .unwrap_or(start);
        let _ = std::fs::write(path, &data[nl..]);
    }
}

/// Delete log files (per-profile `*.log` and rolled `agent.log*`) not modified
/// within [`LOG_RETENTION_DAYS`]. Best-effort.
pub fn cleanup_old_logs() {
    let dir = logs_dir();
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(LOG_RETENTION_DAYS * 86_400));
    let Some(cutoff) = cutoff else {
        return;
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_log = name.ends_with(".log") || name.contains("agent.log");
        if !is_log {
            continue;
        }
        if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
            if modified < cutoff {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

pub fn profile_log_path(pid: &str) -> PathBuf {
    logs_dir().join(format!("{}.log", pid))
}

pub fn read_profile_log(pid: &str, tail_lines: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let path = profile_log_path(pid);
    if tail_lines == 0 {
        return std::fs::read_to_string(&path).unwrap_or_default();
    }
    // Читаем только хвост файла (лог может быть до MAX_LOG_BYTES = 50 МБ).
    let Ok(mut f) = std::fs::File::open(&path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    // ~1 КБ на строку с запасом, но не больше 8 МБ.
    let want = (tail_lines as u64).saturating_mul(1024).min(8 * 1024 * 1024);
    let from = len.saturating_sub(want);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let content = String::from_utf8_lossy(&buf).into_owned();
    let lines: Vec<&str> = content.lines().collect();
    // Первая строка может быть обрезана посередине — отбрасываем её.
    let lines = if from > 0 && lines.len() > tail_lines { &lines[1..] } else { &lines[..] };
    let start = lines.len().saturating_sub(tail_lines);
    lines[start..].join("\n")
}

pub fn clear_profile_log(pid: &str) {
    let _ = std::fs::remove_file(profile_log_path(pid));
}

/// Enable Win32 long path support (> 260 chars) via registry.
/// No reboot required on Windows Server 2016 / Windows 10 1607+.
/// Skips silently if already enabled or if not running on Windows.
#[cfg(windows)]
pub fn enable_long_paths() {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let path = r"SYSTEM\CurrentControlSet\Control\FileSystem";
    let Ok(key) = hklm.open_subkey_with_flags(path, KEY_READ | KEY_WRITE) else {
        return;
    };
    let current: u32 = key.get_value("LongPathsEnabled").unwrap_or(0);
    if current == 1 {
        return;
    }
    match key.set_value("LongPathsEnabled", &1u32) {
        Ok(_) => tracing::info!("LongPathsEnabled установлен в реестре"),
        Err(e) => tracing::warn!("Не удалось включить LongPathsEnabled: {e}"),
    }
}

#[cfg(not(windows))]
pub fn enable_long_paths() {}
