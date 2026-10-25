//! Медиапак DeiX: тяжёлая некритичная среда живёт на /userdata, а не в
//! /system и не в ядре.
//!
//! Что входит (см. tools/make_media_pack.py и docs/MEDIA.md):
//! * обои рабочего стола (media/wallpapers) — 13 встроенных стилей темы;
//! * картинки-иллюстрации (media/pictures) для менеджера файлов;
//! * демо-видео DXV и музыка (media/video, media/audio);
//! * голосовые сообщения (media/voice).
//!
//! Раздел /system (EROFS, 16 МиБ) несёт только критичное: загрузчик,
//! ядро, etc/, иконки приложений и короткие UI-звуки. Медиапак
//! скачивается с сайта ОС одним фоновым заданием при первом входе в
//! рабочий стол (или вручную: `media install`) и проверяется по SHA-256
//! каждого файла. Обои и звуки при отсутствии медиапака не ломаются:
//! рабочий стол рисует процедурный стиль, UI-звуки лежат в /system.

use crate::net::http;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// Корень медиапака на сайте (production-алиас Vercel). Статические
/// файлы отдаются без API-функций — лимиты Supabase/Vercel не тратятся.
pub const BASE_URL: &str = "https://deix-sepia.vercel.app/media";

/// Корень установленного пака на диске (зашифрованный ext2-том).
pub const ROOT: &str = "/userdata/media";

/// Реестр установленной версии: "version|files|bytes".
const REG_PATH: &str = "/userdata/media/pack.txt";

/// Локальная копия манифеста (нужна для `media remove` без сети).
const LOCAL_MANIFEST: &str = "/userdata/media/manifest.txt";

/// Предел размера одного файла пака: с запасом над самыми крупными
/// обоями, но далеко до ext2-лимита файла (~64 МиБ).
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Один файл манифеста.
pub struct Entry {
    /// Путь относительно /userdata/media (ASCII: [a-z0-9._/-]).
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

fn hex(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        s.push(char::from_digit((b & 0xF) as u32, 16).unwrap_or('0'));
    }
    s
}

fn valid_rel_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 96
        && !p.starts_with('/')
        && !p.contains("..")
        && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_' | b'/'))
        && !p.ends_with('/')
}

fn valid_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Разбирает manifest.txt: "#version N" + строки "path|size|sha256".
pub fn parse_manifest(text: &str) -> Result<(String, Vec<Entry>), String> {
    let mut version = String::new();
    let mut entries = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#version ") {
            version = rest.trim().to_string();
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() != 3 {
            return Err(format!("строка манифеста не в формате path|size|sha256: '{}'", line));
        }
        let size: u64 = parts[1]
            .parse()
            .map_err(|_| format!("размер '{}' — не число", parts[1]))?;
        let sha = parts[2].to_ascii_lowercase();
        if !valid_rel_path(parts[0]) {
            return Err(format!("недопустимый путь в манифесте: '{}'", parts[0]));
        }
        if !valid_sha(&sha) {
            return Err(format!("недопустимый sha256 для '{}'", parts[0]));
        }
        if size == 0 || size > MAX_FILE_BYTES {
            return Err(format!("'{}': размер {} вне 1..{} байт", parts[0], size, MAX_FILE_BYTES));
        }
        entries.push(Entry { path: parts[0].to_string(), size, sha256: sha });
    }
    if version.is_empty() {
        return Err(String::from("в манифесте нет строки '#version'"));
    }
    if entries.is_empty() {
        return Err(String::from("манифест пуст: ни одного файла"));
    }
    Ok((version, entries))
}

fn fetch_text(url: &str) -> Result<String, String> {
    let resp = http::request_bytes_max("GET", url, &[], &[], MAX_FILE_BYTES as usize)?;
    if resp.status != 200 {
        return Err(format!("HTTP {} {} для {}", resp.status, resp.reason, url));
    }
    String::from_utf8(resp.body).map_err(|_| String::from("ответ не текст UTF-8"))
}

/// Скачивает манифест с сайта.
pub fn fetch_manifest() -> Result<(String, Vec<Entry>), String> {
    let text = fetch_text(&format!("{}/manifest.txt", BASE_URL))?;
    parse_manifest(&text)
}

/// Версия установленного пака (None — не установлен).
pub fn installed_version() -> Option<String> {
    let data = crate::vfs::read_file(REG_PATH).ok()?;
    let text = core::str::from_utf8(&data).ok()?;
    text.split('|').next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Скачивает один файл пака и проверяет его по размеру и SHA-256.
fn fetch_file(e: &Entry) -> Result<Vec<u8>, String> {
    let url = format!("{}/{}", BASE_URL, e.path);
    let resp = http::request_bytes_max("GET", &url, &[], &[], MAX_FILE_BYTES as usize)?;
    if resp.status != 200 {
        return Err(format!("HTTP {} {}", resp.status, resp.reason));
    }
    if resp.body.len() as u64 != e.size {
        return Err(format!(
            "размер {} байт вместо заявленных {} — скачивание оборвалось?",
            resp.body.len(),
            e.size
        ));
    }
    let digest = hex(&crate::crypto::sha256::sha256(&resp.body));
    if digest != e.sha256 {
        return Err(format!("sha256 не совпал: {} != {}", digest, e.sha256));
    }
    Ok(resp.body)
}

fn parent_dir(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(dir, _)| dir)
}

/// Установка медиапака: уже лежащие на диске файлы с верным размером и
/// SHA-256 пропускаются (повторный `media install` докачивает только
/// недостающее). Возвращает человекочитаемый итог.
pub fn install(verbose: bool) -> Result<String, String> {
    let text = fetch_text(&format!("{}/manifest.txt", BASE_URL))?;
    let (version, entries) = parse_manifest(&text)?;

    crate::vfs::mkdir(ROOT).map_err(|e| format!("не создан {}: {}", ROOT, e.message()))?;

    let mut installed = 0usize;
    let mut skipped = 0usize;
    let mut bytes = 0u64;
    let mut failures: Vec<String> = Vec::new();
    for e in entries.iter() {
        let dest = format!("{}/{}", ROOT, e.path);
        if let Ok(existing) = crate::vfs::read_file(&dest) {
            if existing.len() as u64 == e.size
                && hex(&crate::crypto::sha256::sha256(&existing)) == e.sha256
            {
                skipped += 1;
                bytes += e.size;
                continue;
            }
        }
        if verbose {
            crate::println!("{}", format!("media: качаю {} ({} КиБ)...", e.path, e.size / 1024));
        }
        match fetch_file(e) {
            Ok(body) => {
                if let Some(dir) = parent_dir(&dest) {
                    crate::vfs::mkdir(dir).map_err(|m| format!("не создан {}: {}", dir, m.message()))?;
                }
                crate::vfs::write_file(&dest, &body)
                    .map_err(|m| format!("не записан {}: {} (место на /userdata?)", dest, m.message()))?;
                installed += 1;
                bytes += e.size;
            }
            Err(why) => failures.push(format!("{}: {}", e.path, why)),
        }
    }

    // Локальная копия манифеста (для remove без сети) и реестр версии.
    let _ = crate::vfs::write_file(LOCAL_MANIFEST, text.as_bytes());
    let reg = format!("{}|{}|{}\n", version, installed + skipped, bytes);
    let _ = crate::vfs::write_file(REG_PATH, reg.as_bytes());

    // Обои рабочего стола читаются из /userdata/media/wallpapers —
    // обновляем кеш обоев, чтобы новые стили появились сразу.
    crate::ui::theme::bump_wallpaper_epoch();

    if failures.is_empty() {
        Ok(format!(
            "медиапак v{} установлен: {} файлов ({} новых, {} уже было), {} КиБ",
            version,
            installed + skipped,
            installed,
            skipped,
            bytes / 1024
        ))
    } else {
        Err(format!(
            "медиапак v{} установлен частично: {} файлов OK, {} с ошибками: {}",
            version,
            installed + skipped,
            failures.len(),
            failures.join("; ")
        ))
    }
}

/// Удаление установленного пака по локальной копии манифеста.
pub fn remove() -> Result<String, String> {
    let data = crate::vfs::read_file(LOCAL_MANIFEST)
        .map_err(|_| String::from("медиапак не установлен (нет локального манифеста)"))?;
    let text = core::str::from_utf8(&data)
        .map_err(|_| String::from("локальный манифест не текст UTF-8"))?;
    let (_, entries) = parse_manifest(text)?;
    let mut removed = 0usize;
    for e in entries.iter() {
        let dest = format!("{}/{}", ROOT, e.path);
        if crate::vfs::remove(&dest).is_ok() {
            removed += 1;
        }
    }
    let _ = crate::vfs::remove(LOCAL_MANIFEST);
    let _ = crate::vfs::remove(REG_PATH);
    crate::ui::theme::bump_wallpaper_epoch();
    Ok(format!("медиапак удалён: {} файлов с /userdata/media", removed))
}

/// Состояние для `media status` (офлайн: что реально лежит на диске).
pub fn status_lines() -> Vec<String> {
    let mut out = Vec::new();
    match installed_version() {
        Some(v) => out.push(format!("установлен: версия {} (реестр {})", v, REG_PATH)),
        None => out.push(String::from("не установлен (реестра нет)")),
    }
    for sub in ["wallpapers", "pictures", "audio", "video", "voice"] {
        let dir = format!("{}/{}", ROOT, sub);
        match crate::vfs::readdir(&dir) {
            Ok(list) => {
                let files: usize = list.iter().filter(|d| !d.is_dir).count();
                let total: u64 = list.iter().map(|d| d.size).sum();
                out.push(format!("{}: {} файлов, {} КиБ", dir, files, total / 1024));
            }
            Err(_) => out.push(format!("{}: нет каталога", dir)),
        }
    }
    out
}

// ---- Автоустановка при первом входе --------------------------------------

static AUTO_TRIED: AtomicBool = AtomicBool::new(false);

/// Однократный фоновый запуск автоустановки (рабочий стол вызывает при
/// старте сессии). Если пак уже установлен или попытка уже была —
/// ничего не делает.
pub fn auto_once() {
    if AUTO_TRIED.swap(true, Ordering::SeqCst) {
        return;
    }
    if installed_version().is_some() {
        return;
    }
    if crate::sched::spawn("media-pack", auto_task).is_none() {
        AUTO_TRIED.store(false, Ordering::SeqCst);
    }
}

extern "C" fn auto_task() {
    // Ждём сеть до 120 с: карта поднимается не мгновенно, а DHCP/статик
    // конфигурация у пользователя может задержаться.
    let mut ready = false;
    for _ in 0..60 {
        if crate::rtl8139::is_ready() {
            ready = true;
            break;
        }
        crate::timer::sleep_ms(2000);
    }
    if !ready {
        crate::ui::notify::push(
            crate::ui::notify::Kind::Warning,
            "Медиапак",
            "сеть не готова — обои и демо-медиа не установлены; повтор: 'media install'",
        );
        return;
    }
    match install(false) {
        Ok(summary) => crate::ui::notify::push(
            crate::ui::notify::Kind::Success,
            "Медиапак установлен",
            &summary,
        ),
        Err(e) => crate::ui::notify::push(
            crate::ui::notify::Kind::Error,
            "Медиапак не установлен",
            &e,
        ),
    }
}

/// CLI `media`: состояние пака, установка/удаление, управление
/// воспроизведением (движок src/media.rs работает и без графики).
pub fn cmd_media(rest: &str) {
    use crate::println;
    let trimmed = rest.trim();
    let (sub, arg) = match trimmed.find(char::is_whitespace) {
        Some(i) => (&trimmed[..i], trimmed[i..].trim()),
        None => (trimmed, ""),
    };
    match sub {
        "" | "status" => {
            for line in status_lines() {
                println!("{}", line);
            }
        }
        "install" => match install(true) {
            Ok(s) => println!("{}", s),
            Err(e) => println!("Ошибка: {}", e),
        },
        "remove" => match remove() {
            Ok(s) => println!("{}", s),
            Err(e) => println!("Ошибка: {}", e),
        },
        "play" => {
            if arg.is_empty() {
                println!("Использование: media play <файл>");
                return;
            }
            match crate::media::play(arg) {
                Ok(()) => println!("Воспроизведение: {}", arg),
                Err(e) => println!("Ошибка: {}", e),
            }
        }
        "pause" => {
            crate::media::toggle_pause();
            let st = crate::media::state();
            if st == crate::media::ST_PAUSED {
                println!("Пауза");
            } else if st == crate::media::ST_PLAYING {
                println!("Продолжено");
            } else {
                println!("Ничего не воспроизводится");
            }
        }
        "stop" => {
            crate::media::stop();
            println!("Остановлено");
        }
        "vol" => match arg {
            "up" => {
                crate::media::volume_up();
                println!("Громкость: {}%", crate::media::volume());
            }
            "down" => {
                crate::media::volume_down();
                println!("Громкость: {}%", crate::media::volume());
            }
            _ => println!("Использование: media vol up|down"),
        },
        _ => println!(
            "Использование: media [status|install|remove|play <файл>|pause|stop|vol up|vol down]"
        ),
    }
}
