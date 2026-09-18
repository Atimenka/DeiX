// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// sound — АУДИО-ДРАЙВЕР (PC speaker через PIT-канал 2, порт 0x61).
// Реализует beep() (звуковой сигнал заданной частоты/длительности) и
// предупреждающие сигналы: warn_triple(), ota_alert().
// no_std-совместимо: только порты ввода-вывода, без аллокаций.


use crate::port::{inb, outb};

const PIT_CMD: u16 = 0x43;
const PIT_CH2: u16 = 0x42;
const SPEAKER: u16 = 0x61;

/// Включает PC speaker (бит 1 порта 0x61).
fn speaker_on() {
    unsafe {
        let v = inb(SPEAKER);
        outb(SPEAKER, (v & !0x02) | 0x02); // keep gate, set speaker bit
    }
}

/// Выключает PC speaker.
fn speaker_off() {
    unsafe {
        let v = inb(SPEAKER);
        outb(SPEAKER, v & !0x02);
    }
}

/// Программирует PIT-канал 2 на частоту `hz`.
fn pit_freq(hz: u32) {
    let divisor: u32 = (1193182u32 / hz.max(20)).clamp(1, 65535);
    unsafe {
        outb(PIT_CMD, 0xB6); // channel 2, lobyte/hibyte, square wave
        outb(PIT_CH2, (divisor & 0xFF) as u8);
        outb(PIT_CH2, ((divisor >> 8) & 0xFF) as u8);
    }
}

/// Блокирующая пауза (busy-wait по PIT-тикам таймера ядра).
fn delay_ms(ms: u64) {
    let t0 = crate::timer::uptime_ms();
    while crate::timer::uptime_ms().saturating_sub(t0) < ms {
        unsafe { core::arch::asm!("nop"); }
    }
}

/// Звуковой сигнал: частота `hz`, длительность `ms`.
pub fn beep(hz: u32, ms: u64) {
    pit_freq(hz);
    speaker_on();
    delay_ms(ms);
    speaker_off();
}

/// Предупреждение «внимание, действие» — три коротких сигнала.
pub fn warn_triple() {
    for _ in 0..3 {
        beep(880, 120);
        delay_ms(80);
    }
}

/// Сигнал OTA-обновления: длинный высокий + короткий.
pub fn ota_alert() {
    beep(1320, 300);
    delay_ms(120);
    beep(1760, 200);
    delay_ms(120);
    beep(1320, 300);
}

