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
    while !is_transmit_empty() {}
    unsafe { outb(COM1, byte) };
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

#[macro_export]
macro_rules! serial_println {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::serial::SerialWriter, $($arg)*);
    }};
}
