//! Учёт процессов, находящихся под супервизией Dinit (PID 1).
//!
//! Dinit отслеживает PID запущенных им Process-служб. Аппаратной
//! изоляции пространств имён (отдельных таблиц страниц, chroot и т.п.)
//! ядро пока не предоставляет — это именно учётная структура.

use alloc::string::String;
use alloc::vec::Vec;

/// Реестр процессов супервизора Dinit
#[derive(Debug, Clone)]
pub struct DinitNamespace {
    /// Имя реестра (например, "root_supervisor")
    pub name: String,
    /// Список PID процессов, запущенных супервизором
    pub active_pids: Vec<u32>,
}

impl DinitNamespace {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            active_pids: Vec::new(),
        }
    }

    /// Добавление PID в реестр
    pub fn attach_process(&mut self, pid: u32) {
        if !self.active_pids.contains(&pid) {
            self.active_pids.push(pid);
        }
    }

    /// Удаление PID при завершении процесса
    pub fn detach_process(&mut self, pid: u32) {
        self.active_pids.retain(|&p| p != pid);
    }
}
