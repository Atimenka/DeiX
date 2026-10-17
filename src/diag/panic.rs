//! Отказ ядра: экран, отчёт и сохранение дампа.
//!
//! Паника означает, что ядро больше не может продолжать работу. Всё, что
//! здесь происходит, выполняется в условиях, когда повреждена может быть
//! любая подсистема — включая кучу. Поэтому отчёт собирается в статический
//! буфер, а не через `String`, и пишется сырыми секторами, а не через
//! файловую систему.
//!
//! Смерть пользовательского процесса или сбой модуля ядра сюда НЕ относятся:
//! они регистрируются как обычные события, и система продолжает работу.

use super::code::{self, ErrorCode, Subsystem};
use super::record::ErrorRecord;
use super::severity::Severity;
use super::{log, ring, NONE};

/// Ёмкость статического буфера отчёта.
///
/// 32 КиБ достаточно для 256 записей журнала плюс полное состояние системы.
const REPORT_CAP: usize = 32 * 1024;

/// Сколько записей журнала попадает в отчёт об отказе.
const REPORT_RECORDS: usize = 256;

/// Сектора для дампа: свободная зона между загрузчиком и системным томом.
///
/// `stage2` занимает сектора ниже 1150, `/system` начинается с 4096, поэтому
/// диапазон 2048..4095 свободен. Запись сюда не зависит от EROFS, EXT2 и
/// выделителя — то есть работает даже тогда, когда повреждена именно ФС.
pub const PANIC_LBA: u32 = 2048;
/// Размер дампа в секторах (32 КиБ).
pub const PANIC_SECTORS: u32 = 64;
/// Маркер дампа в первом секторе.
pub const PANIC_MARKER: [u8; 8] = *b"DEIXPNIC";

/// Контекст процессора на момент отказа.
#[derive(Clone, Copy)]
pub struct PanicContext {
    /// Вектор исключения; 0, если отказ не связан с исключением.
    pub vector: u8,
    /// Код ошибки, выданный процессором.
    pub error_code: u64,
    /// Адрес, вызвавший сбой (CR2 для #PF).
    pub fault_address: u64,
    /// Адрес инструкции.
    pub rip: u64,
    /// Указатель стека.
    pub rsp: u64,
    /// Селектор сегмента кода: по нему определяется кольцо.
    pub code_segment: u64,
}

impl PanicContext {
    /// Пустой контекст — для отказов вне обработчика исключений.
    pub const fn none() -> Self {
        Self {
            vector: 0,
            error_code: 0,
            fault_address: 0,
            rip: 0,
            rsp: 0,
            code_segment: 0,
        }
    }

    /// Собирает контекст из кадра стека прерывания.
    pub fn from_frame(frame: &crate::interrupts::InterruptStackFrame, vector: u8, error_code: u64) -> Self {
        let fault_address = if vector == 14 { read_cr2() } else { 0 };
        Self {
            vector,
            error_code,
            fault_address,
            rip: frame.instruction_pointer,
            rsp: frame.stack_pointer,
            code_segment: frame.code_segment,
        }
    }

    /// Произошёл ли сбой в пользовательском кольце.
    ///
    /// Это ключевая проверка: отказ кода в Ring 3 — авария приложения, а не
    /// паника ядра, и система обязана продолжить работу.
    pub fn from_user(&self) -> bool {
        (self.code_segment & 3) == 3
    }
}

/// Читает CR2 — адрес, вызвавший ошибку страницы.
fn read_cr2() -> u64 {
    let cr2: u64;
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2) };
    cr2
}

/// Название исключения по вектору.
pub fn vector_name(vector: u8) -> &'static str {
    match vector {
        0 => "Division by zero (#DE)",
        1 => "Debug (#DB)",
        2 => "Non-maskable interrupt",
        3 => "Breakpoint (#BP)",
        4 => "Overflow (#OF)",
        5 => "Bound range exceeded (#BR)",
        6 => "Invalid opcode (#UD)",
        7 => "Device not available (#NM)",
        8 => "Double fault (#DF)",
        10 => "Invalid TSS (#TS)",
        11 => "Segment not present (#NP)",
        12 => "Stack-segment fault (#SS)",
        13 => "General protection fault (#GP)",
        14 => "Page fault (#PF)",
        16 => "x87 floating-point exception (#MF)",
        17 => "Alignment check (#AC)",
        18 => "Machine check (#MC)",
        19 => "SIMD floating-point exception (#XM)",
        _ => "Неизвестное исключение",
    }
}

/// Код ошибки ядра, соответствующий вектору исключения.
pub fn code_for_vector(vector: u8) -> ErrorCode {
    match vector {
        0 => ErrorCode::new(Subsystem::Kernel, 4),
        8 => ErrorCode::new(Subsystem::Kernel, 5),
        13 => ErrorCode::new(Subsystem::Kernel, 4),
        14 => ErrorCode::new(Subsystem::Memory, 1),
        18 => ErrorCode::new(Subsystem::Kernel, 12),
        _ => ErrorCode::new(Subsystem::Kernel, 4),
    }
}

// ==================== Сборка отчёта ====================

/// Статический буфер отчёта.
///
/// Текст отчёта не хранится в куче: 32 КиБ в куче во время паники — это
/// запрос к выделителю, который может быть повреждён. Форматирование
/// отдельных строк всё же использует `format!`; от рекурсивной паники при
/// мёртвой куче защищает `PANIC_IN_PROGRESS`.
static mut REPORT_BUF: [u8; REPORT_CAP] = [0; REPORT_CAP];

/// Писатель в статический буфер отчёта.
///
/// Доступ через сырой указатель: паника выполняется с выключенными
/// прерываниями на единственном CPU, поэтому гонок по буферу нет.
struct Report {
    len: usize,
}

impl Report {
    fn new() -> Self {
        Self { len: 0 }
    }

    fn buf_ptr() -> *mut u8 {
        (&raw mut REPORT_BUF) as *mut u8
    }

    fn push(&mut self, b: u8) {
        if self.len < REPORT_CAP {
            unsafe { Self::buf_ptr().add(self.len).write(b) };
            self.len += 1;
        }
    }

    fn str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.push(b);
        }
    }

    fn line(&mut self, s: &str) {
        self.str(s);
        self.push(b'\n');
    }

    fn hex(&mut self, label: &str, v: u64) {
        self.str(label);
        self.str("0x");
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for i in (0..16).rev() {
            self.push(HEX[((v >> (i * 4)) & 0xF) as usize]);
        }
        self.push(b'\n');
    }

    fn dec(&mut self, label: &str, v: u64) {
        self.str(label);
        let mut digits = [0u8; 20];
        let mut n = 0;
        let mut x = v;
        if x == 0 {
            digits[0] = b'0';
            n = 1;
        }
        while x > 0 {
            digits[n] = b'0' + (x % 10) as u8;
            n += 1;
            x /= 10;
        }
        for i in (0..n).rev() {
            self.push(digits[i]);
        }
        self.push(b'\n');
    }

    fn code(&mut self, label: &str, c: ErrorCode) {
        self.str(label);
        let mut tmp = [0u8; 16];
        let n = c.write_to(&mut tmp);
        self.str(core::str::from_utf8(&tmp[..n]).unwrap_or("DX-???-0000"));
        self.push(b'\n');
    }

    fn section(&mut self, title: &str) {
        self.push(b'\n');
        self.line("--- --- --- --- --- --- --- --- --- --- --- --- --- --- --- ---");
        self.str(title);
        self.push(b'\n');
        self.line("--- --- --- --- --- --- --- --- --- --- --- --- --- --- --- ---");
    }

    fn as_bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(Self::buf_ptr(), self.len) }
    }
}

/// Собирает полный отчёт об отказе.
fn build_report(code: ErrorCode, message: &str, ctx: &PanicContext) -> Report {
    let mut r = Report::new();

    r.line("=== DeiX Kernel Panic Report ===");
    r.line(&alloc::format!(
        "generated: {}",
        crate::rtc::now().as_string()
    ));
    r.dec("uptime_ms: ", crate::timer::uptime_ms());
    r.dec("boot_session: ", ring::boot_session() as u64);

    r.section("FAILURE");
    r.code("code: ", code);
    r.line(&alloc::format!("type: {}", code::summary_of(code)));
    r.line(&alloc::format!("reason: {}", code::detail_of(code)));
    r.line(&alloc::format!("action: {}", code::action_of(code)));
    r.line(&alloc::format!("message: {}", message));

    r.section("EXCEPTION");
    if ctx.vector != 0 {
        r.dec("vector: ", ctx.vector as u64);
        r.line(&alloc::format!("name: {}", vector_name(ctx.vector)));
        r.hex("error_code: ", ctx.error_code);
        r.hex("fault_address: ", ctx.fault_address);
        r.hex("rip: ", ctx.rip);
        r.hex("rsp: ", ctx.rsp);
        r.hex("cs: ", ctx.code_segment);
        r.line(&alloc::format!("ring: {}", ctx.code_segment & 3));
    } else {
        r.line("vector: (отказ вне обработчика исключения)");
    }

    r.section("PROCESS");
    let pid = crate::process::current_pid().unwrap_or(0);
    r.dec("pid: ", pid as u64);
    r.dec("tid: ", crate::sched::current_id() as u64);
    if let Some(task) = crate::sched::current_task_name() {
        r.line(&alloc::format!("thread: {}", task));
    }
    match crate::process::name_of(pid) {
        Some(name) => r.line(&alloc::format!("process: {}", name)),
        None => r.line("process: kernel"),
    }

    r.section("SYSTEM");
    r.line(&alloc::format!("kernel: DeiX v{}", crate::KERNEL_VERSION));
    r.line(&alloc::format!("build: {}", crate::BUILD_ID));
    if let Some(cpu) = crate::cpuid::brand_string() {
        r.line(&alloc::format!("cpu: {}", cpu.trim()));
    }
    r.line("core: 0 (BSP, однопроцессорный режим)");

    r.section("MEMORY");
    let heap = crate::allocator::stats();
    r.dec("heap_total_bytes: ", heap.total as u64);
    r.dec("heap_used_bytes: ", heap.used as u64);
    r.dec("heap_free_bytes: ", heap.free as u64);

    r.section("SCHEDULER");
    r.dec("ticks: ", crate::sched::ticks_total() as u64);
    r.dec("switches: ", crate::sched::switch_count() as u64);
    r.dec("threads: ", crate::sched::list().len() as u64);

    r.section("LOADED MODULES");
    let modules = crate::module::get_loaded_modules();
    if modules.is_empty() {
        r.line("(нет загруженных модулей)");
    }
    for m in &modules {
        let status = match m.status {
            crate::module::ModuleStatus::Initialized => "initialized",
            crate::module::ModuleStatus::Failed => "failed",
            crate::module::ModuleStatus::Quarantined => "quarantined",
        };
        r.line(&alloc::format!(
            "  {} v{}.{} @ {:#x} ({} байт, {})",
            m.name,
            m.version.0,
            m.version.1,
            m.load_addr,
            m.body_size,
            status
        ));
    }

    r.section("MOUNTS");
    for line in crate::vfs::describe_mounts() {
        r.line(&alloc::format!("  {}", line));
    }

    r.section("BACKTRACE");
    write_backtrace(&mut r, ctx.rsp);

    r.section(&alloc::format!(
        "EVENT LOG (last {} of {} buffered)",
        REPORT_RECORDS.min(ring::len()),
        ring::len()
    ));
    ring::for_each(REPORT_RECORDS, |record: &ErrorRecord| {
        write_record(&mut r, record);
    });

    r.section("END OF REPORT");
    let mut tagged = ErrorRecord::empty();
    tagged.code = code;
    tagged.timestamp_ms = crate::timer::uptime_ms();
    r.line(&alloc::format!("report id: {}", super::extended_id(&tagged)));

    r
}

/// Пишет одну запись журнала в отчёт в машиночитаемом виде.
fn write_record(r: &mut Report, record: &ErrorRecord) {
    let mut buf = [0u8; 160];
    let n = log::format_compact(record, &mut buf);
    r.str(core::str::from_utf8(&buf[..n]).unwrap_or(""));
    r.push(b'\n');
}

/// Обходит цепочку кадров по RBP.
///
/// Требует компиляции с `-C force-frame-pointers=yes`; без неё цепочка
/// обрывается на первом же кадре, что само по себе полезно знать.
fn write_backtrace(r: &mut Report, rsp: u64) {
    if rsp == 0 {
        r.line("(указатель стека недоступен)");
        return;
    }

    // Кадр обработчика исключений содержит сохранённый RBP вызывающего кода.
    let mut frame = unsafe { (rsp as *const u64).read() };
    for depth in 0..16 {
        // Проверка на разумность: адрес кадра должен быть выровнен и
        // находиться в области ядра.
        if frame < 0x1000 || frame % 8 != 0 || frame > 0x7FFF_FFFF_FFFF {
            r.line(&alloc::format!("  #{} <цепочка оборвана>", depth));
            break;
        }
        let return_addr = unsafe { (frame as *const u64).add(1).read() };
        r.line(&alloc::format!("  #{} {:#x}", depth, return_addr));

        let next = unsafe { (frame as *const u64).read() };
        // Цепочка обязана расти вверх по стеку; иначе это цикл.
        if next <= frame {
            break;
        }
        frame = next;
    }
}

// ==================== Обработчик отказа ====================

/// Защита от рекурсивной паники.
///
/// Сборка отчёта использует кучу (`format!`). Если паника вызвана именно
/// повреждением кучи, попытка собрать отчёт может упасть снова. При
/// повторном входе отчёт не собирается: в serial уходит только сообщение,
/// и система останавливается.
static PANIC_IN_PROGRESS: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Точка входа из `#[panic_handler]`.
///
/// Разворачивает `PanicInfo` в код и сообщение и передаёт управление
/// общему обработчику отказа.
pub fn rust_panic(info: &core::panic::PanicInfo) -> ! {
    unsafe { core::arch::asm!("cli") };

    if PANIC_IN_PROGRESS.swap(true, core::sync::atomic::Ordering::SeqCst) {
        // Повторная паника: никакой кучи, только серийный порт.
        for &b in b"\r\n[PANIC] recursive panic, halting\r\n" {
            crate::serial::write_byte(b);
        }
        loop {
            unsafe { core::arch::asm!("hlt") };
        }
    }

    let code = ErrorCode::new(Subsystem::Kernel, 12);
    let message = alloc::format!("{}", info);
    kernel_panic(code, &message, &PanicContext::none())
}

/// Обрабатывает отказ ядра: сохраняет отчёт, рисует экран и останавливает систему.
///
/// Не возвращает управление.
pub fn kernel_panic(code: ErrorCode, message: &str, ctx: &PanicContext) -> ! {
    // Прерывания больше недопустимы: любое прерывание поверх паники
    // перезапишет состояние, которое мы собираемся сохранить.
    unsafe { core::arch::asm!("cli") };
    PANIC_IN_PROGRESS.store(true, core::sync::atomic::Ordering::SeqCst);

    // 1. Регистрация самого факта отказа в кольцевом буфере — чтобы он
    //    попал и в дамп, и в serial.
    let _ = log::log(
        Severity::Panic,
        code,
        super::record::Action::SaveReportAndHalt,
        message,
    );

    // 2. Сборка отчёта в статический буфер.
    let report = build_report(code, message, ctx);

    // 3. Вывод в serial — единственный канал, работающий безусловно.
    for &b in report.as_bytes() {
        crate::serial::write_byte(b);
    }
    crate::serial::write_byte(b'\n');

    // 4. Сохранение на диск.
    let saved = save_report(report.as_bytes());

    // 5. Экран.
    draw_panic_screen(code, message, ctx, saved);

    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}

/// Обрабатывает исключение, пришедшее из пользовательского кольца.
///
/// Это авария приложения, а не ядра: процесс останавливается, событие
/// регистрируется, система продолжает работу.
pub fn user_fault(ctx: &PanicContext) -> ! {
    let code = match ctx.vector {
        14 => ErrorCode::new(Subsystem::Memory, 8),
        13 => ErrorCode::new(Subsystem::Memory, 9),
        6 => ErrorCode::new(Subsystem::Elf, 5),
        _ => code_for_vector(ctx.vector),
    };

    let pid = crate::process::current_pid().unwrap_or(0);
    let name = crate::process::name_of(pid)
        .unwrap_or_else(|| alloc::string::String::from("неизвестно"));

    let _ = log::log_ctx(
        Severity::Error,
        code,
        super::record::Action::TerminateProcess,
        &alloc::format!(
            "{} (pid {}): {} по адресу {:#x}",
            name,
            pid,
            vector_name(ctx.vector),
            ctx.fault_address
        ),
        pid,
        pid,
        0,
        ctx.fault_address,
    );

    // Сохраняем последнюю ошибку процесса — её покажет Диспетчер задач
    // и диалог «АВАРИЯ ПРИЛОЖЕНИЯ».
    crate::process::record_crash(pid, code, &alloc::format!(
        "{} по адресу {:#x} (rip {:#x})",
        vector_name(ctx.vector),
        ctx.fault_address,
        ctx.rip
    ));

    draw_app_crash_screen(pid, &name, code, ctx);

    // Процесс помечается завершённым, после чего управление возвращается
    // коду, запустившему программу, — тем же путём, каким выходит syscall
    // `exit`. Ядро продолжает работу.
    if pid != 0 {
        let _ = crate::process::kill(pid, -(ctx.vector as i32));
    }
    if crate::usermode::user_program_active() {
        unsafe { crate::usermode::abort_user_program(-(ctx.vector as i64)) };
    }

    // Исключение из Ring 3 без активной программы — рассинхронизация
    // состояния, продолжать небезопасно.
    kernel_panic(
        super::code::ErrorCode::new(Subsystem::Kernel, 12),
        "исключение Ring 3 без активной пользовательской программы",
        ctx,
    )
}

// ==================== Сохранение ====================

/// Пишет отчёт сырыми секторами и, если файловая система жива, файлом.
///
/// Возвращает `true`, если отчёт удалось сохранить хотя бы одним способом.
fn save_report(data: &[u8]) -> bool {
    // Сырые сектора — основной путь: он не зависит от состояния ФС.
    let mut raw_saved = true;

    // Первый сектор начинается с маркера и длины, чтобы при следующей
    // загрузке можно было отличить дамп от мусора и понять его размер.
    let mut first = [0u8; 512];
    first[..8].copy_from_slice(&PANIC_MARKER);
    first[8..16].copy_from_slice(&(data.len() as u64).to_le_bytes());
    let body = 512 - 16;
    let n = body.min(data.len());
    first[16..16 + n].copy_from_slice(&data[..n]);
    if crate::ata::write_sectors(PANIC_LBA, 1, &first).is_err() {
        raw_saved = false;
    }
    let mut offset = n;

    for i in 1..PANIC_SECTORS {
        if offset >= data.len() {
            // Хвост затираем нулями, чтобы старый дамп не читался как новый.
            let zeros = [0u8; 512];
            let _ = crate::ata::write_sectors(PANIC_LBA + i, 1, &zeros);
            continue;
        }
        let end = (offset + 512).min(data.len());
        let mut sector = [0u8; 512];
        sector[..end - offset].copy_from_slice(&data[offset..end]);
        if crate::ata::write_sectors(PANIC_LBA + i, 1, &sector).is_err() {
            raw_saved = false;
        }
        offset = end;
    }

    // Файл — дополнительный путь: он удобнее для просмотра из оболочки,
    // но доступен только если EXT2 и выделитель в порядке.
    let file_saved = crate::vfs::write_file(super::persist::FILE_LAST_PANIC, data).is_ok();

    raw_saved || file_saved
}

/// Читает дамп отказа с сырых секторов.
pub fn load_raw_report() -> Option<alloc::string::String> {
    let mut first = [0u8; 512];
    if crate::ata::read_sectors(PANIC_LBA, 1, &mut first).is_err() {
        return None;
    }
    if first[..8] != PANIC_MARKER {
        return None;
    }
    let len = u64::from_le_bytes(first[8..16].try_into().ok()?) as usize;
    if len == 0 || len > (PANIC_SECTORS as usize * 512) {
        return None;
    }

    let mut data: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(len);
    let body = (512 - 16).min(len);
    data.extend_from_slice(&first[16..16 + body]);

    let mut remaining = len.saturating_sub(body);
    for i in 1..PANIC_SECTORS {
        if remaining == 0 {
            break;
        }
        let mut sector = [0u8; 512];
        if crate::ata::read_sectors(PANIC_LBA + i, 1, &mut sector).is_err() {
            break;
        }
        let n = remaining.min(512);
        data.extend_from_slice(&sector[..n]);
        remaining -= n;
    }

    Some(alloc::string::String::from_utf8_lossy(&data).into_owned())
}

/// Есть ли несохранённый отчёт об отказе от предыдущей загрузки.
pub fn has_previous_failure() -> bool {
    load_raw_report().is_some()
}

/// Удаляет дамп после того, как пользователь его просмотрел.
pub fn clear_previous_failure() {
    let zeros = [0u8; 512];
    for i in 0..PANIC_SECTORS.min(4) {
        let _ = crate::ata::write_sectors(PANIC_LBA + i, 1, &zeros);
    }
    let _ = crate::vfs::remove(super::persist::FILE_LAST_PANIC);
}

/// Извлекает код ошибки из сохранённого отчёта — для заголовка диалога.
pub fn previous_failure_code() -> ErrorCode {
    let Some(text) = load_raw_report() else {
        return NONE;
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("code: ") {
            if let Some(code) = ErrorCode::parse(rest.trim()) {
                return code;
            }
        }
    }
    NONE
}

/// Краткое описание предыдущего сбоя для диалога при загрузке.
/// Полный текст отчёта читается отдельно через `load_raw_report`.
pub struct PreviousFailure {
    pub code: ErrorCode,
    pub kind: &'static str,
    pub module: alloc::string::String,
}

/// Собирает сведения о предыдущем сбое.
///
/// Дамп в сырых секторах пишет только обработчик отказа, поэтому его
/// наличие и есть признак аварийного завершения: плановая перезагрузка
/// дампа не оставляет, и диалог не показывается.
pub fn previous_failure() -> Option<PreviousFailure> {
    let report = load_raw_report()?;
    let code = previous_failure_code();

    // Модуль, в котором произошёл сбой, берём из RIP: попадающий в регион
    // модуля адрес указывает на него.
    let mut module = alloc::string::String::from("ядро");
    if let Some(rip) = report
        .lines()
        .find_map(|l| l.strip_prefix("rip: 0x"))
        .and_then(|h| u64::from_str_radix(h.trim(), 16).ok())
    {
        for m in crate::module::get_loaded_modules() {
            let base = m.load_addr as u64;
            if rip >= base && rip < base + m.body_size as u64 {
                module = m.name.clone();
                break;
            }
        }
    }

    Some(PreviousFailure {
        code,
        kind: code::summary_of(code),
        module,
    })
}

// ==================== Экраны ====================

/// Цвета экрана отказа — тёмная техническая палитра без оформления.
const BG: crate::renderer::Color = crate::renderer::Color::rgb(0x0A, 0x0A, 0x0C);
const PANEL: crate::renderer::Color = crate::renderer::Color::rgb(0x14, 0x15, 0x19);
const BORDER: crate::renderer::Color = crate::renderer::Color::rgb(0x3F, 0x2A, 0x2E);
const ACCENT: crate::renderer::Color = crate::renderer::Color::rgb(0xE5, 0x4B, 0x4B);
const TEXT: crate::renderer::Color = crate::renderer::Color::rgb(0xD8, 0xDE, 0xE6);
const DIM: crate::renderer::Color = crate::renderer::Color::rgb(0x7A, 0x82, 0x8C);

/// Рисует экран отказа ядра.
///
/// Экран намеренно скупой: код ошибки, тип сбоя, модуль и статус отчёта.
/// Полный дамп уходит в serial и на диск — экран не должен превращаться
/// в стену текста, которую невозможно прочитать.
fn draw_panic_screen(code: ErrorCode, message: &str, ctx: &PanicContext, saved: bool) {
    // Текстовый режим — запасной путь, когда framebuffer недоступен.
    crate::println!("");
    crate::println!("==================== DEIX KERNEL PANIC ====================");
    crate::println!("  code:    {}", code.as_string());
    crate::println!("  type:    {}", code::summary_of(code));
    crate::println!("  detail:  {}", message);
    if ctx.vector != 0 {
        crate::println!("  except:  {} (rip {:#x})", vector_name(ctx.vector), ctx.rip);
    }
    crate::println!(
        "  report:  {}",
        if saved { "saved" } else { "НЕ СОХРАНЁН" }
    );
    crate::println!("===========================================================");

    if !crate::renderer::has_active_framebuffer() {
        return;
    }

    // try: блокировку рендерера могла держать прерванная задача
    // композитора — ожидание здесь зависило бы обработчик сбоя.
    let _ = crate::renderer::try_with_renderer(|r| {
        let w = r.width() as i32;
        let h = r.height() as i32;

        r.clear(BG);

        let panel_w = 520.min(w - 80);
        let panel_h = 260.min(h - 80);
        let px = (w - panel_w) / 2;
        let py = (h - panel_h) / 2;

        r.fill_rect(px, py, panel_w as u32, panel_h as u32, PANEL);
        r.fill_rect(px, py, panel_w as u32, 3, ACCENT);
        r.fill_rect(px, py, 3, panel_h as u32, BORDER);
        r.fill_rect(px + panel_w - 3, py, 3, panel_h as u32, BORDER);
        r.fill_rect(px, py + panel_h - 3, panel_w as u32, 3, BORDER);

        let mut y = py + 34;
        r.draw_text(px + 28, y, "DEIX KERNEL PANIC", ACCENT, None);

        y += 34;
        r.draw_text(px + 28, y, "Error Code", DIM, None);
        r.draw_text(px + 160, y, &code.as_string(), TEXT, None);

        y += 22;
        r.draw_text(px + 28, y, "Failure", DIM, None);
        r.draw_text(px + 160, y, code::summary_of(code), TEXT, None);

        if ctx.vector != 0 {
            y += 22;
            r.draw_text(px + 28, y, "Exception", DIM, None);
            r.draw_text(px + 160, y, vector_name(ctx.vector), TEXT, None);

            y += 22;
            r.draw_text(px + 28, y, "Address", DIM, None);
            r.draw_text(px + 160, y, &alloc::format!("{:#x}", ctx.rip), TEXT, None);
        }

        y += 22;
        r.draw_text(px + 28, y, "Detail", DIM, None);
        r.draw_text(px + 160, y, &truncate(message, 44), TEXT, None);

        y += 22;
        r.draw_text(px + 28, y, "Uptime", DIM, None);
        r.draw_text(
            px + 160,
            y,
            &alloc::format!("{} ms", crate::timer::uptime_ms()),
            TEXT,
            None,
        );

        y += 40;
        let status = if saved {
            alloc::format!("CRASH REPORT SAVED / {}", code.as_string())
        } else {
            alloc::format!("CRASH REPORT NOT SAVED / {}", code.as_string())
        };
        r.draw_text(px + 28, y, &status, if saved { TEXT } else { ACCENT }, None);

        y += 20;
        r.draw_text(
            px + 28,
            y,
            "Система остановлена. Полный отчёт — в serial и /userdata/log/panic/last.panic",
            DIM,
            None,
        );
    });
}

/// Рисует диалог аварии приложения.
///
/// Ядро при этом продолжает работу — это принципиальное отличие от экрана
/// отказа: экран рисуется как модальное окно, а не как полная замена
/// содержимого дисплея.
fn draw_app_crash_screen(pid: u32, name: &str, code: ErrorCode, ctx: &PanicContext) {
    crate::println!(
        "[app crash] {} (pid {}): {} — {}",
        name,
        pid,
        code.as_string(),
        code::summary_of(code)
    );

    if !crate::renderer::has_active_framebuffer() {
        return;
    }

    let _ = crate::renderer::try_with_renderer(|r| {
        let w = r.width() as i32;
        let h = r.height() as i32;

        // Затемняем фон, но не стираем его: видно, что система жива.
        r.fill_rect(0, 0, w as u32, h as u32, crate::renderer::Color::rgb(0x00, 0x00, 0x00));

        let panel_w = 440.min(w - 60);
        let panel_h = 200.min(h - 60);
        let px = (w - panel_w) / 2;
        let py = (h - panel_h) / 2;

        r.fill_rect(px, py, panel_w as u32, panel_h as u32, PANEL);
        r.fill_rect(px, py, panel_w as u32, 3, ACCENT);

        let mut y = py + 30;
        r.draw_text(px + 24, y, "APPLICATION CRASH", ACCENT, None);

        y += 30;
        r.draw_text(px + 24, y, "Program", DIM, None);
        r.draw_text(px + 130, y, &truncate(name, 32), TEXT, None);

        y += 22;
        r.draw_text(px + 24, y, "PID", DIM, None);
        r.draw_text(px + 130, y, &alloc::format!("{}", pid), TEXT, None);

        y += 22;
        r.draw_text(px + 24, y, "Reason", DIM, None);
        r.draw_text(px + 130, y, &code.as_string(), TEXT, None);

        y += 22;
        r.draw_text(px + 24, y, "", DIM, None);
        r.draw_text(px + 130, y, &truncate(code::summary_of(code), 36), TEXT, None);

        if ctx.fault_address != 0 {
            y += 22;
            r.draw_text(px + 24, y, "Address", DIM, None);
            r.draw_text(
                px + 130,
                y,
                &alloc::format!("{:#x}", ctx.fault_address),
                TEXT,
                None,
            );
        }

        y += 34;
        r.draw_text(
            px + 24,
            y,
            "Ядро продолжает работу. Процесс будет остановлен.",
            DIM,
            None,
        );
    });
}

/// Усекает строку до `max` символов по границе UTF-8.
fn truncate(text: &str, max: usize) -> alloc::string::String {
    if text.chars().count() <= max {
        return alloc::string::String::from(text);
    }
    let mut out: alloc::string::String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
