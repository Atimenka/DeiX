//! Nano-Core — нанослой ядра DeiX.
//!
//! Реализация, написанная с нуля для релиза 3.0:
//! - **dag** — граф зависимостей boot-этапов и служб (топологические
//!   волны, проверка циклов);
//! - **manifests** — текстовые описания служб;
//! - **heartbeat** — трекер живости служб с дедлайнами и счётчиками;
//! - **ipc** — встроенная кольцевая IPC-шина между службами;
//! - **log** — структурированный журнал (serial + diag + IPC);
//! - **orchestrator** — загрузка служб в порядке DAG и мониторинг;
//! - **cli** — встроенные команды `nc` / `nc-status` / `nc-boot`.
//!
//! Нano-кора не заменяет dinit/process/sched — она работает
//! ПОВЕРХ них: запускается после `dinit::init()`, использует
//! `sched::spawn` для задач ядра и `process::spawn` для реальных
//! процессов, сохраняет единый журнал через `diag`.

pub mod cli;
pub mod dag;
pub mod heartbeat;
pub mod ipc;
pub mod log;
pub mod manifests;
pub mod orchestrator;

/// Запустить нано-кору: инициализация + boot-цепочка + monitor.
///
/// Вызывается из `kernel_main` после `dinit::init()`.
pub fn run(mode: orchestrator::RunMode) {
    if !orchestrator::init(mode) {
        log::error("nanocore init failed (dag cycle)");
        return;
    }
    orchestrator::boot_chain();
    // Запуск monitor'а — его задача продолжает жить после возврата.
    log::info("nanocore started");
}

/// Остановить нано-кору (остановить все запущенные службы).
pub fn stop() {
    {
        let mut orch = orchestrator::ORCHESTRATOR.lock();
        for job in orch.jobs.iter_mut() {
            if job.running {
                job.running = false;
                job.stop_reason = Some(alloc::string::String::from("stopped by nc stop"));
            }
        }
        orch.phase = orchestrator::BootPhase::Complete;
    }
    log::info("nanocore stopped");
}
