//! HTTP/1.1 клиент поверх TCP и TLS 1.3.
//!
//! Поддерживает: GET, разбор строки статуса и заголовков, Content-Length,
//! Transfer-Encoding: chunked (RFC 9112 §7.1), Content-Encoding: gzip
//! (распаковка в net::inflate), переход по redirect (301/302/303/307/308,
//! до 3 переходов), https:// через собственный TLS 1.3 клиент (net::tls).
//! Имена хостов резолвятся настоящим DNS-клиентом (net::dns) через UDP/53.
//! Честные ограничения: только GET и Connection: close (без keep-alive и
//! конвейеризации); TLS только 1.3, сертификаты проверяются по цепочке
//! до встроенных корней без OCSP/CRL (см. net/tls.rs, net/x509.rs).

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use crate::net::{dns, inflate, tcp, tls};

const MAX_REDIRECTS: u32 = 3;
const MAX_BODY: usize = 1024 * 1024;

/// Ответ HTTP-запроса после всех redirect.
pub struct HttpResponse {
    pub status: u16,
    pub reason: String,
    pub body: String,
    /// URL, с которого фактически пришёл ответ (после redirect).
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

    fn close(&mut self) {
        match self {
            Conn::Plain => tcp::close(),
            Conn::Tls(t) => t.close(),
        }
    }
}

/// Один HTTP GET без redirect: (status, reason, location, body).
fn fetch_once(url: &str) -> Result<(u16, String, Option<String>, String), String> {
    let (host, port, path, https) = parse_url(url)
        .ok_or_else(|| "Некорректный URL адрес".to_string())?;
    let ip = resolve(&host)
        .ok_or_else(|| format!("DNS: не удалось разрешить хост '{}'", host))?;

    let mut conn = if https {
        Conn::Tls(tls::connect(ip, port, &host)?)
    } else {
        if !tcp::connect(ip, port) {
            return Err(format!(
                "Не удалось установить TCP-соединение с {} ({}.{}.{}.{}:{})",
                host, ip[0], ip[1], ip[2], ip[3], port
            ));
        }
        Conn::Plain
    };

    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: DeiX-Browser/3.0 (DeiX OS x86_64)\r\nAccept: text/html,text/plain\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
        path, host
    );
    if let Err(e) = conn.send(req.as_bytes()) {
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
        if crate::timer::uptime_ms() - t0 > 4000 {
            break;
        }
    }
    conn.close();

    if raw.is_empty() {
        return Err("Сервер не прислал ответа".into());
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

    let body = String::from_utf8_lossy(&body_bytes).into_owned();
    Ok((status, reason, location, body))
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

/// HTTP GET с переходом по redirect (до MAX_REDIRECTS).
pub fn fetch(url: &str) -> Result<HttpResponse, String> {
    let mut current = if url.trim().starts_with("http://") || url.trim().starts_with("https://") {
        url.trim().to_string()
    } else {
        format!("http://{}", url.trim())
    };

    for _ in 0..=MAX_REDIRECTS {
        let (status, reason, location, body) = fetch_once(&current)?;
        let is_redirect = matches!(status, 301 | 302 | 303 | 307 | 308);
        if is_redirect {
            if let Some(loc) = location {
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

/// Превращает Location в абсолютный URL относительно base
/// (с сохранением схемы base для относительных ссылок).
fn absolutize(loc: &str, base: &str) -> String {
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
