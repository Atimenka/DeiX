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
//! Каждому модулю выделяется уникальная непересекающаяся область памяти (динамический базис от KMOD_LOAD_BASE).
//! Модуль регистрирует свои сервисы через KernelApi.

use crate::ext2;
use crate::{print, println};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const KMOD_MAGIC: u32 = 0x444F_4D4B; // "KMOD" LE
const KMOD_HEADER_SIZE: usize = 44;   // 0x2C
const KMOD_LOAD_BASE: usize = 0x0080_0000; // 8 МиБ — базис динамического выделения регионов модулей

/// Максимальный размер одного модуля.
const KMOD_MAX_SIZE: usize = 2 * 1024 * 1024; // 2 МиБ регион под модуль

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
}

/// Реестр загруженных модулей.
static LOADED_MODULES: crate::spinlock::SpinLock<Vec<LoadedModule>> =
    crate::spinlock::SpinLock::new(Vec::new());

/// Список модулей, которые ядро попытается загрузить при старте (в порядке загрузки).
const BOOT_MODULES: &[&str] = &[
    "NET.KMOD",
    "CRYPTO.KMOD",
    "GFX.KMOD",
];

/// Таблица сервисов ядра, доступных модулям.
#[repr(C)]
pub struct KernelApi {
    pub api_version: u32,
    pub kprint: extern "C" fn(ptr: *const u8, len: usize),
    pub kprintln: extern "C" fn(ptr: *const u8, len: usize),
    pub kalloc: extern "C" fn(size: usize) -> *mut u8,
    pub kfree: extern "C" fn(ptr: *mut u8, size: usize),
    pub kread_file: extern "C" fn(name_ptr: *const u8, name_len: usize, out_ptr: *mut u8, out_cap: usize) -> i64,
    pub kwrite_file: extern "C" fn(name_ptr: *const u8, name_len: usize, data_ptr: *const u8, data_len: usize) -> i64,
    pub kpci_find: extern "C" fn(vendor: u16, device: u16) -> i64,
    pub koutb: extern "C" fn(port: u16, value: u8),
    pub kinb: extern "C" fn(port: u16) -> u8,
    pub koutw: extern "C" fn(port: u16, value: u16),
    pub kinw: extern "C" fn(port: u16) -> u16,
    pub koutl: extern "C" fn(port: u16, value: u32),
    pub kinl: extern "C" fn(port: u16) -> u32,
    pub kuptime_ms: extern "C" fn() -> u64,
    pub ksleep_ms: extern "C" fn(ms: u64),
}

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
            match crate::pci::read_bar0_io(_dev.bus, _dev.slot, _dev.function) {
                Some(io_base) => io_base as i64,
                None => -1,
            }
        }
        None => -1,
    }
}

extern "C" fn kapi_outb(port: u16, value: u8) {
    unsafe { crate::port::outb(port, value) };
}

extern "C" fn kapi_inb(port: u16) -> u8 {
    unsafe { crate::port::inb(port) }
}

extern "C" fn kapi_outw(port: u16, value: u16) {
    unsafe { crate::port::outw(port, value) };
}

extern "C" fn kapi_inw(port: u16) -> u16 {
    unsafe { crate::port::inw(port) }
}

extern "C" fn kapi_outl(port: u16, value: u32) {
    unsafe { crate::port::outl(port, value) };
}

extern "C" fn kapi_inl(port: u16) -> u32 {
    unsafe { crate::port::inl(port) }
}

extern "C" fn kapi_uptime_ms() -> u64 {
    crate::timer::uptime_ms()
}

extern "C" fn kapi_sleep_ms(ms: u64) {
    let t0 = crate::timer::uptime_ms();
    while crate::timer::uptime_ms() - t0 < ms {
        unsafe { core::arch::asm!("hlt") };
    }
}

fn build_kernel_api() -> KernelApi {
    KernelApi {
        api_version: 0x0001_0000,
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

pub fn load_boot_modules() {
    if !ext2::is_formatted() {
        println!("  [module] ext2 not formatted yet — skipping module loading.");
        return;
    }

    for &name in BOOT_MODULES {
        match load_module(name) {
            Ok(()) => {}
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

    // Динамический вызов выделения уникального региона под загружаемый модуль
    let loaded_count = LOADED_MODULES.lock().len();
    let module_load_addr = KMOD_LOAD_BASE + loaded_count * KMOD_MAX_SIZE;
    let load_ptr = module_load_addr as *mut u8;

    unsafe {
        core::ptr::write_bytes(load_ptr, 0, total);
        core::ptr::copy_nonoverlapping(
            data[KMOD_HEADER_SIZE..KMOD_HEADER_SIZE + body_size].as_ptr(),
            load_ptr,
            body_size,
        );
    }

    let name_bytes = &data[0x18..0x18 + core::cmp::min(header.name_len as usize, 28)];
    let name = core::str::from_utf8(name_bytes)
        .unwrap_or("???")
        .trim_end_matches('\0')
        .to_string();

    println!(
        "  [module] Loading {} v{}.{} @ {:#010X} ({} bytes)...",
        name, header.version_major, header.version_minor, module_load_addr, body_size
    );

    let init_addr = module_load_addr + header.init_offset as usize;
    let api = build_kernel_api();

    type KmodInitFn = extern "C" fn(*const KernelApi) -> i64;
    let init_fn: KmodInitFn = unsafe { core::mem::transmute(init_addr) };
    let ret = init_fn(&api as *const KernelApi);

    if ret != 0 {
        let mut modules = LOADED_MODULES.lock();
        modules.push(LoadedModule {
            name: name.clone(),
            version: (header.version_major, header.version_minor),
            load_addr: module_load_addr,
            body_size,
            status: ModuleStatus::Failed,
        });
        return Err(ModuleError::InitFailed(ret));
    }

    let mut modules = LOADED_MODULES.lock();
    modules.push(LoadedModule {
        name: name.clone(),
        version: (header.version_major, header.version_minor),
        load_addr: module_load_addr,
        body_size,
        status: ModuleStatus::Initialized,
    });

    println!("  [module] {} initialized OK @ {:#010X}.", name, module_load_addr);
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

pub fn reset_loaded_modules() {
    *LOADED_MODULES.lock() = alloc::vec::Vec::new();
}

/// Возвращает список текущих загруженных модулей ядра.
pub fn get_loaded_modules() -> Vec<LoadedModule> {
    LOADED_MODULES.lock().clone()
}

/// Регистрирует встроенный модуль ядра (например, GFX.KMOD Compositor).
pub fn register_builtin_module(name: &str, version: (u16, u16), size: usize) {
    let mut modules = LOADED_MODULES.lock();
    for m in modules.iter() {
        if m.name == name {
            return;
        }
    }
    let addr = KMOD_LOAD_BASE + modules.len() * KMOD_MAX_SIZE;
    modules.push(LoadedModule {
        name: name.to_string(),
        version,
        load_addr: addr,
        body_size: size,
        status: ModuleStatus::Initialized,
    });
}
