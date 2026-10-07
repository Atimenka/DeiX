// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// vault — правила монтирования: /system (EROFS RO) и /userdata (EXT2 RW).
//
// Нарушение правила — это ОТКАЗ В МОНТИРОВАНИИ (регистрируется в аудите
// dinit как VaultRejection), а не отказ ядра: некорректная строка в
// init-скрипте не должна ронять систему.

use crate::init_parser::{BootStage, MountMode};
use alloc::string::String;
use alloc::format;

/// Список системных EROFS-разделов ядра.
pub const SYSTEM_PARTITIONS: [&str; 1] = ["/system"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultRejection {
    UserdataPolicy(String),
    InvalidUserdataFs(String),
    SystemPolicy(String),
}

impl VaultRejection {
    pub fn detail(&self) -> String {
        match self {
            VaultRejection::UserdataPolicy(msg) => msg.clone(),
            VaultRejection::InvalidUserdataFs(msg) => msg.clone(),
            VaultRejection::SystemPolicy(msg) => msg.clone(),
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            VaultRejection::UserdataPolicy(_) => "userdata-policy-violation",
            VaultRejection::InvalidUserdataFs(_) => "invalid-userdata-fs",
            VaultRejection::SystemPolicy(_) => "system-policy-violation",
        }
    }
}

/// ОЦЕНКА ПРАВИЛА VAULT ДЛЯ КОМАНДЫ МОНТИРОВАНИЯ.
pub fn evaluate(dst: &str, fs_type: &str, mode: MountMode, _stage: BootStage) -> Result<(), VaultRejection> {
    let is_system = SYSTEM_PARTITIONS.contains(&dst);

    match dst {
        "/userdata" => {
            if mode == MountMode::ReadOnly {
                return Err(VaultRejection::UserdataPolicy(String::from(
                    "/userdata не должен монтироваться ReadOnly",
                )));
            }
            if fs_type == "ext2" {
                Ok(())
            } else {
                Err(VaultRejection::InvalidUserdataFs(format!(
                    "файловая система '{}' для /userdata отличается от ext2",
                    fs_type
                )))
            }
        }
        _ if is_system => {
            if fs_type == "erofs" && mode == MountMode::ReadOnly {
                Ok(())
            } else {
                Err(VaultRejection::SystemPolicy(format!(
                    "{} монтируется только как erofs ReadOnly (запрошено: {} {})",
                    dst,
                    fs_type,
                    match mode {
                        MountMode::ReadOnly => "ro",
                        MountMode::ReadWrite => "rw",
                    }
                )))
            }
        }
        _ => Ok(()),
    }
}
