//! Публичный программный интерфейс (Kernel API) для подсистемы Dinit (PID 1)
//!
//! Позволяет подсистемам ядра, драйверам и слою системных вызовов
//! взаимодействовать с супервизором PID 1, запрашивать проверки прав,
//! регистрировать события безопасности и управлять службами.

#![allow(dead_code)]

use alloc::vec::Vec;
use super::authorize::{FileOp, AccessError};
use super::audit::AuditEntry;
use super::DINIT;
use crate::init_parser::BootStage;
use crate::security_monitor::SecurityEvent;

/// Инициализация Dinit ядром
pub fn dinit_init() {
    super::init();
}

/// Периодический квант супервизора
pub fn dinit_tick() {
    super::tick();
}

/// Запуск службы по имени
pub fn dinit_start_service(name: &str) -> Result<u32, &'static str> {
    let mut lock = DINIT.lock();
    if let Some(dinit) = lock.as_mut() {
        dinit.start_service(name)
    } else {
        Err("Dinit не инициализирован")
    }
}

/// Остановка службы по имени
pub fn dinit_stop_service(name: &str) -> Result<(), &'static str> {
    let mut lock = DINIT.lock();
    if let Some(dinit) = lock.as_mut() {
        dinit.stop_service(name)
    } else {
        Err("Dinit не инициализирован")
    }
}

/// Перезапуск службы по имени
pub fn dinit_restart_service(name: &str) -> Result<u32, &'static str> {
    let mut lock = DINIT.lock();
    if let Some(dinit) = lock.as_mut() {
        dinit.restart_service(name)
    } else {
        Err("Dinit не инициализирован")
    }
}

/// Проверка прав доступа через матрицу авторизации Dinit
pub fn dinit_check_permission(
    uid: u32,
    username: &str,
    op: FileOp,
    path: &str,
) -> Result<(), AccessError> {
    let mut lock = DINIT.lock();
    if let Some(dinit) = lock.as_mut() {
        dinit.check_permission(uid, username, op, path)
    } else {
        Ok(())
    }
}

/// Регистрация события безопасности в эвристическом мониторе Dinit
pub fn dinit_report_security_event(
    pid: u32,
    action: &str,
    target_path: &str,
    syscall_frequency: u64,
    entropy_score: f32,
) -> bool {
    let event = SecurityEvent::new(pid, action, target_path, syscall_frequency, entropy_score);
    let mut lock = DINIT.lock();
    if let Some(dinit) = lock.as_mut() {
        dinit.report_security_event(&event)
    } else {
        false
    }
}

/// Регистрация нового активного пользователя
pub fn dinit_register_user(uid: u32, username: &str) {
    let mut lock = DINIT.lock();
    if let Some(dinit) = lock.as_mut() {
        dinit.register_user(uid, username);
    }
}

/// Завершение сессии пользователя
pub fn dinit_unregister_user(uid: u32) {
    let mut lock = DINIT.lock();
    if let Some(dinit) = lock.as_mut() {
        dinit.unregister_user(uid);
    }
}

/// Переключение фазы загрузки
pub fn dinit_advance_stage(stage: BootStage) {
    let mut lock = DINIT.lock();
    if let Some(dinit) = lock.as_mut() {
        dinit.advance_stage(stage);
    }
}

/// Получение полного дампа журнала аудита
pub fn dinit_audit_dump() -> Vec<AuditEntry> {
    let lock = DINIT.lock();
    if let Some(dinit) = lock.as_ref() {
        dinit.audit.all()
    } else {
        Vec::new()
    }
}
