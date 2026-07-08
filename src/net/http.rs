//! HTTP/1.0 клиент поверх TCP.
//!
//! Поддерживает: GET, парсинг статуса (200/301/404), заголовки, тело.
//! Этого достаточно для простого браузера.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::net::tcp;

pub struct HttpResponse {
    pub status: u16,           // 200, 301, 404...
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

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

/// DNS-запрос (заглушка — всегда возвращает 10.0.2.2 для QEMU slirp).
/// В реальности нужно: отправить DNS-запрос → распарсить ответ.
pub fn resolve(host: &str) -> Option<[u8; 4]> {
    // Если host уже IP-адрес — парсим.
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() == 4 {
        if let (Ok(a),Ok(b),Ok(c),Ok(d)) =
            (parts[0].parse::<u8>(), parts[1].parse::<u8>(),
             parts[2].parse::<u8>(), parts[3].parse::<u8>()) {
            return Some([a,b,c,d]);
        }
    }

    // Заглушка DNS: резолвим несколько известных сайтов.
    match host {
        "google.com" | "www.google.com" => Some([142, 250, 80, 4]),
        "example.com" => Some([93, 184, 215, 14]),
        "info.cern.ch" => Some([188, 184, 9, 234]),
        _ => {
            // Пытаемся сделать реальный DNS через шлюз QEMU (10.0.2.3).
            // Но для этого нужен UDP, которого нет. Возвращаем заглушку.
            crate::println!("  [dns] No resolver for '{}' — try IP directly.", host);
            None
        }
    }
}

/// Выполняет HTTP GET и возвращает ответ.
pub fn get(url: &str) -> Result<HttpResponse, String> {
    let (host, port, path) = parse_url(url)
        .ok_or_else(|| "Invalid URL".to_string())?;

    let ip = resolve(&host)
        .ok_or_else(|| format!("Cannot resolve '{}'", host))?;

    crate::print!("  [http] Connecting to {}:{}... ", host, port);

    if !tcp::connect(ip, port) {
        return Err("TCP connection failed".into());
    }
    crate::println!("connected.");

    // Формируем HTTP-запрос.
    let req = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: DeiX/0.2\r\nAccept: text/html\r\nConnection: close\r\n\r\n",
        path, host
    );
    tcp::send(req.as_bytes());

    // Получаем ответ (до 5 секунд).
    let mut raw = Vec::new();
    let t0 = crate::timer::uptime_ms();
    loop {
        let chunk = tcp::recv(1000);
        if chunk.is_empty() { break; }
        raw.extend_from_slice(&chunk);
        if crate::timer::uptime_ms() - t0 > 5000 { break; }
    }
    tcp::close();

    if raw.is_empty() {
        return Err("No response".into());
    }

    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let text = core::str::from_utf8(raw).map_err(|_| "Invalid UTF-8".to_string())?;
    let mut lines = text.lines();

    // Статусная строка: "HTTP/1.0 200 OK"
    let status_line = lines.next().ok_or("Empty response")?;
    let parts: Vec<&str> = status_line.split_whitespace().collect();
    if parts.len() < 2 { return Err("Bad status line".into()); }
    let status: u16 = parts[1].parse().map_err(|_| "Bad status code")?;

    // Заголовки
    let mut headers = Vec::new();
    for line in &mut lines {
        let line = line.trim();
        if line.is_empty() { break; }
        if let Some(colon) = line.find(':') {
            let key = line[..colon].trim().to_string();
            let val = line[colon+1..].trim().to_string();
            headers.push((key, val));
        }
    }

    // Всё остальное — тело.
    let header_end = text.find("\r\n\r\n").unwrap_or(0) + 4;
    let body = if header_end < raw.len() {
        Vec::from(&raw[header_end..])
    } else {
        Vec::new()
    };

    Ok(HttpResponse { status, headers, body })
}
