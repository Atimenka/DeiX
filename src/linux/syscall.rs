//! Linux syscall ABI (x86-64) — номера и семантика как в настоящем Linux.
//!
//! Номера взяты из `arch/x86/entry/syscalls/syscall_64.tbl`, чтобы
//! бинарники, собранные для Linux, работали без пересборки.
//!
//! Реализовано подмножество, которого хватает статическим программам:
//! вывод, файловые заглушки, память (`brk`/`mmap`), сведения о процессе.
//! Всё остальное возвращает `-ENOSYS`, как это делает Linux для
//! неизвестного вызова, — программа получает честную ошибку, а не
//! молчаливый мусор.

use core::sync::atomic::{AtomicUsize, Ordering};

use super::{USER_AREA_END, USER_HEAP_BASE, USER_HEAP_MAX, USER_IMAGE_BASE};

// --- номера системных вызовов Linux x86-64 ---
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_WRITEV: u64 = 20;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_GETPID: u64 = 39;
pub const SYS_EXIT: u64 = 60;
pub const SYS_UNAME: u64 = 63;
pub const SYS_GETTIMEOFDAY: u64 = 96;
pub const SYS_GETUID: u64 = 102;
pub const SYS_GETGID: u64 = 104;
pub const SYS_GETEUID: u64 = 107;
pub const SYS_GETEGID: u64 = 108;
pub const SYS_ARCH_PRCTL: u64 = 158;
pub const SYS_SET_TID_ADDRESS: u64 = 218;
pub const SYS_CLOCK_GETTIME: u64 = 228;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_SET_ROBUST_LIST: u64 = 273;
pub const SYS_PRLIMIT64: u64 = 302;
pub const SYS_GETRANDOM: u64 = 318;
pub const SYS_RSEQ: u64 = 334;

// --- коды ошибок (возвращаются как отрицательные значения) ---
const EBADF: i64 = -9;
const ENOMEM: i64 = -12;
const EFAULT: i64 = -14;
const EINVAL: i64 = -22;
const ENOSYS: i64 = -38;

/// Счётчик обработанных вызовов — для отчёта после запуска.
static COUNT: AtomicUsize = AtomicUsize::new(0);
/// Текущая граница кучи программы (`brk`).
static BRK: AtomicUsize = AtomicUsize::new(0);
/// Следующий свободный адрес для `mmap`.
static MMAP_NEXT: AtomicUsize = AtomicUsize::new(0);

pub fn reset_stats() {
    COUNT.store(0, Ordering::Relaxed);
    BRK.store(USER_HEAP_BASE as usize, Ordering::Relaxed);
    MMAP_NEXT.store((USER_HEAP_BASE + USER_HEAP_MAX / 2) as usize, Ordering::Relaxed);
}

pub fn count() -> usize {
    COUNT.load(Ordering::Relaxed)
}

/// Проверяет, что буфер целиком лежит в пользовательской области.
///
/// Без этой проверки программа в Ring 3 могла бы передать адрес внутри
/// ядра и заставить его прочитать или напечатать собственную память.
fn user_range_ok(ptr: u64, len: u64) -> bool {
    if len == 0 {
        return true;
    }
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    // Область программ Linux...
    if ptr >= USER_IMAGE_BASE && end <= USER_AREA_END {
        return true;
    }
    // ...либо страница самопроверки Ring 3 (usermode.rs): она пользуется
    // тем же диспетчером системных вызовов.
    let r3 = crate::usermode::user_page_range();
    ptr >= r3.0 && end <= r3.1
}

/// Диспетчер. Возвращает значение, которое попадёт в RAX программы.
pub fn dispatch(nr: u64, a1: u64, a2: u64, a3: u64, _a4: u64, _a5: u64) -> u64 {
    COUNT.fetch_add(1, Ordering::Relaxed);

    let r: i64 = match nr {
        SYS_WRITE => sys_write(a1, a2, a3),
        SYS_WRITEV => sys_writev(a1, a2, a3),
        SYS_READ => 0, // EOF: устройств ввода для программ пока нет
        SYS_CLOSE => 0,
        SYS_BRK => sys_brk(a1),
        SYS_MMAP => sys_mmap(a2),
        SYS_MUNMAP => 0,
        SYS_MPROTECT => 0, // страницы уже RW+USER, менять нечего
        SYS_GETPID => 1,
        SYS_GETUID | SYS_GETEUID | SYS_GETGID | SYS_GETEGID => 0, // root
        SYS_UNAME => sys_uname(a1),
        SYS_CLOCK_GETTIME | SYS_GETTIMEOFDAY => sys_clock_gettime(a2.max(a1)),
        SYS_NANOSLEEP => sys_nanosleep(a1),
        SYS_GETRANDOM => sys_getrandom(a1, a2),
        SYS_FSTAT => EBADF,
        SYS_IOCTL => EINVAL, // не терминал — как Linux для не-tty
        // Вызовы инициализации libc: безвредно подтверждаем.
        SYS_ARCH_PRCTL | SYS_SET_TID_ADDRESS | SYS_SET_ROBUST_LIST | SYS_RSEQ
        | SYS_RT_SIGPROCMASK | SYS_PRLIMIT64 => 0,
        SYS_EXIT | SYS_EXIT_GROUP => {
            // Обрабатывается вызывающим (usermode) — сюда не доходит.
            0
        }
        _ => {
            crate::println!("  [linux] неизвестный syscall {} -> ENOSYS", nr);
            ENOSYS
        }
    };
    r as u64
}

fn sys_write(fd: u64, buf: u64, len: u64) -> i64 {
    if fd != 1 && fd != 2 {
        return EBADF;
    }
    if len > 65536 {
        return EINVAL;
    }
    if !user_range_ok(buf, len) {
        return EFAULT;
    }
    let s = unsafe { core::slice::from_raw_parts(buf as *const u8, len as usize) };
    // Программа вправе печатать произвольные байты; не-UTF-8 заменяем,
    // чтобы не отбрасывать вывод целиком.
    match core::str::from_utf8(s) {
        Ok(t) => {
            crate::print!("{}", t);
            crate::serial_print!("{}", t);
        }
        Err(_) => {
            for &b in s {
                let c = if (0x20..0x7F).contains(&b) || b == b'\n' || b == b'\r' {
                    b as char
                } else {
                    '.'
                };
                crate::print!("{}", c);
                crate::serial_print!("{}", c);
            }
        }
    }
    len as i64
}

/// `writev(fd, iov, iovcnt)` — им пользуется printf в glibc/musl.
fn sys_writev(fd: u64, iov: u64, cnt: u64) -> i64 {
    if cnt > 64 {
        return EINVAL;
    }
    if !user_range_ok(iov, cnt.saturating_mul(16)) {
        return EFAULT;
    }
    let mut total = 0i64;
    for i in 0..cnt {
        let e = iov + i * 16;
        let base = unsafe { core::ptr::read_unaligned(e as *const u64) };
        let len = unsafe { core::ptr::read_unaligned((e + 8) as *const u64) };
        if len == 0 {
            continue;
        }
        let r = sys_write(fd, base, len);
        if r < 0 {
            return r;
        }
        total += r;
    }
    total
}

/// `brk(addr)`: 0 — запросить текущую границу, иначе — установить.
fn sys_brk(addr: u64) -> i64 {
    let cur = BRK.load(Ordering::Relaxed) as u64;
    if addr == 0 {
        return cur as i64;
    }
    if addr < USER_HEAP_BASE || addr > USER_HEAP_BASE + USER_HEAP_MAX {
        // Linux при неудаче возвращает текущую границу, а не ошибку.
        return cur as i64;
    }
    BRK.store(addr as usize, Ordering::Relaxed);
    addr as i64
}

/// Упрощённый `mmap`: только анонимная память, выдаём последовательно.
/// Освобождение не поддерживается — программы такого класса живут
/// недолго, а полноценный VMM это отдельный этап.
fn sys_mmap(len: u64) -> i64 {
    if len == 0 || len > USER_HEAP_MAX {
        return EINVAL;
    }
    let aligned = (len + 0xFFF) & !0xFFF;
    let cur = MMAP_NEXT.load(Ordering::Relaxed) as u64;
    let end = cur + aligned;
    if end > USER_AREA_END {
        return ENOMEM;
    }
    MMAP_NEXT.store(end as usize, Ordering::Relaxed);
    unsafe { core::ptr::write_bytes(cur as *mut u8, 0, aligned as usize) };
    cur as i64
}

/// `uname` — заполняем struct utsname (6 полей по 65 байт).
fn sys_uname(buf: u64) -> i64 {
    const FIELD: usize = 65;
    if !user_range_ok(buf, (FIELD * 6) as u64) {
        return EFAULT;
    }
    let fields = ["DeiX", "deix", "0.2-dev", "DeiX kernel", "x86_64", ""];
    unsafe {
        core::ptr::write_bytes(buf as *mut u8, 0, FIELD * 6);
        for (i, f) in fields.iter().enumerate() {
            let dst = (buf as usize + i * FIELD) as *mut u8;
            let b = f.as_bytes();
            let n = b.len().min(FIELD - 1);
            core::ptr::copy_nonoverlapping(b.as_ptr(), dst, n);
        }
    }
    0
}

/// `clock_gettime(clk, tp)` — секунды и наносекунды из аптайма.
fn sys_clock_gettime(tp: u64) -> i64 {
    if !user_range_ok(tp, 16) {
        return EFAULT;
    }
    let ms = crate::timer::uptime_ms();
    unsafe {
        core::ptr::write_unaligned(tp as *mut u64, ms / 1000);
        core::ptr::write_unaligned((tp + 8) as *mut u64, (ms % 1000) * 1_000_000);
    }
    0
}

/// `nanosleep(req, rem)` — ждём по таймеру ядра.
fn sys_nanosleep(req: u64) -> i64 {
    if !user_range_ok(req, 16) {
        return EFAULT;
    }
    let (sec, nsec) = unsafe {
        (
            core::ptr::read_unaligned(req as *const u64),
            core::ptr::read_unaligned((req + 8) as *const u64),
        )
    };
    let ms = sec.saturating_mul(1000) + nsec / 1_000_000;
    let until = crate::timer::uptime_ms() + ms.min(10_000);
    while crate::timer::uptime_ms() < until {
        unsafe { core::arch::asm!("hlt") };
    }
    0
}

/// `getrandom` — источник энтропии из счётчика тактов.
fn sys_getrandom(buf: u64, len: u64) -> i64 {
    if !user_range_ok(buf, len) {
        return EFAULT;
    }
    let mut state = crate::timer::uptime_ms().wrapping_mul(6364136223846793005).wrapping_add(1);
    for i in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        unsafe { core::ptr::write((buf + i) as *mut u8, (state >> 24) as u8) };
    }
    len as i64
}
