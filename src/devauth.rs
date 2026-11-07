// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// devauth — серверная активация dev-режима (см. docs/DEVAUTH.md).
//
// Поток:
//   dev on      — требует активации: кешированный ключ в /userdata или
//                 статус 'approved' на сервере (ключ кешируется, дальше
//                 работает офлайн);
//   dev revoke  — удаляет локальный ключ (повторная активация — снова
//                 через сервер).
//
//
// Честная граница (§42): это реестр устройств, а не защита от владельца
// машины — исходники открыты, проверку можно пропатчить. Никаких других
// гарантий код не заявляет.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;


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
    let inner = rest.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

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

pub fn cmd_request() {
    let id = match device_id() {
        Ok(id) => id,
        Err(e) => {
            println!("  [devauth] cannot create device id: {}", e);
            return;
        }
    };
    println!("  [devauth] device id: {}", id);
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
    }
    let unlocked = match crate::vfs::read_file(UNLOCK_KEY_FILE) {
        Ok(_) => "yes (cached)",
        Err(_) => "no",
    };
    out.push(format!("  activation: {}", unlocked));
    out
}
