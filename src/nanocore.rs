//! NANOCORE — Минимальное ядро DeiX OS.
//!
//! Наноклас живёт в Ring 0 и обеспечивает абсолютный минимум:
//!   - Управление памятью (физический+виртуальный атор)
//!   - Планирование задач (вытесняющая многозадачность)
//!   - Межпроцессное общение (IPC)
//!   - Разделение адресных пространств
//!
//! Всё остальное (Dinit, службы, GUI, драйверы) — пользовательские
//! процессы в Ring 3, управляемые нанокором через IPC.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use crate::spinlock::SpinLock;

/// Состояние нанокласа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NanoCoreState {
    /// До инициализации.
    Uninitialized,
    /// Минимальное ядро запущено.
    Running,
    /// Критическая ошибка.
    Fatal,
}

/// Основная структура нанокласа.
pub struct NanoCore {
    /// Состояние ядра.
    pub state: NanoCoreState,
    /// Время старта (uptime_ms).
    pub start_time: u64,
    /// Количество запущенных процессов.
    pub process_count: u32,
    /// Количество активных IPC-каналов.
    pub ipc_channels: u32,
}

/// Глобальный экземпляр нанокласа.
pub static NANOCORE: SpinLock<Option<NanoCore>> = SpinLock::new(None);

impl NanoCore {
    /// Создаёт и инициализирует нанокор.
    pub fn new() -> Self {
        Self {
            state: NanoCoreState::Running,
            start_time: crate::timer::uptime_ms(),
            process_count: 0,
            ipc_channels: 0,
        }
    }

    /// Регистрирует новый процесс.
    pub fn register_process(&mut self) -> u32 {
        self.process_count = self.process_count.wrapping_add(1);
        self.process_count
    }

    /// Регистрирует новый IPC-канал.
    pub fn register_ipc(&mut self) -> u32 {
        self.ipc_channels = self.ipc_channels.wrapping_add(1);
        self.ipc_channels
    }

    /// Удаляет IPC-канал.
    pub fn unregister_ipc(&mut self) {
        if self.ipc_channels > 0 {
            self.ipc_channels = self.ipc_channels - 1;
        }
    }
}

/// Инициализация нанокласа.
///
/// Должна вызываться ПЕРВОЙ в kernel_main(), до любого другого инициализации.
pub fn init() {
    let mut lock = NANOCORE.lock();
    if lock.is_none() {
        *lock = Some(NanoCore::new());
        crate::serial_println!("[nanocore] Минимальное ядро запущено");
    }
}

/// Получает глобальный экземпляр нанокласа.
pub fn get() -> Option<NanoCore> {
    NANOCORE.lock().clone()
}

/// Регистрирует новый процесс в нанокоре.
/// Возвращает идентификатор процесса в нанокоре.
pub fn register_process(_pid: u32) -> u32 {
    let mut lock = NANOCORE.lock();
    if let Some(nc) = lock.as_mut() {
        nc.register_process()
    } else {
        0
    }
}

/// Удаляет процесс из нанокласа.
pub fn unregister_process(_pid: u32) {
    let mut lock = NANOCORE.lock();
    if let Some(nc) = lock.as_mut() {
        if nc.process_count > 0 {
            nc.process_count = nc.process_count - 1;
        }
    }
}

/// Заглушка для тестирования DAG-загрузки.
/// Возвращает true, если нанокор готов к загрузке DAG-процессов.
pub fn is_ready_for_dag() -> bool {
    match NANOCORE.lock().as_ref() {
        Some(nc) => nc.state == NanoCoreState::Running,
        None => false,
    }
}

/// Проверка DAG-загрузки: может ли нанокор пройти всю загрузку?
///
/// Даг (Directed Acyclic Graph) — это граф зависимостей процессов.
/// Если нанокор не готов, весь граф загрузки не может быть построен.
pub fn dag_load_check() -> Result<(), String> {
    match NANOCORE.lock().as_ref() {
        Some(nc) => {
            if nc.state == NanoCoreState::Running {
                Ok(())
            } else {
                Err(String::from("nanocore not running"))
            }
        }
        None => Err(String::from("nanocore not initialized")),
    }
}

/// Heartbeat для нанокласа — проверка, что ядро живо.
pub fn heartbeat() -> bool {
    match NANOCORE.lock().as_ref() {
        Some(nc) => {
            // Простая проверка: состояние Running и uptime растёт.
            nc.state == NanoCoreState::Running
                && crate::timer::uptime_ms() > nc.start_time
        }
        None => false,
    }
}

/// Перезапуск нанокласа (только для тестирования).
pub fn restart() {
    let mut lock = NANOCORE.lock();
    *lock = Some(NanoCore::new());
    crate::serial_println!("[nanocore] Перезапуск нанокласа");
}
