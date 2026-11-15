use crate::error::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub profile: String,
    pub pid: String,
    pub status: String,
    pub snapshot_id: Option<String>,
    pub started_at: DateTime<Local>,
    pub duration_s: u64,
    pub files_new: u64,
    #[serde(default)]
    pub files_changed: u64,
    pub bytes_added: u64,
    #[serde(default)]
    pub source_paths: Vec<String>,
    #[serde(default)]
    pub dest_path: String,
    pub errors: Vec<String>,
    pub host: String,
    pub agent_version: String,
    #[serde(default)]
    pub launcher_version: String,
}

/// Проверка HTTP-ответа сервера отчётов. Раньше `let _ = req.send()` считал 401/500
/// успехом — после ротации report_api_key отчёты молча переставали доходить.
/// Тело в ошибку кладём максимум 200 символов (может содержать эхо секретов).
async fn check_ok(resp: reqwest::Response) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    let snippet: String = body.chars().take(200).collect();
    Err(crate::Error::Other(format!(
        "report HTTP {}: {}",
        status.as_u16(),
        snippet.trim()
    )))
}

/// API-ключ, оставшийся в виде `enc:...` (DPAPI не смог расшифровать), нельзя
/// слать в заголовке — это шифроблоб.
fn checked_key(api_key: &str) -> Result<&str> {
    if crate::crypto::is_undecrypted(api_key) {
        return Err(crate::Error::Other(
            "report_api_key не расшифрован (DPAPI): отправка отменена".into(),
        ));
    }
    Ok(api_key)
}

pub async fn send(url: &str, api_key: &str, report: &Report) -> Result<()> {
    if url.is_empty() {
        return Ok(());
    }
    // Ensure URL ends with /report
    let url = if url.trim_end_matches('/').ends_with("/report") {
        url.to_string()
    } else {
        format!("{}/report", url.trim_end_matches('/'))
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| crate::Error::Other(e.to_string()))?;
    let mut req = client.post(&url).json(report);
    if !api_key.is_empty() {
        req = req.header("X-Api-Key", checked_key(api_key)?);
    }
    let resp = req.send().await.map_err(|e| crate::Error::Other(e.to_string()))?;
    check_ok(resp).await
}

/// Send a report with retries (for background use after a backup). Returns Ok on
/// the first success; otherwise the last error after `attempts` tries spaced
/// `interval_s` apart. A non-positive `attempts` is treated as 1.
pub async fn send_with_retry(
    url: &str,
    api_key: &str,
    report: &Report,
    attempts: u32,
    interval_s: u64,
) -> Result<()> {
    if url.is_empty() {
        return Ok(());
    }
    let attempts = attempts.max(1);
    let mut last = Ok(());
    for i in 0..attempts {
        match send(url, api_key, report).await {
            Ok(()) => return Ok(()),
            Err(e) => last = Err(e),
        }
        if i + 1 < attempts {
            tokio::time::sleep(std::time::Duration::from_secs(interval_s.max(1))).await;
        }
    }
    last
}

pub fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into())
}

/// Announce this agent (host + current version) to the report server without
/// waiting for the next scheduled backup. After an in-place upgrade the web UI
/// otherwise keeps showing the previously reported version until a backup runs.
/// Sent with status `"heartbeat"` so the server can distinguish it from a real
/// backup result.
pub async fn send_heartbeat(url: &str, api_key: &str, agent_version: &str) -> Result<()> {
    if url.is_empty() {
        return Ok(());
    }
    let report = Report {
        profile: "—".into(),
        pid: "__heartbeat__".into(),
        status: "heartbeat".into(),
        snapshot_id: None,
        started_at: Local::now(),
        duration_s: 0,
        files_new: 0,
        files_changed: 0,
        bytes_added: 0,
        source_paths: Vec::new(),
        dest_path: String::new(),
        errors: Vec::new(),
        host: hostname(),
        agent_version: agent_version.into(),
        launcher_version: crate::read_launcher_version(),
    };
    send(url, api_key, &report).await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateReport {
    pub component: String,    // "agent" or "gui"
    pub from_version: String,
    pub to_version: String,
    pub host: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Текущее состояние circuit breaker обновлений на хосте — для вывода в админке
/// сервера отчётов (отдельный канал, чтобы не засорять историю update_reports).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CbStatus {
    pub host: String,
    /// unix-секунды, до которых обновления заблокированы; 0 = не заблокирован.
    pub blocked_until: i64,
    pub fail_count: u32,
}

/// Команды серверу→агенту (агент тянет их сам, т.к. за NAT недостижим).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentCommands {
    #[serde(default)]
    pub reset_circuit_breaker: bool,
}

/// Сообщить серверу отчётов текущее состояние circuit breaker (POST /cb-status).
pub async fn send_cb_status(url: &str, api_key: &str, status: &CbStatus) -> Result<()> {
    if url.is_empty() {
        return Ok(());
    }
    let url = format!("{}/cb-status", url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| crate::Error::Other(e.to_string()))?;
    let mut req = client.post(&url).json(status);
    if !api_key.is_empty() {
        req = req.header("X-Api-Key", checked_key(api_key)?);
    }
    let resp = req.send().await.map_err(|e| crate::Error::Other(e.to_string()))?;
    check_ok(resp).await
}

/// Забрать (и тем самым потребить) ожидающие команды для данного хоста с сервера
/// отчётов (GET /commands/{host}). Сервер сбрасывает флаг при выдаче.
pub async fn fetch_commands(url: &str, api_key: &str, host: &str) -> Result<AgentCommands> {
    if url.is_empty() {
        return Ok(AgentCommands::default());
    }
    let url = format!("{}/commands/{}", url.trim_end_matches('/'), urlencoding_host(host));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| crate::Error::Other(e.to_string()))?;
    let mut req = client.get(&url);
    if !api_key.is_empty() {
        req = req.header("X-Api-Key", checked_key(api_key)?);
    }
    let resp = req.send().await.map_err(|e| crate::Error::Other(e.to_string()))?;
    if !resp.status().is_success() {
        return Ok(AgentCommands::default());
    }
    resp.json::<AgentCommands>().await.map_err(|e| crate::Error::Other(e.to_string()))
}

/// Минимальное percent-encoding для имени хоста в пути (хосты обычно ASCII, но
/// подстрахуемся от пробелов/слэшей).
fn urlencoding_host(host: &str) -> String {
    host.chars().map(|c| match c {
        'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' => c.to_string(),
        _ => format!("%{:02X}", c as u32 & 0xFF),
    }).collect()
}

pub async fn send_update_report(url: &str, api_key: &str, report: &UpdateReport) -> Result<()> {
    if url.is_empty() {
        return Ok(());
    }
    let url = format!("{}/update-report", url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| crate::Error::Other(e.to_string()))?;
    let mut req = client.post(&url).json(report);
    if !api_key.is_empty() {
        req = req.header("X-Api-Key", checked_key(api_key)?);
    }
    let resp = req.send().await.map_err(|e| crate::Error::Other(e.to_string()))?;
    check_ok(resp).await
}
