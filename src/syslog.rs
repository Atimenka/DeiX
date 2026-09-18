// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// syslog — КОЛЬЦЕВОЙ ЖУРНАЛ ЯДРА (аналог dmesg / kernel log buffer).
// Хранит последние MAX_LINES строк ядра в кольцевом буфере; используется
// отладчиком (bugreport) и экранами ошибок («что произошло перед паникой»).
// no_std-совместимо: alloc (VecDeque, String), потокобезопасно через SpinLock.


use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::spinlock::SpinLock;

/// Максимум строк в кольце (как /dev/kmsg: ограниченный буфер).
const MAX_LINES: usize = 512;
/// Максимальная длина одной строки (обрезаем, чтобы не раздувать память).
const MAX_LINE_LEN: usize = 200;

/// Кольцевой буфер журнала.
static RING: SpinLock<VecDeque<String>> = SpinLock::new(VecDeque::new());

/// Добавляет строку в журнал (усекает до MAX_LINE_LEN, вытесняет старые).
pub fn log_line(msg: &str) {
    let mut ring = RING.lock();
    let line: String = if msg.len() > MAX_LINE_LEN {
        msg[..MAX_LINE_LEN].to_string()
    } else {
        msg.to_string()
    };
    if ring.len() >= MAX_LINES {
        ring.pop_front();
    }
    ring.push_back(line);
}

/// Копия всех строк журнала (для bugreport / экрана ошибок).
pub fn snapshot() -> Vec<String> {
    RING.lock().iter().cloned().collect()
}


/// Очистка журнала (например, после успешного старта ОС).
pub fn clear() {
    RING.lock().clear();
}

/// Последние `n` строк (для экрана «Произошла ошибка»).
pub fn last_lines(n: usize) -> Vec<String> {
    let all = snapshot();
    let skip = all.len().saturating_sub(n);
    all[skip..].to_vec()
}
