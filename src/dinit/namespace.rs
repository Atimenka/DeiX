//! Dinit Namespace & Capabilities
//!
//! Границы ответственности и мандаты Dinit (PID 1). Dinit работает в Ring 0,
//! но ограничен собственным пространством имён и списком привилегий (Capabilities).
//! Ядро выдаёт Dinit ограниченные права и может динамически урезать их (dinit_limit).

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use super::user::UserState;
use super::service::ServiceHandle;
use super::mount::MountHandle;
use super::audit::AuditEntry;

/// Привилегии Dinit, делегированные ядром
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub mount: bool,
    pub spawn_service: bool,
    pub manage_users: bool,
    pub read_kernel_memory: bool,   // Всегда false (прямой доступ к приватным таблицам ядра запрещён)
    pub write_kernel_memory: bool,  // Всегда false
    pub access_port: bool,          // Всегда false (только через ядерные драйверы)
    pub reboot: bool,
    pub halt: bool,
}

impl Capabilities {
    /// Стандартный набор полномочий для PID 1
    pub const fn default_dinit() -> Self {
        Self {
            mount: true,
            spawn_service: true,
            manage_users: true,
            read_kernel_memory: false,
            write_kernel_memory: false,
            access_port: false,
            reboot: true,
            halt: true,
        }
    }

    /// Безопасный урезанный режим (при обнаружении аномалий)
    pub const fn restricted() -> Self {
        Self {
            mount: false,
            spawn_service: false,
            manage_users: true,
            read_kernel_memory: false,
            write_kernel_memory: false,
            access_port: false,
            reboot: true,
            halt: true,
        }
    }
}

/// Пространство имён Dinit (PID 1)
pub struct DinitNamespace {
    pub id: u32,
    pub capabilities: Capabilities,
    pub services: BTreeMap<u32, ServiceHandle>,
    pub mounts: BTreeMap<String, MountHandle>,
    pub users: BTreeMap<u32, UserState>,
    pub audit_log: VecDeque<AuditEntry>,
}

impl DinitNamespace {
    pub fn new(id: u32, capabilities: Capabilities) -> Self {
        Self {
            id,
            capabilities,
            services: BTreeMap::new(),
            mounts: BTreeMap::new(),
            users: BTreeMap::new(),
            audit_log: VecDeque::with_capacity(1024),
        }
    }
}
