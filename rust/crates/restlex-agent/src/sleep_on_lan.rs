//! Sleep-on-LAN: выключение этого ПК по сетевому пакету, без учёток.
//! Пакет — обычный magic packet (6×FF + 16×MAC), но MAC записан задом наперёд
//! (формат совместим с утилитой SleepOnLan). Сетевая карта сама ПК не выключит —
//! пакет ловит агент и делает `shutdown /s`.
//! Включается глобальной настройкой `sleep_on_lan_listen` (по умолчанию выкл):
//! любой в локалке может отправить такой пакет — это осознанный выбор админа.

use std::sync::Arc;
use std::time::Duration;

use crate::scheduler;

pub const SOL_PORT: u16 = 9;
/// Таймер перед выключением: пользователь за тем ПК успеет увидеть предупреждение.
/// ponytail: константа; вынести в настройку, если попросят.
const SOL_SHUTDOWN_DELAY_S: u32 = 30;
const FW_RULE: &str = "RestLex Sleep-on-LAN";

fn enabled(state: &scheduler::State) -> bool {
    state.cfg.globals().extra.get("sleep_on_lan_listen")
        .and_then(|v| v.as_bool()).unwrap_or(false)
}

/// MAC из magic packet (6×FF + 16 одинаковых 6-байтовых блоков), иначе None.
fn parse_magic(b: &[u8]) -> Option<[u8; 6]> {
    // Ищем 6×FF в любом месте: некоторые отправители добавляют заголовок.
    let start = b.windows(6).position(|w| w == [0xFF; 6])? + 6;
    let body = b.get(start..start + 96)?;
    let mac: [u8; 6] = body[..6].try_into().ok()?;
    body.chunks(6).all(|c| c == mac).then_some(mac)
}

/// MAC всех сетевых адаптеров этого ПК (через `getmac`).
async fn local_macs() -> Vec<[u8; 6]> {
    let Ok(out) = tokio::process::Command::new("getmac").args(["/fo", "csv", "/nh"])
        .creation_flags(0x08000000).output().await else { return vec![] };
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| {
        let hex: String = l.split(',').next()?.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        if hex.len() != 12 { return None; }
        let mut m = [0u8; 6];
        for (i, b) in m.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(m)
    }).collect()
}

async fn firewall_allow() {
    let _ = tokio::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "delete", "rule", &format!("name={FW_RULE}")])
        .creation_flags(0x08000000).output().await;
    let _ = tokio::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "add", "rule", &format!("name={FW_RULE}"),
               "dir=in", "action=allow", "protocol=UDP", &format!("localport={SOL_PORT}")])
        .creation_flags(0x08000000).output().await;
}

pub async fn listen_loop(state: Arc<scheduler::State>) {
    let mut sock: Option<tokio::net::UdpSocket> = None;
    let mut buf = [0u8; 1500];
    let mut macs: Vec<[u8; 6]> = vec![];
    let mut macs_at: Option<std::time::Instant> = None;
    loop {
        if !enabled(&state) {
            if sock.take().is_some() {
                tracing::info!("sleep-on-lan: выключено, UDP {SOL_PORT} закрыт");
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
            continue;
        }
        if sock.is_none() {
            match tokio::net::UdpSocket::bind(("0.0.0.0", SOL_PORT)).await {
                Ok(s) => {
                    firewall_allow().await;
                    tracing::info!("sleep-on-lan: слушаю UDP {SOL_PORT}");
                    sock = Some(s);
                }
                Err(e) => {
                    tracing::warn!("sleep-on-lan: UDP {SOL_PORT} занят ({e}) — повтор через минуту");
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                }
            }
        }
        let Some(s) = sock.as_ref() else { continue };
        // таймаут — чтобы периодически перечитывать настройку
        let Ok(Ok((n, from))) = tokio::time::timeout(Duration::from_secs(30), s.recv_from(&mut buf)).await
            else { continue };
        let Some(mut mac) = parse_magic(&buf[..n]) else { continue };
        mac.reverse();
        // getmac — не на каждый пакет: чужие WOL в сети тоже приходят сюда
        if macs_at.is_none_or(|t| t.elapsed() > Duration::from_secs(300)) {
            macs = local_macs().await;
            macs_at = Some(std::time::Instant::now());
        }
        if !macs.contains(&mac) {
            continue; // обычный WOL или пакет для другого ПК
        }
        tracing::warn!("sleep-on-lan: пакет выключения от {from} — shutdown через {SOL_SHUTDOWN_DELAY_S}с");
        // Текст /c видит пользователь выключаемого ПК.
        let comment = if state.cfg.globals().en() {
            format!("RestLex: network shutdown (Sleep-on-LAN) from {}", from.ip())
        } else {
            format!("RestLex: выключение по сети (Sleep-on-LAN) от {}", from.ip())
        };
        let result = tokio::process::Command::new("shutdown")
            .args(["/s", "/f", "/t", &SOL_SHUTDOWN_DELAY_S.to_string(), "/c", &comment])
            .creation_flags(0x08000000).output().await;
        match result {
            Ok(out) if out.status.success() => tracing::info!(
                "sleep-on-lan: shutdown запланирован, источник {from}, код 0"
            ),
            Ok(out) => tracing::error!(
                "sleep-on-lan: shutdown НЕ запланирован, источник {from}, код {:?}",
                out.status.code()
            ),
            Err(err) => tracing::error!(
                "sleep-on-lan: ошибка запуска shutdown, источник {from}: {err}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_magic_and_rejects_junk() {
        let mac = [0x74, 0x56, 0x3C, 0x93, 0x7F, 0xE8];
        let mut p = vec![0xFF; 6];
        for _ in 0..16 { p.extend_from_slice(&mac); }
        assert_eq!(parse_magic(&p), Some(mac));
        let mut hdr = vec![1, 2, 3];
        hdr.extend_from_slice(&p);
        assert_eq!(parse_magic(&hdr), Some(mac));
        p[50] ^= 1;
        assert_eq!(parse_magic(&p), None);
        assert_eq!(parse_magic(&[0xFF; 20]), None);
    }
}
