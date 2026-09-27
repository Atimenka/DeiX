// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// vault — KERNEL SECURITY VAULT: /system (EROFS RO) и /userdata (EXT2/EXT4 RW).

use crate::init_parser::{BootStage, MountMode};
use alloc::string::String;
use alloc::format;

/// Список системных EROFS-разделов ядра.
pub const SYSTEM_PARTITIONS: [&str; 1] = ["/system"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultRejection {
    UserdataPolicy(String),
    InvalidUserdataFs(String),
}

impl VaultRejection {
    pub fn detail(&self) -> String {
        match self {
            VaultRejection::UserdataPolicy(msg) => msg.clone(),
            VaultRejection::InvalidUserdataFs(msg) => msg.clone(),
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            VaultRejection::UserdataPolicy(_) => "userdata-policy-violation",
            VaultRejection::InvalidUserdataFs(_) => "invalid-userdata-fs",
        }
    }
}

/// ОЦЕНКА ПРАВИЛА VAULT ДЛЯ КОМАНДЫ МОНТИРОВАНИЯ.
pub fn evaluate(dst: &str, fs_type: &str, mode: MountMode, _stage: BootStage) -> Result<(), VaultRejection> {
    let is_system = SYSTEM_PARTITIONS.contains(&dst);

    match dst {
        "/userdata" => {
            if fs_type == "ext2" || fs_type == "ext4" {
                Ok(())
            } else {
                Err(VaultRejection::InvalidUserdataFs(format!(
                    "файловая система '{}' для /userdata отличается от ext2/ext4",
                    fs_type
                )))
            }
        }
        other if is_system => {
            let _: &str = other;
            if fs_type == "erofs" && mode == MountMode::ReadOnly {
                Ok(())
            } else {
                panic!("SECURITY_VIOLATION: /system is EROFS ReadOnly!");
            }
        }
        _ => Ok(()),
    }
}
