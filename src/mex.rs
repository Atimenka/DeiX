//! Загрузчик и исполнитель программ в формате `.mex` (DeiX EXecutable).
//! Полная спецификация формата — см. docs/MEX_FORMAT.md.
//!
//! ## Версия 1.1 (текущая)
//!
//! MEX API v1.1 добавляет сетевые функции для .mex-программ:
//!   - `ping`       — отправить ICMP echo и получить RTT
//!   - `get_mac`    — получить MAC-адрес сетевой карты
//!   - `get_ip`     — получить текущий IP-адрес
//!   - `arp_resolve` — разрешить IP в MAC через ARP
//!
//! Обратная совместимость: v1.0-программы продолжают работать —
//! они просто не видят новых полей в конце структуры MexApi.
//!
//! Модель исполнения: программы выполняются в ring 0 (нет пользовательского
//! режима, нет изоляции процессов — как в MS-DOS), загружаются по
//! фиксированному физическому адресу и вызываются как обычная функция
//! с указателем на таблицу системных функций ядра (MexApi) в RDI.

use crate::{ext2, keyboard, println, println_t, t, timer};

const MEX_MAGIC: u32 = 0x3158_454D; // "MEX1" little-endian
const MEX_HEADER_SIZE_V1: usize = 32;

/// Фиксированный физический/виртуальный адрес загрузки тела программы.
/// Выбран выше кучи ядра (которая начинается на 0x200000) и выше
/// типичного размера кучи (16 МиБ, до ~0x1200000).
const MEX_LOAD_ADDR: usize = 0x0060_0000;

/// Максимальный размер тела программы + bss.
const MEX_MAX_SIZE: usize = 4 * 1024 * 1024; // 4 МиБ

#[repr(C)]
struct MexHeader {
    magic: u32,
    version_major: u16,
    version_minor: u16,
    header_size: u32,
    entry_offset: u32,
    body_size: u32,
    bss_size: u32,
    _reserved: u64,
}

/// Таблица системных функций, передаваемая программе — единственный
/// способ ей взаимодействовать с ядром.
///
/// ## Версионирование
///
/// v1.0 (поля 0x00–0x28): print, read_char, try_read_char, uptime_ms,
///                         read_file, write_file
/// v1.1 (поля 0x30–0x48): ping, get_mac, get_ip, arp_resolve
///
/// Программа определяет доступные поля по version_minor в заголовке .mex:
/// версия 1.0 видит только первые 6 полей, версия 1.1 — все 10.
#[repr(C)]
pub struct MexApi {
    // --- v1.0 ---
    pub print: extern "C" fn(ptr: *const u8, len: usize),
    pub read_char: extern "C" fn() -> u8,
    pub try_read_char: extern "C" fn() -> i32,
    pub uptime_ms: extern "C" fn() -> u64,
    pub read_file: extern "C" fn(name_ptr: *const u8, name_len: usize, out_ptr: *mut u8, out_cap: usize) -> i64,
    pub write_file: extern "C" fn(name_ptr: *const u8, name_len: usize, data_ptr: *const u8, data_len: usize) -> i64,

    // --- v1.1: сетевые функции ---
    /// Отправляет ICMP echo request на указанный IPv4-адрес.
    /// ip_ptr: 4 байта IPv4 (big-endian порядок как в сети).
    /// ip_len: должно быть 4.
    /// timeout_ms: максимальное время ожидания ответа в миллисекундах.
    /// Возвращает: RTT в миллисекундах (>=0), или -1 при ошибке/таймауте.
    pub ping: extern "C" fn(ip_ptr: *const u8, ip_len: usize, timeout_ms: u64) -> i64,

    /// Записывает MAC-адрес сетевой карты (6 байт) в mac_out.
    /// Возвращает 0 при успехе, -1 если сетевая карта не найдена.
    pub get_mac: extern "C" fn(mac_out: *mut u8) -> i64,

    /// Записывает текущий IPv4-адрес (4 байта, network order) в ip_out.
    /// Возвращает 0 при успехе, -1 если сеть не настроена.
    pub get_ip: extern "C" fn(ip_out: *mut u8) -> i64,

    /// Разрешает IPv4-адрес в MAC-адрес через ARP-кэш (без отправки
    /// ARP-запроса — только поиск в уже существующем кэше).
    /// ip_ptr/ip_len: IPv4-адрес (4 байта).
    /// mac_out: буфер на 6 байт для результата.
    /// Возвращает 0 при успехе, -1 если адрес не найден в кэше.
    pub arp_resolve: extern "C" fn(ip_ptr: *const u8, ip_len: usize, mac_out: *mut u8) -> i64,
}

// ==================== v1.0 API implementation ====================

extern "C" fn api_print(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 || len > 1024 * 1024 {
        return;
    }
    let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
    if let Ok(s) = core::str::from_utf8(slice) {
        crate::print!("{}", s);
    }
}

extern "C" fn api_read_char() -> u8 {
    keyboard::read_char()
}

extern "C" fn api_try_read_char() -> i32 {
    match keyboard::try_read_char() {
        Some(c) => c as i32,
        None => -1,
    }
}

extern "C" fn api_uptime_ms() -> u64 {
    timer::uptime_ms()
}

extern "C" fn api_read_file(name_ptr: *const u8, name_len: usize, out_ptr: *mut u8, out_cap: usize) -> i64 {
    if name_ptr.is_null() || name_len == 0 || name_len > 64 {
        return -1;
    }
    let name_slice = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
    let name = match core::str::from_utf8(name_slice) {
        Ok(s) => s,
        Err(_) => return -1,
    };
    match ext2::read_file(name) {
        Ok(data) => {
            if data.len() > out_cap || out_ptr.is_null() {
                return -1;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(data.as_ptr(), out_ptr, data.len());
            }
            data.len() as i64
        }
        Err(_) => -1,
    }
}

extern "C" fn api_write_file(name_ptr: *const u8, name_len: usize, data_ptr: *const u8, data_len: usize) -> i64 {
    if name_ptr.is_null() || name_len == 0 || name_len > 64 || data_ptr.is_null() {
        return -1;
    }
    let name_slice = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
    let name = match core::str::from_utf8(name_slice) {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let data = unsafe { core::slice::from_raw_parts(data_ptr, data_len) };

    if !ext2::is_formatted() && ext2::format().is_err() {
        return -1;
    }

    match ext2::write_file(name, data) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

// ==================== v1.1 network API implementation ====================

pub extern "C" fn api_ping(ip_ptr: *const u8, ip_len: usize, timeout_ms: u64) -> i64 {
    if ip_ptr.is_null() || ip_len != 4 {
        return -1;
    }
    let ip_slice = unsafe { core::slice::from_raw_parts(ip_ptr, 4) };
    let ip: [u8; 4] = [ip_slice[0], ip_slice[1], ip_slice[2], ip_slice[3]];

    if !crate::rtl8139::is_ready() {
        return -1;
    }

    match crate::net::icmp::ping(ip, 0x5678, 0, timeout_ms) {
        Some(rtt) => rtt as i64,
        None => -1,
    }
}

pub extern "C" fn api_get_mac(mac_out: *mut u8) -> i64 {
    if mac_out.is_null() {
        return -1;
    }
    if !crate::rtl8139::is_ready() {
        return -1;
    }
    let mac = crate::rtl8139::mac_address();
    unsafe {
        core::ptr::copy_nonoverlapping(mac.as_ptr(), mac_out, 6);
    }
    0
}

pub extern "C" fn api_get_ip(ip_out: *mut u8) -> i64 {
    if ip_out.is_null() {
        return -1;
    }
    let ip = crate::net::my_ip();
    unsafe {
        core::ptr::copy_nonoverlapping(ip.as_ptr(), ip_out, 4);
    }
    0
}

extern "C" fn api_arp_resolve(ip_ptr: *const u8, ip_len: usize, mac_out: *mut u8) -> i64 {
    if ip_ptr.is_null() || ip_len != 4 || mac_out.is_null() {
        return -1;
    }
    let ip_slice = unsafe { core::slice::from_raw_parts(ip_ptr, 4) };
    let ip: [u8; 4] = [ip_slice[0], ip_slice[1], ip_slice[2], ip_slice[3]];

    match crate::net::arp::cache_lookup(ip) {
        Some(mac) => {
            unsafe {
                core::ptr::copy_nonoverlapping(mac.as_ptr(), mac_out, 6);
            }
            0
        }
        None => -1,
    }
}

// ==================== API construction ====================

fn build_api() -> MexApi {
    MexApi {
        // v1.0
        print: api_print,
        read_char: api_read_char,
        try_read_char: api_try_read_char,
        uptime_ms: api_uptime_ms,
        read_file: api_read_file,
        write_file: api_write_file,
        // v1.1
        ping: api_ping,
        get_mac: api_get_mac,
        get_ip: api_get_ip,
        arp_resolve: api_arp_resolve,
    }
}

// ==================== MEX loader ====================

/// Загружает и исполняет программу `name` (короткое имя ext2-файла, обычно
/// оканчивающееся на .MEX) — вызывается из `cli.rs::cmd_run`.
///
/// Поддерживает версии 1.0 и 1.1 формата .mex. Для v1.0-программ поля
/// v1.1 в MexApi будут просто не прочитаны программой (она знает только
/// о первых 6 полях структуры), так что обратная совместимость полная.
pub fn run(name: &str, _args: &str) {
    let data = match ext2::read_file(name) {
        Ok(d) => d,
        Err(ext2::Ext2Error::FileNotFound) => {
            println!("{}", t!(en: "Program not found.", ru: "Программа не найдена."));
            return;
        }
        Err(_) => {
            println!("{}", t!(en: "Failed to read program file.", ru: "Не удалось прочитать файл программы."));
            return;
        }
    };

    if data.len() < MEX_HEADER_SIZE_V1 {
        println!("{}", t!(en: "Not a valid .mex file (too short).", ru: "Не похоже на .mex файл (слишком короткий)."));
        return;
    }

    let header = parse_header(&data);
    let header = match header {
        Some(h) => h,
        None => {
            println!(
                "{}",
                t!(
                    en: "Not a valid .mex file (bad magic number).",
                    ru: "Не похоже на .mex файл (неверная сигнатура)."
                )
            );
            return;
        }
    };

    if header.version_major != 1 {
        println_t!(
            en: "Unsupported .mex version {}.{} (this kernel supports version 1.x).",
            ru: "Неподдерживаемая версия .mex {}.{} (это ядро поддерживает версию 1.x).";
            header.version_major, header.version_minor
        );
        return;
    }

    // Версии 1.0 и 1.1 — обе поддерживаются.
    // v1.0: только базовые функции (print, read_char, ...)
    // v1.1: + сетевые функции (ping, get_mac, get_ip, arp_resolve)
    if header.version_minor > 1 {
        println_t!(
            en: "Warning: .mex v1.{} — some new API fields may be unavailable.",
            ru: "Предупреждение: .mex v1.{} — некоторые новые поля API могут быть недоступны.";
            header.version_minor
        );
    }

    let header_size = header.header_size as usize;
    if header_size < MEX_HEADER_SIZE_V1 || header_size > data.len() {
        println!("{}", t!(en: "Corrupt .mex header.", ru: "Повреждённый заголовок .mex."));
        return;
    }

    let body_size = header.body_size as usize;
    let bss_size = header.bss_size as usize;
    let total_size = body_size.saturating_add(bss_size);

    if total_size == 0 || total_size > MEX_MAX_SIZE {
        println!(
            "{}",
            t!(
                en: "Program too large or empty (max 4 MiB).",
                ru: "Программа слишком большая или пустая (максимум 4 МиБ)."
            )
        );
        return;
    }

    if header_size + body_size > data.len() {
        println!("{}", t!(en: "Corrupt .mex file (body truncated).", ru: "Повреждённый .mex файл (тело обрезано)."));
        return;
    }

    if (header.entry_offset as usize) >= body_size {
        println!(
            "{}",
            t!(en: "Corrupt .mex file (entry point out of range).", ru: "Повреждённый .mex файл (точка входа вне диапазона).")
        );
        return;
    }

    // Копируем тело программы по фиксированному адресу и обнуляем bss.
    unsafe {
        let dst = MEX_LOAD_ADDR as *mut u8;
        core::ptr::write_bytes(dst, 0, total_size);
        core::ptr::copy_nonoverlapping(
            data[header_size..header_size + body_size].as_ptr(),
            dst,
            body_size,
        );
    }

    let api = build_api();
    let entry_addr = MEX_LOAD_ADDR + header.entry_offset as usize;

    println_t!(
        en: "Running '{}' (v1.{}, entry {:#x}, {} bytes)...",
        ru: "Запускаем '{}' (v1.{}, точка входа {:#x}, {} байт)...";
        name, header.version_minor, entry_addr, body_size
    );

    type EntryFn = extern "C" fn(*const MexApi) -> i64;
    let entry: EntryFn = unsafe { core::mem::transmute(entry_addr) };
    let ret_code = entry(&api as *const MexApi);

    println_t!(
        en: "Program '{}' exited with code {}.",
        ru: "Программа '{}' завершилась с кодом {}.";
        name, ret_code
    );
}

fn parse_header(data: &[u8]) -> Option<MexHeader> {
    if data.len() < MEX_HEADER_SIZE_V1 {
        return None;
    }
    let magic = u32::from_le_bytes(data[0..4].try_into().ok()?);
    if magic != MEX_MAGIC {
        return None;
    }
    Some(MexHeader {
        magic,
        version_major: u16::from_le_bytes(data[4..6].try_into().ok()?),
        version_minor: u16::from_le_bytes(data[6..8].try_into().ok()?),
        header_size: u32::from_le_bytes(data[8..12].try_into().ok()?),
        entry_offset: u32::from_le_bytes(data[12..16].try_into().ok()?),
        body_size: u32::from_le_bytes(data[16..20].try_into().ok()?),
        bss_size: u32::from_le_bytes(data[20..24].try_into().ok()?),
        _reserved: u64::from_le_bytes(data[24..32].try_into().ok()?),
    })
}

/// Проверяет, похож ли файл на .mex по расширению.
pub fn is_mex_filename(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.ends_with(".MEX")
}
