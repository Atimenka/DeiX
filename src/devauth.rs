// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// devauth — серверная активация dev-режима (см. docs/DEVAUTH.md).
//
// Поток:
//   dev request — создаёт device_id и подаёт заявку на сервер;
//   dev on      — требует активации: кешированный ключ в /userdata или
//                 статус 'approved' на сервере (ключ кешируется, дальше
//                 работает офлайн);
//   dev revoke  — удаляет локальный ключ (повторная активация — снова
//                 через сервер).
//
// Сервер — Supabase PostgREST (таблица public.dev_unlocks, см.
// supabase/migrations). ОС ходит туда по HTTPS с publishable-ключом:
// он ПУБЛИЧНЫЙ по дизайну, RLS-политики позволяют ему только подать
// заявку и прочитать статус. Одобряет заявки владелец проекта из
// дашборда; service-ключи и пароль БД в ОС не встраиваются.
//
// Честная граница (§42): это реестр устройств, а не защита от владельца
// машины — исходники открыты, проверку можно пропатчить. Никаких других
// гарантий код не заявляет.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::println;

const SUPABASE_URL: &str = "https://xtsrvmwfnsxunvwuaplz.supabase.co";
const ANON_KEY: &str = "sb_publishable_gFxef7M8SrR-yUO_KyLkMg_AoNsUins";

const DEVICE_ID_FILE: &str = "/userdata/etc/device.id";
const UNLOCK_KEY_FILE: &str = "/userdata/etc/dev_unlock.key";

// ==================== device_id ====================

/// Возвращает device_id этой установки (32 hex-символа), создавая и
/// сохраняя его при первом обращении. Ошибка — если /userdata недоступен
/// (id негде хранить, активация без него не имеет смысла).
pub fn device_id() -> Result<String, String> {
    if let Ok(data) = crate::vfs::read_file(DEVICE_ID_FILE) {
        let id: String = core::str::from_utf8(&data)
            .unwrap_or("")
            .trim()
            .to_string();
        if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(id);
        }
        // Файл повреждён — генерируем заново ниже.
    }

    let mut raw = [0u8; 16];
    crate::rng::fill_random(&mut raw);
    let mut id = String::with_capacity(32);
    for b in raw.iter() {
        id.push_str(&format!("{:02x}", b));
    }

    crate::vfs::mkdir("/userdata/etc").map_err(|e| e.message())?;
    crate::vfs::write_file(DEVICE_ID_FILE, id.as_bytes())
        .map_err(|e| e.message())?;
    Ok(id)
}

// ==================== локальный ключ ====================

/// Кешированный ключ разблокировки: активация уже проходила на этом
/// device_id. Содержимое файла: "<device_id>:<unlock_key>".
fn cached_unlock(id: &str) -> bool {
    match crate::vfs::read_file(UNLOCK_KEY_FILE) {
        Ok(data) => {
            let text = String::from_utf8_lossy(&data);
            text.trim().strip_prefix(id).map_or(false, |rest| rest.starts_with(':'))
        }
        Err(_) => false,
    }
}

fn store_unlock(id: &str, key: &str) -> Result<(), String> {
    crate::vfs::mkdir("/userdata/etc").map_err(|e| e.message())?;
    let content = format!("{}:{}", id, key);
    crate::vfs::write_file(UNLOCK_KEY_FILE, content.as_bytes())
        .map_err(|e| e.message())
}

// ==================== HTTP к Supabase ====================

/// Статус заявки на сервере.
enum ServerStatus {
    /// Строки нет: заявка не подавалась.
    NotRequested,
    Pending,
    /// approved + unlock_key (пустая строка, если администратор ключ
    /// не заполнил — одобрение всё равно действительно).
    Approved(String),
    Denied,
}

fn server_status(id: &str) -> Result<ServerStatus, String> {
    let url = format!(
        "{}/rest/v1/dev_unlocks?device_id=eq.{}&select=status,unlock_key",
        SUPABASE_URL, id
    );
    let resp = crate::net::http::request_bytes(
        "GET",
        &url,
        &[("apikey", ANON_KEY)],
        &[],
    )?;
    if resp.status != 200 {
        return Err(http_error("status check", resp.status, &resp.body));
    }
    let body = String::from_utf8_lossy(&resp.body).into_owned();
    let trimmed = body.trim();
    if trimmed == "[]" {
        return Ok(ServerStatus::NotRequested);
    }
    match json_str_field(trimmed, "status").as_deref() {
        Some("approved") => Ok(ServerStatus::Approved(
            json_str_field(trimmed, "unlock_key").unwrap_or_default(),
        )),
        Some("pending") => Ok(ServerStatus::Pending),
        Some("denied") => Ok(ServerStatus::Denied),
        _ => Err(format!("unexpected server response: {}", clip(trimmed, 120))),
    }
}

fn server_request(id: &str) -> Result<(), String> {
    let url = format!("{}/rest/v1/dev_unlocks", SUPABASE_URL);
    let body = format!(
        "{{\"device_id\":\"{}\",\"hostname\":\"{}\"}}",
        id,
        crate::OS_RELEASE
    );
    let resp = crate::net::http::request_bytes(
        "POST",
        &url,
        &[
            ("apikey", ANON_KEY),
            ("Content-Type", "application/json"),
            ("Prefer", "return=minimal"),
        ],
        body.as_bytes(),
    )?;
    match resp.status {
        201 => Ok(()),
        409 => Err(String::from("request already exists (duplicate device_id)")),
        _ => Err(http_error("request", resp.status, &resp.body)),
    }
}

fn http_error(what: &str, status: u16, body: &[u8]) -> String {
    format!(
        "{} failed: HTTP {} {}",
        what,
        status,
        clip(String::from_utf8_lossy(body).trim(), 120)
    )
}

/// Обрезает строку до limit символов, не разрывая UTF-8.
fn clip(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let mut cut = limit;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", &s[..cut])
}

/// Значение строкового поля в JSON-ответе PostgREST. Общего парсера нет
/// намеренно: ответы dev_unlocks — плоский объект с ASCII-значениями
/// (hex-ключи и статусы) без экранирования.
fn json_str_field(obj: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\":", key);
    let start = obj.find(&pat)? + pat.len();
    let rest = obj[start..].trim_start();
    let inner = rest.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

// ==================== публичный интерфейс ====================

/// Проверяет активацию для `dev on`. Ok(описание источника) — активация
/// пройдена; Err(причина) — dev-режим включать нельзя.
pub fn ensure_unlocked() -> Result<&'static str, String> {
    let id = device_id()?;
    if cached_unlock(&id) {
        return Ok("cached unlock key");
    }
    match server_status(&id)? {
        ServerStatus::Approved(key) => {
            store_unlock(&id, &key)?;
            Ok("server approval (key cached)")
        }
        ServerStatus::Pending => Err(String::from(
            "request is still pending approval on the server",
        )),
        ServerStatus::Denied => Err(String::from("request was denied on the server")),
        ServerStatus::NotRequested => Err(String::from(
            "no request on the server - run 'dev request' first",
        )),
    }
}

/// `dev request`: подаёт заявку на активацию.
pub fn cmd_request() {
    let id = match device_id() {
        Ok(id) => id,
        Err(e) => {
            println!("  [devauth] cannot create device id: {}", e);
            return;
        }
    };
    println!("  [devauth] device id: {}", id);
    // Если заявка уже есть — честно показываем её состояние вместо
    // заведомого 409 от сервера.
    match server_status(&id) {
        Ok(ServerStatus::NotRequested) => match server_request(&id) {
            Ok(()) => {
                println!("  [devauth] request filed. Ask the project owner to approve");
                println!("  [devauth] this device id, then run 'dev on'.");
            }
            Err(e) => println!("  [devauth] {}", e),
        },
        Ok(ServerStatus::Pending) => {
            println!("  [devauth] request already filed and pending approval.");
        }
        Ok(ServerStatus::Approved(_)) => {
            println!("  [devauth] already approved - run 'dev on'.");
        }
        Ok(ServerStatus::Denied) => {
            println!("  [devauth] request was denied by the project owner.");
        }
        Err(e) => println!("  [devauth] server unreachable: {}", e),
    }
}

/// `dev revoke`: удаляет локальный ключ разблокировки.
pub fn cmd_revoke() {
    if crate::vfs::remove(UNLOCK_KEY_FILE).is_ok() {
        println!("  [devauth] local unlock key removed.");
        println!("  [devauth] next 'dev on' will need the server again.");
    } else {
        println!("  [devauth] no local unlock key.");
    }
    if crate::devmode::sudo_allowed() {
        crate::devmode::disable_dev_mode();
    }
}

/// Строки состояния активации для `dev status`.
pub fn status_lines() -> Vec<String> {
    let mut out = Vec::new();
    match crate::vfs::read_file(DEVICE_ID_FILE) {
        Ok(data) => out.push(format!(
            "  device id: {}",
            String::from_utf8_lossy(&data).trim()
        )),
        Err(_) => out.push(String::from("  device id: not created yet ('dev request')")),
    }
    let unlocked = match crate::vfs::read_file(UNLOCK_KEY_FILE) {
        Ok(_) => "yes (cached)",
        Err(_) => "no",
    };
    out.push(format!("  activation: {}", unlocked));
    out
}
