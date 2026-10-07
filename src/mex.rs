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
//! ## Модель исполнения: Ring 3
//!
//! Программа загружается по фиксированному адресу линковки, получает в RDI
//! указатель на таблицу MexApi и исполняется в пользовательском кольце.
//! Указатели в таблице ведут не в ядро, а в стабы на shim-странице внутри
//! области программы: каждый стаб кладёт номер функции в RAX и выполняет
//! `syscall`. Ядро диспетчеризует номер обратно в реализацию MexApi.
//!
//! Падение программы (page fault, недопустимая инструкция) — авария
//! приложения: процесс завершается, событие регистрируется, ядро
//! продолжает работу.
//!
//! ## Раскладка области MEX (0x0300_0000..0x0380_0000, USER)
//!
//! ```text
//! 0x0300_0000  образ программы + bss   (до 4 МиБ)
//! 0x037B_0000  стек программы          (256 КиБ, растёт вниз от 0x037F_0000)
//! 0x037F_0000  shim: стабы, exit-стаб, таблица MexApi (64 КиБ)
//! ```

use alloc::string::String;

use crate::{ext2, keyboard, println, println_t, t, timer};

const MEX_MAGIC: u32 = 0x3158_454D; // "MEX1" little-endian
const MEX_HEADER_SIZE_V1: usize = 32;

/// Фиксированный физический/виртуальный адрес загрузки тела программы.
/// Выбран выше кучи ядра (0x200000..0x1200000), выше Ring 3 (0x1400000),
/// выше Linux ELF (0x1800000..0x1EC0000) и выше RAM-диска (0x2000000..0x2A00000).
const MEX_LOAD_ADDR: usize = 0x0300_0000;

/// Максимальный размер тела программы + bss.
const MEX_MAX_SIZE: usize = 4 * 1024 * 1024; // 4 МиБ

/// Конец области MEX: образ + стек + shim, четыре 2-МиБ страницы.
const MEX_REGION_END: u64 = 0x0380_0000;

/// Shim-страница: стабы системных вызовов и таблица MexApi.
const MEX_SHIM_BASE: u64 = 0x037F_0000;
/// Смещение exit-стаба внутри shim-страницы.
const SHIM_EXIT_OFF: u64 = 0x200;
/// Смещение таблицы MexApi внутри shim-страницы.
const SHIM_TABLE_OFF: u64 = 0x300;

/// Вершина стека программы — сразу под shim-страницей.
const MEX_STACK_TOP: u64 = MEX_SHIM_BASE;

/// База номеров системных вызовов MEX («MX» в старших байтах).
/// Младшие 16 бит — индекс функции в таблице MexApi.
pub const MEX_SYSCALL_BASE: u64 = 0x4D58_0000;

/// Число функций в таблице MexApi (v1.1).
const MEX_API_COUNT: usize = 10;

// Стабы на shim-странице генерируются по индексам 0..MEX_API_COUNT в порядке
// полей MexApi. Если в структуру добавят поле и забудут поднять счётчик,
// сборка остановится здесь, а не загадочным мусором в Ring 3.
const _: () = assert!(core::mem::size_of::<MexApi>() == MEX_API_COUNT * 8);

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

/// Приводит путь, пришедший от программы, к абсолютному пути VFS.
///
/// Исторически MEX-программы работали с именами файлов в корне ext2-тома.
/// VFS таких путей не понимает — ему нужна точка монтирования, поэтому
/// относительные имена относятся к `/userdata`.
fn resolve_program_path(name: &str) -> alloc::string::String {
    let name = name.trim();
    if name.starts_with('/') {
        return alloc::string::String::from(name);
    }
    alloc::format!("/userdata/{}", name)
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
    // Файловый доступ программы идёт через VFS, а не напрямую в ext2:
    // только так путь попадает в единую точку монтирования и проверки прав.
    // Если программа исполняется как процесс — чтение идёт через её own
    // таблицу дескрипторов, иначе (запуск из CLI) — напрямую через VFS.
    let path = resolve_program_path(name);
    let opened = match crate::process::current_pid() {
        Some(pid) => crate::process::open(pid, &path, false).and_then(|fd| {
            crate::process::read_fd(pid, fd).map(|d| (fd, d))
        }),
        None => Err(crate::process::ProcessError::NotFound(0)),
    };
    let result = match opened {
        Ok((fd, data)) => {
            if let Some(pid) = crate::process::current_pid() {
                let _ = crate::process::close(pid, fd);
            }
            Ok(data)
        }
        Err(_) => crate::vfs::read_file(&path),
    };
    match result {
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

    let path = resolve_program_path(name);
    // Запись идёт через таблицу дескрипторов процесса, если программа
    // исполняется как процесс: только там проверяется возможность записи.
    let written = match crate::process::current_pid() {
        Some(pid) => crate::process::open(pid, &path, true).and_then(|fd| {
            let r = crate::process::write_fd(pid, fd, data);
            let _ = crate::process::close(pid, fd);
            r
        }),
        None => Err(crate::process::ProcessError::NotFound(0)),
    };
    match written {
        Ok(()) => 0,
        Err(_) => match crate::vfs::write_file(&path, data) {
            Ok(()) => 0,
            Err(_) => -1,
        },
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

/// Строит таблицу системных вызовов, передаваемую программе в RDI.
pub fn build_api() -> MexApi {
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

// ==================== Исполнение в Ring 3 ====================

/// Заполняет shim-страницу: стабы, exit-стаб и таблицу MexApi.
///
/// Каждый стаб (16 байт, по одному на функцию API):
///
/// ```text
/// 49 89 ca             mov r10, rcx    ; 4-й аргумент SysV переживает syscall
/// b8 xx xx 58 4d       mov eax, imm32  ; MEX_SYSCALL_BASE + индекс
/// 0f 05                syscall
/// c3                   ret
/// ```
///
/// Exit-стаб — возвратный адрес, который кладётся на стек программы:
/// когда точка входа делает `ret`, управление приходит сюда и код возврата
/// из RAX уходит в syscall 60 (exit).
///
/// Возвращает пользовательский адрес таблицы MexApi.
fn build_shim_page() -> u64 {
    const STUB_STRIDE: usize = 16;

    unsafe {
        let shim = MEX_SHIM_BASE as *mut u8;
        core::ptr::write_bytes(shim, 0, 0x1000);

        for idx in 0..MEX_API_COUNT {
            let nr = (MEX_SYSCALL_BASE as u32) + idx as u32;
            let p = shim.add(idx * STUB_STRIDE);
            // mov r10, rcx
            p.add(0).write(0x49);
            p.add(1).write(0x89);
            p.add(2).write(0xCA);
            // mov eax, imm32
            p.add(3).write(0xB8);
            p.add(4).write((nr & 0xFF) as u8);
            p.add(5).write(((nr >> 8) & 0xFF) as u8);
            p.add(6).write(((nr >> 16) & 0xFF) as u8);
            p.add(7).write(((nr >> 24) & 0xFF) as u8);
            // syscall; ret
            p.add(8).write(0x0F);
            p.add(9).write(0x05);
            p.add(10).write(0xC3);
        }

        // Exit-стаб: mov rdi, rax; mov eax, 60; syscall; jmp $.
        let exit = shim.add(SHIM_EXIT_OFF as usize);
        for (i, b) in [0x48u8, 0x89, 0xC7, 0xB8, 60, 0, 0, 0, 0x0F, 0x05, 0xEB, 0xFE]
            .iter()
            .enumerate()
        {
            exit.add(i).write(*b);
        }

        // Таблица MexApi: указатели на стабы в том же порядке, что поля
        // структуры. Программа получает её адрес в RDI.
        let table = shim.add(SHIM_TABLE_OFF as usize) as *mut u64;
        for idx in 0..MEX_API_COUNT {
            table.add(idx).write(MEX_SHIM_BASE + (idx * STUB_STRIDE) as u64);
        }
    }

    MEX_SHIM_BASE + SHIM_TABLE_OFF
}

/// Запускает загруженную MEX-программу в Ring 3 и возвращает код выхода.
///
/// Предполагает, что образ уже размещён по адресу линковки
/// (`load_into` / `run`). Стек получает адрес exit-стаба как адрес
/// возврата: `ret` из точки входа превращается в syscall exit.
pub fn run_ring3(entry: u64) -> i64 {
    unsafe {
        crate::usermode::make_region_user(MEX_LOAD_ADDR as u64, MEX_REGION_END);
    }
    let api_table = build_shim_page();

    // Адрес возврата на вершине стека. После него RSP % 16 == 8 — ровно
    // то состояние, которое точка входа SysV ожидает после call.
    let stack = MEX_STACK_TOP - 8;
    unsafe {
        (stack as *mut u64).write(MEX_SHIM_BASE + SHIM_EXIT_OFF);
    }

    crate::diag::info(
        crate::diag::NONE,
        &alloc::format!("MEX: переход в Ring 3 (entry {:#x})", entry),
    );

    unsafe { crate::usermode::run_user_program_arg(entry, stack, api_table) }
}

/// Проверяет, что указатель из Ring 3 лежит внутри области MEX.
///
/// Программа не обязана быть корректной: указатель за пределами её области
/// не передаётся реализациям API, а регистрируется как нарушение доступа.
fn user_range_ok(ptr: u64, len: u64) -> bool {
    if ptr == 0 {
        return false;
    }
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    ptr >= MEX_LOAD_ADDR as u64 && end <= MEX_REGION_END
}

/// Регистрирует попытку программы передать ядру чужой указатель.
fn reject_pointer(func: &str, ptr: u64, len: u64) -> u64 {
    crate::diag::warn_with(
        crate::diag::ErrorCode::new(crate::diag::Subsystem::Memory, 9),
        crate::diag::Action::Continue,
        &alloc::format!("MEX {}: указатель {:#x}+{} вне области программы", func, ptr, len),
    );
    (-1i64) as u64
}

/// Диспетчер системных вызовов MEX: индекс функции → реализация ядра.
///
/// Вызывается из `usermode::syscall_handler` для номеров `0x4D58_xxxx`.
/// На время вызова прерывания включаются: блокирующие функции
/// (`read_char`, `ping`) ждут IRQ клавиатуры и сети, а `syscall` входит
/// в ядро с замаскированным IF (SFMASK).
pub fn ring3_dispatch(idx: usize, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    unsafe { core::arch::asm!("sti") };
    let ret = dispatch_inner(idx, a1, a2, a3, a4);
    // Хвост syscall_entry (восстановление стека, swapgs) обязан пройти
    // с выключенными прерываниями; IF программы восстановит sysretq из R11.
    unsafe { core::arch::asm!("cli") };
    ret
}

fn dispatch_inner(idx: usize, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    // Реализации берутся из той же таблицы, что отдавалась программам при
    // исполнении в Ring 0: порядок индексов — порядок полей MexApi.
    let api = build_api();
    match idx {
        // print(ptr, len)
        0 => {
            if !user_range_ok(a1, a2) {
                return reject_pointer("print", a1, a2);
            }
            (api.print)(a1 as *const u8, a2 as usize);
            0
        }
        // read_char() -> u8
        1 => (api.read_char)() as u64,
        // try_read_char() -> i32
        2 => ((api.try_read_char)() as i64) as u64,
        // uptime_ms() -> u64
        3 => (api.uptime_ms)(),
        // read_file(name_ptr, name_len, out_ptr, out_cap) -> i64
        4 => {
            if !user_range_ok(a1, a2) {
                return reject_pointer("read_file(имя)", a1, a2);
            }
            if !user_range_ok(a3, a4) {
                return reject_pointer("read_file(буфер)", a3, a4);
            }
            (api.read_file)(a1 as *const u8, a2 as usize, a3 as *mut u8, a4 as usize) as u64
        }
        // write_file(name_ptr, name_len, data_ptr, data_len) -> i64
        5 => {
            if !user_range_ok(a1, a2) {
                return reject_pointer("write_file(имя)", a1, a2);
            }
            if !user_range_ok(a3, a4) {
                return reject_pointer("write_file(данные)", a3, a4);
            }
            (api.write_file)(a1 as *const u8, a2 as usize, a3 as *const u8, a4 as usize) as u64
        }
        // ping(ip_ptr, ip_len, timeout_ms) -> i64
        6 => {
            if !user_range_ok(a1, a2) {
                return reject_pointer("ping", a1, a2);
            }
            (api.ping)(a1 as *const u8, a2 as usize, a3) as u64
        }
        // get_mac(mac_out) -> i64
        7 => {
            if !user_range_ok(a1, 6) {
                return reject_pointer("get_mac", a1, 6);
            }
            (api.get_mac)(a1 as *mut u8) as u64
        }
        // get_ip(ip_out) -> i64
        8 => {
            if !user_range_ok(a1, 4) {
                return reject_pointer("get_ip", a1, 4);
            }
            (api.get_ip)(a1 as *mut u8) as u64
        }
        // arp_resolve(ip_ptr, ip_len, mac_out) -> i64
        9 => {
            if !user_range_ok(a1, a2) {
                return reject_pointer("arp_resolve(ip)", a1, a2);
            }
            if !user_range_ok(a3, 6) {
                return reject_pointer("arp_resolve(mac)", a3, 6);
            }
            (api.arp_resolve)(a1 as *const u8, a2 as usize, a3 as *mut u8) as u64
        }
        other => {
            crate::diag::warn(
                crate::diag::NONE,
                &alloc::format!("MEX: вызов несуществующей функции API #{}", other),
            );
            (-1i64) as u64
        }
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
    // Сначала каталог установленных пакетов (/userdata/apps), затем
    // корень тома (файлы, записанные пользователем через `write`).
    let apps_path = alloc::format!("/userdata/apps/{}", name);
    let data = match crate::vfs::read_file(&apps_path) {
        Ok(d) => d,
        Err(_) => match ext2::read_file(name) {
            Ok(d) => d,
            Err(ext2::Ext2Error::FileNotFound) => {
                println!(
                    "{}",
                    t!(
                        en: "Program not found (searched /userdata/apps and the volume root).",
                        ru: "Программа не найдена (искали в /userdata/apps и в корне тома)."
                    )
                );
                return;
            }
            Err(_) => {
                println!("{}", t!(en: "Failed to read program file.", ru: "Не удалось прочитать файл программы."));
                return;
            }
        },
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

    let entry_addr = MEX_LOAD_ADDR + header.entry_offset as usize;

    println_t!(
        en: "Running '{}' (v1.{}, entry {:#x}, {} bytes, Ring 3)...",
        ru: "Запускаем '{}' (v1.{}, точка входа {:#x}, {} байт, Ring 3)...";
        name, header.version_minor, entry_addr, body_size
    );

    let ret_code = run_ring3(entry_addr as u64);

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

// ==================== Загрузка в адресное пространство процесса ====================

/// Адрес, по которому линкуются `.mex`-программы (см. tools/mex.ld).
/// Образы формата MEX не позиционно-независимы: тело нельзя разместить
/// по другой базе без перелинковки, поэтому менеджер процессов обязан
/// выделять под MEX именно эту область.
pub fn load_address() -> u64 {
    MEX_LOAD_ADDR as u64
}

/// Проверяет сигнатуру образа MEX.
pub fn is_mex_image(data: &[u8]) -> bool {
    parse_header(data).is_some()
}

/// Размещает образ MEX в памяти и возвращает адрес точки входа.
///
/// `base` обязана совпадать с [`load_address`]: сегменты `.mex`
/// слинкованы на этот адрес, и загрузка по другому сместит все
/// абсолютные ссылки в коде.
pub fn load_into(data: &[u8], base: u64, capacity: u64) -> Result<u64, String> {
    if base != MEX_LOAD_ADDR as u64 {
        return Err(alloc::format!(
            "MEX слинкован на {:#x}, загрузка по {:#x} невозможна (образ не PIC)",
            MEX_LOAD_ADDR,
            base
        ));
    }

    let header = parse_header(data).ok_or_else(|| String::from("неверная сигнатура MEX"))?;
    if header.version_major != 1 {
        return Err(alloc::format!(
            "неподдерживаемая версия MEX {}.{}",
            header.version_major,
            header.version_minor
        ));
    }

    let header_size = header.header_size as usize;
    if header_size < MEX_HEADER_SIZE_V1 || header_size > data.len() {
        return Err(String::from("повреждённый заголовок MEX"));
    }

    let body_size = header.body_size as usize;
    let bss_size = header.bss_size as usize;
    let total_size = body_size.saturating_add(bss_size);

    if total_size == 0 || total_size > MEX_MAX_SIZE {
        return Err(String::from("образ MEX пуст или больше 4 МиБ"));
    }
    if total_size as u64 > capacity {
        return Err(alloc::format!(
            "образ MEX ({} Б) не помещается в адресное пространство ({} Б)",
            total_size,
            capacity
        ));
    }
    if header_size + body_size > data.len() {
        return Err(String::from("тело MEX обрезано"));
    }
    if (header.entry_offset as usize) >= body_size {
        return Err(String::from("точка входа MEX вне тела"));
    }

    unsafe {
        let dst = MEX_LOAD_ADDR as *mut u8;
        core::ptr::write_bytes(dst, 0, total_size);
        core::ptr::copy_nonoverlapping(
            data[header_size..header_size + body_size].as_ptr(),
            dst,
            body_size,
        );
    }

    Ok(MEX_LOAD_ADDR as u64 + header.entry_offset as u64)
}
