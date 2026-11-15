use crate::error::Result;
use serde_json::{json, Value};

fn build_payload(chat: &str, text: &str, thread_id: Option<&str>) -> Value {
    let mut m = json!({ "chat_id": chat, "text": text, "parse_mode": "HTML" });
    if let Some(tid) = thread_id {
        // Telegram ждёт message_thread_id ЧИСЛОМ. Строка "793" молча игнорируется —
        // сообщение уходит в General вместо темы. Парсим в i64.
        if let Ok(n) = tid.trim().parse::<i64>() {
            m["message_thread_id"] = json!(n);
        }
    }
    m
}

/// reqwest::Error печатает URL в Display, а URL содержит bot-токен. Любая
/// сетевая ошибка иначе утекает токеном в agent.log плейнтекстом.
fn scrub(e: reqwest::Error) -> crate::Error {
    crate::Error::Other(e.without_url().to_string())
}

/// Проверка ответа Telegram: HTTP-статус + поле `ok` в теле. Раньше ЛЮБОЙ ответ
/// (в т.ч. 404 «Unauthorized» на битый токен) считался успехом.
fn check_response(status: reqwest::StatusCode, body: &str) -> Result<()> {
    let desc = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        if v.get("ok").and_then(|o| o.as_bool()) == Some(true) {
            None
        } else {
            Some(
                v.get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("ok=false")
                    .to_string(),
            )
        }
    });
    match desc {
        None if status.is_success() => Ok(()),
        None => Err(crate::Error::Other(format!("telegram HTTP {}", status.as_u16()))),
        Some(d) => Err(crate::Error::Other(format!(
            "telegram HTTP {}: {}",
            status.as_u16(),
            d
        ))),
    }
}

/// Токен, оставшийся в виде `enc:...` (DPAPI не смог расшифровать), нельзя
/// отправлять в сеть — это шифроблоб, а не токен.
fn reject_undecrypted(token: &str) -> Result<()> {
    if crate::crypto::is_undecrypted(token) {
        return Err(crate::Error::Other(
            "tg_token не расшифрован (DPAPI): отправка отменена".into(),
        ));
    }
    Ok(())
}

pub async fn send(token: &str, chat: &str, text: &str) -> Result<()> {
    send_threaded(token, chat, text, None).await
}

pub async fn send_threaded(token: &str, chat: &str, text: &str, thread_id: Option<&str>) -> Result<()> {
    if token.is_empty() || chat.is_empty() {
        return Ok(());
    }
    reject_undecrypted(token)?;
    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(scrub)?;
    let resp = client
        .post(url)
        .json(&build_payload(chat, text, thread_id))
        .send()
        .await
        .map_err(scrub)?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    check_response(status, &body)
}

/// Send a Telegram message with retries (background notifications after a
/// backup). `attempts` < 1 is treated as 1.
pub async fn send_with_retry(
    token: &str,
    chat: &str,
    text: &str,
    thread_id: Option<&str>,
    attempts: u32,
    interval_s: u64,
) -> Result<()> {
    if token.is_empty() || chat.is_empty() {
        return Ok(());
    }
    let attempts = attempts.max(1);
    let mut last = Ok(());
    for i in 0..attempts {
        match send_threaded(token, chat, text, thread_id).await {
            Ok(()) => return Ok(()),
            Err(e) => last = Err(e),
        }
        if i + 1 < attempts {
            tokio::time::sleep(std::time::Duration::from_secs(interval_s.max(1))).await;
        }
    }
    last
}

/// Synchronous variant (used from GUI thread for test button).
pub fn send_blocking(token: &str, chat: &str, text: &str) -> Result<()> {
    send_blocking_threaded(token, chat, text, None)
}

pub fn send_blocking_threaded(token: &str, chat: &str, text: &str, thread_id: Option<&str>) -> Result<()> {
    if token.is_empty() || chat.is_empty() {
        return Ok(());
    }
    reject_undecrypted(token)?;
    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(scrub)?;
    let resp = client
        .post(url)
        .json(&build_payload(chat, text, thread_id))
        .send()
        .map_err(scrub)?;
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    check_response(status, &body)
}

pub fn test_connection(token: &str, chat: &str) -> bool {
    send_blocking(token, chat, "[Restlex] Тестовое сообщение").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thread_id_is_number_not_string() {
        let p = build_payload("-100123", "hi", Some("793"));
        assert_eq!(p["message_thread_id"], json!(793)); // число, не "793"
        assert!(build_payload("-100123", "hi", Some("")).get("message_thread_id").is_none());
        assert!(build_payload("-100123", "hi", None).get("message_thread_id").is_none());
    }

    #[test]
    fn bad_token_response_is_error() {
        let s404 = reqwest::StatusCode::NOT_FOUND;
        let e = check_response(s404, r#"{"ok":false,"error_code":404,"description":"Not Found"}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("Not Found"), "описание Telegram должно попасть в ошибку: {e}");
        assert!(check_response(reqwest::StatusCode::OK, r#"{"ok":true,"result":{}}"#).is_ok());
        // Пустое/нечитаемое тело с плохим статусом — тоже ошибка.
        assert!(check_response(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "").is_err());
    }

    #[test]
    fn undecrypted_token_not_sent() {
        assert!(send_blocking("enc:AQAAA-blob", "-100123", "hi").is_err());
    }
}
