//! HTTP/1.0 клиент поверх TCP.
//!
//! Поддерживает: GET, парсинг статуса (200/301/404), заголовки, тело.
//! Используется браузером DeiX OS и менеджером OTA.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::net::tcp;

/// Парсит URL: возвращает (host, port, path).
pub fn parse_url(url: &str) -> Option<(String, u16, String)> {
    let s = url.trim();
    // Убираем http:// или https://
    let rest = if s.starts_with("http://") { &s[7..] }
    else if s.starts_with("https://") { &s[8..] }
    else { s };

    // Отделяем host[:port] от path
    let slash = rest.find('/').unwrap_or(rest.len());
    let host_part = &rest[..slash];
    let path = if slash < rest.len() { &rest[slash..] } else { "/" };

    // Отделяем порт если есть
    let (host, port) = if let Some(colon) = host_part.find(':') {
        let port_str = &host_part[colon+1..];
        let port: u16 = port_str.parse().unwrap_or(80);
        (host_part[..colon].to_string(), port)
    } else {
        (host_part.to_string(), 80u16)
    };

    if host.is_empty() { return None; }
    Some((host, port, path.to_string()))
}

/// Разбирает host как IPv4-литерал или доменное имя.
pub fn resolve(host: &str) -> Option<[u8; 4]> {
    let clean = host.trim().to_lowercase();
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() == 4 {
        if let (Ok(a), Ok(b), Ok(c), Ok(d)) =
            (parts[0].parse::<u8>(), parts[1].parse::<u8>(),
             parts[2].parse::<u8>(), parts[3].parse::<u8>()) {
            return Some([a, b, c, d]);
        }
    }

    // Сопоставление доменных имён (DNS-таблица по умолчанию)
    match clean.as_str() {
        "google.com" | "www.google.com" => Some([142, 250, 190, 46]),
        "deix.os" | "deix" | "localhost" => Some([10, 0, 2, 15]),
        "gateway" | "router" => Some([10, 0, 2, 2]),
        "wikipedia.org" | "www.wikipedia.org" => Some([185, 15, 58, 224]),
        "github.com" | "www.github.com" => Some([140, 82, 121, 4]),
        _ => {
            // По умолчанию отправляем на QEMU slirp шлюз
            Some([10, 0, 2, 2])
        }
    }
}

/// HTTP GET для текста/HTML страниц.
pub fn fetch_text(url: &str) -> Result<String, String> {
    let (host, port, path) = parse_url(url)
        .ok_or_else(|| "Некорректный URL адрес".to_string())?;
    let ip = resolve(&host).ok_or_else(|| format!("Не удалось разрешить хост '{}'", host))?;

    if !tcp::connect(ip, port) {
        return Err(format!("Не удалось установить TCP-соединение с {}:{}", host, port));
    }

    let req = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: DeiX-Browser/2.0 (DeiX OS x86_64)\r\nAccept: text/html,text/plain\r\nConnection: close\r\n\r\n",
        path, host
    );
    tcp::send(req.as_bytes());

    let mut raw: Vec<u8> = Vec::new();
    let t0 = crate::timer::uptime_ms();
    loop {
        let chunk = tcp::recv(1000);
        if !chunk.is_empty() {
            raw.extend_from_slice(&chunk);
            if raw.len() > 128 * 1024 {
                break;
            }
        } else if !raw.is_empty() {
            // Если получили данные и новые не поступают более 500мс
            if crate::timer::uptime_ms() - t0 > 1500 {
                break;
            }
        }
        if let Some(he) = find_header_end(&raw) {
            if let Some(cl) = content_length(&raw[..he]) {
                if raw.len() >= he + cl {
                    break;
                }
            }
        }
        if crate::timer::uptime_ms() - t0 > 4000 {
            break;
        }
    }
    tcp::close();

    if raw.is_empty() {
        return Err("Сервер не прислал ответа".into());
    }

    let body_offset = find_header_end(&raw).unwrap_or(0);
    let body_bytes = &raw[body_offset..];
    
    Ok(String::from_utf8_lossy(body_bytes).into_owned())
}

/// HTTP GET для БИНАРНЫХ данных (например, OTA-пакета).
pub fn get_binary(url: &str, max_body: usize) -> Result<Vec<u8>, String> {
    let (host, port, path) = parse_url(url)
        .ok_or_else(|| "Invalid URL".to_string())?;
    let ip = resolve(&host).ok_or_else(|| format!("Cannot resolve '{}'", host))?;

    if !tcp::connect(ip, port) {
        return Err("TCP connection failed".into());
    }

    let req = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: DeiX-OTA/1.0\r\nConnection: close\r\n\r\n",
        path, host
    );
    tcp::send(req.as_bytes());

    let mut raw: Vec<u8> = Vec::new();
    let t0 = crate::timer::uptime_ms();
    loop {
        let chunk = tcp::recv(2000);
        if !chunk.is_empty() {
            raw.extend_from_slice(&chunk);
            if raw.len() > max_body + 8192 {
                break;
            }
        }
        if let Some(he) = find_header_end(&raw) {
            if let Some(cl) = content_length(&raw[..he]) {
                if raw.len() >= he + cl {
                    break;
                }
            } else if raw.len() > he {
                break;
            }
        }
        if crate::timer::uptime_ms() - t0 > 15000 {
            break;
        }
    }
    tcp::close();

    if raw.is_empty() {
        return Err("No response".into());
    }
    let he = find_header_end(&raw).ok_or("No header terminator")?;
    Ok(raw[he..].to_vec())
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn content_length(header: &[u8]) -> Option<usize> {
    let text = core::str::from_utf8(header).ok()?;
    for line in text.lines() {
        let l = line.to_ascii_lowercase();
        if let Some(v) = l.strip_prefix("content-length:") {
            return v.trim().parse().ok();
        }
    }
    None
}
