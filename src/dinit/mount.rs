//! Диспетчер точек монтирования и файловых систем в Dinit (PID 1)
//!
//! Интегрирован с KERNEL SECURITY VAULT (src/vault.rs):
//! - Защита системных разделов (/kernel, /init_boot, /boot, /vendor_boot, /super, /system, /recovery)
//!   от монтирования в режиме RW вне контекста прошивки (Fastbootd, EDL, Recovery).
//! - Абсолютная изоляция раздела /TPM от монтирования любыми пользователями.
//! - Контроль прав /userdata (ext4/ext2).

#![allow(dead_code)]

use alloc::format;
use alloc::string::String;
use crate::init_parser::{BootStage, MountMode};
use crate::vault::{evaluate as vault_evaluate, VaultRejection};

/// Флаги монтирования файловой системы
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountFlags {
    pub read_only: bool,
    pub nosuid: bool,
    pub nodev: bool,
    pub noexec: bool,
}

impl MountFlags {
    pub const RO: Self = Self {
        read_only: true,
        nosuid: true,
        nodev: true,
        noexec: false,
    };

    pub const RW: Self = Self {
        read_only: false,
        nosuid: false,
        nodev: false,
        noexec: false,
    };
}

/// Активная точка монтирования в ядре
#[derive(Debug, Clone)]
pub struct MountPoint {
    /// Целевой каталог монтирования (например, "/system", "/userdata")
    pub target: String,
    /// Исходное блочное устройство или образ (например, "/dev/block/by-name/system")
    pub source: String,
    /// Тип файловой системы ("erofs", "ext4", "ext2", "ramdisk", "procfs", "devfs")
    pub fs_type: String,
    /// Флаги монтирования
    pub flags: MountFlags,
    /// Время монтирования (uptime_ms)
    pub mounted_at: u64,
    /// Счётчик выполненных операций ввода-вывода
    pub io_operations: u64,
}

impl MountPoint {
    pub fn new(target: &str, source: &str, fs_type: &str, read_only: bool, now: u64) -> Self {
        Self {
            target: String::from(target),
            source: String::from(source),
            fs_type: String::from(fs_type),
            flags: MountFlags {
                read_only,
                nosuid: true,
                nodev: true,
                noexec: false,
            },
            mounted_at: now,
            io_operations: 0,
        }
    }

    pub fn format_line(&self) -> String {
        let mode_str = if self.flags.read_only { "ro" } else { "rw" };
        format!(
            "{:<16} on {:<14} type {:<8} ({})",
            self.source, self.target, self.fs_type, mode_str
        )
    }
}

/// Команда на монтирование из init.deix или API
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountCmd {
    pub source: String,
    pub target: String,
    pub fs_type: String,
    pub read_only: bool,
}

impl MountCmd {
    pub fn new(source: &str, target: &str, fs_type: &str, read_only: bool) -> Self {
        Self {
            source: String::from(source),
            target: String::from(target),
            fs_type: String::from(fs_type),
            read_only,
        }
    }

    /// Проверка команды через KERNEL SECURITY VAULT
    pub fn validate_with_vault(&self, stage: BootStage) -> Result<(), &'static str> {
        // 1. Проверка защищённого хранилища TPM: /TPM закрыт категорически
        if self.target.starts_with("/tpm") || self.target.starts_with("/TPM") {
            return Err("VAULT_SECURITY_VIOLATION: /TPM partition is strictly sealed");
        }

        let mode = if self.read_only {
            MountMode::ReadOnly
        } else {
            MountMode::ReadWrite
        };

        // 2. Оценка через эталонный Vault-движок ядра (src/vault.rs)
        match vault_evaluate(&self.target, &self.fs_type, mode, stage) {
            Ok(()) => Ok(()),
            Err(VaultRejection::UserdataPolicy(_)) => {
                Err("SECURITY_WARNING: Userdata RW attempted in invalid stage")
            }
            Err(VaultRejection::NonExt4Userdata(_)) => {
                Err("SECURITY_WARNING: Userdata requested non-ext4 filesystem")
            }
            Err(VaultRejection::TpmPartitionDenied(_)) => {
                Err("SECURITY_VIOLATION: Attempt to mount TPM enclave directly")
            }
        }
    }
}
