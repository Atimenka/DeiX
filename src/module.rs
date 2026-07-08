#![allow(dead_code)]
//! Модульная система ядра DeiX — загружает и инициализирует `.kmod`
//! (Kernel Module) файлы с ext2-диска при старте системы.
//!
//! ## Архитектура
//!
//! Ядро DeiX разделено на две части:
//!
//! 1. **Core kernel** (вкомпилирован в stage2.bin):
//!    - Память (allocator), прерывания (IDT/PIC/PIT), VGA-текст,
//!      клавиатура/мышь, ext2, ATA, серийный порт, загрузчик модулей.
//!
//! 2. **Загружаемые модули** (`.kmod` файлы на ext2-диске):
//!    - `net.kmod`    — сетевой стек (RTL8139, IPv4, ARP, ICMP)
//!    - `gfx.kmod`    — графическая подсистема (VBE, рендерер, UI)
//!    - `crypto.kmod` — криптография (AES, SHA, шифрование диска)
//!
//! Каждый модуль — это плоский бинарник (как .mex), загружаемый в
//! фиксированную область памяти, с функцией инициализации. Модуль
//! регистрирует свои сервисы через KernelApi.
//!
//! ## Формат .kmod (бинарный, little-endian)
//!
//! Смещение  Размер  Поле
//! 0x00      4       magic        = "KMOD" (0x444F4D4B)
//! 0x04      2       version_major = 1
//! 0x06      2       version_minor = 0
//! 0x08      4       header_size   = 44
//! 0x0C      4       init_offset   (смещение init() от начала тела)
//! 0x10      4       body_size
//! 0x14      4       bss_size
//! 0x18      4       name_len
//! 0x1C      28      name (ASCII, null-terminated, добито нулями)
//! 0x38      ...     body (плоский машинный код + данные)
//!
//! ## API ядра для модулей (KernelApi)
//!
//! Модуль получает указатель на KernelApi через аргумент своей init():
//!
//!   extern "C" fn kmod_init(api: *const KernelApi) -> i64;
//!
//! Возвращает 0 при успехе, отрицательное значение при ошибке.

use crate::ext2;
use alloc::format;
use crate::{print, println, t};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const KMOD_MAGIC: u32 = 0x444F_4D4B; // "KMOD" LE
const KMOD_HEADER_SIZE: usize = 44;   // 0x2C
const KMOD_LOAD_BASE: usize = 0x0080_0000; // 8 МиБ — выше кучи и .mex-области

/// Максимальный размер одного модуля.
const KMOD_MAX_SIZE: usize = 2 * 1024 * 1024; // 2 МиБ

/// Информация о загруженном модуле.
#[derive(Clone)]
pub struct LoadedModule {
    pub name: String,
    pub version: (u16, u16),
    pub load_addr: usize,
    pub body_size: usize,
    pub status: ModuleStatus,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModuleStatus {
    Loaded,
    Initialized,
    Failed,
}

#[repr(C)]
struct KmodHeader {
    magic: u32,
    version_major: u16,
    version_minor: u16,
    header_size: u32,
    init_offset: u32,
    body_size: u32,
    bss_size: u32,
    name_len: u32,
    // name follows inline (28 bytes padded)
}

/// Реестр загруженных модулей.
static LOADED_MODULES: crate::spinlock::SpinLock<Vec<LoadedModule>> =
    crate::spinlock::SpinLock::new(Vec::new());

/// Список модулей, которые ядро попытается загрузить при старте
/// (в порядке загрузки).
const BOOT_MODULES: &[&str] = &[
    "NET.KMOD",
    "CRYPTO.KMOD",
    "GFX.KMOD",
];

/// Таблица сервисов ядра, доступных модулям. Расширяется по мере
/// необходимости — модуль проверяет version_major/minor чтобы понять,
/// какие поля доступны.
#[repr(C)]
pub struct KernelApi {
    /// Версия API ядра (major << 16 | minor).
    pub api_version: u32,

    // --- Базовый ввод/вывод ---
    pub kprint: extern "C" fn(ptr: *const u8, len: usize),
    pub kprintln: extern "C" fn(ptr: *const u8, len: usize),

    // --- Работа с памятью ---
    /// Выделяет `size` байт из кучи ядра, возвращает указатель или null.
    pub kalloc: extern "C" fn(size: usize) -> *mut u8,
    /// Освобождает ранее выделенный блок.
    pub kfree: extern "C" fn(ptr: *mut u8, size: usize),

    // --- Файловая система ---
    pub kread_file: extern "C" fn(name_ptr: *const u8, name_len: usize, out_ptr: *mut u8, out_cap: usize) -> i64,
    pub kwrite_file: extern "C" fn(name_ptr: *const u8, name_len: usize, data_ptr: *const u8, data_len: usize) -> i64,

    // --- Регистрация оборудования ---
    /// Зарегистрировать PCI-устройство (vendor_id, device_id) и получить
    /// его в ответ для дальнейшей работы.
    pub kpci_find: extern "C" fn(vendor: u16, device: u16) -> i64, // возвращает bar0 или -1

    // --- Порты ввода-вывода ---
    pub koutb: extern "C" fn(port: u16, value: u8),
    pub kinb: extern "C" fn(port: u16) -> u8,
    pub koutw: extern "C" fn(port: u16, value: u16),
    pub kinw: extern "C" fn(port: u16) -> u16,
    pub koutl: extern "C" fn(port: u16, value: u32),
    pub kinl: extern "C" fn(port: u16) -> u32,

    // --- Таймер ---
    pub kuptime_ms: extern "C" fn() -> u64,
    pub ksleep_ms: extern "C" fn(ms: u64),
}

// ==================== Реализация KernelApi ====================

extern "C" fn kapi_print(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 || len > 4096 {
        return;
    }
    let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
    if let Ok(s) = core::str::from_utf8(slice) {
        print!("{}", s);
    }
}

extern "C" fn kapi_println(ptr: *const u8, len: usize) {
    kapi_print(ptr, len);
    print!("\n");
}

extern "C" fn kapi_alloc(size: usize) -> *mut u8 {
    if size == 0 || size > 1024 * 1024 {
        return core::ptr::null_mut();
    }
    let mut v: Vec<u8> = Vec::with_capacity(size);
    let ptr = v.as_mut_ptr();
    core::mem::forget(v);
    ptr
}

extern "C" fn kapi_free(ptr: *mut u8, size: usize) {
    if ptr.is_null() || size == 0 {
        return;
    }
    unsafe {
        let _v = Vec::from_raw_parts(ptr, size, size);
        // _v дропается здесь — память возвращается в кучу.
    }
}

extern "C" fn kapi_read_file(name_ptr: *const u8, name_len: usize, out_ptr: *mut u8, out_cap: usize) -> i64 {
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
            unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), out_ptr, data.len()) };
            data.len() as i64
        }
        Err(_) => -1,
    }
}

extern "C" fn kapi_write_file(name_ptr: *const u8, name_len: usize, data_ptr: *const u8, data_len: usize) -> i64 {
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

extern "C" fn kapi_pci_find(vendor: u16, device: u16) -> i64 {
    match crate::pci::find_device(vendor, device) {
        Some(_dev) => {
            // Return BAR0 I/O base for found device
            match crate::pci::read_bar0_io(_dev.bus, _dev.slot, _dev.function) {
                Some(io_base) => io_base as i64,
                None => -1,
            }
        }
        None => -1,
    }
}

extern "C" fn kapi_outb(port: u16, value: u8) {
    unsafe { crate::port::outb(port, value); }
}
extern "C" fn kapi_inb(port: u16) -> u8 {
    unsafe { crate::port::inb(port) }
}
extern "C" fn kapi_outw(port: u16, value: u16) {
    unsafe { crate::port::outw(port, value); }
}
extern "C" fn kapi_inw(port: u16) -> u16 {
    unsafe { crate::port::inw(port) }
}
extern "C" fn kapi_outl(port: u16, value: u32) {
    unsafe { crate::port::outl(port, value); }
}
extern "C" fn kapi_inl(port: u16) -> u32 {
    unsafe { crate::port::inl(port) }
}

extern "C" fn kapi_uptime_ms() -> u64 {
    crate::timer::uptime_ms()
}

extern "C" fn kapi_sleep_ms(ms: u64) {
    let target = crate::timer::uptime_ms() + ms;
    while crate::timer::uptime_ms() < target {
        unsafe { core::arch::asm!("hlt") };
    }
}

fn build_kernel_api() -> KernelApi {
    KernelApi {
        api_version: (1u32 << 16) | 0, // v1.0
        kprint: kapi_print,
        kprintln: kapi_println,
        kalloc: kapi_alloc,
        kfree: kapi_free,
        kread_file: kapi_read_file,
        kwrite_file: kapi_write_file,
        kpci_find: kapi_pci_find,
        koutb: kapi_outb,
        kinb: kapi_inb,
        koutw: kapi_outw,
        kinw: kapi_inw,
        koutl: kapi_outl,
        kinl: kapi_inl,
        kuptime_ms: kapi_uptime_ms,
        ksleep_ms: kapi_sleep_ms,
    }
}

// ==================== Загрузчик модулей ====================

/// Вызывается при старте ядра. Проходит по BOOT_MODULES и пытается
/// загрузить каждый из них с ext2-диска. Модули, которых нет на диске
/// (например, система только что установлена и модули ещё не скопированы)
/// — просто пропускаются с предупреждением.
pub fn load_boot_modules() {
    if !ext2::is_formatted() {
        println!("  [module] ext2 not formatted yet — skipping module loading.");
        return;
    }

    for &name in BOOT_MODULES {
        match load_module(name) {
            Ok(()) => {} // успех уже напечатан в load_module
            Err(ModuleError::NotFound) => {
                println!("  [module] {} not found on disk (skipped — system will work without it).", name);
            }
            Err(ModuleError::BadFormat) => {
                println!("  [module] {} has invalid format — skipped.", name);
            }
            Err(ModuleError::InitFailed(code)) => {
                println!("  [module] {} init() returned error code {}.", name, code);
            }
            Err(ModuleError::TooLarge) => {
                println!("  [module] {} is too large — skipped.", name);
            }
        }
    }
}

#[derive(Debug)]
pub enum ModuleError {
    NotFound,
    BadFormat,
    InitFailed(i64),
    TooLarge,
}

/// Загружает один .kmod-файл с диска и инициализирует его.
fn load_module(filename: &str) -> Result<(), ModuleError> {
    let data = ext2::read_file(filename).map_err(|e| match e {
        ext2::Ext2Error::FileNotFound | ext2::Ext2Error::NotFormatted => ModuleError::NotFound,
        _ => ModuleError::NotFound,
    })?;

    if data.len() < KMOD_HEADER_SIZE {
        return Err(ModuleError::BadFormat);
    }

    let header = parse_kmod_header(&data).ok_or(ModuleError::BadFormat)?;

    let body_size = header.body_size as usize;
    let bss_size = header.bss_size as usize;
    let total = body_size.saturating_add(bss_size);

    if total == 0 || total > KMOD_MAX_SIZE {
        return Err(ModuleError::TooLarge);
    }

    if (KMOD_HEADER_SIZE + body_size) > data.len() {
        return Err(ModuleError::BadFormat);
    }

    if header.init_offset as usize >= body_size {
        return Err(ModuleError::BadFormat);
    }

    // Копируем тело в память модуля.
    let load_addr = KMOD_LOAD_BASE as *mut u8;
    unsafe {
        core::ptr::write_bytes(load_addr, 0, total);
        core::ptr::copy_nonoverlapping(
            data[KMOD_HEADER_SIZE..KMOD_HEADER_SIZE + body_size].as_ptr(),
            load_addr,
            body_size,
        );
    }

    let name_bytes = &data[0x18..0x18 + core::cmp::min(header.name_len as usize, 28)];
    let name = core::str::from_utf8(name_bytes)
        .unwrap_or("???")
        .trim_end_matches('\0')
        .to_string();

    println!(
        "  [module] Loading {} v{}.{} ({} bytes)...",
        name, header.version_major, header.version_minor, body_size
    );

    // Вызываем init()
    let init_addr = KMOD_LOAD_BASE + header.init_offset as usize;
    let api = build_kernel_api();

    type KmodInitFn = extern "C" fn(*const KernelApi) -> i64;
    let init_fn: KmodInitFn = unsafe { core::mem::transmute(init_addr) };
    let ret = init_fn(&api as *const KernelApi);

    if ret != 0 {
        let mut modules = LOADED_MODULES.lock();
        modules.push(LoadedModule {
            name: name.clone(),
            version: (header.version_major, header.version_minor),
            load_addr: KMOD_LOAD_BASE,
            body_size,
            status: ModuleStatus::Failed,
        });
        return Err(ModuleError::InitFailed(ret));
    }

    let mut modules = LOADED_MODULES.lock();
    modules.push(LoadedModule {
        name: name.clone(),
        version: (header.version_major, header.version_minor),
        load_addr: KMOD_LOAD_BASE,
        body_size,
        status: ModuleStatus::Initialized,
    });

    println!("  [module] {} initialized OK.", name);
    Ok(())
}

fn parse_kmod_header(data: &[u8]) -> Option<KmodHeader> {
    if data.len() < KMOD_HEADER_SIZE {
        return None;
    }
    let magic = u32::from_le_bytes(data[0..4].try_into().ok()?);
    if magic != KMOD_MAGIC {
        return None;
    }
    Some(KmodHeader {
        magic,
        version_major: u16::from_le_bytes(data[4..6].try_into().ok()?),
        version_minor: u16::from_le_bytes(data[6..8].try_into().ok()?),
        header_size: u32::from_le_bytes(data[8..12].try_into().ok()?),
        init_offset: u32::from_le_bytes(data[12..16].try_into().ok()?),
        body_size: u32::from_le_bytes(data[16..20].try_into().ok()?),
        bss_size: u32::from_le_bytes(data[20..24].try_into().ok()?),
        name_len: u32::from_le_bytes(data[24..28].try_into().ok()?),
    })
}

/// Возвращает список загруженных модулей (для CLI-команды `modules`).
pub fn list_modules() -> Vec<LoadedModule> {
    LOADED_MODULES.lock().clone()
}

/// Печатает статус загрузки модулей (вызывается CLI-командой `modules`).
pub fn cmd_modules() {
    let modules = list_modules();
    if modules.is_empty() {
        println!(
            "{}",
            t!(
                en: "No kernel modules loaded. (Modules are .kmod files on the ext2 disk.)",
                ru: "Модули ядра не загружены. (Модули — это .kmod файлы на ext2-диске.)"
            )
        );
        return;
    }

    println!("{}", t!(en: "Loaded kernel modules:", ru: "Загруженные модули ядра:"));
    for m in &modules {
        let status_str = match m.status {
            ModuleStatus::Initialized => t!(en: "OK", ru: "OK"),
            ModuleStatus::Loaded => t!(en: "loaded", ru: "загружен"),
            ModuleStatus::Failed => t!(en: "FAILED", ru: "ОШИБКА"),
        };
        println!(
            "  {:<16} v{}.{}  {:<8}  {}",
            m.name,
            m.version.0,
            m.version.1,
            t!(en: format!("{}b", m.body_size), ru: format!("{}б", m.body_size)),
            status_str
        );
    }
}
