//! API ядра для управления Dinit
//!
//! Даёт ядру контроль над PID 1: возможность урезать права, перезапустить или остановить.

use alloc::vec::Vec;
use super::namespace::Capabilities;
use super::audit::AuditEntry;
use super::DINIT;

/// Остановка Dinit ядром
pub fn dinit_stop() -> Result<(), &'static str> {
    let mut lock = DINIT.lock();
    if let Some(dinit) = lock.as_mut() {
        dinit.state = super::DinitState::Stopped;
        Ok(())
    } else {
        Err("Dinit не инициализирован")
    }
}

/// Перезапуск Dinit ядром
pub fn dinit_restart() -> Result<(), &'static str> {
    let mut lock = DINIT.lock();
    if let Some(dinit) = lock.as_mut() {
        dinit.state = super::DinitState::Running;
        dinit.namespace.capabilities = Capabilities::default_dinit();
        Ok(())
    } else {
        Err("Dinit не инициализирован")
    }
}

/// Ограничение привилегий Dinit ядром
pub fn dinit_limit(cap: Capabilities) -> Result<(), &'static str> {
    let mut lock = DINIT.lock();
    if let Some(dinit) = lock.as_mut() {
        dinit.namespace.capabilities = cap;
        Ok(())
    } else {
        Err("Dinit не инициализирован")
    }
}

/// Получение полного дампа журнала аудита
pub fn dinit_audit_dump() -> Vec<AuditEntry> {
    let lock = DINIT.lock();
    if let Some(dinit) = lock.as_ref() {
        dinit.namespace.audit_log.iter().cloned().collect()
    } else {
        Vec::new()
    }
}
