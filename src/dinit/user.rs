//! Управление пользователями в Dinit
//!
//! Отслеживает сессии, UID, процессы и квоты пользователей.

use alloc::string::String;
use alloc::vec::Vec;

/// Состояние зарегистрированного пользователя
#[derive(Debug, Clone)]
pub struct UserState {
    pub uid: u32,
    pub username: String,
    pub login_time: u64,
    pub processes: Vec<u32>,
    pub open_files: usize,
    pub memory_used: usize,
}

impl UserState {
    pub fn new(uid: u32, username: &str, login_time: u64) -> Self {
        Self {
            uid,
            username: String::from(username),
            login_time,
            processes: Vec::new(),
            open_files: 0,
            memory_used: 0,
        }
    }

    pub fn attach_process(&mut self, pid: u32) {
        if !self.processes.contains(&pid) {
            self.processes.push(pid);
        }
    }

    pub fn detach_process(&mut self, pid: u32) {
        self.processes.retain(|&p| p != pid);
    }
}
