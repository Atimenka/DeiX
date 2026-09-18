//! DINIT — Подсистема PID 1 для DeiX OS (Ring 0).
//!
//! Dinit живёт в пространстве ядра (Ring 0), обладает собственным
//! изолированным пространством имён (DinitNamespace) и управляет пользователями,
//! точками монтирования, системными сервисами и журналом аудита.
//!
//! Иерархия контроля:
//!   Загрузчик -> Ядро -> Dinit (PID 1, Ring 0) -> Пользовательские процессы.

pub mod namespace;
pub mod user;
pub mod service;
pub mod mount;
pub mod authorize;
pub mod audit;
pub mod api;

use alloc::string::String;
use alloc::vec::Vec;
use crate::sync::Spinlock;
use namespace::{Capabilities, DinitNamespace};
use user::UserState;
use service::{ServiceHandle, ServiceStatus, ServiceError};
use mount::{MountHandle, MountCmd};
use audit::{AuditEntry, AuditOp, AuditResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DinitState {
    Initializing,
    Running,
    Stopped,
    Faulted,
}

/// Экземпляр Dinit (PID 1)
pub struct Dinit {
    pub pid: u32,
    pub state: DinitState,
    pub namespace: DinitNamespace,
    pub start_time: u64,
}

impl Dinit {
    pub fn new() -> Self {
        let caps = Capabilities::default_dinit();
        let ns = DinitNamespace::new(1, caps);
        Self {
            pid: 1,
            state: DinitState::Initializing,
            namespace: ns,
            start_time: crate::timer::uptime_ms(),
        }
    }

    /// Запись события в кольцевой буфер аудита
    pub fn log_audit(&mut self, uid: u32, op: AuditOp, target: &str, result: AuditResult) {
        if self.namespace.audit_log.len() >= 1024 {
            self.namespace.audit_log.pop_front();
        }
        let now = crate::timer::uptime_ms();
        self.namespace.audit_log.push_back(AuditEntry::new(now, uid, op, target, result));
    }

    /// Регистрация активного пользователя
    pub fn register_user(&mut self, uid: u32, username: &str) {
        let now = crate::timer::uptime_ms();
        let user = UserState::new(uid, username, now);
        self.namespace.users.insert(uid, user);
        self.log_audit(uid, AuditOp::Login, username, AuditResult::Allowed);
        crate::serial_println!("[dinit] Пользователь '{}' (UID {}) зарегистрирован в namespace", username, uid);
    }

    /// Завершение сессии пользователя
    pub fn unregister_user(&mut self, uid: u32) {
        if let Some(user) = self.namespace.users.remove(&uid) {
            self.log_audit(uid, AuditOp::Logout, &user.username, AuditResult::Allowed);
            crate::serial_println!("[dinit] Пользователь '{}' (UID {}) отключён", user.username, uid);
        }
    }

    /// Запрос на монтирование раздела через прокси с проверкой Vault
    pub fn request_mount(&mut self, cmd: MountCmd) -> Result<(), &'static str> {
        if !self.namespace.capabilities.mount {
            self.log_audit(0, AuditOp::Mount, &cmd.target, AuditResult::Denied);
            return Err("Dinit: привилегия mount отозвана ядром");
        }

        // Проверка Vault: раздел /TPM закрыт навсегда
        if cmd.target.starts_with("/tpm") || cmd.target.starts_with("/TPM") {
            self.log_audit(0, AuditOp::Violation, &cmd.target, AuditResult::Denied);
            return Err("Dinit: Vault блокирует доступ к TPM");
        }

        let handle = MountHandle::new(&cmd.target, &cmd.source, &cmd.fs_type, cmd.read_only);
        self.namespace.mounts.insert(cmd.target.clone(), handle);
        self.log_audit(0, AuditOp::Mount, &cmd.target, AuditResult::Allowed);
        Ok(())
    }

    /// Запрос на запуск системного сервиса
    pub fn request_spawn(&mut self, name: &str, path: &str) -> Result<u32, ServiceError> {
        if !self.namespace.capabilities.spawn_service {
            self.log_audit(0, AuditOp::ServiceSpawn, name, AuditResult::Denied);
            return Err(ServiceError::PermissionDenied);
        }

        let next_id = (self.namespace.services.len() as u32) + 1;
        let mut handle = ServiceHandle::new(next_id, name, path);
        handle.status = ServiceStatus::Running;
        handle.pid = Some(100 + next_id);

        self.namespace.services.insert(next_id, handle);
        self.log_audit(0, AuditOp::ServiceSpawn, name, AuditResult::Allowed);
        Ok(100 + next_id)
    }

    /// Остановка сервиса
    pub fn request_stop_service(&mut self, name: &str) -> Result<(), ServiceError> {
        for s in self.namespace.services.values_mut() {
            if s.name == name {
                s.status = ServiceStatus::Stopped;
                self.log_audit(0, AuditOp::ServiceStop, name, AuditResult::Allowed);
                return Ok(());
            }
        }
        Err(ServiceError::NotRunning)
    }

    /// Периодический тик Dinit (контроль лимитов и состояния сервисов)
    pub fn tick(&mut self) {
        if self.state != DinitState::Running {
            return;
        }
        // Мониторинг упавших сервисов
    }
}

pub static DINIT: Spinlock<Option<Dinit>> = Spinlock::new(None);

/// Инициализация подсистемы Dinit (PID 1)
pub fn init() {
    let mut dinit = Dinit::new();
    dinit.state = DinitState::Running;

    // Регистрация суперпользователя root (UID 0)
    dinit.register_user(0, "root");

    // Выполняем базовые монтирования
    let _ = dinit.request_mount(MountCmd {
        source: String::from("/dev/block/by-name/system"),
        target: String::from("/system"),
        fs_type: String::from("ext2"),
        read_only: false,
    });

    let _ = dinit.request_mount(MountCmd {
        source: String::from("/dev/block/by-name/init_boot"),
        target: String::from("/init_boot"),
        fs_type: String::from("erofs"),
        read_only: true,
    });

    // Запускаем фоновые сервисы
    let _ = dinit.request_spawn("vfs_sync", "/system/bin/vfs_sync");
    let _ = dinit.request_spawn("auditd", "/system/bin/auditd");

    crate::serial_println!("[dinit] PID 1 активирован (Ring 0, namespace ID 1)");
    *DINIT.lock() = Some(dinit);
}

/// Периодический вызов из таймера ядра
pub fn tick() {
    if let Some(dinit) = DINIT.lock().as_mut() {
        dinit.tick();
    }
}

/// CLI-интерфейс: `dinit [status|services|users|audit|stop <имя>]`
pub fn cmd_dinit(arg: &str) {
    let args: Vec<&str> = arg.split_whitespace().collect();
    let sub = args.first().copied().unwrap_or("status");

    let mut lock = DINIT.lock();
    let dinit = match lock.as_mut() {
        Some(d) => d,
        None => {
            crate::println!("  [dinit] Подсистема не активна.");
            return;
        }
    };

    match sub {
        "status" => {
            crate::println!("=== Dinit (PID 1, Ring 0) ===");
            crate::println!("  Состояние:     {:?}", dinit.state);
            crate::println!("  Аптайм PID 1:  {} мс", crate::timer::uptime_ms().saturating_sub(dinit.start_time));
            crate::println!("  Namespace ID:  {}", dinit.namespace.id);
            crate::println!("  Пользователи:  {}", dinit.namespace.users.len());
            crate::println!("  Сервисы:       {}", dinit.namespace.services.len());
            crate::println!("  Точки mount:   {}", dinit.namespace.mounts.len());
            crate::println!("  Записей audit: {}", dinit.namespace.audit_log.len());
            crate::println!("  Привилегии:    mount={}, spawn={}, manage_users={}",
                dinit.namespace.capabilities.mount,
                dinit.namespace.capabilities.spawn_service,
                dinit.namespace.capabilities.manage_users,
            );
        }
        "services" => {
            crate::println!("=== Сервисы под управлением Dinit ===");
            if dinit.namespace.services.is_empty() {
                crate::println!("  (нет активных сервисов)");
            } else {
                for s in dinit.namespace.services.values() {
                    crate::println!("  [{:>3}] {:<14} PID {:<5?} {:?}",
                        s.id, s.name, s.pid.unwrap_or(0), s.status
                    );
                }
            }
        }
        "users" => {
            crate::println!("=== Пользователи в namespace Dinit ===");
            for u in dinit.namespace.users.values() {
                crate::println!("  UID {:<4} {:<12} в сети с {} мс",
                    u.uid, u.username, u.login_time
                );
            }
        }
        "audit" => {
            crate::println!("=== Журнал аудита Dinit (последние события) ===");
            let count = dinit.namespace.audit_log.len();
            let start_idx = count.saturating_sub(10);
            for (i, entry) in dinit.namespace.audit_log.iter().enumerate().skip(start_idx) {
                crate::println!("  #{:<3} +{}мс UID {} {:?} {} -> {:?}",
                    i + 1, entry.timestamp, entry.uid, entry.op, entry.target, entry.result
                );
            }
        }
        "stop" => {
            if args.len() < 2 {
                crate::println!("  Использование: dinit stop <service_name>");
                return;
            }
            let name = args[1];
            match dinit.request_stop_service(name) {
                Ok(()) => crate::println!("  Сервис '{}' успешно остановлен.", name),
                Err(e) => crate::println!("  Ошибка остановки '{}': {:?}", name, e),
            }
        }
        _ => {
            crate::println!("Команды dinit:");
            crate::println!("  dinit status     - состояние подсистемы PID 1");
            crate::println!("  dinit services   - список контролируемых сервисов");
            crate::println!("  dinit users      - список активных пользователей в namespace");
            crate::println!("  dinit audit      - журнал аудита безопасности");
            crate::println!("  dinit stop <svc> - остановить системный сервис");
        }
    }
}
