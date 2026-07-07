//! Программируемый интервальный таймер (PIT, чип 8253/8254) — настраиваем
//! его на генерацию прерываний с частотой около 100 Гц, чтобы вести счёт
//! времени (аптайм).

use crate::port::outb;
use core::sync::atomic::{AtomicU64, Ordering};

static TICKS: AtomicU64 = AtomicU64::new(0);

const PIT_FREQUENCY_HZ: u32 = 1_193_182;
const TARGET_HZ: u32 = 100;

pub fn init() {
    let divisor = (PIT_FREQUENCY_HZ / TARGET_HZ) as u16;
    unsafe {
        outb(0x43, 0x36); // channel 0, lobyte/hibyte, mode 3 (square wave)
        outb(0x40, (divisor & 0xFF) as u8);
        outb(0x40, ((divisor >> 8) & 0xFF) as u8);
    }
}

pub fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Возвращает аптайм в миллисекундах (при частоте таймера 100 Гц каждый
/// тик — это 10 мс).
pub fn uptime_ms() -> u64 {
    ticks() * (1000 / TARGET_HZ as u64)
}
