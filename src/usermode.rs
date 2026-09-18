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
struct KStack([u8; KSTACK_SIZE]);
static mut KERNEL_STACK: KStack = KStack([0; KSTACK_SIZE]);

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

// ==================== Код пользователя ====================

/// Программа, исполняемая в Ring 3. Пишется на ассемблере и копируется
/// в страницу с битом USER: брать адрес Rust-функции нельзя — её код
/// лежит в страницах ядра без USER.
///
/// Делает три системных вызова и завершается:
///   write(1, msg, len) -> getpid() -> uptime() -> exit(0)
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
/// ВАЖНО: 0x200000 (2 МиБ) занимать НЕЛЬЗЯ — там начинается `.bss` ядра
/// (см. boot/linker_kernel.ld), включая 16-МиБ `HEAP_STORAGE`. Секция
/// тянется до ~0x121c000, поэтому пользовательскую страницу кладём на
/// 20 МиБ — выше кучи, но внутри первого гигабайта, отображённого
/// загрузчиком. Раньше здесь стояло 0x200000, и Ring 3 писал свой код и
/// стек прямо поверх кучи ядра.
const USER_BASE: u64 = 0x0140_0000; // 20 МиБ
const USER_CODE: u64 = USER_BASE + 0x1000;
const USER_STACK_TOP: u64 = USER_BASE + 0x8000;

// ==================== Обработчик syscall ====================

/// Точка входа `syscall`. Процессор сюда прыгает из Ring 3, сохранив
/// RIP в RCX и RFLAGS в R11. Стек всё ещё пользовательский — переключаем
/// его на стек ядра, иначе Ring 3 мог бы подсунуть ядру любой адрес.
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
    // Перекладываем регистры соглашения syscall в соглашение System V C:
    //   syscall: nr=rax, a1=rdi, a2=rsi, a3=rdx
    //   C ABI:   arg1=rdi, arg2=rsi, arg3=rdx, arg4=rcx
    // Порядок важен, иначе значения затрут друг друга.
    "  mov rcx, rdx",
    "  mov rdx, rsi",
    "  mov rsi, rdi",
    "  mov rdi, rax",
    "  call {handler}",
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

/// Признак того, что Ring 3 завершился через `exit`.
static mut USER_EXITED: bool = false;
/// Куда вернуться из `exit` (стек ядра на момент входа в Ring 3).
static mut RETURN_RSP: u64 = 0;
/// Код возврата последней запущенной программы.
static mut EXIT_CODE: i64 = 0;

/// Диспетчер системных вызовов. Номера — как в Linux x86-64, чтобы
/// поведение было предсказуемым: 1=write, 39=getpid, 60=exit, 40=uptime.
extern "C" fn syscall_handler(nr: u64, a1: u64, a2: u64, a3: u64) -> u64 {
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
                    rip = in(reg) ring3_return as u64,
                    options(noreturn));
            }
        }
        // Остальное — прослойка Linux (src/linux/syscall.rs).
        _ => crate::linux::syscall::dispatch(nr, a1, a2, a3, 0, 0),
    }
}

// ==================== Инициализация ====================

/// Настраивает GDT, TSS, MSR для syscall и делает страницу пользователя
/// доступной из Ring 3.
pub fn init() {
    unsafe {
        init_gdt();

        // Стек ядра для входов из Ring 3.
        let kstack_top = (&raw const KERNEL_STACK as u64) + KSTACK_SIZE as u64;
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
        wrmsr(0xC000_0082, syscall_entry as u64); // LSTAR
        wrmsr(0xC000_0084, 0x0000_0300); // SFMASK: гасим IF и TF

        // EFER.SCE — без него инструкция syscall вызывает #UD.
        let efer = rdmsr(0xC000_0080);
        wrmsr(0xC000_0080, efer | 1);

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

/// Вызывается после `selftest()` — сообщает, вернулись ли мы из Ring 3.
pub fn selftest_ok() -> bool {
    unsafe { core::ptr::read_volatile(&raw const USER_EXITED) }
}
