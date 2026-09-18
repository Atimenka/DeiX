//! Журнал аудита безопасности Dinit (Audit Subsystem)
//!
//! Непрерывно фиксирует все критические события безопасности ядра:
//! авторизацию, запуск процессов, перехват системных вызовов,
//! монтирование, срабатывания эвристического монитора и попытки НСД.

#![allow(dead_code)]

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Типы регистрируемых операций
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOp {
    Login,
    Logout,
    FileRead,
    FileWrite,
    FileDelete,
    ServiceSpawn,
    ServiceStop,
    ServiceCrash,
    ServiceRestart,
    Mount,
    Unmount,
    Reboot,
    Violation,
    VaultRejection,
    SyscallInterception,
    PrivilegeEscalationAttempt,
    ThreatAlert,
    KillDispatched,
}

impl AuditOp {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditOp::Login => "LOGIN",
            AuditOp::Logout => "LOGOUT",
            AuditOp::FileRead => "FILE_READ",
            AuditOp::FileWrite => "FILE_WRITE",
            AuditOp::FileDelete => "FILE_DEL",
            AuditOp::ServiceSpawn => "SVC_SPAWN",
            AuditOp::ServiceStop => "SVC_STOP",
            AuditOp::ServiceCrash => "SVC_CRASH",
            AuditOp::ServiceRestart => "SVC_RESTART",
            AuditOp::Mount => "MOUNT",
            AuditOp::Unmount => "UMOUNT",
            AuditOp::Reboot => "REBOOT",
            AuditOp::Violation => "VIOLATION",
            AuditOp::VaultRejection => "VAULT_REJECT",
            AuditOp::SyscallInterception => "SYSCALL_HOOK",
            AuditOp::PrivilegeEscalationAttempt => "PRIV_ESC",
            AuditOp::ThreatAlert => "THREAT_ALERT",
            AuditOp::KillDispatched => "KILL_DISPATCH",
        }
    }
}

/// Результат или статус операции
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditResult {
    Allowed,
    Denied,
    Terminated,
    Failed,
    Warning,
}

impl AuditResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditResult::Allowed => "ALLOWED",
            AuditResult::Denied => "DENIED",
            AuditResult::Terminated => "TERMINATED",
            AuditResult::Failed => "FAILED",
            AuditResult::Warning => "WARNING",
        }
    }
}

/// Отдельная запись журнала аудита
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub id: u64,
    pub timestamp: u64,
    pub pid: u32,
    pub uid: u32,
    pub op: AuditOp,
    pub target: String,
    pub result: AuditResult,
    pub detail: String,
}

impl AuditEntry {
    pub fn format_line(&self) -> String {
        format!(
            "[{:08}ms] #{:<5} PID:{:<5} UID:{:<4} {:<12} {:<10} {:<24} {}",
            self.timestamp,
            self.id,
            self.pid,
            self.uid,
            self.op.as_str(),
            self.result.as_str(),
            self.target,
            self.detail
        )
    }
}

/// Статистика журнала аудита
#[derive(Debug, Clone, Default)]
pub struct AuditStats {
    pub total_events: u64,
    pub allowed_events: u64,
    pub denied_events: u64,
    pub violations: u64,
    pub threats_intercepted: u64,
}

/// Кольцевой буфер журнала аудита
pub struct AuditLog {
    entries: VecDeque<AuditEntry>,
    max_capacity: usize,
    next_id: u64,
    pub stats: AuditStats,
}

impl AuditLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            max_capacity: capacity,
            next_id: 1,
            stats: AuditStats::default(),
        }
    }

    /// Добавление события в кольцевой буфер
    pub fn record(
        &mut self,
        now: u64,
        pid: u32,
        uid: u32,
        op: AuditOp,
        target: &str,
        result: AuditResult,
        detail: &str,
    ) {
        if self.entries.len() >= self.max_capacity {
            self.entries.pop_front();
        }

        self.stats.total_events += 1;
        match result {
            AuditResult::Allowed => self.stats.allowed_events += 1,
            AuditResult::Denied => self.stats.denied_events += 1,
            AuditResult::Terminated => self.stats.threats_intercepted += 1,
            AuditResult::Warning => {}
            AuditResult::Failed => {}
        }
        if op == AuditOp::Violation || op == AuditOp::VaultRejection || op == AuditOp::PrivilegeEscalationAttempt {
            self.stats.violations += 1;
        }

        let entry = AuditEntry {
            id: self.next_id,
            timestamp: now,
            pid,
            uid,
            op,
            target: String::from(target),
            result,
            detail: String::from(detail),
        };
        self.next_id += 1;
        self.entries.push_back(entry);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Получение последних N записей
    pub fn tail(&self, n: usize) -> Vec<AuditEntry> {
        let skip = self.entries.len().saturating_sub(n);
        self.entries.iter().skip(skip).cloned().collect()
    }

    /// Фильтрация записей по нарушениям
    pub fn violations(&self) -> Vec<AuditEntry> {
        self.entries
            .iter()
            .filter(|e| {
                e.result == AuditResult::Denied
                    || e.result == AuditResult::Terminated
                    || e.op == AuditOp::Violation
                    || e.op == AuditOp::VaultRejection
            })
            .cloned()
            .collect()
    }

    /// Дамп всех записей
    pub fn all(&self) -> Vec<AuditEntry> {
        self.entries.iter().cloned().collect()
    }
}
