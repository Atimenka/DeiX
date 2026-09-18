//! HTTP/1.0 клиент поверх TCP.
//!
//! Поддерживает: GET, парсинг статуса (200/301/404), заголовки, тело.
//! Этого достаточно для простого браузера.

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

/// Разбирает host как IPv4-литерал (`93.184.215.14`).
///
/// Резолвинга имён нет: DNS работает поверх UDP, а в стеке реализован
/// только TCP (см. net/tcp.rs). Пока UDP не появится, адрес нужно
/// указывать напрямую — молча подставлять захардкоженные IP было бы
/// враньём о возможностях системы.
pub fn resolve(host: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() == 4 {
        if let (Ok(a),Ok(b),Ok(c),Ok(d)) =
            (parts[0].parse::<u8>(), parts[1].parse::<u8>(),
             parts[2].parse::<u8>(), parts[3].parse::<u8>()) {
            return Some([a,b,c,d]);
        }
    }

    crate::println!("  [dns] Разрешение имён не поддерживается (нужен UDP); укажите IP-адрес.");
    None
}

/// HTTP GET для БИНАРНЫХ данных (например, OTA-пакета): накапливает ВСЕ
/// чанки TCP, находит границу заголовков "\r\n\r\n" и возвращает сырое тело.
pub fn get_binary(url: &str, max_body: usize) -> Result<Vec<u8>, String> {
    let (host, port, path) = parse_url(url)
        .ok_or_else(|| "Invalid URL".to_string())?;
    let ip = resolve(&host).ok_or_else(|| format!("Cannot resolve '{}'", host))?;

    crate::print!("  [http] GET {}... ", url);
    if !tcp::connect(ip, port) {
        return Err("TCP connection failed".into());
    }
    crate::println!("connected.");

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
        // Завершаем, если накоплен полный ответ (заголовки + тело по Content-Length)
        if let Some(he) = find_header_end(&raw) {
            if let Some(cl) = content_length(&raw[..he]) {
                if raw.len() >= he + cl {
                    break;
                }
            } else if raw.len() > he {
                break; // нет Content-Length — берём всё
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


