//! Учёт пользовательских сессий в Dinit.
//!
//! Сессии регистрируются при входе пользователя (см. auth.rs) и
//! отображаются командой `dinit users`.

use alloc::format;
use alloc::string::String;

/// Активная сессия пользователя
#[derive(Debug, Clone)]
pub struct UserState {
    pub uid: u32,
    pub gid: u32,
    pub username: String,
    pub home_dir: String,
    pub login_time: u64,
}

impl UserState {
    pub fn new(uid: u32, username: &str, now: u64) -> Self {
        let home = if uid == 0 {
            String::from("/userdata/home/root")
        } else {
            format!("/userdata/home/{}", username)
        };

        Self {
            uid,
            gid: if uid == 0 { 0 } else { 1000 },
            username: String::from(username),
            home_dir: home,
            login_time: now,
        }
    }

    pub fn format_line(&self, now: u64) -> String {
        let active_sec = now.saturating_sub(self.login_time) / 1000;
        format!(
            "UID:{:<4} GID:{:<4} {:<12} Home:{:<22} Up:{:<5}s",
            self.uid,
            self.gid,
            self.username,
            self.home_dir,
            active_sec
        )
    }
}
