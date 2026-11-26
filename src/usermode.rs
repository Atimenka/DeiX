//! RING 3 — НАСТОЯЩИЙ пользовательский режим.
//!
//! В отличие от прежней заглушки (которая печатала «syscall MSRs ready» и
//! ничего не делала), здесь выполняется всё, что физически требуется
//! процессору x86-64 для перехода в CPL=3 и возврата обратно:
//!
//!  1. **GDT** — загрузчик (boot/stage2.asm) создаёт только null + kernel
//!     code. Мы строим полноценную GDT: kernel code/data, user code/data
//!     и 16-байтовый системный дескриптор TSS.
//!  2. **TSS** — при переходе Ring3 -> Ring0 процессор берёт стек ядра из
//!     `TSS.rsp0`. Без загруженного TSS любое прерывание в Ring 3 —
//!     немедленный тройной сбой.
//!  3. **Страницы пользователя** — загрузчик отображает память huge-страницами
//!     2 МиБ БЕЗ бита USER (0b10000011). Ring 3 не может исполнять такой код.
//!     Мы выставляем бит USER (0x4) на конкретной 2-МиБ странице, где лежат
//!     код и стек пользователя.
//!  4. **syscall/sysret** — настройка MSR STAR/LSTAR/SFMASK и обработчик,
//!     который переключает стек на стек ядра. GS base (MSR KERNEL_GS_BASE)
//!     реально инициализируется — в прежней версии `swapgs` читал мусор.
//!
//! Проверяемость: `usermode::selftest()` реально уходит в Ring 3, выполняет
//! оттуда системные вызовы (write/getpid/uptime) и корректно возвращается
//! в ядро через `exit`. Результат виден в serial-логе при загрузке.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, Ordering};

// Scheduler currently runs only on the bootstrap CPU. Any future SMP support
// must enable NXE separately on every CPU before it runs private tasks.
static NX_ENABLED: AtomicBool = AtomicBool::new(false);
pub fn nx_enabled() -> bool { NX_ENABLED.load(Ordering::Acquire) }


// ==================== Селекторы GDT ====================

pub const KERNEL_CS: u16 = 0x08;
pub const KERNEL_DS: u16 = 0x10;
/// Селекторы Ring 3: индекс | RPL=3.
/// Порядок user data перед user code обязателен для sysret.
pub const USER_DS: u16 = 0x18 | 3; // 0x1B
pub const USER_CS: u16 = 0x20 | 3; // 0x23
const TSS_SEL: u16 = 0x28;

// ==================== TSS ====================

/// Task State Segment (x86-64). Нужен ровно ради `rsp0`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Tss {
    reserved0: u32,
    /// Стек для CPL=0 — сюда процессор переключается при входе в ядро.
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

impl Tss {
    const fn new() -> Self {
        Tss {
            reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved1: 0,
            ist: [0; 7],
            reserved2: 0,
            reserved3: 0,
            // Больше размера TSS => карта портов ввода-вывода отсутствует,
            // Ring 3 не имеет доступа к портам (in/out вызовут #GP).
            iomap_base: core::mem::size_of::<Tss>() as u16,
        }
    }
}

static mut TSS: Tss = Tss::new();

/// Стек ядра, на который процессор переключается при syscall/прерывании
/// из Ring 3. Отдельный от основного, чтобы не зависеть от стека Ring 3.
const KSTACK_SIZE: usize = 32 * 1024;
#[repr(align(16))]
struct KStack {
    bytes: [u8; KSTACK_SIZE],
}
static mut KERNEL_STACK: KStack = KStack { bytes: [0; KSTACK_SIZE] };

// ==================== GDT ====================

/// 7 записей: null, kcode, kdata, udata, ucode + TSS (занимает 2 слота).
static mut GDT: [u64; 7] = [0; 7];

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

/// Сегмент кода/данных в long mode: значимы только биты флагов.
const fn segment(code: bool, dpl: u64) -> u64 {
    let mut v = (1 << 44)   // S: сегмент кода/данных
        | (1 << 47)         // P: present
        | (dpl << 45);      // DPL
    if code {
        v |= (1 << 43)      // тип: код
            | (1 << 53);    // L: 64-битный сегмент
    } else {
        v |= 1 << 41; // тип: данные, writable
    }
    v
}

/// Инициализирует GDT (с user-сегментами и TSS), загружает её и TSS.
///
/// # Safety
/// Вызывается один раз на этапе загрузки, до перехода в Ring 3.
unsafe fn init_gdt() {
    let tss_addr = &raw const TSS as u64;
    let tss_limit = (core::mem::size_of::<Tss>() - 1) as u64;

    // Системный дескриптор TSS в long mode занимает 16 байт.
    let tss_low = tss_limit
        | ((tss_addr & 0xFFFF) << 16)
        | (((tss_addr >> 16) & 0xFF) << 32)
        | (0b1001 << 40)  // тип: 64-битный TSS (available)
        | (1 << 47)       // present
        | (((tss_addr >> 24) & 0xFF) << 56);
    let tss_high = (tss_addr >> 32) & 0xFFFF_FFFF;

    let gdt = &mut *(&raw mut GDT);
    let _ = USER_CS;
    gdt[0] = 0;
    gdt[1] = segment(true, 0); // 0x08 kernel code
    gdt[2] = segment(false, 0); // 0x10 kernel data
    gdt[3] = segment(false, 3); // 0x18 user data  (sysret: +0)
    gdt[4] = segment(true, 3); // 0x20 user code  (sysret: +8)
    gdt[5] = tss_low; // 0x28 TSS (низкая половина)
    gdt[6] = tss_high; //      TSS (высокая половина)

    let ptr = DescriptorPointer {
        limit: (core::mem::size_of::<[u64; 7]>() - 1) as u16,
        base: &raw const GDT as u64,
    };

    // Загружаем GDT и перезагружаем сегментные регистры. CS меняем через
    // far return: в long mode нет прямого `mov cs`.
    asm!(
        "lgdt [{ptr}]",
        "push {kcs}",
        "lea {tmp}, [rip + 2f]",
        "push {tmp}",
        "retfq",
        "2:",
        "mov ds, {kds:x}",
        "mov es, {kds:x}",
        "mov ss, {kds:x}",
        "mov fs, {kds:x}",
        "mov gs, {kds:x}",
        ptr = in(reg) &ptr,
        kcs = const KERNEL_CS as u64,
        kds = in(reg) KERNEL_DS,
        tmp = out(reg) _,
        options(preserves_flags)
    );

    // Загружаем TSS (после того как дескриптор оказался в GDT).
    asm!("ltr {0:x}", in(reg) TSS_SEL, options(nomem, nostack, preserves_flags));
}

// ==================== Страницы пользователя ====================

/// Выставляет бит USER на 2-МиБ huge-странице, содержащей `addr`.
///
/// Загрузчик отобразил первый гигабайт huge-страницами с флагами
/// `present|writable|huge`, но БЕЗ `user`. Чтобы Ring 3 мог исполнять код
/// и пользоваться стеком, бит USER нужен на всех уровнях таблиц: PML4,
/// PDPT и PD.
///
/// # Safety
/// Меняет активные таблицы страниц.
unsafe fn make_user_accessible(addr: u64) {
    const PRESENT: u64 = 1 << 0;
    const USER: u64 = 1 << 2;
    const HUGE: u64 = 1 << 7;
    const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    let cr3: u64;
    asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));

    let pml4 = (cr3 & ADDR_MASK) as *mut u64;
    let i4 = ((addr >> 39) & 0x1FF) as usize;
    if *pml4.add(i4) & PRESENT == 0 {
        return;
    }
    *pml4.add(i4) |= USER;

    let pdpt = (*pml4.add(i4) & ADDR_MASK) as *mut u64;
    let i3 = ((addr >> 30) & 0x1FF) as usize;
    if *pdpt.add(i3) & PRESENT == 0 {
        return;
    }
    *pdpt.add(i3) |= USER;

    let pd = (*pdpt.add(i3) & ADDR_MASK) as *mut u64;
    let i2 = ((addr >> 21) & 0x1FF) as usize;
    let e2 = *pd.add(i2);
    if e2 & PRESENT == 0 {
        return;
    }
    *pd.add(i2) = e2 | USER;

    // Если это не huge-страница, нужен ещё уровень PT.
    if e2 & HUGE == 0 {
        let pt = (e2 & ADDR_MASK) as *mut u64;
        let i1 = ((addr >> 12) & 0x1FF) as usize;
        if *pt.add(i1) & PRESENT != 0 {
            *pt.add(i1) |= USER;
        }
    }

    // Сбрасываем TLB для этого адреса.
    asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags));
}

/// Делает диапазон адресов доступным из Ring 3.
///
/// Открыта для загрузчика MEX: его образ, стек и shim-страница лежат в
/// фиксированной области, которую загрузчик отобразил без бита USER.
/// После правки битов TLB сбрасывается перезагрузкой CR3 — иначе процессор
/// продолжит использовать закешированные записи без USER и Ring 3 получит
/// #PF на первой же инструкции.
///
/// # Safety
/// Меняет активные таблицы страниц. Диапазон обязан быть уже отображён.
pub unsafe fn make_region_user(start: u64, end: u64) {
    const HUGE_PAGE: u64 = 2 * 1024 * 1024;
    let mut addr = start & !(HUGE_PAGE - 1);
    while addr < end {
        make_user_accessible(addr);
        addr += HUGE_PAGE;
    }
    let cr3: u64;
    asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags));
}

// ==================== Код пользователя ====================

// Программа, исполняемая в Ring 3. Пишется на ассемблере и копируется
// в страницу с битом USER: брать адрес Rust-функции нельзя — её код
// лежит в страницах ядра без USER.
//
// Делает три системных вызова и завершается:
//   write(1, msg, len) -> getpid() -> uptime() -> exit(0)
global_asm!(
    ".section .rodata",
    ".global user_program_start",
    ".global user_program_end",
    "user_program_start:",
    // write(fd=1, buf=rsi, len=rdx); адрес строки вычисляем от rip,
    // чтобы код был позиционно-независимым.
    "  mov rax, 1",
    "  mov rdi, 1",
    "  lea rsi, [rip + 9f]",
    "  mov rdx, 50",          // длина строки ниже (чистый ASCII)
    "  syscall",
    // getpid()
    "  mov rax, 39",
    "  syscall",
    // Доказательство кольца: syscall 1000 печатает CS (младшие 2 бита = CPL).
    "  mov rax, 1000",
    "  mov rdi, cs",
    "  syscall",
    // uptime_ms()
    "  mov rax, 40",
    "  syscall",
    // exit(0)
    "  mov rax, 60",
    "  xor rdi, rdi",
    "  syscall",
    // Если exit не сработал — не даём процессору уехать в мусор.
    "8: hlt",
    "  jmp 8b",
    "9:",
    ".ascii \"  [ring3] Hello from Ring 3: syscall write works!\\n\"",
    "user_program_end:",
    ".section .text",           // вернуть секцию по умолчанию
);

extern "C" {
    static user_program_start: u8;
    static user_program_end: u8;
    /// Точка входа syscall (определена в global_asm! ниже).
    fn syscall_entry();
}

/// Адрес, куда копируется программа пользователя, и стек Ring 3.
/// Берём вторую 2-МиБ huge-страницу (2..4 МиБ): она отображена
/// загрузчиком и не пересекается с кодом ядра (ядро < 1 МиБ + heap).
/// ВАЖНО: 0x300000 (3 МиБ) занимать НЕЛЬЗЯ — там начинается `.bss` ядра
/// (см. boot/linker_kernel.ld), включая 16-МиБ `HEAP_STORAGE`. Секция
/// тянется до ~0x1360000, поэтому пользовательскую страницу кладём на
/// 20 МиБ — выше кучи, но внутри первого гигабайта, отображённого
/// загрузчиком. Раньше здесь стояло 0x200000, и Ring 3 писал свой код и
/// стек прямо поверх кучи ядра.
const USER_BASE: u64 = 0x0140_0000; // 20 МиБ
const USER_CODE: u64 = USER_BASE + 0x1000;
const USER_STACK_TOP: u64 = USER_BASE + 0x8000;

// ==================== Обработчик syscall ====================

// Точка входа `syscall`. Процессор сюда прыгает из Ring 3, сохранив
// RIP в RCX и RFLAGS в R11. Стек всё ещё пользовательский — переключаем
// его на стек ядра, иначе Ring 3 мог бы подсунуть ядру любой адрес.
global_asm!(
    ".section .text",           // код syscall обязан быть исполняемым
    ".global syscall_entry",
    "syscall_entry:",
    "  swapgs",                  // GS -> KERNEL_GS_BASE (стек ядра)
    "  mov gs:[8], rsp",         // сохранить стек пользователя
    "  mov rsp, gs:[0]",         // взять стек ядра
    "  push rcx",                // RIP возврата
    "  push r11",                // RFLAGS возврата
    "  push rbx",
    "  push rbp",
    "  push r12",
    "  push r13",
    "  push r14",
    "  push r15",
    // x86-64 syscall ABI preserves argument registers (except RCX/R11).
    // The Rust handler may freely clobber caller-saved registers, so save
    // all six here before rearranging syscall arguments for the C ABI.
    "  push rdi",
    "  push rsi",
    "  push rdx",
    "  push r8",
    "  push r9",
    "  push r10",
    // Перекладываем регистры соглашения syscall в соглашение System V C:
    //   syscall: nr=rax, a1=rdi, a2=rsi, a3=rdx, a4=r10, a5=r8
    //            (4-й аргумент приходит в r10: rcx затирается самой
    //             инструкцией syscall, и библиотечные стабы кладут его в r10)
    //   C ABI:   arg1=rdi, arg2=rsi, arg3=rdx, arg4=rcx, arg5=r8, arg6=r9
    // Порядок важен, иначе значения затрут друг друга.
    "  mov r9, r8",
    "  mov r8, r10",
    "  mov rcx, rdx",
    "  mov rdx, rsi",
    "  mov rsi, rdi",
    "  mov rdi, rax",
    "  call {handler}",
    "  pop r10",
    "  pop r9",
    "  pop r8",
    "  pop rdx",
    "  pop rsi",
    "  pop rdi",
    "  pop r15",
    "  pop r14",
    "  pop r13",
    "  pop r12",
    "  pop rbp",
    "  pop rbx",
    "  pop r11",
    "  pop rcx",
    "  mov rsp, gs:[8]",         // вернуть стек пользователя
    "  swapgs",
    "  sysretq",
    handler = sym syscall_handler,
);

/// Область, на которую указывает GS в режиме ядра.
#[repr(C)]
struct CpuLocal {
    kernel_rsp: u64, // gs:[0]
    user_rsp: u64,   // gs:[8]
}
static mut CPU_LOCAL: CpuLocal = CpuLocal {
    kernel_rsp: 0,
    user_rsp: 0,
};

/// Scheduler calls this with interrupts disabled before resuming a task.
/// Zero restores the legacy shared stack for existing synchronous programs.
pub unsafe fn set_task_kernel_stack(stack_top: u64) {
    let top = if stack_top == 0 {
        (core::ptr::addr_of!(KERNEL_STACK.bytes) as u64) + KSTACK_SIZE as u64
    } else { stack_top };
    (*(&raw mut TSS)).rsp0 = top;
    (*(&raw mut CPU_LOCAL)).kernel_rsp = top;
}

/// Признак того, что Ring 3 завершился через `exit`.
static mut USER_EXITED: bool = false;
/// Куда вернуться из `exit` (стек ядра на момент входа в Ring 3).
static mut RETURN_RSP: u64 = 0;
/// Код возврата последней запущенной программы.
static mut EXIT_CODE: i64 = 0;

/// Исполняется ли сейчас программа в Ring 3 (между `enter_ring3` и выходом).
pub fn user_program_active() -> bool {
    unsafe { core::ptr::read_volatile(&raw const RETURN_RSP) != 0 }
}

/// Аварийно завершает программу Ring 3 из обработчика исключения.
///
/// Делает то же, что syscall `exit`: восстанавливает стек ядра, сохранённый
/// `enter_ring3`, и возвращает управление коду, запустившему программу.
/// Кадр исключения при этом отбрасывается — возвращаться в упавшую
/// инструкцию бессмысленно.
///
/// # Safety
/// Вызывать можно только когда `user_program_active()` — иначе RETURN_RSP
/// не указывает на живой стек.
pub unsafe fn abort_user_program(code: i64) -> ! {
    EXIT_CODE = code;
    USER_EXITED = true;
    asm!("mov rsp, {rsp}", "jmp {rip}",
        rsp = in(reg) RETURN_RSP,
        rip = in(reg) (ring3_return as *const () as u64),
        options(noreturn));
}

/// Диспетчер системных вызовов. Номера — как в Linux x86-64, чтобы
/// поведение было предсказуемым: 1=write, 39=getpid, 60=exit, 40=uptime.
/// Диапазон 0x4D58_xxxx («MX») зарезервирован под MEX API: его вызывают
/// стабы, которые ядро кладёт в shim-страницу MEX-программы.
extern "C" fn syscall_handler(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> u64 {
    if crate::sched::current_is_private_user() {
        // This isolated task interface is deliberately narrow until a proper
        // capability-checked process ABI exists. Never fall through to MEX or
        // Linux handlers that assume the legacy shared user mapping.
        if nr == 0x4458_564d && option_env!("DEIX_TEST_ADDRESS_SPACE") == Some("1") {
            crate::sched::record_ring3_test(a1, a2);
            crate::sched::finish_private_user(0);
        }
        if nr == 60 || nr == 231 { crate::sched::finish_private_user(a1 as i64); }
        // Version 1 process mailbox ABI. Sender identity is derived from the
        // current scheduler slot, never accepted from userspace.
        match nr {
            0x4458_0001 => { // send(destination, tag, word0, word1)
                let Ok(target) = u32::try_from(a1) else { return (-22i64) as u64; };
                let Some(sender) = crate::process::ipc_sender_and_target(target) else { return (-1i64) as u64; };
                let msg = crate::process_ipc::Message { sender, receiver: target, tag: a2, a: a3, b: a4 };
                return if crate::process_ipc::send(msg) { 0 } else { (-11i64) as u64 };
            }
            0x4458_0002 => { // receive(out: *mut 32 bytes)
                let Some(receiver) = crate::process::ipc_receiver() else { return (-1i64) as u64; };
                if !crate::sched::current_user_range_writable(a1, 32) { return (-14i64) as u64; }
                let Some(msg) = crate::process_ipc::receive(receiver) else { return (-11i64) as u64; };
                let mut out = [0u8; 32];
                out[..4].copy_from_slice(&msg.sender.to_le_bytes());
                out[8..16].copy_from_slice(&msg.tag.to_le_bytes());
                out[16..24].copy_from_slice(&msg.a.to_le_bytes());
                out[24..32].copy_from_slice(&msg.b.to_le_bytes());
                return if crate::sched::current_copy_to_user(a1, &out) { 0 } else { (-14i64) as u64 };
            }
            0x4458_0003 => return crate::process::current_pid().map_or((-1i64) as u64, |pid| pid as u64),
            0x4458_0005 => { // heartbeat; identity and time are kernel-derived
                return if crate::process::heartbeat_current() { 0 } else { (-1i64) as u64 };
            }
            0x4458_000e => { // asynchronous log submission (message ptr, len)
                if !crate::process::current_has_capability(crate::process::cap::WRITE_LOG) { return (-1i64) as u64; }
                let Some(pid) = crate::process::current_pid() else { return (-1i64) as u64; };
                let Ok(len) = usize::try_from(a2) else { return (-22i64) as u64; };
                if len == 0 || len > crate::diag::record::MESSAGE_LEN { return (-22i64) as u64; }
                let mut text = [0u8; crate::diag::record::MESSAGE_LEN];
                if !crate::sched::current_copy_from_user(a1, &mut text[..len]) { return (-14i64) as u64; }
                return crate::service_rpc::submit_log(pid, &text[..len]).map_or_else(|e| e as i64 as u64, |token| token);
            }
            0x4458_000d => { // async reload boot config from disk
                if !crate::process::current_has_capability(crate::process::cap::MANAGE_SERVICES | crate::process::cap::READ_BOOT_CONFIG) { return (-1i64) as u64; }
                let Some(pid) = crate::process::current_pid() else { return (-1i64) as u64; };
                return crate::service_rpc::submit_config_reload(pid).map_or_else(|e| e as i64 as u64, |token| token);
            }
            0x4458_000c => return crate::process::current_parent().map_or((-1i64) as u64, |pid| pid as u64),
            0x4458_000b => { // async start with explicit delegated caps in a5
                if !crate::process::current_has_capability(crate::process::cap::MANAGE_SERVICES) { return (-1i64) as u64; }
                let Some(pid) = crate::process::current_pid() else { return (-1i64) as u64; };
                if !crate::process::pid_can_delegate(pid, a5) { return (-1i64) as u64; }
                let Ok(name_len) = usize::try_from(a2) else { return (-22i64) as u64; };
                let Ok(path_len) = usize::try_from(a4) else { return (-22i64) as u64; };
                if name_len == 0 || name_len > crate::service_rpc::MAX_NAME || path_len == 0 || path_len > crate::service_rpc::MAX_PATH { return (-22i64) as u64; }
                let mut name = [0u8; crate::service_rpc::MAX_NAME];
                let mut path = [0u8; crate::service_rpc::MAX_PATH];
                if !crate::sched::current_copy_from_user(a1, &mut name[..name_len]) || !crate::sched::current_copy_from_user(a3, &mut path[..path_len]) { return (-14i64) as u64; }
                return crate::service_rpc::submit_start_delegated(pid, &name[..name_len], &path[..path_len], a5).map_or_else(|e| e as i64 as u64, |token| token);
            }
            0x4458_0008 => { // async start(name ptr, len, path ptr, len)
                if !crate::process::current_has_capability(crate::process::cap::MANAGE_SERVICES) { return (-1i64) as u64; }
                let Some(pid) = crate::process::current_pid() else { return (-1i64) as u64; };
                let Ok(name_len) = usize::try_from(a2) else { return (-22i64) as u64; };
                let Ok(path_len) = usize::try_from(a4) else { return (-22i64) as u64; };
                if name_len == 0 || name_len > crate::service_rpc::MAX_NAME || path_len == 0 || path_len > crate::service_rpc::MAX_PATH { return (-22i64) as u64; }
                let mut name = [0u8; crate::service_rpc::MAX_NAME];
                let mut path = [0u8; crate::service_rpc::MAX_PATH];
                if !crate::sched::current_copy_from_user(a1, &mut name[..name_len]) || !crate::sched::current_copy_from_user(a3, &mut path[..path_len]) { return (-14i64) as u64; }
                return crate::service_rpc::submit_start(pid, &name[..name_len], &path[..path_len]).map_or_else(|e| e as i64 as u64, |token| token);
            }
            0x4458_0009 => { // async stop(name ptr, len, expected PID)
                if !crate::process::current_has_capability(crate::process::cap::MANAGE_SERVICES) { return (-1i64) as u64; }
                let Some(pid) = crate::process::current_pid() else { return (-1i64) as u64; };
                let Ok(name_len) = usize::try_from(a2) else { return (-22i64) as u64; };
                let Ok(target) = u32::try_from(a3) else { return (-22i64) as u64; };
                if name_len == 0 || name_len > crate::service_rpc::MAX_NAME { return (-22i64) as u64; }
                let mut name = [0u8; crate::service_rpc::MAX_NAME];
                if !crate::sched::current_copy_from_user(a1, &mut name[..name_len]) { return (-14i64) as u64; }
                return crate::service_rpc::submit_stop(pid, &name[..name_len], target).map_or_else(|e| e as i64 as u64, |token| token);
            }
            0x4458_000a => { // poll(token, output pointer)
                if !crate::process::current_has_capability(crate::process::cap::MANAGE_SERVICES)
                    && !crate::process::current_has_capability(crate::process::cap::WRITE_LOG) { return (-1i64) as u64; }
                let Some(pid) = crate::process::current_pid() else { return (-1i64) as u64; };
                if !crate::sched::current_user_range_writable(a2, 8) { return (-14i64) as u64; }
                let (result, result_pid) = match crate::service_rpc::take(pid, a1) { Ok(v) => v, Err(e) => return e as i64 as u64 };
                let mut out = [0u8; 8];
                out[..4].copy_from_slice(&result.to_le_bytes()); out[4..].copy_from_slice(&result_pid.to_le_bytes());
                return if crate::sched::current_copy_to_user(a2, &out) { 0 } else { (-14i64) as u64 };
            }
            0x4458_0007 => { // process status (pid, output pointer)
                if !crate::process::current_has_capability(crate::process::cap::MANAGE_SERVICES) { return (-1i64) as u64; }
                let Ok(pid) = u32::try_from(a1) else { return (-22i64) as u64; };
                let Some(snapshot) = crate::process::status_snapshot(pid) else { return (-2i64) as u64; };
                let state = match snapshot.state {
                    crate::process::ProcessState::Created => 1u32,
                    crate::process::ProcessState::Running => 2,
                    crate::process::ProcessState::Zombie => 3,
                    _ => return (-2i64) as u64,
                };
                let mut out = [0u8; 16];
                out[..4].copy_from_slice(&snapshot.pid.to_le_bytes());
                out[4..8].copy_from_slice(&state.to_le_bytes());
                out[8..12].copy_from_slice(&snapshot.exit_code.unwrap_or(0).to_le_bytes());
                out[12..16].copy_from_slice(&(snapshot.exit_code.is_some() as u32).to_le_bytes());
                return if crate::sched::current_copy_to_user(a2, &out) { 0 } else { (-14i64) as u64 };
            }
            0x4458_0014 => { // reap exact managed zombie (name, len, pid, out)
                if !crate::process::current_has_capability(crate::process::cap::MANAGE_SERVICES) { return (-1i64) as u64; }
                let Ok(len) = usize::try_from(a2) else { return (-22i64) as u64; };
                let Ok(pid) = u32::try_from(a3) else { return (-22i64) as u64; };
                if len == 0 || len > crate::service_rpc::MAX_NAME { return (-22i64) as u64; }
                let mut name = [0u8; crate::service_rpc::MAX_NAME];
                if !crate::sched::current_copy_from_user(a1, &mut name[..len]) || !crate::sched::current_user_range_writable(a4, 4) { return (-14i64) as u64; }
                let Ok(name) = core::str::from_utf8(&name[..len]) else { return (-22i64) as u64; };
                let code = match crate::process::reap_managed(name, pid) { Ok(code) => code, Err(e) => return e as i64 as u64 };
                return if crate::sched::current_copy_to_user(a4, &code.to_le_bytes()) { 0 } else { (-14i64) as u64 };
            }
            0x4458_0015 => { // enumerate managed zombies (after PID, out)
                if !crate::process::current_has_capability(crate::process::cap::MANAGE_SERVICES) { return (-1i64) as u64; }
                let Ok(after) = u32::try_from(a1) else { return (-22i64) as u64; };
                let Some((pid, code, name, len)) = crate::process::next_managed_zombie(after) else { return (-2i64) as u64; };
                let mut out = [0u8; 144];
                out[..4].copy_from_slice(&pid.to_le_bytes());
                out[4..8].copy_from_slice(&(len as u32).to_le_bytes());
                out[8..12].copy_from_slice(&code.to_le_bytes());
                out[16..].copy_from_slice(&name);
                return if crate::sched::current_copy_to_user(a2, &out) { 0 } else { (-14i64) as u64 };
            }
            0x4458_0013 => { // authenticated local session snapshot
                if !crate::process::current_has_capability(crate::process::cap::READ_SESSION) { return (-1i64) as u64; }
                let snapshot = match crate::session_snapshot::snapshot() { Ok(v) => v, Err(e) => return e as i64 as u64 };
                return if crate::sched::current_copy_to_user(a1, &snapshot) { 0 } else { (-14i64) as u64 };
            }
            0x4458_0011 => { // boot config generation
                if !crate::process::current_has_capability(crate::process::cap::READ_BOOT_CONFIG) { return (-1i64) as u64; }
                return crate::boot_config::generation() as u64;
            }
            0x4458_0012 => { // read config with expected generation
                if !crate::process::current_has_capability(crate::process::cap::READ_BOOT_CONFIG) { return (-1i64) as u64; }
                return crate::boot_config::read_generation_to_user(a1, a2, a3, a4) as u64;
            }
            0x4458_0010 => { // monotonic uptime in milliseconds
                if crate::process::current_pid().is_none() { return (-1i64) as u64; }
                return crate::timer::uptime_ms().min(i64::MAX as u64);
            }
            0x4458_000f => { // managed identity snapshot (name, len, out)
                if !crate::process::current_has_capability(crate::process::cap::MANAGE_SERVICES) { return (-1i64) as u64; }
                let Ok(len) = usize::try_from(a2) else { return (-22i64) as u64; };
                if len == 0 || len > crate::service_rpc::MAX_NAME { return (-22i64) as u64; }
                let mut name = [0u8; crate::service_rpc::MAX_NAME];
                if !crate::sched::current_copy_from_user(a1, &mut name[..len]) { return (-14i64) as u64; }
                let Ok(name) = core::str::from_utf8(&name[..len]) else { return (-22i64) as u64; };
                let snapshot = match crate::process::managed_snapshot(name) {
                    Ok(Some(value)) => value, Ok(None) => return (-2i64) as u64,
                    Err(()) => return (-75i64) as u64,
                };
                let state = match snapshot.state {
                    crate::process::ProcessState::Created => 1u32,
                    crate::process::ProcessState::Running => 2,
                    _ => return (-2i64) as u64,
                };
                let mut out = [0u8; 280];
                out[0..4].copy_from_slice(&snapshot.pid.to_le_bytes());
                out[4..8].copy_from_slice(&state.to_le_bytes());
                out[8..12].copy_from_slice(&snapshot.uid.to_le_bytes());
                out[12..16].copy_from_slice(&(snapshot.path_len as u32).to_le_bytes());
                out[16..24].copy_from_slice(&snapshot.caps.to_le_bytes());
                out[24..].copy_from_slice(&snapshot.path);
                return if crate::sched::current_copy_to_user(a3, &out) { 0 } else { (-14i64) as u64 };
            }
            0x4458_0006 => { // find managed service (utf8 pointer, byte length)
                if !crate::process::current_has_capability(crate::process::cap::MANAGE_SERVICES) { return (-1i64) as u64; }
                let Ok(len) = usize::try_from(a2) else { return (-22i64) as u64; };
                if len == 0 || len > 128 { return (-22i64) as u64; }
                let mut name = [0u8; 128];
                if !crate::sched::current_copy_from_user(a1, &mut name[..len]) { return (-14i64) as u64; }
                let Ok(name) = core::str::from_utf8(&name[..len]) else { return (-22i64) as u64; };
                return crate::process::managed_pid(name).map_or((-2i64) as u64, |pid| pid as u64);
            }
            0x4458_0004 => { // read boot config (out, offset, length)
                if !crate::process::current_has_capability(crate::process::cap::READ_BOOT_CONFIG) { return (-1i64) as u64; }
                return crate::boot_config::read_to_user(a1, a2, a3) as u64;
            }
            _ => {}
        }
        return (-38i64) as u64; // ENOSYS
    }
    match nr {
        1000 => {
            crate::println!(
                "  [ring3] CS={:#x} -> CPL={} (3 = пользовательское кольцо)", a1, a1 & 3);
            0
        }
        60 | 231 => {
            crate::println!("  [ring3] процесс завершился с кодом {}", a1 as i64);
            unsafe {
                EXIT_CODE = a1 as i64;
                USER_EXITED = true;
                asm!("mov rsp, {rsp}", "jmp {rip}",
                    rsp = in(reg) RETURN_RSP,
                    rip = in(reg) (ring3_return as *const () as u64),
                    options(noreturn));
            }
        }
        n if n & 0xFFFF_0000 == crate::mex::MEX_SYSCALL_BASE => {
            crate::mex::ring3_dispatch((n & 0xFFFF) as usize, a1, a2, a3, a4)
        }
        // Остальное — прослойка Linux (src/linux/syscall.rs).
        _ => crate::linux::syscall::dispatch(nr, a1, a2, a3, a4, a5),
    }
}

// ==================== Инициализация ====================

/// Настраивает GDT, TSS, MSR для syscall и делает страницу пользователя
/// доступной из Ring 3.
pub fn init() {
    unsafe {
        init_gdt();

        // Стек ядра для входов из Ring 3.
        let kstack_top = (core::ptr::addr_of!(KERNEL_STACK.bytes) as u64) + KSTACK_SIZE as u64;
        (*(&raw mut TSS)).rsp0 = kstack_top;
        (*(&raw mut CPU_LOCAL)).kernel_rsp = kstack_top;

        // GS_BASE (ядро) и KERNEL_GS_BASE — swapgs меняет их местами.
        // Прежняя версия делала swapgs, не задав эти MSR: чтение gs:[…]
        // било в нулевой адрес.
        let cpu_local = &raw const CPU_LOCAL as u64;
        wrmsr(0xC000_0101, cpu_local); // MSR_GS_BASE
        wrmsr(0xC000_0102, cpu_local); // MSR_KERNEL_GS_BASE

        // STAR: [47:32] = kernel CS, [63:48] = базовый селектор для sysret.
        // sysret берёт CS = base+16, SS = base+8, поэтому base = USER_DS-8.
        let star = ((KERNEL_CS as u64) << 32) | (((USER_DS - 8) as u64) << 48);
        wrmsr(0xC000_0081, star);
        wrmsr(0xC000_0082, syscall_entry as *const () as u64); // LSTAR
        wrmsr(0xC000_0084, 0x0000_0300); // SFMASK: гасим IF и TF

        // NX requires extended CPUID leaf 0x80000001 EDX bit 20. CPUs
        // without it continue booting, but private ELF execution fails closed.
        let max_ext = core::arch::x86_64::__cpuid(0x8000_0000).eax;
        let nx = max_ext >= 0x8000_0001 && (core::arch::x86_64::__cpuid(0x8000_0001).edx & (1 << 20) != 0);
        // EFER.SCE enables syscall; NXE makes leaf bit 63 non-executable.
        let efer = rdmsr(0xC000_0080);
        wrmsr(0xC000_0080, efer | 1 | if nx { 1 << 11 } else { 0 });
        NX_ENABLED.store(nx && rdmsr(0xC000_0080) & (1 << 11) != 0, Ordering::Release);

        // Открываем пользователю доступ к его 2-МиБ странице.
        make_user_accessible(USER_BASE);
        // Область программ Linux больше 2 МиБ — huge-страницами.
        let mut a = crate::linux::USER_IMAGE_BASE;
        while a < crate::linux::USER_AREA_END {
            make_user_accessible(a);
            a += 0x20_0000;
        }
    }
    crate::println!("  [usermode] GDT (user CS/DS) + TSS + syscall MSR: OK");
    selftest();
}

unsafe fn wrmsr(msr: u32, val: u64) {
    asm!("wrmsr", in("ecx") msr, in("eax") val as u32, in("edx") (val >> 32) as u32,
         options(nostack, preserves_flags));
}

unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
         options(nomem, nostack, preserves_flags));
    ((hi as u64) << 32) | lo as u64
}

/// Переходит в Ring 3 по адресу `entry` со стеком `stack`.
///
/// Возврат происходит через syscall `exit`, который восстанавливает
/// сохранённый здесь стек ядра.
///
/// # Safety
/// Требует выполненного `init()`; страница `entry`/`stack` должна иметь
/// бит USER.
#[inline(never)]
unsafe fn enter_ring3(entry: u64, stack: u64) {
    // Возврат из Ring 3 работает как longjmp:
    //   enter_ring3: push регистров + адрес метки 4, RETURN_RSP = rsp
    //   exit:        rsp = RETURN_RSP, jmp ring3_return
    //   ring3_return: pop регистров, ret -> метка 4
    // После метки 4 функция возвращается в selftest() обычным путём.
    asm!(
        "lea rax, [rip + 4f]",
        "push rax",                 // адрес возврата для ret
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [{ret_rsp}], rsp",
        "swapgs",                   // GS -> пользовательский
        "mov r11, {rflags}",        // RFLAGS для Ring 3 (IF выключен)
        "mov rcx, {entry}",         // sysret прыгает по RCX
        "mov rsp, {stack}",
        "sysretq",
        "4:",                       // <- сюда приводит ret из ring3_return
        ret_rsp = in(reg) &raw mut RETURN_RSP,
        // IF=1 обязателен: иначе программа в Ring 3 неперываема, а
        // главное — флаг остаётся сброшенным ПОСЛЕ возврата в ядро, и
        // всё, что ждёт прерываний (клавиатура, мышь, таймер), виснет.
        rflags = const 0x0202u64,
        entry = in(reg) entry,
        stack = in(reg) stack,
        out("rax") _,
        clobber_abi("sysv64"),
    );
}

/// Как `enter_ring3`, но перед `sysretq` кладёт `arg` в RDI —
/// первый аргумент System V ABI для точки входа программы.
///
/// # Safety
/// Те же требования, что у `enter_ring3`.
#[inline(never)]
unsafe fn enter_ring3_arg(entry: u64, stack: u64, arg: u64) {
    asm!(
        "lea rax, [rip + 4f]",
        "push rax",
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [{ret_rsp}], rsp",
        "swapgs",
        "mov r11, {rflags}",
        "mov rcx, {entry}",
        "mov rdi, {arg}",
        "mov rsp, {stack}",
        "sysretq",
        "4:",
        ret_rsp = in(reg) &raw mut RETURN_RSP,
        rflags = const 0x0202u64,
        entry = in(reg) entry,
        arg = in(reg) arg,
        stack = in(reg) stack,
        out("rax") _,
        clobber_abi("sysv64"),
    );
}

// Точка возврата из Ring 3: стек восстановлен обработчиком exit,
// снимаем сохранённые регистры и возвращаемся по адресу с вершины стека.
global_asm!(
    ".section .text",
    ".global ring3_return",
    "ring3_return:",
    // Возврат идёт не через iretq, поэтому RFLAGS сам не
    // восстанавливается. Без явного sti после первой же программы
    // умирают IRQ0/IRQ1/IRQ12 — клавиатура и мышь перестают работать.
    "  sti",
    "  pop r15",
    "  pop r14",
    "  pop r13",
    "  pop r12",
    "  pop rbp",
    "  pop rbx",
    "  ret",
);

extern "C" {
    fn ring3_return();
}

/// САМОПРОВЕРКА RING 3: копирует программу в пользовательскую страницу,
/// уходит в CPL=3, выполняет оттуда системные вызовы и возвращается.
///
/// Печатает результат в serial-лог — это и есть доказательство, что
/// Ring 3 работает, а не «подготовлен».
pub fn selftest() {
    unsafe {
        let src = &raw const user_program_start as *const u8;
        let end = &raw const user_program_end as *const u8;
        let len = end as usize - src as usize;

        // Копируем код пользователя в страницу с битом USER.
        core::ptr::copy_nonoverlapping(src, USER_CODE as *mut u8, len);

        crate::println!("  [ring3] переход в CPL=3 (entry {:#x})...", USER_CODE);
        USER_EXITED = false;
        enter_ring3(USER_CODE, USER_STACK_TOP);
        if selftest_ok() {
            crate::println!("  [ring3] selftest OK: return from CPL=3 verified");
        }
    }
}

/// Диапазон страницы самопроверки Ring 3: прослойке Linux он нужен
/// для проверки указателей из пользовательского кода.
pub fn user_page_range() -> (u64, u64) {
    (USER_BASE, USER_BASE + 0x20_0000)
}

/// ЗАПУСКАЕТ произвольную программу в Ring 3, возвращает код выхода.
///
/// # Safety
/// `entry` и `stack` должны лежать в области, отображённой с USER.
pub unsafe fn run_user_program(entry: u64, stack: u64) -> i64 {
    EXIT_CODE = 0;
    USER_EXITED = false;
    enter_ring3(entry, stack);
    EXIT_CODE
}

/// Запускает программу в Ring 3, передав ей аргумент в RDI.
///
/// Нужен MEX-программам: их точка входа принимает указатель на таблицу
/// MexApi первым аргументом System V ABI.
///
/// # Safety
/// Как у `run_user_program`; `arg` должен указывать в память, доступную
/// из Ring 3, иначе первое же обращение программы к таблице даст #PF.
pub unsafe fn run_user_program_arg(entry: u64, stack: u64, arg: u64) -> i64 {
    EXIT_CODE = 0;
    USER_EXITED = false;
    enter_ring3_arg(entry, stack, arg);
    EXIT_CODE
}

/// Вызывается после `selftest()` — сообщает, вернулись ли мы из Ring 3.
pub fn selftest_ok() -> bool {
    unsafe { core::ptr::read_volatile(&raw const USER_EXITED) }
}
