//! HTTP/1.0 клиент поверх TCP.
//!
//! Поддерживает: GET, разбор строки статуса и заголовков, Content-Length,
//! переход по redirect (301/302/303/307/308, до 3 переходов). Имена хостов
//! резолвятся настоящим DNS-клиентом (net::dns) через UDP/53.
//! Честные ограничения: нет TLS — https:// URL не загружаются, редирект
//! на https:// завершается понятной ошибкой.

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use crate::net::{dns, tcp};

const MAX_REDIRECTS: u32 = 3;
const MAX_BODY: usize = 128 * 1024;

/// Ответ HTTP-запроса после всех redirect.
pub struct HttpResponse {
    pub status: u16,
    pub reason: String,
    pub body: String,
    /// URL, с которого фактически пришёл ответ (после redirect).
    pub final_url: String,
}

/// Парсит URL: возвращает (host, port, path).
pub fn parse_url(url: &str) -> Option<(String, u16, String)> {
    let s = url.trim();
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

/// Разбирает host: IPv4-литерал, локальные алиасы или настоящий DNS-запрос.
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

    // Локальные алиасы — реальные адреса этой машины и шлюза.
    match clean.as_str() {
        "deix.os" | "deix" | "localhost" => Some(crate::net::my_ip()),
        "gateway" | "router" => Some(crate::net::gateway_ip()),
        // Всё остальное — настоящий DNS-запрос (A-запись по UDP/53).
        _ => dns::resolve_a(&clean),
    }
}

/// Один HTTP GET без redirect: (status, reason, location, body).
fn fetch_once(url: &str) -> Result<(u16, String, Option<String>, String), String> {
    let (host, port, path) = parse_url(url)
        .ok_or_else(|| "Некорректный URL адрес".to_string())?;
    let ip = resolve(&host)
        .ok_or_else(|| format!("DNS: не удалось разрешить хост '{}'", host))?;

    if !tcp::connect(ip, port) {
        return Err(format!(
            "Не удалось установить TCP-соединение с {} ({}.{}.{}.{}:{})",
            host, ip[0], ip[1], ip[2], ip[3], port
        ));
    }

    let req = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: DeiX-Browser/3.0 (DeiX OS x86_64)\r\nAccept: text/html,text/plain\r\nConnection: close\r\n\r\n",
        path, host
    );
    tcp::send(req.as_bytes());

    let mut raw: Vec<u8> = Vec::new();
    let t0 = crate::timer::uptime_ms();
    loop {
        let chunk = tcp::recv(1000);
        if !chunk.is_empty() {
            raw.extend_from_slice(&chunk);
            if raw.len() > MAX_BODY {
                break;
            }
        } else if !raw.is_empty() {
            // Если получили данные и новые не поступают более 1500мс
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
    let header_text = String::from_utf8_lossy(&raw[..body_offset]).into_owned();
    let (status, reason) = parse_status_line(&header_text);
    let location = header_value(&header_text, "location");
    let body = String::from_utf8_lossy(&raw[body_offset..]).into_owned();
    Ok((status, reason, location, body))
}

/// HTTP GET с переходом по redirect (до MAX_REDIRECTS).
pub fn fetch(url: &str) -> Result<HttpResponse, String> {
    let mut current = if url.trim().starts_with("https://") {
        return Err("HTTPS не поддерживается: в DeiX нет TLS".into());
    } else if url.trim().starts_with("http://") {
        url.trim().to_string()
    } else {
        format!("http://{}", url.trim())
    };

    for _ in 0..=MAX_REDIRECTS {
        let (status, reason, location, body) = fetch_once(&current)?;
        let is_redirect = matches!(status, 301 | 302 | 303 | 307 | 308);
        if is_redirect {
            if let Some(loc) = location {
                if loc.starts_with("https://") {
                    return Err(format!(
                        "Сервер перенаправляет на {} — HTTPS/TLS не поддерживается",
                        loc
                    ));
                }
                current = absolutize(&loc, &current);
                continue;
            }
        }
        return Ok(HttpResponse {
            status,
            reason,
            body,
            final_url: current,
        });
    }
    Err(format!("Слишком много перенаправлений (>{})", MAX_REDIRECTS))
}

/// Превращает Location в абсолютный URL относительно base.
fn absolutize(loc: &str, base: &str) -> String {
    if loc.starts_with("http://") {
        return loc.to_string();
    }
    let (host, port, _) = match parse_url(base) {
        Some(v) => v,
        None => return loc.to_string(),
    };
    let path = if loc.starts_with('/') {
        loc.to_string()
    } else {
        format!("/{}", loc)
    };
    if port == 80 {
        format!("http://{}{}", host, path)
    } else {
        format!("http://{}:{}{}", host, port, path)
    }
}

/// "HTTP/1.1 200 OK" -> (200, "OK"). Если строка не распознана — (0, "").
fn parse_status_line(header: &str) -> (u16, String) {
    let line = header.lines().next().unwrap_or("");
    if !line.starts_with("HTTP/") {
        return (0, String::new());
    }
    let mut it = line.splitn(3, ' ');
    let _ver = it.next();
    let code = it.next().and_then(|c| c.parse::<u16>().ok()).unwrap_or(0);
    let reason = it.next().unwrap_or("").trim().to_string();
    (code, reason)
}

/// Значение заголовка (регистронезависимо), без пробелов по краям.
fn header_value(header: &str, name: &str) -> Option<String> {
    for line in header.lines().skip(1) {
        if let Some(colon) = line.find(':') {
            if line[..colon].trim().eq_ignore_ascii_case(name) {
                return Some(line[colon + 1..].trim().to_string());
            }
        }
    }
    None
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
