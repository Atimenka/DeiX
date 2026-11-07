//! Nano-Core — разбитый Stage 2 для сверхбыстрой загрузки.
//!
//! Вместо монолита Boot chain загружает крошечный оркестратор (~50 КБ),
//! который параллельно поднимает десятки строго именованных изолированных
//! процессов (Nano-Threads). Каждый процесс запускается только когда его
//! зависимости готовы (DAG-граф), логирует свои действия и завершается
//! или переходит в режим ожидания.
//!
//! ## Архитектура
//!
//! ```text
//! Boot chain → Nano-Core Orchestrator (PID 1)
//!   ├── DAG manifest (50 процессов)
//!   ├── drv-*   (10 — драйверы)
//!   ├── fs-*    (8  — файловая система)
//!   ├── sec-*   (6  — безопасность)
//!   ├── mem-*   (4  — память)
//!   ├── net-*   (4  — сеть)
//!   ├── log-*   (6  — логирование)
//!   ├── sys-*   (6  — системные)
//!   ├── ui-*    (4  — интерфейс)
//!   └── svc-*   (2  — фоновые)
//! ```
//!
//! ## Ключевые API
//!
//! * [`Orchestrator`] — дирижёр: spawn, monitor, restart, log.
//! * [`manifest::ProcessManifest`] — описание одного процесса.
//! * [`dag::Dag`] — граф зависимостей (topological sort, waves).
//! * [`process::NanoThread`] — состояние одного процесса.
//! * [`heartbeat::HeartbeatTracker`] — мониторинг живости.
//! * [`ipc::IpcRing`] — IPC-шина (shared memory + ring buffer).
//! * [`log`] — система логирования (ring → serial → disk).
//! * [`cli`] — `nanocore status/logs/trace/restart/graph`.
//! * [`manifests`] — 50 встроенных манифестов.

pub mod dag;
pub mod heartbeat;
pub mod cli;
pub mod ipc;
pub mod log;
pub mod manifest;
pub mod manifests;
pub mod orchestrator;
pub mod process;

pub use orchestrator::Orchestrator;
pub use orchestrator::BootPhase;
pub use orchestrator::RunMode;
pub use orchestrator::ORCHESTRATOR_PID;
pub use orchestrator::boot;
pub use orchestrator::init_global;
pub use orchestrator::with_orchestrator;
