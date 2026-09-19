//! ВЫТЕСНЯЮЩАЯ МНОГОЗАДАЧНОСТЬ — планировщик процессов и потоков ядра.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::spinlock::SpinLock;

/// Размер стека задачи (64 КиБ).
const STACK_SIZE: usize = 64 * 1024;
/// Максимум задач в системе.
pub const MAX_TASKS: usize = 32;

/// Приоритет выполнения задачи.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Priority {
    Realtime,
    High,
    Normal,
    Idle,
}

impl Priority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Priority::Realtime => "Realtime",
            Priority::High => "High",
            Priority::Normal => "Normal",
            Priority::Idle => "Idle",
        }
    }
}

/// Вычисление длины кванта времени в тиках PIT (100 Гц = 10 мс на тик).
pub fn quantum_ticks(p: Priority) -> u64 {
    match p {
        Priority::Realtime => 1,  // 10 мс
        Priority::High => 2,      // 20 мс
        Priority::Normal => 4,    // 40 мс
        Priority::Idle => 20,     // 200 мс
    }
}

/// Состояние задачи.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Empty,
    Ready,
    Sleeping(u64),
    Finished,
    Suspended,
}

/// Снапшот метрик задачи для менеджмера задач (taskmgr)
#[derive(Clone, Debug)]
pub struct TaskInfo {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub state: State,
    pub priority: Priority,
    pub cpu_ticks: u64,
    pub user_ticks: u64,
    pub kernel_ticks: u64,
    pub switches_vol: u64,
    pub switches_invol: u64,
    pub start_ms: u64,
    pub exit_code: i64,
    pub hung: bool,
    pub owner_pid: u32,
}

/// Задача планировщика ядра.
pub struct Task {
    pub state: State,
    pub rsp: u64,
    pub _stack: Option<Box<[u8]>>,
    pub name: String,
    pub pid: u32,
    pub ppid: u32,
    pub priority: Priority,
    pub cpu_ticks: u64,
    pub user_ticks: u64,
    pub kernel_ticks: u64,
    pub switches_vol: u64,
    pub switches_invol: u64,
    pub start_ms: u64,
    pub exit_code: i64,
    pub hung: bool,
    pub quantum_used: u64,
    pub owner_pid: u32,
    pub tls_base: u64,
}

impl Task {
    const fn empty() -> Self {
        Task {
            state: State::Empty,
            rsp: 0,
            _stack: None,
            name: String::new(),
            pid: 0,
            ppid: 0,
            priority: Priority::Normal,
            cpu_ticks: 0,
            user_ticks: 0,
            kernel_ticks: 0,
            switches_vol: 0,
            switches_invol: 0,
            start_ms: 0,
            exit_code: 0,
            hung: false,
            quantum_used: 0,
            owner_pid: 0,
            tls_base: 0,
        }
    }
}

static mut TASKS: [Task; MAX_TASKS] = [const { Task::empty() }; MAX_TASKS];
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static SCHED_ACTIVE: AtomicBool = AtomicBool::new(false);
static SWITCHES: AtomicUsize = AtomicUsize::new(0);
static NEXT_PID: AtomicUsize = AtomicUsize::new(10);
static SAME_TASK_STREAK: AtomicUsize = AtomicUsize::new(0);
static TABLE_LOCK: SpinLock<()> = SpinLock::new(());

pub fn switch_count() -> usize {
    SWITCHES.load(Ordering::Relaxed)
}

pub fn current_id() -> usize {
    CURRENT.load(Ordering::Relaxed)
}

pub fn current_pid() -> u32 {
    unsafe {
        let cur = CURRENT.load(Ordering::Relaxed);
        (*(&raw mut TASKS))[cur].pid
    }
}

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
    "  mov rdi, rsp",
    "  call {pick}",
    "  mov rsp, rax",
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
    pub fn timer_switch_stub();
}

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

        if tasks[cur].state != State::Empty {
            tasks[cur].rsp = rsp;
            tasks[cur].cpu_ticks += 1;
            tasks[cur].kernel_ticks += 1;
            tasks[cur].quantum_used += 1;
        }

        // Пробуждение спящих задач
        for t in tasks.iter_mut() {
            if let State::Sleeping(until) = t.state {
                if now >= until {
                    t.state = State::Ready;
                }
            }
        }

        // Проверка кванта времени текущей задачи
        let cur_quantum = quantum_ticks(tasks[cur].priority);
        let force_resched = tasks[cur].quantum_used >= cur_quantum;

        // Поиск задач по высшему приоритету: Realtime > High > Normal > Idle
        let mut best_idx = cur;
        let mut found = false;

        for prio in [Priority::Realtime, Priority::High, Priority::Normal, Priority::Idle] {
            let mut cand = (cur + 1) % MAX_TASKS;
            for _ in 0..MAX_TASKS {
                if tasks[cand].state == State::Ready && tasks[cand].priority == prio && !tasks[cand].hung {
                    // Если текущая ещё не исчерпала квант и имеет тот же высший приоритет, продолжаем её
                    if cand == cur && !force_resched {
                        best_idx = cur;
                        found = true;
                        break;
                    }
                    if cand != cur || force_resched {
                        best_idx = cand;
                        found = true;
                        break;
                    }
                }
                cand = (cand + 1) % MAX_TASKS;
            }
            if found {
                break;
            }
        }

        if !found || (best_idx == cur && !force_resched) {
            let streak = SAME_TASK_STREAK.fetch_add(1, Ordering::Relaxed);
            if streak > 100 && tasks[cur].pid != 1 && tasks[cur].pid != 0 {
                // Watchdog: снятие зависшей задачи Ring 0
                tasks[cur].hung = true;
                tasks[cur].state = State::Finished;
                tasks[cur].exit_code = -11;
                crate::syslog::log_line(&alloc::format!("[watchdog] Задача '{}' (PID {}) зависла — снята супервизором", tasks[cur].name, tasks[cur].pid));
            }
            return rsp;
        }

        SAME_TASK_STREAK.store(0, Ordering::Relaxed);
        tasks[cur].switches_invol += 1;
        tasks[best_idx].quantum_used = 0;
        tasks[best_idx].switches_vol += 1;

        if tasks[best_idx].tls_base != 0 {
            // Установка MSR_FS_BASE для Thread Local Storage
            const MSR_FS_BASE: u32 = 0xC0000100;
            let val = tasks[best_idx].tls_base;
            let low = val as u32;
            let high = (val >> 32) as u32;
            core::arch::asm!("wrmsr", in("ecx") MSR_FS_BASE, in("eax") low, in("edx") high, options(nomem, nostack));
        }

        CURRENT.store(best_idx, Ordering::Relaxed);
        SWITCHES.fetch_add(1, Ordering::Relaxed);
        tasks[best_idx].rsp
    }
}

extern "C" fn task_entry(func: extern "C" fn()) -> ! {
    func();
    exit_current();
}

pub fn exit_current() -> ! {
    unsafe {
        let cur = CURRENT.load(Ordering::Relaxed);
        (*(&raw mut TASKS))[cur].state = State::Finished;
    }
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}

pub fn terminate_by_pid(pid: u32) {
    let _guard = TABLE_LOCK.lock();
    unsafe {
        let tasks = &mut *(&raw mut TASKS);
        for t in tasks.iter_mut() {
            if t.pid == pid && t.state != State::Empty {
                t.state = State::Finished;
                t.exit_code = -9;
            }
        }
    }
}

pub fn pid_to_index(pid: u32) -> Option<usize> {
    unsafe {
        let tasks = &*(&raw const TASKS);
        for (i, t) in tasks.iter().enumerate() {
            if t.pid == pid && t.state != State::Empty {
                return Some(i);
            }
        }
    }
    None
}

pub fn task_info(pid: u32) -> Option<TaskInfo> {
    let _guard = TABLE_LOCK.lock();
    unsafe {
        let tasks = &*(&raw const TASKS);
        for t in tasks.iter() {
            if t.pid == pid && t.state != State::Empty {
                return Some(TaskInfo {
                    pid: t.pid,
                    ppid: t.ppid,
                    name: t.name.clone(),
                    state: t.state,
                    priority: t.priority,
                    cpu_ticks: t.cpu_ticks,
                    user_ticks: t.user_ticks,
                    kernel_ticks: t.kernel_ticks,
                    switches_vol: t.switches_vol,
                    switches_invol: t.switches_invol,
                    start_ms: t.start_ms,
                    exit_code: t.exit_code,
                    hung: t.hung,
                    owner_pid: t.owner_pid,
                });
            }
        }
    }
    None
}

pub fn set_priority(pid: u32, prio: Priority) -> bool {
    let _guard = TABLE_LOCK.lock();
    unsafe {
        let tasks = &mut *(&raw mut TASKS);
        for t in tasks.iter_mut() {
            if t.pid == pid && t.state != State::Empty {
                t.priority = prio;
                return true;
            }
        }
    }
    false
}

pub fn set_state(pid: u32, st: State) -> bool {
    let _guard = TABLE_LOCK.lock();
    unsafe {
        let tasks = &mut *(&raw mut TASKS);
        for t in tasks.iter_mut() {
            if t.pid == pid && t.state != State::Empty {
                t.state = st;
                return true;
            }
        }
    }
    false
}

fn build_initial_frame(stack_top: u64, entry: u64, arg: u64) -> u64 {
    unsafe {
        let mut sp = (stack_top & !0xF) as *mut u64;

        sp = sp.sub(1);
        *sp = 0x10;
        sp = sp.sub(1);
        *sp = stack_top & !0xF;
        sp = sp.sub(1);
        *sp = 0x202;
        sp = sp.sub(1);
        *sp = 0x08;
        sp = sp.sub(1);
        *sp = entry;

        sp = sp.sub(1);
        *sp = 0;
        sp = sp.sub(1);
        *sp = 0;
        sp = sp.sub(1);
        *sp = 0;
        sp = sp.sub(1);
        *sp = 0;
        sp = sp.sub(1);
        *sp = 0;
        sp = sp.sub(1);
        *sp = arg;
        sp = sp.sub(1);
        *sp = 0;
        for _ in 0..8 {
            sp = sp.sub(1);
            *sp = 0;
        }

        sp as u64
    }
}

pub fn spawn_pid(name: &str, func: extern "C" fn(), pid: u32, priority: Priority) -> Option<u32> {
    let _g = TABLE_LOCK.lock();
    unsafe {
        let tasks = &mut *(&raw mut TASKS);
        for (i, t) in tasks.iter_mut().enumerate() {
            if t.state != State::Empty && t.state != State::Finished {
                continue;
            }
            if i == 0 && pid != 1 {
                continue;
            }
            let stack: Box<[u8]> = vec![0u8; STACK_SIZE].into_boxed_slice();
            let top = stack.as_ptr() as u64 + STACK_SIZE as u64;
            let rsp = build_initial_frame(top, task_entry as *const () as u64, func as *const () as u64);

            t.rsp = rsp;
            t._stack = Some(stack);
            t.name = String::from(name);
            t.pid = pid;
            t.ppid = 1;
            t.priority = priority;
            t.cpu_ticks = 0;
            t.user_ticks = 0;
            t.kernel_ticks = 0;
            t.switches_vol = 0;
            t.switches_invol = 0;
            t.start_ms = crate::timer::uptime_ms();
            t.exit_code = 0;
            t.hung = false;
            t.quantum_used = 0;
            t.owner_pid = pid;
            t.tls_base = 0;

            core::sync::atomic::compiler_fence(Ordering::SeqCst);
            t.state = State::Ready;
            return Some(pid);
        }
    }
    None
}

pub fn spawn(name: &str, func: extern "C" fn()) -> Option<u32> {
    let new_pid = NEXT_PID.fetch_add(1, Ordering::Relaxed) as u32;
    spawn_pid(name, func, new_pid, Priority::Normal)
}

pub fn sleep_ms(ms: u64) {
    let until = crate::timer::uptime_ms() + ms;
    unsafe {
        let cur = CURRENT.load(Ordering::Relaxed);
        (*(&raw mut TASKS))[cur].state = State::Sleeping(until);
    }
    while crate::timer::uptime_ms() < until {
        unsafe { core::arch::asm!("hlt") };
    }
}

pub fn yield_now() {
    unsafe { core::arch::asm!("int 32", options(nostack)) };
}

pub fn start() {
    unsafe {
        let tasks = &mut *(&raw mut TASKS);
        tasks[0].state = State::Ready;
        tasks[0].name = String::from("dinit");
        tasks[0].pid = 1;
        tasks[0].priority = Priority::High;
        tasks[0].start_ms = crate::timer::uptime_ms();
    }
    SCHED_ACTIVE.store(true, Ordering::SeqCst);
}

pub fn stop() {
    SCHED_ACTIVE.store(false, Ordering::SeqCst);
}

pub fn join(pid: u32) {
    loop {
        let done = unsafe {
            let tasks = &*(&raw const TASKS);
            let mut is_done = true;
            for t in tasks.iter() {
                if t.pid == pid {
                    if t.state != State::Finished && t.state != State::Empty {
                        is_done = false;
                    }
                    break;
                }
            }
            is_done
        };
        if done {
            return;
        }
        unsafe { core::arch::asm!("hlt") };
    }
}

pub fn list_info() -> Vec<TaskInfo> {
    let _g = TABLE_LOCK.lock();
    let mut out = Vec::new();
    unsafe {
        for t in (*(&raw const TASKS)).iter() {
            if t.state != State::Empty {
                out.push(TaskInfo {
                    pid: t.pid,
                    ppid: t.ppid,
                    name: t.name.clone(),
                    state: t.state,
                    priority: t.priority,
                    cpu_ticks: t.cpu_ticks,
                    user_ticks: t.user_ticks,
                    kernel_ticks: t.kernel_ticks,
                    switches_vol: t.switches_vol,
                    switches_invol: t.switches_invol,
                    start_ms: t.start_ms,
                    exit_code: t.exit_code,
                    hung: t.hung,
                    owner_pid: t.owner_pid,
                });
            }
        }
    }
    out
}
