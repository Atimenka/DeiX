//! HTTP/1.1 клиент поверх TCP и TLS 1.3.
//!
//! Поддерживает: GET и запросы с произвольным методом/заголовками/телом
//! (request_bytes — для REST API, например активации dev-режима), разбор
//! строки статуса и заголовков, Content-Length, Transfer-Encoding:
//! chunked (RFC 9112 §7.1), Content-Encoding: gzip (распаковка в
//! net::inflate), переход по redirect (301/302/303/307/308, до 3
//! переходов, только GET), https:// через собственный TLS 1.3 клиент
//! (net::tls). Имена хостов резолвятся настоящим DNS-клиентом (net::dns)
//! через UDP/53. Честные ограничения: Connection: close (без keep-alive
//! и конвейеризации); TLS только 1.3, сертификаты проверяются по цепочке
//! до встроенных корней без OCSP/CRL (см. net/tls.rs, net/x509.rs).

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use crate::net::{dns, inflate, tcp, tls};

const MAX_REDIRECTS: u32 = 3;
const MAX_BODY: usize = 1024 * 1024;
/// Сколько ждать ответ на один запрос целиком.
const RESPONSE_TIMEOUT_MS: u64 = 10_000;

/// Ответ HTTP-запроса после всех redirect, байты тела как приняты
/// (текстовые потребители сами делают from_utf8_lossy; бинарные —
/// картинки, загрузки — получают данные неиспорченными).
pub struct HttpBytes {
    pub status: u16,
    pub reason: String,
    pub body: Vec<u8>,
    /// Значение заголовка Content-Type как прислал сервер (может быть пустым).
    pub content_type: String,
    pub final_url: String,
}

/// Парсит URL: возвращает (host, port, path, https).
pub fn parse_url(url: &str) -> Option<(String, u16, String, bool)> {
    let s = url.trim();
    let (rest, https) = if s.starts_with("http://") { (&s[7..], false) }
    else if s.starts_with("https://") { (&s[8..], true) }
    else { (s, false) };

    // Отделяем host[:port] от path
    let slash = rest.find('/').unwrap_or(rest.len());
    let host_part = &rest[..slash];
    let path = if slash < rest.len() { &rest[slash..] } else { "/" };

    // Отделяем порт если есть
    let default_port: u16 = if https { 443 } else { 80 };
    let (host, port) = if let Some(colon) = host_part.find(':') {
        let port_str = &host_part[colon+1..];
        let port: u16 = port_str.parse().unwrap_or(default_port);
        (host_part[..colon].to_string(), port)
    } else {
        (host_part.to_string(), default_port)
    };

    if host.is_empty() { return None; }
    Some((host, port, path.to_string(), https))
}

/// Все адреса хоста: литерал и алиасы дают один адрес, DNS — до четырёх.
pub fn resolve_all(host: &str) -> Vec<[u8; 4]> {
    let clean = host.trim().to_lowercase();
    match clean.as_str() {
        "deix.os" | "deix" | "localhost" | "gateway" | "router" => {}
        _ if clean.split('.').count() == 4 && clean.split('.').all(|p| p.parse::<u8>().is_ok()) => {}
        _ => return dns::resolve_all(&clean),
    }
    resolve(host).into_iter().collect()
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

/// Транспорт одного запроса: открытый TCP или TLS 1.3 поверх него.
enum Conn {
    Plain,
    Tls(tls::TlsStream),
}

impl Conn {
    fn send(&mut self, data: &[u8]) -> Result<(), String> {
        match self {
            Conn::Plain => {
                if tcp::send(data) { Ok(()) } else { Err("TCP: отправка не удалась".into()) }
            }
            Conn::Tls(t) => t.send(data),
        }
    }

    fn recv(&mut self, timeout_ms: u64) -> Vec<u8> {
        match self {
            Conn::Plain => tcp::recv(timeout_ms),
            Conn::Tls(t) => t.recv(timeout_ms),
        }
    }

    fn diagnosis(&self) -> String {
        match self {
            Conn::Plain => format!("{}; TCP принято {} байт", tcp::last_end(), tcp::rx_bytes()),
            Conn::Tls(t) => t.diagnosis(),
        }
    }

    fn close(&mut self) {
        match self {
            Conn::Plain => tcp::close(),
            Conn::Tls(t) => t.close(),
        }
    }
}

/// Один HTTP GET без redirect: (status, reason, location, тело-байты, content-type).
fn fetch_once(url: &str) -> Result<(u16, String, Option<String>, Vec<u8>, String), String> {
    request_once("GET", url, &[], &[])
}

/// Один HTTP-запрос произвольным методом без redirect.
/// `extra_headers` добавляются к базовым; `body` отправляется как есть
/// (Content-Length ставится для любого метода, кроме GET без тела).
fn request_once(
    method: &str,
    url: &str,
    extra_headers: &[(&str, &str)],
    body: &[u8],
) -> Result<(u16, String, Option<String>, Vec<u8>, String), String> {
    let (host, port, path, https) = parse_url(url)
        .ok_or_else(|| "Некорректный URL адрес".to_string())?;
    let ips = resolve_all(&host);
    if ips.is_empty() {
        return Err(format!("DNS: не удалось разрешить хост '{}'", host));
    }

    let mut conn = if https {
        Conn::Tls(tls::connect(&ips, port, &host)?)
    } else {
        if let Err(why) = tcp::connect_any(&ips, port) {
            return Err(format!("Не удалось установить TCP-соединение с {} ({})", host, why));
        }
        Conn::Plain
    };

    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: DeiX-Browser/3.0 (DeiX OS x86_64)\r\nAccept: */*\r\nAccept-Encoding: gzip\r\nConnection: close\r\n",
        method, path, host
    );
    for (name, value) in extra_headers {
        req.push_str(name);
        req.push_str(": ");
        req.push_str(value);
        req.push_str("\r\n");
    }
    if !body.is_empty() || method != "GET" {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    let mut req_bytes = req.into_bytes();
    req_bytes.extend_from_slice(body);
    if let Err(e) = conn.send(&req_bytes) {
        conn.close();
        return Err(e);
    }

    let mut raw: Vec<u8> = Vec::new();
    let t0 = crate::timer::uptime_ms();
    loop {
        let chunk = conn.recv(1000);
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
            let head = String::from_utf8_lossy(&raw[..he]).into_owned();
            if header_contains(&head, "transfer-encoding", "chunked") {
                // Ответ chunked: ждём финальный нулевой chunk.
                if dechunk(&raw[he..]).1 {
                    break;
                }
            } else if let Some(cl) = content_length(&raw[..he]) {
                if raw.len() >= he + cl {
                    break;
                }
            }
        }
        if crate::timer::uptime_ms() - t0 > RESPONSE_TIMEOUT_MS {
            break;
        }
    }
    let diagnosis = conn.diagnosis();
    conn.close();

    if raw.is_empty() {
        return Err(format!("Сервер не прислал ответа: {}", diagnosis));
    }

    let body_offset = find_header_end(&raw).unwrap_or(0);
    let header_text = String::from_utf8_lossy(&raw[..body_offset]).into_owned();
    let (status, reason) = parse_status_line(&header_text);
    let location = header_value(&header_text, "location");

    let mut body_bytes: Vec<u8> = raw[body_offset..].to_vec();
    if header_contains(&header_text, "transfer-encoding", "chunked") {
        body_bytes = dechunk(&body_bytes).0;
    }
    if header_contains(&header_text, "content-encoding", "gzip") {
        // При усечении по лимиту распаковки показываем, что успели
        // распаковать — страница будет честно обрезана.
        let (data, _truncated) = inflate::gunzip(&body_bytes).map_err(|e| {
            format!(
                "{} — ответ сжат gzip, но повреждён или обрезан (принято {} байт, лимит {} КиБ)",
                e,
                raw.len(),
                MAX_BODY / 1024
            )
        })?;
        body_bytes = data;
    }

    let content_type = header_value(&header_text, "content-type").unwrap_or_default();
    Ok((status, reason, location, body_bytes, content_type))
}

/// true, если заголовок name присутствует и содержит value (без учёта
/// регистра), например transfer-encoding: chunked.
fn header_contains(header: &str, name: &str, value: &str) -> bool {
    match header_value(header, name) {
        Some(v) => v.to_ascii_lowercase().contains(value),
        None => false,
    }
}

/// Разбирает chunked-тело (RFC 9112 §7.1): (собранные данные, завершено).
/// Трейлеры после нулевого chunk игнорируются.
fn dechunk(raw: &[u8]) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
        let line_end = match raw[pos.min(raw.len())..].windows(2).position(|w| w == b"\r\n") {
            Some(p) => pos + p,
            None => return (out, false),
        };
        let line = String::from_utf8_lossy(&raw[pos..line_end]).into_owned();
        // после размера могут идти расширения chunk: "1a3f;ext=..."
        let size_str = line.split(';').next().unwrap_or("").trim().to_string();
        let size = match usize::from_str_radix(&size_str, 16) {
            Ok(v) => v,
            Err(_) => return (out, false),
        };
        pos = line_end + 2;
        if size == 0 {
            return (out, true);
        }
        if raw.len() < pos + size + 2 {
            return (out, false); // chunk ещё не докачан
        }
        out.extend_from_slice(&raw[pos..pos + size]);
        pos += size + 2; // данные + завершающий CRLF
    }
}

/// HTTP GET с переходом по redirect (до MAX_REDIRECTS), байтовое тело.
pub fn fetch_bytes(url: &str) -> Result<HttpBytes, String> {
    let mut current = if url.trim().starts_with("http://") || url.trim().starts_with("https://") {
        url.trim().to_string()
    } else {
        format!("http://{}", url.trim())
    };

    for _ in 0..=MAX_REDIRECTS {
        let (status, reason, location, body, content_type) = fetch_once(&current)?;
        let is_redirect = matches!(status, 301 | 302 | 303 | 307 | 308);
        if is_redirect {
            if let Some(loc) = location {
                current = absolutize(&loc, &current);
                continue;
            }
        }
        return Ok(HttpBytes {
            status,
            reason,
            body,
            content_type,
            final_url: current,
        });
    }
    Err(format!("Слишком много перенаправлений (>{})", MAX_REDIRECTS))
}

/// Запрос произвольным методом (POST/GET/...) с заголовками и телом,
/// БЕЗ перехода по redirect: REST API (PostgREST) redirect'ов не шлёт,
/// а молча повторять POST по Location было бы неверно.
pub fn request_bytes(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<HttpBytes, String> {
    let (status, reason, _location, resp_body, content_type) =
        request_once(method, url, headers, body)?;
    Ok(HttpBytes {
        status,
        reason,
        body: resp_body,
        content_type,
        final_url: url.trim().to_string(),
    })
}



/// Превращает Location в абсолютный URL относительно base
/// (с сохранением схемы base для относительных ссылок).
pub fn absolutize(loc: &str, base: &str) -> String {
    if loc.starts_with("http://") || loc.starts_with("https://") {
        return loc.to_string();
    }
    let (host, port, _, https) = match parse_url(base) {
        Some(v) => v,
        None => return loc.to_string(),
    };
    let path = if loc.starts_with('/') {
        loc.to_string()
    } else {
        format!("/{}", loc)
    };
    let scheme = if https { "https" } else { "http" };
    let default_port = if https { 443 } else { 80 };
    if port == default_port {
        format!("{}://{}{}", scheme, host, path)
    } else {
        format!("{}://{}:{}{}", scheme, host, port, path)
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
