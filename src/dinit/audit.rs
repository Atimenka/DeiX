//! Журнал аудита Dinit
//!
//! Фиксирует действия пользователей и системных сервисов в кольцевой буфер.

use alloc::string::String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOp {
    Login,
    Logout,
    FileRead,
    FileWrite,
    ServiceSpawn,
    ServiceStop,
    Mount,
    Reboot,
    Violation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditResult {
    Allowed,
    Denied,
    Failed,
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub uid: u32,
    pub op: AuditOp,
    pub target: String,
    pub result: AuditResult,
}

impl AuditEntry {
    pub fn new(timestamp: u64, uid: u32, op: AuditOp, target: &str, result: AuditResult) -> Self {
        Self {
            timestamp,
            uid,
            op,
            target: String::from(target),
            result,
        }
    }
}
