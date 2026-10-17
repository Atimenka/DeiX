//! Диспетчер точек монтирования в Dinit (PID 1)
//!
//! Правила монтирования проверяются через src/vault.rs:
//! - Системный раздел /system монтируется только EROFS и только RO.
//! - Пользовательский раздел /userdata монтируется только EXT2 и только RW.

use alloc::format;
use alloc::string::String;
use crate::init_parser::{BootStage, MountMode};
use crate::vault::{evaluate as vault_evaluate, VaultRejection};

/// Активная точка монтирования в ядре
#[derive(Debug, Clone)]
pub struct MountPoint {
    /// Целевой каталог монтирования (например, "/system", "/userdata")
    pub target: String,
    /// Исходное блочное устройство или образ (например, "/dev/block/by-name/system")
    pub source: String,
    /// Тип файловой системы ("erofs", "ext2", "procfs", "devfs")
    pub fs_type: String,
    /// Монтирование только для чтения
    pub read_only: bool,
}

impl MountPoint {
    pub fn new(target: &str, source: &str, fs_type: &str, read_only: bool) -> Self {
        Self {
            target: String::from(target),
            source: String::from(source),
            fs_type: String::from(fs_type),
            read_only,
        }
    }

    pub fn format_line(&self) -> String {
        let mode_str = if self.read_only { "ro" } else { "rw" };
        format!(
            "{:<16} on {:<14} type {:<8} ({})",
            self.source, self.target, self.fs_type, mode_str
        )
    }
}

/// Команда на монтирование из init.deix
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

    /// Проверка команды по правилам монтирования (src/vault.rs)
    pub fn validate_with_vault(&self, stage: BootStage) -> Result<(), &'static str> {
        let mode = if self.read_only {
            MountMode::ReadOnly
        } else {
            MountMode::ReadWrite
        };

        match vault_evaluate(&self.target, &self.fs_type, mode, stage) {
            Ok(()) => Ok(()),
            Err(VaultRejection::UserdataPolicy(_)) => {
                Err("отклонено: RW-монтирование /userdata вне допустимой стадии")
            }
            Err(VaultRejection::InvalidUserdataFs(_)) => {
                Err("отклонено: для /userdata допустима только файловая система ext2")
            }
            Err(VaultRejection::SystemPolicy(_)) => {
                Err("отклонено: /system монтируется только как erofs ReadOnly")
            }
        }
    }
}
