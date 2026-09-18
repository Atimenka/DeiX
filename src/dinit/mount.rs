//! Управление монтированием в Dinit
//!
//! Контроль точек монтирования и проверка через Vault.

use alloc::string::String;

#[derive(Debug, Clone)]
pub struct MountHandle {
    pub target: String,
    pub source: String,
    pub fs_type: String,
    pub read_only: bool,
}

impl MountHandle {
    pub fn new(target: &str, source: &str, fs_type: &str, read_only: bool) -> Self {
        Self {
            target: String::from(target),
            source: String::from(source),
            fs_type: String::from(fs_type),
            read_only,
        }
    }
}

/// Команда монтирования
#[derive(Debug, Clone)]
pub struct MountCmd {
    pub source: String,
    pub target: String,
    pub fs_type: String,
    pub read_only: bool,
}
