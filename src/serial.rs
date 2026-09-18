//! Минимальный драйвер последовательного порта COM1 (0x3F8) — нужен для
//! отладочного вывода, который виден независимо от текущего видеорежима
//! (в отличие от VGA text buffer, который не отображается, пока активен
//! графический режим Bochs VBE).

use crate::port::{inb, outb};
use core::fmt;

const COM1: u16 = 0x3F8;

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00); // отключаем прерывания
        outb(COM1 + 3, 0x80); // включаем DLAB для настройки скорости
        outb(COM1 + 0, 0x03); // делитель = 3 (38400 бод)
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03); // 8 бит, без чётности, 1 стоп-бит
        outb(COM1 + 2, 0xC7); // включаем FIFO
        outb(COM1 + 4, 0x0B); // RTS/DSR
    }
}

fn is_transmit_empty() -> bool {
    unsafe { inb(COM1 + 5) & 0x20 != 0 }
}

fn write_byte(byte: u8) {
    // Ждём готовности передатчика (THR empty), но с таймаутом: если по
    // какой-то причине UART не поднимает LSR bit5 (например, экзотическая
    // эмуляция или сбой инициализации), ядро НЕ должно висеть на выводе.
    let mut tries: u32 = 0;
    while !is_transmit_empty() {
        tries += 1;
        if tries > 1_000_000 {
            break;
        }
        core::hint::spin_loop();
    }
    unsafe { outb(COM1, byte) };
}


/// Есть ли принятый байт в приёмном буфере COM1 (LSR bit 0 — Data Ready).
pub fn is_data_ready() -> bool {
    unsafe { inb(COM1 + 5) & 0x01 != 0 }
}

/// Блокирующее чтение одного байта из COM1.
pub fn read_byte() -> u8 {
    while !is_data_ready() {}
    unsafe { inb(COM1) }
}


pub struct SerialWriter;

impl fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            write_byte(byte);
        }
        Ok(())
    }
}

/// Вывод в COM1 БЕЗ перевода строки (парный к `serial_println!`).
/// Нужен, когда текст уже содержит свои переводы строк — например,
/// вывод syscall `write` из Ring 3.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::serial::SerialWriter, $($arg)*);
    }};
}

#[macro_export]
macro_rules! serial_println {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::serial::SerialWriter, $($arg)*);
    }};
}
