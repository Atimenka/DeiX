// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// devauth — серверная активация dev-режима (см. docs/DEVAUTH.md).
//
// Поток:
//   dev link    — показывает одноразовый код привязки (XXXX-XXXX, 10 минут);
//                 пользователь вводит его на сайте в разделе «Аккаунт», и
//                 устройство записывается на его аккаунт. Фоновая проверка
//                 сообщает, когда привязка выполнена. Без привязки сервер
//                 заявки не принимает;
//   dev request — создаёт device_id, отправляет заявку одним POST и
//                 сразу отключается; дальше ответ сервера ждёт не команда,
//                 а фоновый опрос статуса (poll_pending), который сам
//                 сообщает об одобрении или отказе;
//   dev on      — требует активации: кешированный ключ в /userdata или
//                 статус 'approved' на сервере (ключ кешируется, дальше
//                 работает офлайн);
//   dev revoke  — удаляет локальный ключ (повторная активация — снова
//                 через сервер).
//
// Сервер — Supabase PostgREST (см. supabase/migrations). ОС ходит туда
// по HTTPS с publishable-ключом: он ПУБЛИЧНЫЙ по дизайну и дает право
// вызвать четыре функции — begin_device_link и device_link_status
// (привязка к аккаунту), request_dev_unlock (подать заявку) и
// claim_dev_unlock (узнать статус и забрать одобренный токен; токен
// выдается один раз). К таблицам у этого ключа доступа нет. Заявки
// одобряет владелец проекта в админ-панели сайта (web/);
// service-ключи и пароль БД в ОС не встраиваются.
//
// Честная граница (§42): это реестр устройств, а не защита от владельца
// машины — исходники открыты, проверку можно пропатчить. Никаких других
// гарантий код не заявляет.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::println;
use crate::ui::notify::{self, Kind};

const SUPABASE_URL: &str = "https://xtsrvmwfnsxunvwuaplz.supabase.co";
const ANON_KEY: &str = "sb_publishable_gFxef7M8SrR-yUO_KyLkMg_AoNsUins";

/// Интервал фонового опроса статуса заявки.
const POLL_INTERVAL_MS: u64 = 20_000;
/// Интервал проверки, привязал ли пользователь устройство на сайте.
const LINK_POLL_INTERVAL_MS: u64 = 5_000;
/// Подряд идущих сбоев связи, после которых опрос прекращается.
const POLL_MAX_FAILURES: u32 = 5;

/// Заявка подана (или уже была на сервере) и решение ещё не получено.
static PENDING: AtomicBool = AtomicBool::new(false);
static POLLING: AtomicBool = AtomicBool::new(false);
static NEXT_POLL_MS: AtomicU64 = AtomicU64::new(0);
static POLL_FAILURES: AtomicU32 = AtomicU32::new(0);
static LINK_PENDING: AtomicBool = AtomicBool::new(false);
static NEXT_LINK_MS: AtomicU64 = AtomicU64::new(0);
static LINK_FAILURES: AtomicU32 = AtomicU32::new(0);

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
    /// Одобрено: одноразовый токен, который сервер отдал сейчас и
    /// больше не вернёт.
    Approved(String),
    Denied,
    /// Токен по этой заявке уже был выдан ранее.
    Used,
}

fn rpc(name: &str, body: &str) -> Result<crate::net::http::HttpBytes, String> {
    let url = format!("{}/rest/v1/rpc/{}", SUPABASE_URL, name);
    crate::net::http::request_bytes(
        "POST",
        &url,
        &[
            ("apikey", ANON_KEY),
            ("Content-Type", "application/json"),
        ],
        body.as_bytes(),
    )
}

fn server_status(id: &str) -> Result<ServerStatus, String> {
    let body = format!("{{\"p_device_id\":\"{}\"}}", id);
    let resp = rpc("claim_dev_unlock", &body)?;
    if resp.status != 200 {
        return Err(http_error("status check", resp.status, &resp.body));
    }
    let text = String::from_utf8_lossy(&resp.body).into_owned();
    let text = text.trim();
    match json_str_field(text, "status").as_deref() {
        Some("none") => Ok(ServerStatus::NotRequested),
        Some("pending") => Ok(ServerStatus::Pending),
        Some("approved") => match json_str_field(text, "unlock_key") {
            Some(key) if !key.is_empty() => Ok(ServerStatus::Approved(key)),
            _ => Err(String::from("server approved the request but sent no token")),
        },
        Some("denied") => Ok(ServerStatus::Denied),
        Some("consumed") => Ok(ServerStatus::Used),
        _ => Err(format!("unexpected server response: {}", clip(text, 120))),
    }
}

enum RequestError {
    /// Заявка с таким device_id уже есть; внутри — её статус.
    Duplicate(String),
    /// До сервера не достучались: DNS, TCP, TLS, таймаут.
    Unreachable(String),
    /// Сервер ответил отказом: заявка в таблицу не попала.
    Rejected(String),
}

fn server_request(id: &str) -> Result<(), RequestError> {
    let body = format!(
        "{{\"p_device_id\":\"{}\",\"p_hostname\":\"{}\",\"p_source\":\"os\"}}",
        id,
        crate::OS_RELEASE
    );
    let resp = rpc("request_dev_unlock", &body).map_err(RequestError::Unreachable)?;
    if resp.status != 200 {
        return Err(RequestError::Rejected(describe_rejection(
            "request_dev_unlock",
            resp.status,
            &resp.body,
        )));
    }
    let text = String::from_utf8_lossy(&resp.body).into_owned();
    match json_str_field(&text, "result").as_deref() {
        Some("created") => Ok(()),
        Some("exists") => Err(RequestError::Duplicate(
            json_str_field(&text, "status").unwrap_or_default(),
        )),
        _ => Err(RequestError::Rejected(format!(
            "unexpected server response: {}",
            clip(text.trim(), 120)
        ))),
    }
}

/// Причина отказа сервера: сообщение PostgREST/Kong и, где причина
/// однозначна по коду ответа, что нужно исправить на стороне проекта.
fn describe_rejection(function: &str, status: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body).into_owned();
    let message = json_str_field(&text, "message").unwrap_or_else(|| clip(text.trim(), 160));
    let hint = match status {
        404 => format!(
            " - function {} not found on this project: apply supabase/migrations (see docs/DEVAUTH.md)",
            function
        ),
        401 | 403 => String::from(" - the project rejected the anon key"),
        _ => String::new(),
    };
    format!("HTTP {}: {}{}", status, clip(&message, 160), hint)
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
/// намеренно: ответы — плоский объект с ASCII-значениями (hex-токены,
/// статусы) без экранирования; пробелы вокруг `:` допускаются, их
/// ставит сериализатор jsonb.
fn json_str_field(obj: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\"", key);
    let start = obj.find(&pat)? + pat.len();
    let rest = obj[start..].trim_start().strip_prefix(':')?.trim_start();
    let inner = rest.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

/// Целое числовое поле JSON-ответа (например, `expires_in`).
fn json_num_field(obj: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{}\"", key);
    let start = obj.find(&pat)? + pat.len();
    let rest = obj[start..].trim_start().strip_prefix(':')?.trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
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
        ServerStatus::Used => Err(String::from(
            "the one-time token was already issued and is not stored here - run 'dev request' again",
        )),
        ServerStatus::NotRequested => Err(String::from(
            "no request on the server - run 'dev request' first",
        )),
    }
}

/// `dev id`: печатает device_id (создаёт его при первом вызове), чтобы
/// подать заявку на сайте проекта вместо `dev request`.
pub fn cmd_id() {
    match device_id() {
        Ok(id) => println!("  [devauth] device id: {}", id),
        Err(e) => println!("  [devauth] cannot create device id: {}", e),
    }
}

/// `dev link`: привязывает устройство к аккаунту сайта. Показывает
/// одноразовый код, который вводится на сайте в разделе «Аккаунт»;
/// о завершении привязки сообщает фоновая проверка.
pub fn cmd_link() {
    let id = match device_id() {
        Ok(id) => id,
        Err(e) => {
            println!("  [devauth] cannot create device id: {}", e);
            return;
        }
    };
    println!("  [devauth] device id: {}", id);
    let body = format!("{{\"p_device_id\":\"{}\"}}", id);
    let resp = match rpc("begin_device_link", &body) {
        Ok(resp) => resp,
        Err(e) => {
            println!("  [devauth] server unreachable: {}", e);
            notify::push(Kind::Warning, "Device link", "Server unreachable. No code was issued.");
            return;
        }
    };
    if resp.status != 200 {
        let e = describe_rejection("begin_device_link", resp.status, &resp.body);
        println!("  [devauth] the server refused to issue a code:");
        println!("  [devauth] {}", e);
        notify::push(Kind::Error, "Device link refused", &e);
        return;
    }
    let text = String::from_utf8_lossy(&resp.body).into_owned();
    match json_str_field(&text, "status").as_deref() {
        Some("linked") => {
            let user = json_str_field(&text, "username").unwrap_or_default();
            LINK_PENDING.store(false, Ordering::Relaxed);
            println!("  [devauth] this device is linked to the account '{}'.", user);
            println!("  [devauth] run 'dev request' to ask for a developer token.");
        }
        Some("code") => {
            let code = json_str_field(&text, "code").unwrap_or_default();
            let minutes = json_num_field(&text, "expires_in").unwrap_or(0).div_ceil(60);
            println!("  [devauth] link code: {}  (valid for about {} min)", code, minutes);
            println!("  [devauth] sign in on the site, open Account -> My devices and enter the code.");
            notify::push(
                Kind::Info,
                "Device link code",
                &format!("{} - enter it on the site under Account.", code),
            );
            start_link_polling();
        }
        _ => {
            println!(
                "  [devauth] unexpected server response: {}",
                clip(text.trim(), 120)
            );
        }
    }
}

fn start_link_polling() {
    LINK_FAILURES.store(0, Ordering::Relaxed);
    NEXT_LINK_MS.store(
        crate::timer::uptime_ms() + LINK_POLL_INTERVAL_MS,
        Ordering::Relaxed,
    );
    LINK_PENDING.store(true, Ordering::Relaxed);
}

/// `dev request`: отправляет заявку одним POST и сразу возвращается.
/// Одобрение команда не ждёт: статус заявки дальше опрашивает
/// `poll_pending` из главного цикла.
pub fn cmd_request() {
    let id = match device_id() {
        Ok(id) => id,
        Err(e) => {
            println!("  [devauth] cannot create device id: {}", e);
            return;
        }
    };
    println!("  [devauth] device id: {}", id);
    if cached_unlock(&id) {
        println!("  [devauth] already activated - run 'dev on'.");
        return;
    }
    match server_request(&id) {
        Ok(()) => {
            println!("  [devauth] request sent, connection closed.");
            println!("  [devauth] The project owner approves requests in the site admin panel.");
            notify::push(
                Kind::Info,
                "Dev access requested",
                "Request sent. Waiting for the project owner to approve this device.",
            );
        }
        Err(RequestError::Duplicate(status)) => {
            println!(
                "  [devauth] a request for this device id is already on the server (status: {}).",
                status
            );
            if status == "denied" {
                println!("  [devauth] the project owner denied it earlier.");
                return;
            }
        }
        Err(RequestError::Unreachable(e)) => {
            println!("  [devauth] server unreachable: {}", e);
            notify::push(Kind::Warning, "Dev access", "Server unreachable. The request was not sent.");
            return;
        }
        Err(RequestError::Rejected(e)) => {
            println!("  [devauth] the server rejected the request, nothing was saved:");
            println!("  [devauth] {}", e);
            notify::push(Kind::Error, "Dev access request rejected", &e);
            return;
        }
    }
    start_polling();
    println!(
        "  [devauth] status is checked in the background every {} s;",
        POLL_INTERVAL_MS / 1000
    );
    println!("  [devauth] you will be told when the server decides.");
}

fn start_polling() {
    POLL_FAILURES.store(0, Ordering::Relaxed);
    NEXT_POLL_MS.store(
        crate::timer::uptime_ms() + POLL_INTERVAL_MS,
        Ordering::Relaxed,
    );
    PENDING.store(true, Ordering::Relaxed);
}

fn announce(kind: Kind, title: &str, message: &str) {
    println!("  [devauth] {}", message);
    crate::diag::notice(crate::diag::NONE, message);
    notify::push(kind, title, message);
}

/// Фоновые проверки: привязка устройства к аккаунту и статус заявки.
/// Вызывается из главных циклов CLI и рабочего стола, вне выполнения
/// команд: сетевой стек однозадачный. Пока проверять нечего — выходит
/// сразу.
pub fn poll_pending() {
    let now = crate::timer::uptime_ms();
    let link_due = LINK_PENDING.load(Ordering::Relaxed)
        && now >= NEXT_LINK_MS.load(Ordering::Relaxed);
    let request_due =
        PENDING.load(Ordering::Relaxed) && now >= NEXT_POLL_MS.load(Ordering::Relaxed);
    if !link_due && !request_due {
        return;
    }
    if POLLING.swap(true, Ordering::Acquire) {
        return;
    }
    if link_due {
        check_link();
        NEXT_LINK_MS.store(
            crate::timer::uptime_ms() + LINK_POLL_INTERVAL_MS,
            Ordering::Relaxed,
        );
    }
    if request_due {
        check_pending();
        NEXT_POLL_MS.store(
            crate::timer::uptime_ms() + POLL_INTERVAL_MS,
            Ordering::Relaxed,
        );
    }
    POLLING.store(false, Ordering::Release);
}

fn check_link() {
    let id = match device_id() {
        Ok(id) => id,
        Err(_) => {
            LINK_PENDING.store(false, Ordering::Relaxed);
            return;
        }
    };
    let body = format!("{{\"p_device_id\":\"{}\"}}", id);
    let outcome = match rpc("device_link_status", &body) {
        Ok(resp) if resp.status == 200 => {
            let text = String::from_utf8_lossy(&resp.body).into_owned();
            match json_str_field(&text, "status").as_deref() {
                Some("linked") => Ok(Some(json_str_field(&text, "username").unwrap_or_default())),
                Some("code") => Ok(None),
                _ => Err(None),
            }
        }
        Ok(resp) => Err(Some(describe_rejection(
            "device_link_status",
            resp.status,
            &resp.body,
        ))),
        Err(e) => Err(Some(e)),
    };
    match outcome {
        Ok(Some(user)) => {
            LINK_PENDING.store(false, Ordering::Relaxed);
            announce(
                Kind::Success,
                "Device linked",
                &format!("Linked to the account '{}'. Run 'dev request' to ask for a token.", user),
            );
        }
        Ok(None) => LINK_FAILURES.store(0, Ordering::Relaxed),
        Err(None) => {
            LINK_PENDING.store(false, Ordering::Relaxed);
            announce(
                Kind::Warning,
                "Device link",
                "The link code expired. Run 'dev link' for a new one.",
            );
        }
        Err(Some(e)) => {
            let failures = LINK_FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
            if failures >= POLL_MAX_FAILURES {
                LINK_PENDING.store(false, Ordering::Relaxed);
                announce(
                    Kind::Warning,
                    "Device link",
                    &format!("Link check stopped after {} failures: {}", failures, e),
                );
            }
        }
    }
}

fn check_pending() {
    let id = match device_id() {
        Ok(id) => id,
        Err(e) => {
            PENDING.store(false, Ordering::Relaxed);
            announce(
                Kind::Error,
                "Dev access",
                &format!("Status polling stopped: no device id ({})", e),
            );
            return;
        }
    };
    match server_status(&id) {
        Ok(ServerStatus::Pending) => POLL_FAILURES.store(0, Ordering::Relaxed),
        Ok(ServerStatus::Approved(key)) => {
            PENDING.store(false, Ordering::Relaxed);
            match store_unlock(&id, &key) {
                Ok(()) => announce(
                    Kind::Success,
                    "Dev access approved",
                    "The server approved this device. Run 'dev on' to enable developer mode.",
                ),
                Err(e) => announce(
                    Kind::Warning,
                    "Dev access approved",
                    &format!("Approved, but the key could not be saved: {}", e),
                ),
            }
        }
        Ok(ServerStatus::Denied) => {
            PENDING.store(false, Ordering::Relaxed);
            announce(
                Kind::Error,
                "Dev access denied",
                "The project owner denied this device request.",
            );
        }
        Ok(ServerStatus::Used) => {
            PENDING.store(false, Ordering::Relaxed);
            announce(
                Kind::Warning,
                "Dev access",
                "The one-time token was already issued. Run 'dev request' for a new one.",
            );
        }
        Ok(ServerStatus::NotRequested) => {
            PENDING.store(false, Ordering::Relaxed);
            announce(
                Kind::Warning,
                "Dev access",
                "The server has no request for this device id. Run 'dev request'.",
            );
        }
        Err(e) => {
            let failures = POLL_FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
            if failures >= POLL_MAX_FAILURES {
                PENDING.store(false, Ordering::Relaxed);
                announce(
                    Kind::Warning,
                    "Dev access",
                    &format!("Status polling stopped after {} failures: {}", failures, e),
                );
            }
        }
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
        Err(_) => out.push(String::from("  device id: not created yet ('dev link')")),
    }
    let unlocked = match crate::vfs::read_file(UNLOCK_KEY_FILE) {
        Ok(_) => "yes (cached)",
        Err(_) => "no",
    };
    out.push(format!("  activation: {}", unlocked));
    if LINK_PENDING.load(Ordering::Relaxed) {
        out.push(String::from("  link: waiting for the code to be entered on the site"));
    }
    if PENDING.load(Ordering::Relaxed) {
        out.push(String::from("  request: waiting for the server decision (background check)"));
    }
    out
}
