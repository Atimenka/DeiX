//! Простейший драйвер текстового VGA-буфера (0xb8000).

use crate::cp866::unicode_to_cp866;
use core::fmt;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

fn color_code(fg: Color, bg: Color) -> u8 {
    (bg as u8) << 4 | (fg as u8)
}

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

#[repr(C)]
#[derive(Clone, Copy)]
struct ScreenChar {
    ascii_character: u8,
    color_code: u8,
}

#[repr(transparent)]
struct Buffer {
    chars: [[ScreenChar; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

pub struct Writer {
    column_position: usize,
    row_position: usize,
    color_code: u8,
    buffer: *mut Buffer,
}

// Writer работает только с MMIO VGA-буфером через raw-указатель — это
// безопасно в нашем однопоточном окружении с отключёнными прерываниями
// на время записи, поэтому помечаем как Send, чтобы можно было положить
// в глобальный статик под мьютексом.
unsafe impl Send for Writer {}

impl Writer {
    pub const fn new(fg: Color, bg: Color) -> Writer {
        Writer {
            column_position: 0,
            row_position: 0,
            color_code: (bg as u8) << 4 | (fg as u8),
            buffer: 0xb8000 as *mut Buffer,
        }
    }

    pub fn set_color(&mut self, fg: Color, bg: Color) {
        self.color_code = color_code(fg, bg);
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            0x08 => self.backspace(),
            byte => {
                if self.column_position >= BUFFER_WIDTH {
                    self.new_line();
                }

                let row = self.row_position;
                let col = self.column_position;
                let color_code = self.color_code;

                let sc = ScreenChar {
                    ascii_character: byte,
                    color_code,
                };
                unsafe {
                    (*self.buffer).chars[row][col] = sc;
                }
                self.column_position += 1;
                self.update_cursor();
            }
        }
    }

    pub fn write_string(&mut self, s: &str) {
        // Строка приходит в UTF-8 (стандартное представление &str в Rust),
        // поэтому разбираем её по символам (char), а не по байтам — иначе
        // многобайтовая кириллица развалится на мусорные байты.
        for ch in s.chars() {
            match ch {
                '\n' => self.write_byte(b'\n'),
                '\u{8}' => self.write_byte(0x08),
                c if c.is_ascii() && (0x20..=0x7e).contains(&(c as u32)) => {
                    self.write_byte(c as u8)
                }
                c => match unicode_to_cp866(c) {
                    Some(code) => self.write_byte(code),
                    None => self.write_byte(0xfe),
                },
            }
        }
    }

    fn backspace(&mut self) {
        if self.column_position > 0 {
            self.column_position -= 1;
            let row = self.row_position;
            let col = self.column_position;
            let blank = ScreenChar {
                ascii_character: b' ',
                color_code: self.color_code,
            };
            unsafe {
                (*self.buffer).chars[row][col] = blank;
            }
            self.update_cursor();
        }
    }

    fn new_line(&mut self) {
        if self.row_position + 1 >= BUFFER_HEIGHT {
            // простая реализация: скроллим весь буфер вверх на 1 строку
            unsafe {
                for row in 1..BUFFER_HEIGHT {
                    for col in 0..BUFFER_WIDTH {
                        let c = (*self.buffer).chars[row][col];
                        (*self.buffer).chars[row - 1][col] = c;
                    }
                }
                let blank = ScreenChar {
                    ascii_character: b' ',
                    color_code: self.color_code,
                };
                for col in 0..BUFFER_WIDTH {
                    (*self.buffer).chars[BUFFER_HEIGHT - 1][col] = blank;
                }
            }
        } else {
            self.row_position += 1;
        }
        self.column_position = 0;
        self.update_cursor();
    }

    pub fn clear_screen(&mut self) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        unsafe {
            for row in 0..BUFFER_HEIGHT {
                for col in 0..BUFFER_WIDTH {
                    (*self.buffer).chars[row][col] = blank;
                }
            }
        }
        self.row_position = 0;
        self.column_position = 0;
        self.update_cursor();
    }

    /// Двигаем аппаратный текстовый курсор VGA через порты 0x3D4/0x3D5,
    /// чтобы в терминале было видно мигающий курсор, как в обычной консоли.
    fn update_cursor(&self) {
        let pos = (self.row_position * BUFFER_WIDTH + self.column_position) as u16;
        unsafe {
            crate::port::outb(0x3D4, 0x0F);
            crate::port::outb(0x3D5, (pos & 0xFF) as u8);
            crate::port::outb(0x3D4, 0x0E);
            crate::port::outb(0x3D5, ((pos >> 8) & 0xFF) as u8);
        }
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}
