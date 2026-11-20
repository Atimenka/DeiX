//! Nano-Core heartbeat — живость служб.
//!
//! Каждая служба берёт «пульс» (beat) регулярно. Для каждой службы
//! задаётся дедлайн: если пульс не приходил дольше deadline_ms, служба
//! считается утонувшей (dead) — наблюдатель инициирует перезапуск
//! по политике restart. Счётчик рестартов и последнее время пульса
//! доступны для отчётов.

use crate::spinlock::SpinLock;
use crate::timer::uptime_ms;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Пульс одной службы.
#[derive(Debug, Clone)]
pub struct Heartbeat {
    /// Имя службы.
    pub name: String,
    /// Последний пришедший пульс (ms от загрузки).
    pub last_beat_ms: u64,
    /// Дедлайн между пульсами (ms).
    pub deadline_ms: u64,
    /// Сколько раз служба считалась утонувшей.
    pub failures: u32,
    /// Сколько раз служба была перезапущена наблюдателем.
    pub restarts: u32,
    /// Признак того, что служба жива (последний пульс был в пределах дедлайна).
    pub alive: bool,
}

impl Heartbeat {
    pub fn new(name: impl Into<String>, deadline_ms: u64) -> Self {
        Heartbeat {
            name: name.into(),
            last_beat_ms: uptime_ms(),
            deadline_ms,
            failures: 0,
            restarts: 0,
            alive: true,
        }
    }

    /// Про registered пульс: обновить время и сбросить счётчик сбоя.
    pub fn beat(&mut self) {
        self.last_beat_ms = uptime_ms();
        self.alive = true;
    }

    /// Проверить живость относительно текущего времени.
    /// Возвращает true, если служба утонула (deadline истек).
    pub fn check_dead(&mut self) -> bool {
        let now = uptime_ms();
        if now.saturating_sub(self.last_beat_ms) > self.deadline_ms {
            self.alive = false;
            self.failures += 1;
            true
        } else {
            self.alive = true;
            false
        }
    }

    /// Отметить перезапуск службы.
    pub fn mark_restarted(&mut self) {
        self.restarts += 1;
        self.beat();
    }
}

/// Регистр пульсов всех служб.
#[derive(Default)]
pub struct HeartbeatTracker {
    pub beats: BTreeMap<String, Heartbeat>,
}

impl HeartbeatTracker {
    pub const fn new() -> Self {
        HeartbeatTracker {
            beats: BTreeMap::new()
        }
    }

    pub fn register(&mut self, name: &str, deadline_ms: u64) {
        self.beats
            .entry(String::from(name))
            .or_insert_with(|| Heartbeat::new(name, deadline_ms));
    }

    pub fn beat(&mut self, name: &str) {
        if let Some(hb) = self.beats.get_mut(name) {
            hb.beat();
        }
    }

    /// Проверить все пульсы; возвращает список утонувших служб.
    pub fn check_all(&mut self) -> Vec<String> {
        let mut dead = Vec::new();
        for (name, hb) in self.beats.iter_mut() {
            if hb.check_dead() {
                dead.push(name.clone());
            }
        }
        dead
    }

    #[allow(dead_code)] // API-поверхность для отчётов
    pub fn len(&self) -> usize {
        self.beats.len()
    }
}

/// Глобальная обёртка для использования из нескольких задач.
pub static HEARTBEATS: SpinLock<HeartbeatTracker> = SpinLock::new(HeartbeatTracker::new());
