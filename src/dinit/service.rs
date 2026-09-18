//! Управление сервисами в Dinit
//!
//! Запуск, остановка, перезапуск и контроль жизненного цикла системных сервисов.

use alloc::string::String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatus {
    Running,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceError {
    AlreadyRunning,
    NotRunning,
    SpawnFailed,
    PermissionDenied,
}

/// Дескриптор зарегистрированного сервиса
#[derive(Debug, Clone)]
pub struct ServiceHandle {
    pub id: u32,
    pub name: String,
    pub path: String,
    pub status: ServiceStatus,
    pub restarts: u32,
    pub pid: Option<u32>,
}

impl ServiceHandle {
    pub fn new(id: u32, name: &str, path: &str) -> Self {
        Self {
            id,
            name: String::from(name),
            path: String::from(path),
            status: ServiceStatus::Stopped,
            restarts: 0,
            pid: None,
        }
    }
}

/// Трейт сервиса Dinit
pub trait Service {
    fn name(&self) -> &str;
    fn start(&mut self) -> Result<(), ServiceError>;
    fn stop(&mut self) -> Result<(), ServiceError>;
    fn status(&self) -> ServiceStatus;
    fn tick(&mut self);
}
