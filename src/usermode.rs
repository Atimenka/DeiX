#![allow(dead_code)]
//! Переход в Ring 3 (пользовательский режим) и системные вызовы.
//! КОД ГОТОВ, НО ТРЕБУЕТ КООПЕРАЦИИ С ЗАГРУЗЧИКОМ ДЛЯ GDT/TSS.
//! Сейчас: только syscall MSR настройка без замены GDT загрузчика.


use crate::spinlock::SpinLock;
use core::arch::asm;

// ==================== GDT (схема, НЕ загружается — загрузчик уже настроил) ====================

pub const KERNEL_CS: u16 = 0x08;
pub const KERNEL_DS: u16 = 0x10;
pub const USER_CS: u16 = 0x1B;
pub const USER_DS: u16 = 0x23;

// ==================== TSS (схема) ====================

#[repr(C, packed)]
pub struct Tss {
    _reserved1: u32,
    pub rsp0: u64,
    pub rsp1: u64,
    pub rsp2: u64,
    _reserved2: [u64; 2],
    pub ist1: u64, pub ist2: u64, pub ist3: u64, pub ist4: u64,
    pub ist5: u64, pub ist6: u64, pub ist7: u64,
    _reserved3: [u64; 2],
    _iopb_offset: u16,
}

static TSS: SpinLock<Tss> = SpinLock::new(Tss {
    _reserved1: 0, rsp0: 0, rsp1: 0, rsp2: 0,
    ist1: 0, ist2: 0, ist3: 0, ist4: 0,
    ist5: 0, ist6: 0, ist7: 0,
    _reserved2: [0; 2], _reserved3: [0; 2], _iopb_offset: 0,
});

pub fn set_kernel_stack(_rsp: u64) { /* TODO: integrate with bootloader TSS */ }

// ==================== Инициализация (БЕЗОПАСНАЯ — не трогает GDT загрузчика) ====================

pub fn init() {
    // НЕ заменяем GDT — загрузчик (stage2.asm) уже настроил 64-битные
    // code/data сегменты. Добавление ring 3 сегментов и TSS требует
    // модификации загрузчика или динамического расширения GDT.
    // Здесь только готовим инфраструктуру syscall.

    // Настраиваем MSR для syscall/sysret.
    unsafe {
        // STAR: kernel CS (32..47), user CS (48..63)
        let star = ((USER_CS as u64 - 16) << 48) | ((KERNEL_CS as u64) << 32);
        asm!("wrmsr", in("ecx") 0xC0000081u32,
             in("edx") (star >> 32) as u32, in("eax") star as u32);

        // LSTAR = адрес syscall_entry
        asm!("lea {tmp}, [rip + syscall_entry_inner]",
             "wrmsr", tmp = out(reg) _,
             in("ecx") 0xC0000082u32);

        // SF_MASK = сбрасываем IF при входе
        asm!("wrmsr", in("ecx") 0xC0000084u32,
             in("edx") 0u32, in("eax") 0x200u32);
    }

    crate::println!("  [usermode] syscall MSRs ready (GDT/TSS from bootloader — ring3 jump needs bootloader cooperation)");
}

// ==================== Syscall entry (ассемблер) ====================

core::arch::global_asm!(
    ".global syscall_entry_inner",
    "syscall_entry_inner:",
    "  swapgs",
    "  mov gs:[0x08], rsp",
    "  mov rsp, gs:[0x10]",
    "  push rcx",
    "  push r11",
    "  push rdi",
    "  push rsi",
    "  push rdx",
    "  push r10",
    "  push r8",
    "  push r9",
    "  mov rdi, rax",
    "  mov rcx, r10",
    "  call {dispatcher}",
    "  pop r9",
    "  pop r8",
    "  pop r10",
    "  pop rdx",
    "  pop rsi",
    "  pop rdi",
    "  pop r11",
    "  pop rcx",
    "  mov rsp, gs:[0x08]",
    "  swapgs",
    "  sysretq",
    dispatcher = sym syscall_dispatcher_rust,
);

// ==================== Таблица syscall ====================

static SYSCALL_TABLE: SpinLock<*const SyscallTable> = SpinLock::new(core::ptr::null());

#[repr(C)]
struct SyscallTable {
    print: extern "C" fn(*const u8, usize) -> i64,
    read_char: extern "C" fn() -> i64,
    uptime: extern "C" fn() -> i64,
    ping: extern "C" fn(*const u8, usize, u64) -> i64,
    get_mac: extern "C" fn(*mut u8) -> i64,
    get_ip: extern "C" fn(*mut u8) -> i64,
    open: extern "C" fn(*const u8, usize) -> i64,
    read: extern "C" fn(i64, *mut u8, usize) -> i64,
    write: extern "C" fn(i64, *const u8, usize) -> i64,
    close: extern "C" fn(i64) -> i64,
}

#[no_mangle]
pub extern "C" fn syscall_dispatcher_rust(
    sysno: u64, a1: u64, a2: u64, a3: u64, _a4: u64, _a5: u64,
) -> u64 {
    let tbl_ptr = *SYSCALL_TABLE.lock();
    if tbl_ptr.is_null() { return !0; }
    let t = unsafe { &*tbl_ptr };

    match sysno {
        0 => { crate::println!("[pid] exit({})", a1 as i64); 0 }
        1 => (t.print)(a1 as *const u8, a2 as usize) as u64,
        2 => (t.read_char)() as u64,
        3 => (t.uptime)() as u64,
        4 => (t.ping)(a1 as *const u8, a2 as usize, a3) as u64,
        5 => (t.get_mac)(a1 as *mut u8) as u64,
        6 => (t.get_ip)(a1 as *mut u8) as u64,
        7 => (t.open)(a1 as *const u8, a2 as usize) as u64,
        8 => (t.read)(a1 as i64, a2 as *mut u8, a3 as usize) as u64,
        9 => (t.write)(a1 as i64, a2 as *const u8, a3 as usize) as u64,
        10 => (t.close)(a1 as i64) as u64,
        _ => !0,
    }
}

// Реальные extern "C" функции вместо замыканий
extern "C" fn sys_print(p: *const u8, l: usize) -> i64 {
    if p.is_null() || l > 4096 { return -1; }
    let s = unsafe { core::slice::from_raw_parts(p, l) };
    if let Ok(t) = core::str::from_utf8(s) { crate::print!("{}", t); }
    l as i64
}
extern "C" fn sys_read_char() -> i64 { crate::keyboard::read_char() as i64 }
extern "C" fn sys_uptime() -> i64 { crate::timer::uptime_ms() as i64 }
extern "C" fn sys_ping(ip: *const u8, len: usize, to: u64) -> i64 { crate::mex::api_ping(ip, len, to) }
extern "C" fn sys_get_mac(b: *mut u8) -> i64 { crate::mex::api_get_mac(b) }
extern "C" fn sys_get_ip(b: *mut u8) -> i64 { crate::mex::api_get_ip(b) }
extern "C" fn sys_open(n: *const u8, l: usize) -> i64 {
    if n.is_null() || l == 0 { return -1; }
    let s = unsafe { core::slice::from_raw_parts(n, l) };
    match crate::ext2::read_file(core::str::from_utf8(s).unwrap_or("")) { Ok(_) => 3, Err(_) => -1 }
}
extern "C" fn sys_read(_fd: i64, _b: *mut u8, _c: usize) -> i64 { -1 }
extern "C" fn sys_write(_fd: i64, _b: *const u8, _c: usize) -> i64 { -1 }
extern "C" fn sys_close(_fd: i64) -> i64 { 0 }

static TABLE_INSTANCE: SyscallTable = SyscallTable {
    print: sys_print, read_char: sys_read_char, uptime: sys_uptime,
    ping: sys_ping, get_mac: sys_get_mac, get_ip: sys_get_ip,
    open: sys_open, read: sys_read, write: sys_write, close: sys_close,
};

pub fn setup_syscall_table() {
    *SYSCALL_TABLE.lock() = &TABLE_INSTANCE as *const SyscallTable;
}

pub struct UserContext {
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub rdi: u64,
}

pub fn jump_to_ring3(_pml4_phys: usize, _ctx: &UserContext) -> ! {
    crate::println!("  [usermode] ring3 jump: needs bootloader GDT with user segments.");
    crate::println!("  Add user CS/DS to stage2.asm GDT and call usermode::init_gdt() first.");
    loop { unsafe { asm!("hlt"); } }
}
