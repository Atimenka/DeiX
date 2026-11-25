//! Nano-Core orchestrator — движок загрузки и мониторинга нано-служб.
//!
//! Работа:
//! 1. `init()` — собирает DAG из манифеста служб, инициализирует
//!    heartbeat-трекер и IPC-шину, проверяет циклы.
//! 2. `boot_chain()` — последовательно запускает службы в порядке
//!    волн DAG (topological order), используя реальный процессо-
//!    вой менеджер ядра (`process::spawn`) или встроенные задачи
//!    (`sched::spawn`), и отмечает этапы в DAG.
//! 3. `run_monitor_tick()` — опрос heartbeat'ов, перезапуск
//!    утонувших служб по политике restart.
//!
//! Запускается после `dinit::init()` в ядре (PID 1 уже занят dinit).
//! Все службы — задачи ядра через `sched::spawn`, реальные
//! процессы (MEX) пускаются через `process::spawn` при необходимости.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::nanocore::{dag, heartbeat, ipc, log, manifests};
use crate::process::{self, SpawnRequest};
use crate::sched;
use crate::spinlock::SpinLock;
use crate::timer::uptime_ms;

/// Режим работы нано-кора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // варианты используются по мере появления служб
pub enum RunMode {
    /// Стандартная работа (boot + мониторинг).
    Normal,
    /// Быстрый старт — минимум служб.
    Fast,
    /// Безопасный режим — только heartbeat, без авто-рестартов.
    Safe,
    /// Отладка — максимальный журнал.
    Debug,
    /// Восстановление после сбоя.
    Recovery,
}

/// Фаза загрузки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootPhase {
    Initializing,
    Launching,
    Monitoring,
    Complete,
    Panic,
}

impl BootPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            BootPhase::Initializing => "INIT",
            BootPhase::Launching => "LAUNCH",
            BootPhase::Monitoring => "MON",
            BootPhase::Complete => "OK",
            BootPhase::Panic => "PANIC",
        }
    }
}

impl RunMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::Normal => "normal",
            RunMode::Fast => "fast",
            RunMode::Safe => "safe",
            RunMode::Debug => "debug",
            RunMode::Recovery => "recovery",
        }
    }
}

/// Обозначение одной службы в оркестраторе (с контекстом запуска).
#[derive(Debug, Clone)]
pub struct ServiceJob {
    /// Манифест службы.
    pub manifest: manifests::ServiceManifest,
    /// PID процесса (0 = встроенная задача ядра).
    pub pid: u32,
    /// Запущена ли служба.
    pub running: bool,
    /// Причина остановки (если есть).
    pub stop_reason: Option<String>,
}

/// Глобальное состояние оркестратора.
pub struct Orchestrator {
    /// DAG зависимостей.
    pub dag: dag::Dag,
    /// heartbeat-трекер.
    pub beats: heartbeat::HeartbeatTracker,
    /// Запущенные службы.
    pub jobs: Vec<ServiceJob>,
    /// Текущий режим.
    pub mode: RunMode,
    /// Текущая фаза.
    pub phase: BootPhase,
    /// Успешно запущенных служб.
    pub launched: u32,
    /// Сбоев загрузки.
    pub boot_failures: u32,
}

impl Orchestrator {
    pub const fn new() -> Self {
        Orchestrator {
            dag: dag::Dag::new(),
            beats: heartbeat::HeartbeatTracker::new(),
            jobs: Vec::new(),
            mode: RunMode::Normal,
            phase: BootPhase::Initializing,
            launched: 0,
            boot_failures: 0,
        }
    }
}

/// Глобальный оркестратор.
pub static ORCHESTRATOR: SpinLock<Orchestrator> = SpinLock::new(Orchestrator::new());

/// Инициализировать нано-кора: DAG, heartbeat, IPC-каналы.
///
/// Возвращает false, если DAG содержит цикл.
pub fn init(mode: RunMode) -> bool {
    {
        let mut orch = ORCHESTRATOR.lock();
        orch.mode = mode;
        orch.phase = BootPhase::Initializing;
        orch.dag = dag::Dag::new();
        orch.beats = heartbeat::HeartbeatTracker::new();
        orch.jobs.clear();
        orch.launched = 0;
        orch.boot_failures = 0;

        // Манифесты служб.
        let services = manifests::builtin();

        // DAG: каждая служба с зависимостями из манифеста.
        let mut dag_ok = true;
        for s in &services {
            let deps: Vec<&str> = s.deps.iter().map(String::as_str).collect();
            if !orch.dag.add(&s.name, &deps) {
                log::error(&format!("DAG cycle at service {}", s.name));
                dag_ok = false;
            }
            orch.beats.register(&s.name, s.deadline_ms);
            orch.jobs.push(ServiceJob {
                manifest: s.clone(),
                pid: 0,
                running: false,
                stop_reason: None,
            });
        }

        if let Some((service, dependency)) = orch.dag.missing_dependency() {
            log::error(&format!("DAG: service {} has unknown dependency {}", service, dependency));
            dag_ok = false;
        }

        // IPC-каналы.
        {
            let mut bus = ipc::IPC.lock();
            let _ = bus.channel("nc-log");
            let _ = bus.channel("nc-cmd");
        }

        log::info(&format!(
            "nanocore init: {} services, dag stages={}, waves={}",
            orch.jobs.len(),
            orch.dag.len(),
            orch.dag.wave_count()
        ));
        if !dag_ok {
            orch.phase = BootPhase::Panic;
            return false;
        }
        orch.phase = BootPhase::Launching;
        true
    }
}

/// Запустить boot-цепочку: в порядке DAG (topological waves).
///
/// Служба `proc:xxx` пускается как встроенная задача ядра
/// (`sched::spawn`), служба `cmd:xxx` — как процесс через
/// `process::spawn`. Встроенные задачи — статические функции.
pub fn boot_chain() {
    let order: Vec<String> = {
        let orch = ORCHESTRATOR.lock();
        orch.dag.execution_order().iter().map(|s| s.name.clone()).collect()
    };

    for name in &order {
        let started_at = uptime_ms();

        // Блок с spawn-ом: берём задачу из оркестратора, парсим спец,
        // запускаем вне лока (spawn может лочить другие подсистемы).
        let spawn_spec = {
            let orch = ORCHESTRATOR.lock();
            orch.jobs
                .iter()
                .find(|j| &j.manifest.name == name)
                .map(|j| String::from(&j.manifest.spawn))
        };
        let Some(spawn_spec) = spawn_spec else {
            continue;
        };
        let ready = ORCHESTRATOR.lock().dag.dependencies_ready(name);
        if !ready {
            let mut orch = ORCHESTRATOR.lock();
            if let Some(job) = orch.jobs.iter_mut().find(|j| &j.manifest.name == name) {
                job.running = false;
                job.stop_reason = Some(String::from("dependency failed"));
            }
            orch.boot_failures += 1;
            log::error(&format!("service {} blocked by failed dependency", name));
            continue;
        }

        let result = match spawn_spec.as_str() {
            sp if sp.starts_with("proc:") => spawn_proc_task(&sp[5..]),
            sp if sp.starts_with("cmd:") => spawn_process(&sp[4..]),
            other => {
                log::error(&format!("service {}: bad spawn spec '{}'", name, other));
                false
            }
        };

        // Записываем результат в оркестратор.
        {
            let mut orch = ORCHESTRATOR.lock();
            let Some(job) = orch.jobs.iter_mut().find(|j| &j.manifest.name == name) else {
                continue;
            };
            if result {
                job.running = true;
                job.stop_reason = None;
                orch.launched += 1;
                if let Some(rec) = orch.dag.stages_mut().iter_mut().find(|s| &s.name == name) {
                    rec.started_ms = started_at;
                    rec.done_ms = uptime_ms();
                    rec.ok = true;
                }
                log::info(&format!("service {} launched", name));
            } else {
                job.running = false;
                job.stop_reason = Some(String::from("spawn failed"));
                orch.boot_failures += 1;
                if let Some(rec) = orch.dag.stages_mut().iter_mut().find(|s| &s.name == name) {
                    rec.started_ms = started_at;
                    rec.done_ms = uptime_ms();
                    rec.ok = false;
                }
                log::error(&format!("service {} failed to spawn", name));
            }
        }
    }

    let mut orch = ORCHESTRATOR.lock();
    orch.phase = BootPhase::Monitoring;
    log::info(&format!(
        "boot chain done: {} launched, {} failed",
        orch.launched, orch.boot_failures
    ));
}

/// Запустить встроенную задачу ядра.
///
/// Функции с именами, известными orchestrator'у:
/// - `nanocore_hb_monitor` → heartbeat-опрос;
/// - `nanocore_ipc_drain` → слив IPC-каналов;
/// - `nanocore_log_report` → слив журнала в serial.
fn spawn_proc_task(fn_name: &str) -> bool {
    match fn_name {
        "nanocore_hb_monitor" => {
            sched::spawn("nanocore-hb-monitor", hb_monitor_task).is_some()
        }
        "nanocore_ipc_drain" => {
            sched::spawn("nanocore-ipc-drain", ipc_drain_task).is_some()
        }
        "nanocore_log_report" => {
            sched::spawn("nanocore-log-report", log_report_task).is_some()
        }
        _ => {
            log::error(&format!("unknown proc task '{}'", fn_name));
            false
        }
    }
}

/// Запустить реальный процесс через ядро (через MEX-образ по пути cmd).
fn spawn_process(cmd: &str) -> bool {
    let req = SpawnRequest {
        name: String::from("nanocore-proc"),
        uid: 0,
        parent: 0,
        caps: 0,
        cwd: String::from("/"),
        image_path: String::from(cmd),
    };
    match process::spawn(&req) {
        Ok(pid) => {
            match process::exec(pid) {
                Ok(()) => {
                    log::info(&format!("process '{}' started pid {}", cmd, pid));
                    true
                }
                Err(e) => {
                    log::error(&format!("process exec '{}' failed: {}", cmd, e.message()));
                    let _ = process::kill(pid, -1);
                    let _ = process::reap(pid);
                    false
                }
            }
        }
        Err(e) => {
            log::error(&format!("process spawn '{}' failed: {}", cmd, e.message()));
            false
        }
    }
}

/// Отправить пульс службы.
fn beat(name: &str) {
    heartbeat::HEARTBEATS.lock().beat(name);
}

/// Цикл heartbeat-мониторинга. Запускается как задача ядра.
///
/// - бьёт пульс собственной службы;
/// - проверяет пульсы других служб;
/// - при падении перезапускает по политике restart.
extern "C" fn hb_monitor_task() {
    log::info("hb-monitor task started");
    loop {
        // Свой пульс.
        beat("hb-monitor");

        // Опрос других служб.
        let dead = heartbeat::HEARTBEATS.lock().check_all();

        if !dead.is_empty() {
            for name in &dead {
                restart_service(name);
            }
        }
        sched::sleep_ms(1000);
    }
}

/// Слив IPC-каналов: прокидывает сообщения nc-log в serial.
extern "C" fn ipc_drain_task() {
    log::info("ipc-drain task started");
    loop {
        if let Some(m) = ipc::try_recv("nc-log") {
            crate::serial_println!("[nanocore] ipc-drain: {}", m.as_str());
        }
        sched::sleep_ms(500);
    }
}

/// Служба-репортер: отправляет liveness в канал nc-cmd.
extern "C" fn log_report_task() {
    log::info("log-report task started");
    loop {
        ipc::send("nc-cmd", "log-report: alive");
        sched::sleep_ms(2000);
    }
}

/// Перезапущить службу по политике restart.
pub fn restart_service(name: &str) -> bool {
    let (policy, was_running, spawn_spec) = {
        let orch = ORCHESTRATOR.lock();
        match orch.jobs.iter().find(|j| &j.manifest.name == name) {
            Some(job) => (
                job.manifest.restart,
                job.running,
                String::from(&job.manifest.spawn),
            ),
            None => return false,
        }
    };

    if policy == manifests::RestartPolicy::Never {
        log::warn(&format!("service {} dead, restart policy is Never", name));
        return false;
    }
    // Если не была запущена и политика on-fail — не трогаем.
    if !was_running && policy == manifests::RestartPolicy::OnFail {
        return false;
    }

    let ok = if let Some(rest) = spawn_spec.strip_prefix("proc:") {
        spawn_proc_task(rest)
    } else if let Some(rest) = spawn_spec.strip_prefix("cmd:") {
        spawn_process(rest)
    } else {
        false
    };

    {
        let mut orch = ORCHESTRATOR.lock();
        if let Some(job) = orch.jobs.iter_mut().find(|j| &j.manifest.name == name) {
            if ok {
                job.running = true;
                job.stop_reason = None;
            } else {
                job.stop_reason = Some(String::from("restart failed"));
            }
        }
    }
    if ok {
        // Отметка в heartbeat-трекере: счётчик рестартов + новый пульс.
        {
            let mut beats = heartbeat::HEARTBEATS.lock();
            if let Some(hb) = beats.beats.get_mut(name) {
                hb.mark_restarted();
            }
        }
        log::info(&format!("service {} restarted", name));
    } else {
        log::error(&format!("service {} restart failed", name));
    }
    ok
}

/// Цикл мониторинга: проверка heartbeat'ов + перезапуски.
///
/// Можно вызывать вручную из CLI.
pub fn run_monitor_tick() -> u32 {
    let dead = heartbeat::HEARTBEATS.lock().check_all();
    log::debug(&format!("monitor tick: {} service(s) dead", dead.len()));
    let mut restarted = 0;
    for name in &dead {
        if restart_service(name) {
            restarted += 1;
            log::debug(&format!("monitor tick: service {} restarted", name));
        }
    }
    restarted
}

/// Статус нано-кора для CLI.
pub fn status_line() -> String {
    let orch = ORCHESTRATOR.lock();
    format!(
        "nanocore[{}] mode={} services={} launched={} failed={}",
        orch.phase.as_str(),
        orch.mode.as_str(),
        orch.jobs.len(),
        orch.launched,
        orch.boot_failures
    )
}
