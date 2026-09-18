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
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::spinlock::SpinLock;

/// Размер стека задачи.
const STACK_SIZE: usize = 64 * 1024;
/// Максимум задач (статическая таблица — без аллокаций в обработчике IRQ).
const MAX_TASKS: usize = 16;

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
    name: String,
}

impl Task {
    const fn empty() -> Self {
        Task {
            state: State::Empty,
            rsp: 0,
            _stack: None,
            name: String::new(),
        }
    }
}

/// Таблица задач. Доступ из обработчика прерывания — только через
/// `SCHED_ACTIVE`, без блокировок (мы уже в прерывании, вытеснить нас
/// некому: одно ядро и IF=0).
static mut TASKS: [Task; MAX_TASKS] = [const { Task::empty() }; MAX_TASKS];
/// Индекс текущей задачи.
static CURRENT: AtomicUsize = AtomicUsize::new(0);
/// Планировщик включён (заглушка IRQ0 переключает контекст).
static SCHED_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Счётчик переключений — для доказательства, что они реально идут.
static SWITCHES: AtomicUsize = AtomicUsize::new(0);
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
    unsafe {
        let cur = CURRENT.load(Ordering::Relaxed);
        (*(&raw mut TASKS))[cur].state = State::Finished;
    }
    // Ждём, пока таймер нас вытеснит.
    loop {
        unsafe { core::arch::asm!("hlt") };
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

/// Создаёт задачу. Возвращает её идентификатор.
pub fn spawn(name: &str, func: extern "C" fn()) -> Option<usize> {
    let _g = TABLE_LOCK.lock();
    unsafe {
        let tasks = &mut *(&raw mut TASKS);
        for (i, t) in tasks.iter_mut().enumerate() {
            if t.state != State::Empty && t.state != State::Finished {
                continue;
            }
            if i == 0 {
                continue; // слот 0 — главная задача ядра
            }
            let stack: Box<[u8]> = vec![0u8; STACK_SIZE].into_boxed_slice();
            let top = stack.as_ptr() as u64 + STACK_SIZE as u64;
            let rsp = build_initial_frame(top, task_entry as u64, func as u64);

            t.rsp = rsp;
            t._stack = Some(stack);
            t.name = String::from(name);
            // Помечаем готовой в последнюю очередь: до этого момента
            // планировщик не должен на неё переключиться.
            core::sync::atomic::compiler_fence(Ordering::SeqCst);
            t.state = State::Ready;
            return Some(i);
        }
    }
    None
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

/// Список задач: `(id, имя, состояние)`.
pub fn list() -> Vec<(usize, String, State)> {
    let _g = TABLE_LOCK.lock();
    let mut out = Vec::new();
    unsafe {
        for (i, t) in (*(&raw const TASKS)).iter().enumerate() {
            if t.state != State::Empty {
                out.push((i, t.name.clone(), t.state));
            }
        }
    }
    out
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
            crate::println!("  Задачи планировщика:");
            for (id, name, state) in list() {
                crate::println!("    [{}] {:<14} {:?}", id, name, state);
            }
            crate::println!("  Переключений контекста всего: {}", switch_count());
        }
        "" | "test" => selftest(),
        _ => crate::println!("threads [list|test] — вытесняющая многозадачность"),
    }
}
