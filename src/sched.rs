//! ВЫТЕСНЯЮЩАЯ МНОГОЗАДАЧНОСТЬ — настоящее переключение контекста.
//!
//! Прежний `sched.rs` (удалён на этапе 1) спавнил 4 фиктивных потока,
//! крутил 40 тиков и печатал строки — никакого переключения контекста
//! там не было. Здесь реализовано то, что действительно переключает
//! исполнение:
//!
//! * каждая задача имеет **собственный стек** (64 КиБ в куче);
//! * обработчик IRQ0 — **naked-заглушка на ассемблере**: сохраняет все
//!   регистры на стек текущей задачи, меняет RSP на стек следующей и
//!   восстанавливает её регистры. `extern "x86-interrupt"` для этого не
//!   годится: компилятор сам генерирует пролог/эпилог и не даёт
//!   подменить стек между ними;
//! * планировщик — round-robin по кругу готовых задач;
//! * переключение **вытесняющее**: задача не обязана ничего вызывать,
//!   её прерывает таймер (PIT, 100 Гц).
//!
//! ## Честное ограничение: одно ядро CPU
//!
//! В ядре нет APIC/LAPIC — только PIC 8259, а значит нет и способа
//! запустить остальные процессоры (для этого нужны IPI INIT/SIPI через
//! LAPIC). Это настоящая вытесняющая многозадачность **на одном
//! процессоре**, как в Linux 2.0: параллелизма нет, есть конкурентность.
//!
//! ## Раскладка кадра переключения
//!
//! При входе в IRQ0 процессор уже положил на стек `RIP/CS/RFLAGS/RSP/SS`.
//! Заглушка добавляет сверху 15 регистров общего назначения. Итоговый
//! кадр (от вершины стека вниз):
//!
//! ```text
//!   r15 r14 r13 r12 r11 r10 r9 r8 rbp rdi rsi rdx rcx rbx rax
//!   RIP CS RFLAGS RSP SS          <- положил процессор
//! ```
//!
//! Переключение задач = подмена RSP между `pop`-ами. Новая задача
//! «просыпается» ровно там, где её прервали.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::spinlock::SpinLock;

/// Размер стека задачи.
const STACK_SIZE: usize = 64 * 1024;
/// Максимум задач (статическая таблица — без аллокаций в обработчике IRQ).
const MAX_TASKS: usize = 64;

/// Состояние задачи.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Слот свободен.
    Empty,
    /// Готова к исполнению / исполняется.
    Ready,
    /// Спит до указанного момента (мс аптайма).
    Sleeping(u64),
    /// Завершилась.
    Finished,
}

/// Задача планировщика.
struct Task {
    state: State,
    /// Вершина стека задачи в момент, когда она НЕ исполняется.
    /// Именно это значение подменяет заглушка IRQ0.
    rsp: u64,
    /// Владение стеком: держим Box, чтобы память не освободилась.
    _stack: Option<Box<[u8]>>,
    /// Owns the private page tables while the scheduler slot is live.
    _address_space: Option<Box<crate::mm::address_space::AddressSpace>>,
    /// Root switched by the IRQ scheduler; zero means the kernel root.
    cr3: u64,
    user_mode: bool,
    kernel_stack_top: u64,
    exit_code: i64,
    name: String,
    /// Сколько тиков PIT эта задача реально провела на процессоре.
    /// Увеличивается в `schedule_from_irq`, то есть учитывается именно
    /// время исполнения, а не время существования задачи.
    runtime_ticks: u64,
    /// PID процесса-владельца (см. `crate::process`), если задача
    /// создана менеджером процессов. 0 — задача ядра без процесса.
    pid: u32,
    /// Задача добровольно простаивает в `idle_wait()` (hlt в ожидании
    /// события). Тики PIT в этом состоянии честно учитываются как
    /// простой процессора, а не как работа задачи — иначе главный
    /// поток ("dinit") показывал бы 100% CPU, просто ожидая ввода.
    idle_waiting: bool,
}

impl Task {
    const fn empty() -> Self {
        Task {
            state: State::Empty,
            rsp: 0,
            _stack: None,
            _address_space: None,
            cr3: 0,
            user_mode: false,
            kernel_stack_top: 0,
            exit_code: 0,
            name: String::new(),
            runtime_ticks: 0,
            pid: 0,
            idle_waiting: false,
        }
    }
}

/// Таблица задач. Доступ из обработчика прерывания — только через
/// `SCHED_ACTIVE`, без блокировок (мы уже в прерывании, вытеснить нас
/// некому: одно ядро и IF=0).
static mut TASKS: [Task; MAX_TASKS] = [const { Task::empty() }; MAX_TASKS];
/// Индекс текущей задачи.
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);
/// Планировщик включён (заглушка IRQ0 переключает контекст).
static SCHED_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Счётчик переключений — для доказательства, что они реально идут.
static SWITCHES: AtomicUsize = AtomicUsize::new(0);
/// Всего тиков PIT, учтённых планировщиком. Знаменатель для доли CPU.
static TICKS_TOTAL: AtomicUsize = AtomicUsize::new(0);
/// Тики PIT, пришедшиеся на добровольный простой (idle_wait).
static IDLE_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Защита операций spawn/join (в обычном коде, не в прерывании).
static TABLE_LOCK: SpinLock<()> = SpinLock::new(());

/// Сколько раз планировщик переключал контекст.
pub fn switch_count() -> usize {
    SWITCHES.load(Ordering::Relaxed)
}

/// Индекс текущей задачи (0 — главная задача ядра).
pub fn current_id() -> usize {
    CURRENT.load(Ordering::Relaxed)
}

/// Имя текущей задачи — для отчёта об отказе.
///
/// Не берёт `TABLE_LOCK`: вызывается из обработчика паники, где блокировка
/// может быть уже захвачена упавшим кодом. Чтение имени без блокировки даёт
/// в худшем случае устаревшую строку, что для отчёта приемлемо.
pub fn current_task_name() -> Option<String> {
    let cur = current_id();
    if cur >= MAX_TASKS {
        return None;
    }
    unsafe {
        let task = &(*(&raw const TASKS))[cur];
        if task.state == State::Empty {
            None
        } else {
            Some(task.name.clone())
        }
    }
}

// ==================== Заглушка IRQ0 ====================

// Naked-обработчик таймера. Порядок push'ей ОБЯЗАН совпадать с порядком
// pop'ов и с раскладкой, которую готовит `build_initial_frame`.
core::arch::global_asm!(
    ".section .text",
    ".global timer_switch_stub",
    "timer_switch_stub:",
    "  push rax",
    "  push rbx",
    "  push rcx",
    "  push rdx",
    "  push rsi",
    "  push rdi",
    "  push rbp",
    "  push r8",
    "  push r9",
    "  push r10",
    "  push r11",
    "  push r12",
    "  push r13",
    "  push r14",
    "  push r15",
    // rdi = текущий RSP; Rust-часть решает, куда переключиться, и
    // возвращает RSP следующей задачи (или тот же самый).
    "  mov rdi, rsp",
    "  call {pick}",
    "  mov rsp, rax",
    // EOI строго перед восстановлением: PIC должен получить ответ до
    // того, как мы уйдём в другую задачу.
    "  mov al, 0x20",
    "  out 0x20, al",
    "  pop r15",
    "  pop r14",
    "  pop r13",
    "  pop r12",
    "  pop r11",
    "  pop r10",
    "  pop r9",
    "  pop r8",
    "  pop rbp",
    "  pop rdi",
    "  pop rsi",
    "  pop rdx",
    "  pop rcx",
    "  pop rbx",
    "  pop rax",
    "  iretq",
    pick = sym schedule_from_irq,
);

extern "C" {
    /// Заглушка IRQ0 (ассемблер выше) — ставится в IDT вместо
    /// обычного обработчика таймера.
    pub fn timer_switch_stub();
}

/// Вызывается из заглушки IRQ0. Получает RSP прерванной задачи,
/// возвращает RSP той, которую надо запустить.
///
/// Здесь НЕЛЬЗЯ аллоцировать и брать блокировки, которые может держать
/// прерванный код, — иначе дедлок.
#[no_mangle]
extern "C" fn schedule_from_irq(rsp: u64) -> u64 {
    crate::timer::tick();

    if !SCHED_ACTIVE.load(Ordering::Relaxed) {
        return rsp;
    }

    let now = crate::timer::uptime_ms();
    let cur = CURRENT.load(Ordering::Relaxed);

    unsafe {
        let tasks = &mut *(&raw mut TASKS);

        // Сохраняем стек прерванной задачи.
        if tasks[cur].state != State::Empty {
            tasks[cur].rsp = rsp;
            // Один тик PIT прошёл, пока эта задача была на процессоре.
            // Простой в idle_wait() учитываем отдельно: это не работа.
            if tasks[cur].idle_waiting {
                IDLE_TICKS.fetch_add(1, Ordering::Relaxed);
            } else {
                tasks[cur].runtime_ticks = tasks[cur].runtime_ticks.wrapping_add(1);
            }
            TICKS_TOTAL.fetch_add(1, Ordering::Relaxed);
        }

        // Будим тех, чей срок сна вышел.
        for t in tasks.iter_mut() {
            if let State::Sleeping(until) = t.state {
                if now >= until {
                    t.state = State::Ready;
                }
            }
        }

        // Round-robin: ищем следующую готовую задачу по кругу.
        let mut next = cur;
        for step in 1..=MAX_TASKS {
            let cand = (cur + step) % MAX_TASKS;
            if tasks[cand].state == State::Ready {
                next = cand;
                break;
            }
        }

        if next == cur {
            // Некому передать управление — продолжаем текущую.
            return rsp;
        }

        let root = if tasks[next].cr3 == 0 {
            KERNEL_CR3.load(Ordering::Relaxed)
        } else { tasks[next].cr3 };
        let current_root: u64;
        core::arch::asm!("mov {}, cr3", out(reg) current_root, options(nostack, preserves_flags));
        if current_root != root {
            // Both roots map all kernel code, data and task stacks. Never
            // deallocate the next root here or touch user memory in IRQ.
            core::arch::asm!("mov cr3, {}", in(reg) root, options(nostack, preserves_flags));
        }
        crate::usermode::set_task_kernel_stack(if tasks[next].user_mode {
            tasks[next].kernel_stack_top
        } else { 0 });
        CURRENT.store(next, Ordering::Relaxed);
        SWITCHES.fetch_add(1, Ordering::Relaxed);
        tasks[next].rsp
    }
}

// ==================== Создание задач ====================

/// Точка входа задачи: вызывает функцию и помечает задачу завершённой.
///
/// Адрес этой обёртки кладётся в начальный кадр как RIP, поэтому задача
/// не может «свалиться» с конца — после возврата она корректно умирает.
extern "C" fn task_entry(func: extern "C" fn()) -> ! {
    func();
    exit_current();
}

/// Завершает текущую задачу и отдаёт управление другой. Не возвращается.
pub fn exit_current() -> ! {
    let cur = CURRENT.load(Ordering::Relaxed);
    // Сообщаем менеджеру процессов, что нить умерла — иначе процесс
    // остался бы помеченным как исполняющийся навсегда.
    crate::process::notify_exit(cur, 0);
    unsafe {
        (*(&raw mut TASKS))[cur].state = State::Finished;
    }
    // Ждём, пока таймер нас вытеснит.
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}

/// Принудительно переводит задачу с заданным ID в состояние Finished.
pub fn terminate(id: usize) {
    if id < MAX_TASKS {
        crate::sync::without_interrupts(|| {
            let _guard = TABLE_LOCK.lock();
            unsafe {
                let tasks = &mut *(&raw mut TASKS);
                if tasks[id].state != State::Empty {
                    tasks[id].state = State::Finished;
                }
            }
        });
    }
}

/// Формирует начальный кадр на стеке задачи так, чтобы `iretq` в
/// заглушке «вернулся» в `task_entry`.
///
/// Кадр строится точно в том порядке, в котором заглушка делает `pop`:
/// сверху 15 регистров, под ними — то, что кладёт процессор.
fn build_initial_frame(stack_top: u64, entry: u64, arg: u64) -> u64 {
    unsafe {
        // Выравнивание на 16 байт обязательно для SSE-инструкций,
        // которые компилятор вставляет в обычный код.
        let mut sp = (stack_top & !0xF) as *mut u64;

        // --- кадр процессора (iretq снимает снизу вверх) ---
        sp = sp.sub(1);
        *sp = 0x10; // SS = kernel data
        sp = sp.sub(1);
        *sp = stack_top & !0xF; // RSP задачи
        sp = sp.sub(1);
        *sp = 0x202; // RFLAGS: IF=1 — задача исполняется с прерываниями
        sp = sp.sub(1);
        *sp = 0x08; // CS = kernel code
        sp = sp.sub(1);
        *sp = entry; // RIP

        // --- 15 регистров (заглушка снимает их до iretq) ---
        // Пишем стек СВЕРХУ ВНИЗ, поэтому кладём в обратном порядке к
        // pop'ам: последним записанным окажется r15 — он и будет на
        // вершине, где заглушка его ждёт.
        //   pop-порядок: r15 r14 r13 r12 r11 r10 r9 r8 rbp rdi rsi rdx rcx rbx rax
        sp = sp.sub(1);
        *sp = 0; // rax
        sp = sp.sub(1);
        *sp = 0; // rbx
        sp = sp.sub(1);
        *sp = 0; // rcx
        sp = sp.sub(1);
        *sp = 0; // rdx
        sp = sp.sub(1);
        *sp = 0; // rsi
        sp = sp.sub(1);
        *sp = arg; // rdi — первый аргумент task_entry
        sp = sp.sub(1);
        *sp = 0; // rbp
        for _ in 0..8 {
            sp = sp.sub(1);
            *sp = 0; // r8, r9, r10, r11, r12, r13, r14, r15
        }

        sp as u64
    }
}

/// Создаёт задачу ядра без привязки к процессу. Возвращает её идентификатор.
pub fn spawn(name: &str, func: extern "C" fn()) -> Option<usize> {
    spawn_pid(name, func, 0)
}

/// Создаёт задачу, привязанную к процессу `pid` (0 — задача ядра).
/// Возвращает идентификатор задачи (индекс слота планировщика).
pub fn spawn_pid(name: &str, func: extern "C" fn(), pid: u32) -> Option<usize> {
    spawn_inner(name, Some(func), pid, None, None)
}

/// Runs a task with privately owned page tables. The caller must ensure all
/// kernel mappings are present; this is not yet a Ring-3 process launcher.
pub fn spawn_with_address_space(name: &str, func: extern "C" fn(), pid: u32,
    space: crate::mm::address_space::AddressSpace) -> Option<usize> {
    spawn_inner(name, Some(func), pid, Some(Box::new(space)), None)
}

/// Schedule code in a private address space at CPL 3. This is a low-level
/// mechanism; it is not yet connected to the process manager or Dinit ABI.
pub fn spawn_private_user(name: &str, entry: u64, stack: u64,
    space: crate::mm::address_space::AddressSpace) -> Option<usize> {
    spawn_private_user_pid(name, entry, stack, space, 0)
}

/// Caller must link the slot to the process atomically before enabling IRQs.
pub fn spawn_private_user_pid(name: &str, entry: u64, stack: u64,
    space: crate::mm::address_space::AddressSpace, pid: u32) -> Option<usize> {
    if !space.user_accessible(entry, false) || stack == 0
        || !space.user_accessible(stack - 1, true) { return None; }
    spawn_inner(name, None, pid, Some(Box::new(space)), Some((entry, stack)))
}

/// Construct a private root from the scheduler's kernel page table.
pub fn new_user_address_space() -> Result<crate::mm::address_space::AddressSpace, crate::mm::address_space::Error> {
    let root = KERNEL_CR3.load(Ordering::SeqCst) & 0x000f_ffff_ffff_f000;
    if root == 0 { return Err(crate::mm::address_space::Error::InvalidPhysicalAddress); }
    let table = unsafe { &*(root as *const [u64; 512]) };
    crate::mm::address_space::AddressSpace::new(table)
}

fn spawn_inner(name: &str, func: Option<extern "C" fn()>, pid: u32,
    mut space: Option<Box<crate::mm::address_space::AddressSpace>>,
    user: Option<(u64, u64)>) -> Option<usize> {
    crate::sync::without_interrupts(|| {
    let _g = TABLE_LOCK.lock();
    unsafe {
        let tasks = &mut *(&raw mut TASKS);
        for (i, t) in tasks.iter_mut().enumerate() {
            if t.state != State::Empty && t.state != State::Finished {
                continue;
            }
            if i == 0 || i == CURRENT.load(Ordering::Relaxed) {
                continue; // Never reclaim the currently active page tables.
            }
            let stack: Box<[u8]> = vec![0u8; STACK_SIZE].into_boxed_slice();
            let top = stack.as_ptr() as u64 + STACK_SIZE as u64;
            let rsp = if let Some((entry, stack)) = user {
                let rsp = build_initial_frame(top, entry, 0);
                let frame = rsp as *mut u64;
                *frame.add(16) = crate::usermode::USER_CS as u64;
                *frame.add(18) = stack;
                *frame.add(19) = crate::usermode::USER_DS as u64;
                rsp
            } else {
                let func = func?;
                build_initial_frame(top, task_entry as *const () as u64, func as *const () as u64)
            };

            t.rsp = rsp;
            t.user_mode = user.is_some();
            t.kernel_stack_top = top & !0xf;
            t.exit_code = 0;
            t._stack = Some(stack);
            t.cr3 = space.as_ref().map_or(0, |s| s.root_physical());
            t._address_space = space.take();
            t.name = String::from(name);
            t.runtime_ticks = 0;
            t.pid = pid;
            // Помечаем готовой в последнюю очередь: до этого момента
            // планировщик не должен на неё переключиться.
            core::sync::atomic::compiler_fence(Ordering::SeqCst);
            t.state = State::Ready;
            return Some(i);
        }
    }
    None
    })
}

/// Усыпляет текущую задачу на `ms` миллисекунд.
pub fn sleep_ms(ms: u64) {
    let until = crate::timer::uptime_ms() + ms;
    unsafe {
        let cur = CURRENT.load(Ordering::Relaxed);
        (*(&raw mut TASKS))[cur].state = State::Sleeping(until);
    }
    // Ждём вытеснения; таймер разбудит нас, когда срок выйдет.
    while crate::timer::uptime_ms() < until {
        unsafe { core::arch::asm!("hlt") };
    }
}

/// Добровольно уступает процессор (не дожидаясь кванта).
pub fn yield_now() {
    unsafe { core::arch::asm!("int 32", options(nostack)) };
}

/// Включает вытесняющее переключение. Слот 0 — текущий поток ядра.
pub fn start() {
    let root: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) root, options(nostack, preserves_flags)); }
    KERNEL_CR3.store(root, Ordering::SeqCst);
    unsafe {
        let tasks = &mut *(&raw mut TASKS);
        tasks[0].state = State::Ready;
        tasks[0].name = String::from("dinit");
    }
    SCHED_ACTIVE.store(true, Ordering::SeqCst);
}

/// Выключает переключение (например, перед kexec).
pub fn stop() {
    SCHED_ACTIVE.store(false, Ordering::SeqCst);
}

/// Ждёт завершения задачи `id`, не блокируя процессор.
pub fn join(id: usize) {
    loop {
        let done = unsafe {
            let s = (*(&raw const TASKS))[id].state;
            s == State::Finished || s == State::Empty
        };
        if done {
            return;
        }
        unsafe { core::arch::asm!("hlt") };
    }
}

/// Снимок состояния задачи для отчётов (CLI `threads`, Task Manager).
#[derive(Clone, Debug)]
pub struct TaskInfo {
    pub id: usize,
    pub name: String,
    pub state: State,
    /// Тики PIT, проведённые задачей на процессоре.
    pub runtime_ticks: u64,
    /// PID процесса-владельца, 0 — задача ядра.
    pub pid: u32,
}

/// Список задач со снимком их состояния.
pub fn list() -> Vec<TaskInfo> {
    crate::sync::without_interrupts(|| {
    let _g = TABLE_LOCK.lock();
    let mut out = Vec::new();
    unsafe {
        for (i, t) in (*(&raw const TASKS)).iter().enumerate() {
            if t.state != State::Empty {
                out.push(TaskInfo {
                    id: i,
                    name: t.name.clone(),
                    state: t.state,
                    runtime_ticks: t.runtime_ticks,
                    pid: t.pid,
                });
            }
        }
    }
    out
    })
}

/// Всего тиков PIT, учтённых планировщиком с момента старта.
pub fn ticks_total() -> usize {
    TICKS_TOTAL.load(Ordering::Relaxed)
}

/// Тики PIT, пришедшиеся на добровольный простой (idle_wait).
pub fn idle_ticks() -> usize {
    IDLE_TICKS.load(Ordering::Relaxed)
}

/// Честный `hlt`: помечает текущую задачу простаивающей, чтобы тики
/// ожидания не записывались ей в счёт работы процессора. Использовать
/// во всех циклах ожидания события (ввод, сеть, сон) вместо голого hlt.
pub fn idle_wait() {
    let cur = CURRENT.load(Ordering::Relaxed);
    unsafe {
        let tasks = &mut *(&raw mut TASKS);
        tasks[cur].idle_waiting = true;
        core::arch::asm!("hlt");
        // Сюда исполнение возвращается только когда планировщик снова
        // поставил наш слот на процессор — флаг снимаем со своего слота.
        tasks[cur].idle_waiting = false;
    }
}
/// Человекочитаемое имя состояния задачи.
pub fn state_str(state: State) -> &'static str {
    match state {
        State::Empty => "empty",
        State::Ready => "running",
        State::Sleeping(_) => "sleeping",
        State::Finished => "finished",
    }
}

/// True only for the scheduler's new private Ring-3 task type.
pub fn current_is_private_user() -> bool {
    let cur = CURRENT.load(Ordering::Relaxed);
    unsafe { (*(&raw const TASKS))[cur].user_mode }
}

/// Only call from a private-task syscall while interrupts are disabled.
pub fn current_user_range_writable(address: u64, len: usize) -> bool {
    let cur = CURRENT.load(Ordering::Relaxed);
    let task = unsafe { &(*(&raw const TASKS))[cur] };
    task.user_mode && task._address_space.as_ref().map_or(false, |s| s.user_range_accessible(address, len, true))
}
pub fn current_copy_to_user(address: u64, data: &[u8]) -> bool {
    let cur = CURRENT.load(Ordering::Relaxed);
    let task = unsafe { &mut (*(&raw mut TASKS))[cur] };
    task.user_mode && task._address_space.as_mut().map_or(false, |s| s.copy_to_user(address, data).is_ok())
}

/// Terminate a private user task from a syscall or fault, then wait for the
/// timer to switch away. The finished slot retains its tables until reused.
pub fn finish_private_user(code: i64) -> ! {
    let cur = CURRENT.load(Ordering::Relaxed);
    // Faults can arrive with interrupts disabled. The process table is protected
    // by an IRQ-safe lock; do not invoke scheduler termination from this path.
    crate::process::notify_private_exit(cur, code as i32);
    unsafe {
        let task = &mut (*(&raw mut TASKS))[cur];
        task.exit_code = code;
        task.state = State::Finished;
        core::arch::asm!("sti", options(nomem, nostack));
        loop { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

static RING3_TEST_RESULTS: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
pub fn record_ring3_test(value: u64, cs: u64) {
    if cs & 3 != 3 { return; }
    match value {
        0x31 => RING3_TEST_RESULTS[0].store(1, Ordering::SeqCst),
        0x72 => RING3_TEST_RESULTS[1].store(1, Ordering::SeqCst),
        _ => {}
    }
}

/// QEMU-only live CPL3 and page-fault isolation test. Two tasks execute the
/// same machine code at the same VA, but read different private data pages.
/// The third task attempts to read supervisor-only kernel memory and must #PF.
pub fn ring3_address_space_selftest() -> bool {
    use crate::mm::address_space::{AddressSpace, Error, USER_BASE};
    const CODE: &[u8] = &[
        0x48,0xbb,0x00,0x10,0x00,0x00,0x80,0x00,0x00,0x00,
        0xb9,0x00,0x00,0x80,0x00,0xf3,0x90,0xff,0xc9,0x75,0xfa,
        0x0f,0xb6,0x3b,0x66,0x8c,0xc8,0x0f,0xb7,0xf0,
        0xb8,0x4d,0x56,0x58,0x44,0x0f,0x05,0x0f,0x0b,
    ];
    const FAULT: &[u8] = &[
        0x48,0xb8,0x00,0x00,0x10,0x00,0x00,0x00,0x00,0x00,0x8a,0x00,
        0xb8,0x3c,0x00,0x00,0x00,0xbf,0x01,0x00,0x00,0x00,0x0f,0x05,0x0f,0x0b,
    ];
    let root = KERNEL_CR3.load(Ordering::SeqCst) & 0x000f_ffff_ffff_f000;
    if root == 0 { return false; }
    let table = unsafe { &*(root as *const [u64; 512]) };
    let make = |code: &[u8], value| -> Result<AddressSpace, Error> {
        let mut space = AddressSpace::new(table)?;
        space.map_zeroed(USER_BASE, false)?;
        space.map_zeroed(USER_BASE + 0x1000, false)?;
        space.map_zeroed(USER_BASE + 0x3000, true)?;
        space.page_for_loading(USER_BASE)?[..code.len()].copy_from_slice(code);
        space.page_for_loading(USER_BASE + 0x1000)?[0] = value;
        Ok(space)
    };
    let Ok(a) = make(CODE, 0x31) else { return false; };
    let Ok(b) = make(CODE, 0x72) else { return false; };
    RING3_TEST_RESULTS[0].store(0, Ordering::SeqCst);
    RING3_TEST_RESULTS[1].store(0, Ordering::SeqCst);
    let stack = USER_BASE + 0x4000 - 16;
    let Some(a_id) = spawn_private_user("ring3-test-a", USER_BASE, stack, a) else { return false; };
    let b_id = spawn_private_user("ring3-test-b", USER_BASE, stack, b);
    join(a_id);
    let Some(b_id) = b_id else { return false; };
    join(b_id);
    if !RING3_TEST_RESULTS.iter().all(|r| r.load(Ordering::SeqCst) == 1) { return false; }
    let Ok(fault_space) = make(FAULT, 0) else { return false; };
    let Some(fault_id) = spawn_private_user("ring3-fault-test", USER_BASE, stack, fault_space) else { return false; };
    join(fault_id);
    let code = unsafe { (*(&raw const TASKS))[fault_id].exit_code };
    code == -14
}

/// QEMU integration: read a separately built ELF through the system VFS,
/// map its segments into private pages, and execute its actual entry in CPL3.
/// The probe does not implement Dinit or any production supervisor policy.
pub fn private_elf_selftest() -> bool {
    use crate::process::{self, ProcessState, SpawnRequest};
    let req = SpawnRequest { name: String::from("elf-test"), uid: 0, parent: 0,
        caps: process::cap::IPC_SEND | process::cap::IPC_RECEIVE, cwd: String::from("/"), image_path: String::from("/system/tests/private_elf_probe.elf") };
    RING3_TEST_RESULTS[0].store(0, Ordering::SeqCst);
    let Ok(pid) = process::spawn(&req) else { return false; };
    if process::exec(pid).is_err() { let _ = process::kill(pid, -1); let _ = process::reap(pid); return false; }
    let Ok(info) = process::info(pid) else { return false; };
    let Some(id) = info.sched_id else { return false; };
    join(id);
    let ok = RING3_TEST_RESULTS[0].load(Ordering::SeqCst) == 1
        && process::info(pid).map_or(false, |p| p.state == ProcessState::Zombie && p.exit_code == Some(0));
    let reaped = process::reap(pid).ok() == Some(Some(0));
    if !ok || !reaped { return false; }
    let denied_req = SpawnRequest { caps: 0, image_path: String::from("/system/tests/private_elf_denied.elf"), ..req.clone() };
    RING3_TEST_RESULTS[1].store(0, Ordering::SeqCst);
    let Ok(denied_pid) = process::spawn(&denied_req) else { return false; };
    if process::exec(denied_pid).is_err() { let _ = process::kill(denied_pid, -1); let _ = process::reap(denied_pid); return false; }
    let Ok(denied_info) = process::info(denied_pid) else { return false; };
    let Some(denied_id) = denied_info.sched_id else { return false; };
    join(denied_id);
    let denied_ok = RING3_TEST_RESULTS[1].load(Ordering::SeqCst) == 1
        && process::info(denied_pid).map_or(false, |p| p.state == ProcessState::Zombie && p.exit_code == Some(0));
    let denied_reaped = process::reap(denied_pid).ok() == Some(Some(0));
    if !denied_ok || !denied_reaped { return false; }
    let fault_req = SpawnRequest { image_path: String::from("/system/tests/private_elf_fault.elf"), ..req };
    let Ok(fault_pid) = process::spawn(&fault_req) else { return false; };
    if process::exec(fault_pid).is_err() { let _ = process::kill(fault_pid, -1); let _ = process::reap(fault_pid); return false; }
    let Ok(fault_info) = process::info(fault_pid) else { return false; };
    let Some(fault_id) = fault_info.sched_id else { return false; };
    join(fault_id);
    let faulted = process::info(fault_pid).map_or(false, |p| p.state == ProcessState::Zombie && p.exit_code == Some(-14));
    let reaped_fault = process::reap(fault_pid).ok() == Some(Some(-14));
    faulted && reaped_fault
}

// Real CR3 context-switch smoke test; it runs only in explicitly enabled
// test builds. This is a supervisor-mode test, not proof of Ring-3 isolation.
static VM_TEST_RESULTS: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
extern "C" fn vm_worker0() { vm_worker(0, 0x31); }
extern "C" fn vm_worker1() { vm_worker(1, 0x72); }
fn vm_worker(index: usize, expected: u8) {
    for _ in 0..8 {
        let value = unsafe { core::ptr::read_volatile(crate::mm::address_space::USER_BASE as *const u8) };
        if value != expected { VM_TEST_RESULTS[index].store(2, Ordering::SeqCst); return; }
        yield_now();
    }
    VM_TEST_RESULTS[index].store(1, Ordering::SeqCst);
}

pub fn address_space_selftest() -> bool {
    use crate::mm::address_space::{AddressSpace, USER_BASE};
    let root = KERNEL_CR3.load(Ordering::SeqCst) & 0x000f_ffff_ffff_f000;
    if root == 0 { return false; }
    let root_table = unsafe { &*(root as *const [u64; 512]) };
    let make = |value| -> Result<AddressSpace, crate::mm::address_space::Error> {
        let mut space = AddressSpace::new(root_table)?;
        space.map_zeroed(USER_BASE, true)?;
        space.page_for_loading(USER_BASE)?[0] = value;
        Ok(space)
    };
    let Ok(a) = make(0x31) else { return false; };
    let Ok(b) = make(0x72) else { return false; };
    VM_TEST_RESULTS[0].store(0, Ordering::SeqCst);
    VM_TEST_RESULTS[1].store(0, Ordering::SeqCst);
    let Some(a_id) = spawn_with_address_space("vm-test-a", vm_worker0, 0, a) else { return false; };
    let b_id = spawn_with_address_space("vm-test-b", vm_worker1, 0, b);
    join(a_id);
    let Some(b_id) = b_id else { return false; };
    join(b_id);
    let after: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) after, options(nostack, preserves_flags)); }
    after == KERNEL_CR3.load(Ordering::SeqCst)
        && VM_TEST_RESULTS.iter().all(|v| v.load(Ordering::SeqCst) == 1)
}

// ==================== Демонстрация и проверка ====================

/// Счётчики рабочих задач — доказательство, что они шли одновременно.
static COUNTERS: [AtomicUsize; 4] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

extern "C" fn worker0() {
    worker(0)
}
extern "C" fn worker1() {
    worker(1)
}
extern "C" fn worker2() {
    worker(2)
}

/// Задача, которая спит: проверяет состояние Sleeping и пробуждение.
extern "C" fn sleeper() {
    for _ in 0..4 {
        COUNTERS[3].fetch_add(1, Ordering::Relaxed);
        sleep_ms(50);
    }
}

/// Рабочая задача: крутит счётчик, НИЧЕГО не вызывая для переключения.
/// Если счётчики всех задач растут — значит их вытесняет таймер.
fn worker(idx: usize) {
    let deadline = crate::timer::uptime_ms() + 400;
    while crate::timer::uptime_ms() < deadline {
        COUNTERS[idx].fetch_add(1, Ordering::Relaxed);
        // Пустой цикл: специально без hlt/yield — проверяем вытеснение.
        for _ in 0..2000 {
            core::hint::spin_loop();
        }
    }
}

/// `threads` — самопроверка вытесняющей многозадачности.
pub fn selftest() {
    crate::println!("  [sched] запуск 4 задач: 3 счётчика без yield + 1 спящая");

    for c in COUNTERS.iter() {
        c.store(0, Ordering::Relaxed);
    }
    let before = switch_count();

    let ids = [
        spawn("worker-0", worker0),
        spawn("worker-1", worker1),
        spawn("worker-2", worker2),
        spawn("sleeper", sleeper),
    ];
    for id in ids.iter().flatten() {
        join(*id);
    }

    let switches = switch_count() - before;
    let c: [usize; 3] = [
        COUNTERS[0].load(Ordering::Relaxed),
        COUNTERS[1].load(Ordering::Relaxed),
        COUNTERS[2].load(Ordering::Relaxed),
    ];

    crate::println!(
        "  [sched] итераций: worker-0={}, worker-1={}, worker-2={}",
        c[0],
        c[1],
        c[2]
    );
    crate::println!(
        "  [sched] спящая задача проснулась {} раз(а)",
        COUNTERS[3].load(Ordering::Relaxed)
    );
    crate::println!("  [sched] переключений контекста: {}", switches);

    let slept = COUNTERS[3].load(Ordering::Relaxed);
    let all_ran = c.iter().all(|&x| x > 0) && slept == 4;
    yield_now();
    stop();
    if all_ran && switches > 0 {
        crate::println!("  [sched] САМОПРОВЕРКА ПРОЙДЕНА: все задачи выполнялись одновременно");
    } else {
        crate::println!("  [sched] САМОПРОВЕРКА НЕ ПРОЙДЕНА");
    }
}

/// CLI: `threads` / `threads list`.
pub fn cmd_threads(arg: &str) {
    match arg.trim() {
        "list" => {
            crate::println!("  Задачи планировщика (текущая: [{}]):", current_id());
            for t in list() {
                crate::println!(
                    "    [{}] {:<14} {:<18} pid={:<4} cpu={} тик",
                    t.id,
                    t.name,
                    state_str(t.state),
                    t.pid,
                    t.runtime_ticks
                );
            }
            crate::println!("  Переключений контекста всего: {}", switch_count());
            crate::println!(
                "  Тиков PIT учтено: {} (из них простой процессора: {})",
                ticks_total(),
                idle_ticks()
            );
        }
        "" | "test" => selftest(),
        _ => crate::println!("threads [list|test] — вытесняющая многозадачность"),
    }
}
