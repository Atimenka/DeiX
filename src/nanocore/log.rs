//! Nano-Core log — структурированный журнал нано-кора.
//!
//! Каждый запись дублируется:
//! 1. в последовательный порт (serial) — для отладки на ноутбуке;
//! 2. в кольцевой буфер ядра (`diag`) — для команд log/error;
//! 3. в IPC-канал `nc-log` — для службы-сливника (реальная IPC-цикл).
//!
//! Для CLI сохраняется кольцо из последних событий.

use crate::diag::{self, ErrorCode};
use crate::spinlock::SpinLock;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Уровень записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "DBG",
            Level::Info => "INF",
            Level::Warn => "WRN",
            Level::Error => "ERR",
        }
    }

    /// Приоритет для отображения (больше — важнее).
    pub fn rank(self) -> u8 {
        match self {
            Level::Debug => 0,
            Level::Info => 1,
            Level::Warn => 2,
            Level::Error => 3,
        }
    }
}

/// Запись журнала.
#[derive(Debug, Clone)]
pub struct Entry {
    pub level: Level,
    pub text: String,
    pub at_ms: u64,
}

/// Кольцевой журнал для CLI (последние события).
#[derive(Default)]
pub struct LogRing {
    entries: Vec<Entry>,
    head: usize,
}

impl LogRing {
    pub const fn new() -> Self {
        LogRing {
            entries: Vec::new(),
            head: 0,
        }
    }

    pub fn push(&mut self, entry: Entry) {
        const MAX: usize = 64;
        if self.entries.len() < MAX {
            self.entries.push(entry);
        } else {
            self.entries[self.head] = entry;
            self.head = (self.head + 1) % MAX;
        }
    }

    /// Все записи в порядке времени (от oldest к newest).
    pub fn all(&self) -> Vec<&Entry> {
        let n = self.entries.len();
        if n == 0 {
            return Vec::new();
        }
        let start = if n < 64 { 0 } else { self.head };
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(&self.entries[(start + i) % n]);
        }
        out
    }
}

/// Глобальный журнал нано-кора.
pub static LOG: SpinLock<LogRing> = SpinLock::new(LogRing::new());

/// Записать запись: serial + diag + IPC-канал.
pub fn log(level: Level, text: &str) {
    let msg = format!("[nanocore] {} {}", level.as_str(), text);
    crate::serial_println!("{}", msg);
    // Код диагностики: подсистема Kernel, номер 0x100 + уровень.
    let code = ErrorCode::new(diag::code::Subsystem::Kernel, 0x100 + level.rank() as u16);
    match level {
        Level::Debug => { diag::debug(code, text); }
        Level::Info => { diag::info(code, text); }
        Level::Warn => { diag::warn(code, text); }
        Level::Error => { diag::error(code, text); }
    }
    // Реальная IPC-посылка — служба-сливник может читать лог по шине.
    crate::nanocore::ipc::send("nc-log", &msg);
    LOG.lock().push(Entry {
        level,
        text: String::from(text),
        at_ms: crate::timer::uptime_ms(),
    });
}

pub fn debug(text: &str) { log(Level::Debug, text); }
pub fn info(text: &str) { log(Level::Info, text); }
pub fn warn(text: &str) { log(Level::Warn, text); }
pub fn error(text: &str) { log(Level::Error, text); }

/// Новые записи (для CLI) — копии с копированием контента.
pub fn recent() -> Vec<Entry> {
    let guard = LOG.lock();
    guard.all().into_iter().cloned().collect()
}
