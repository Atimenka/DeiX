//! Stub для dinit — Dinit supervisor (заглушка).

pub fn cmd_dinit(_rest: &str) {
    crate::println!("dinit: управление сервисами (заглушка)");
}

pub fn init() {
    // Stub: инициализация Dinit.
}

pub fn tick() {
    // Stub: тикер Dinit.
}

pub static DINIT: crate::spinlock::SpinLock<Option<u32>> = crate::spinlock::SpinLock::new(None);
