//! Управление пользователями и пользовательскими сессиями в Dinit
//!
//! Интегрирован с базой пользователей DeiX OS (USERS.DB) и подсистемой auth.rs.

#![allow(dead_code)]

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Состояние пользователя и его активных сессий
#[derive(Debug, Clone)]
pub struct UserState {
    pub uid: u32,
    pub gid: u32,
    pub username: String,
    pub home_dir: String,
    pub shell: String,
    pub login_time: u64,
    pub last_active_time: u64,
    pub active_pids: Vec<u32>,
    pub is_locked: bool,
}

impl UserState {
    pub fn new(uid: u32, username: &str, now: u64) -> Self {
        let home = if uid == 0 {
            String::from("/root")
        } else {
            format!("/users/{}", username)
        };

        Self {
            uid,
            gid: if uid == 0 { 0 } else { 1000 },
            username: String::from(username),
            home_dir: home,
            shell: String::from("/bin/sh"),
            login_time: now,
            last_active_time: now,
            active_pids: Vec::new(),
            is_locked: false,
        }
    }

    pub fn is_root(&self) -> bool {
        self.uid == 0
    }

    pub fn add_pid(&mut self, pid: u32) {
        if !self.active_pids.contains(&pid) {
            self.active_pids.push(pid);
        }
    }

    pub fn remove_pid(&mut self, pid: u32) {
        self.active_pids.retain(|&p| p != pid);
    }

    pub fn touch(&mut self, now: u64) {
        self.last_active_time = now;
    }

    pub fn format_line(&self, now: u64) -> String {
        let active_sec = now.saturating_sub(self.login_time) / 1000;
        format!(
            "UID:{:<4} GID:{:<4} {:<12} Home:{:<18} PIDs:{:<2} Up:{:<5}s",
            self.uid,
            self.gid,
            self.username,
            self.home_dir,
            self.active_pids.len(),
            active_sec
        )
    }
}
