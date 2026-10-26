// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// cloudauth — вход в аккаунт DeiX через Supabase (см. docs/CLOUD_AUTH.md).
//
// Поток:
//   cloud login <email> <password>  — отправляет email+пароль на шлюз
//     сайта; шлюз вызывает GoTrue password grant и, если аккаунт
//     защищён 2FA, отвечает «нужен код» (без challenge — challenge
//     делается только перед проверкой кода, чтобы challenge
//     не expired до ввода кода); ОС
//     печатает 6-значный код из приложения Authenticator, шлюз
//     делает challenge+verify и получает aal2-сессию. Ник и user_id
//     берутся из профиля (service-ключ на шлюзе). Результат
//     кешируется: USERS.DB получает запись с паролем из облака
//     (force-set, без проверки старого — это синхронизация смены
//     пароля на сайте), а связь user_id → локальный ник — в
//     /userdata/etc/cloud_link; дальше вход работает офлайн.
//
// Честная граница (§42): это ВХОД, а не защита аккаунта. Пароль
// пересылается в открытом виде по TLS на шлюз (Supabase Голд) —
// как на сайте; публичный ключ anon-key в ОС встраивается
// намеренно: шлюз имеет тот же уровень доступа, что и сайт. RLS
// и лимиты GoTrue при этом работают как на сайте. Ник в USERS.DB —
// имя в профиле сайта; если оно занято несвязанным локальным
// аккаунтом, добавляется суффикс «_c<n>» (ни при каких
// обстоятельствах входящий облачный аккаунт не перезахватывает
// существующий локальный).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::println;

/// Шлюз сайта: ОС ходит на домен сайта, а не на supabase.co (его
/// режут провайдеры по SNI). То же, что devauth.
const GATEWAY_URL: &str = "https://deix-atimenkas-projects.vercel.app/api/os";

/// Связь локального никнейма с облачным user_id:
/// `<user_id>:<email>:<username>\n`. Один файл — одна облачная
/// привязка устройства (вход поддерживает один аккаунт).
const LINK_FILE: &str = "/userdata/etc/cloud_link";

/// Этап облачного входа после первого запроса.
pub enum CloudStep {
    /// Вход завершён: локальный ник и облачный user_id известны.
    Done {
        username: String,
        user_id: String,
    },
    /// Требуется 6-значный TOTP-код; challenge делается в verify-фазе.
    NeedTotp {
        factor_id: String,
        access_token: String,
    },
}

// ==================== HTTP + JSON-помощники ====================

fn http_call(name: &str, body: &str) -> Result<crate::net::http::HttpBytes, String> {
    let url = format!("{}?fn={}", GATEWAY_URL, name);
    crate::net::http::request_bytes(
        "POST",
        &url,
        &[("Content-Type", "application/json")],
        body.as_bytes(),
    )
}

fn http_error(what: &str, status: u16, body: &[u8]) -> String {
    format!(
        "{} failed: HTTP {} {}",
        what,
        status,
        clip(&String::from_utf8_lossy(body), 120)
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

/// Экранирует `"` и `\` для вложенного JSON-строкового значения.
/// Email/пароль по сайтной валидации не содержат these символов,
/// но это cheap insurance против некорректного тела.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out
}

/// Значение строкового поля в JSON-ответе шлюза. Ответы — плоские
/// объекты с ASCII-значениями без экранирования.
fn json_str_field(obj: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\":", key);
    let start = obj.find(&pat)? + pat.len();
    let rest = obj[start..].trim_start().strip_prefix(':')?.trim_start();
    let inner = rest.strip_prefix('\"')?;
    let end = inner.find('\"')?;
    Some(inner[..end].to_string())
}

// ==================== device_id ====================

/// device_id этой установки (shared с devauth): нужен шлюзу для
/// потолка запросов с device и для device_links.
fn device_id() -> Result<String, String> {
    crate::devauth::device_id()
}

// ==================== локальная привязка ====================

/// Читает связь: `(user_id, email, username)`, если файл есть.
pub fn linked() -> Option<(String, String, String)> {
    let data = crate::vfs::read_file(LINK_FILE).ok()?;
    let text = String::from_utf8_lossy(&data);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, ':');
        let user_id = parts.next()?;
        let email = parts.next()?;
        let username = parts.next()?;
        if !user_id.is_empty() && !username.is_empty() {
            return Some((user_id.to_string(), email.to_string(), username.to_string()));
        }
    }
    None
}

/// Записывает связь.
fn store_link(user_id: &str, email: &str, username: &str) -> Result<(), String> {
    crate::vfs::mkdir("/userdata/etc").map_err(|e| e.message())?;
    let content = format!("{}:{}:{}\n", user_id, email, username);
    crate::vfs::write_file(LINK_FILE, content.as_bytes())
        .map_err(|e| e.message())
}

// ==================== имя пользователя ====================

/// Санитизация ника профиля: как auth.rs (ascii alnum/_/-, ≤32).
fn sanitize_username(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        }
        if out.len() >= 32 {
            break;
        }
    }
    out
}

/// Возвращает локальный ник для облачного аккаунта. Если ник
/// занят несвязанным (локальным) аккаунтом — добавляет суффикс
/// `_c<n>`, чтобы входящий облачный аккаунт не перезахватил
/// существующий локальный. Ник из link-файла (cached) при
/// повторном входе не переименовывается — он уже согласован.
fn resolve_username(cloud_username: &str, email: &str) -> Result<String, String> {
    let mut name = sanitize_username(cloud_username);
    if name.is_empty() {
        return Err(String::from("cloud username is empty"));
    }

    let users = crate::auth::list_usernames().map_err(|e| format!("{:?}", e))?;
    let cached = linked();

    // Занято локальным (не облачным) аккаунтом — суффикс.
    let is_local_taken = users.iter().any(|u| u == &name)
        && cached.map_or(true, |(_, _, cname)| cname != name);
    if is_local_taken {
        let mut n = 1u32;
        loop {
            let candidate = format!("{}_c{}", name, n);
            if !users.iter().any(|u| u == &candidate) {
                name = candidate;
                break;
            }
            n += 1;
        }
    }
    Ok(name)
}

/// Кеширует успешный облачный вход: force-set пароля в USERS.DB
/// (пароль сайта — авторитетный), запись link-файла. Возвращает
/// локальный ник.
pub fn cache_login(
    cloud_username: &str,
    cloud_password: &str,
    email: &str,
    user_id: &str,
) -> Result<String, String> {
    let username = resolve_username(cloud_username, email)?;

    // force-set: пароль сайта переписывает локальный хэш без
    // проверки старого пароля и без перестановки LUKS-слотов.
    crate::auth::force_set_password(&username, cloud_password)
        .map_err(|e| format!("{:?}", e))?;

    store_link(user_id, email, &username)?;
    Ok(username)
}

// ==================== публичный интерфейс ====================

/// Первый этап облачного входа: email+пароль → шлюз → GoTrue.
/// Ok(Done) — вход завершён (можно кешировать). Ok(NeedTotp) —
/// требуется 6-значный код. Err — причина отказа (показана UI).
pub fn begin(email: &str, password: &str) -> Result<CloudStep, String> {
    let device = device_id()?;
    let body = format!(
        "{{\"p_email\":\"{}\",\"p_password\":\"{}\",\"p_device_id\":\"{}\"}}",
        json_escape(email),
        json_escape(password),
        device
    );
    let resp = http_call("cloud_login", &body)?;
    if resp.status != 200 {
        return Err(http_error("cloud login", resp.status, &resp.body));
    }
    let text = String::from_utf8_lossy(&resp.body).into_owned();
    match json_str_field(&text, "status").as_deref() {
        Some("ok") => Ok(CloudStep::Done {
            username: json_str_field(&text, "username").unwrap_or_default(),
            user_id: json_str_field(&text, "user_id").unwrap_or_default(),
        }),
        Some("totp_required") => Ok(CloudStep::NeedTotp {
            factor_id: json_str_field(&text, "factor_id").unwrap_or_default(),
            access_token: json_str_field(&text, "access_token").unwrap_or_default(),
        }),
        _ => Err(format!("unexpected server response: {}", clip(&text, 120))),
    }
}

/// Второй этап: 6-значный TOTP-код → шлюз → challenge+verify →
/// aal2-сессия → ник и user_id. Возвращает (username, user_id).
pub fn finish_totp(
    access_token: &str,
    factor_id: &str,
    code: &str,
) -> Result<(String, String), String> {
    let device = device_id()?;
    let body = format!(
        "{{\"p_access_token\":\"{}\",\"p_factor_id\":\"{}\",\"p_code\":\"{}\",\"p_device_id\":\"{}\"}}",
        json_escape(access_token),
        json_escape(factor_id),
        json_escape(code),
        device
    );
    let resp = http_call("cloud_verify", &body)?;
    if resp.status != 200 {
        return Err(http_error("cloud verify", resp.status, &resp.body));
    }
    let text = String::from_utf8_lossy(&resp.body).into_owned();
    if json_str_field(&text, "status").as_deref() != Some("ok") {
        return Err(format!("unexpected server response: {}", clip(&text, 120)));
    }
    Ok((
        json_str_field(&text, "username").unwrap_or_default(),
        json_str_field(&text, "user_id").unwrap_or_default(),
    ))
}

/// CLI: `cloud status` — состояние облачной привязки.
pub fn cmd_status() {
    match linked() {
        Some((user_id, email, username)) => {
            let short = &user_id[..user_id.len().min(8)];
            println!("  [cloud] linked: {} ({}) as {}", username, email, short);
            println!("  [cloud] offline sign-in available after first online login");
        }
        None => {
            println!("  [cloud] not linked. Sign in on the boot screen with");
            println!("  [cloud] email + password, or run 'cloud login' after boot.");
        }
    }
}

/// CLI: `cloud unlink` — удаляет связь. Локальный аккаунт остаётся.
pub fn cmd_unlink() {
    match crate::vfs::remove(LINK_FILE) {
        Ok(()) => println!("  [cloud] link removed. The local account stays."),
        Err(_) => println!("  [cloud] no link to remove."),
    }
}
