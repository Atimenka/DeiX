//! Драйвер PS/2-клавиатуры: переводит скан-коды (набор 1, как отдаёт
//! стандартный контроллер, эмулируемый QEMU) в ASCII-символы и складывает
//! их в маленькую кольцевую очередь, откуда их читает CLI.

use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;
use core::sync::atomic::{AtomicBool, Ordering};

const ESCAPE_SCANCODE: u8 = 0x01;
static ESCAPE_PRESSED: AtomicBool = AtomicBool::new(false);

const QUEUE_SIZE: usize = 128;

struct KeyQueue {
    buf: [u8; QUEUE_SIZE],
    head: usize,
    tail: usize,
}

impl KeyQueue {
    const fn new() -> Self {
        KeyQueue {
            buf: [0; QUEUE_SIZE],
            head: 0,
            tail: 0,
        }
    }

    fn push(&mut self, byte: u8) {
        let next = (self.tail + 1) % QUEUE_SIZE;
        if next != self.head {
            self.buf[self.tail] = byte;
            self.tail = next;
        }
        // если очередь переполнена — просто теряем символ, ядро мини-размера
    }

    fn pop(&mut self) -> Option<u8> {
        if self.head == self.tail {
            None
        } else {
            let byte = self.buf[self.head];
            self.head = (self.head + 1) % QUEUE_SIZE;
            Some(byte)
        }
    }
}

static QUEUE: SpinLock<KeyQueue> = SpinLock::new(KeyQueue::new());
static SHIFT_PRESSED: AtomicBool = AtomicBool::new(false);
static EXTENDED_PREFIX: AtomicBool = AtomicBool::new(false);

// Скан-коды набора 1 для отпускания клавиши имеют старший бит установлен
// (release = make-code | 0x80).
const LEFT_SHIFT_MAKE: u8 = 0x2A;
const RIGHT_SHIFT_MAKE: u8 = 0x36;
const LEFT_SHIFT_BREAK: u8 = LEFT_SHIFT_MAKE | 0x80;
const RIGHT_SHIFT_BREAK: u8 = RIGHT_SHIFT_MAKE | 0x80;
const BACKSPACE: u8 = 0x0E;
const ENTER: u8 = 0x1C;
const EXTENDED_PREFIX_BYTE: u8 = 0xE0;

// Стрелки приходят как двухбайтовая последовательность 0xE0 0x48 (вверх) /
// 0xE0 0x50 (вниз). Мы транслируем их в управляющие ASCII-байты DC1/DC2,
// которые никогда не встречаются при обычном наборе текста — CLI (cli.rs)
// уже умеет их различать и использовать для навигации по истории команд.
const ARROW_UP_SCANCODE: u8 = 0x48;
const ARROW_DOWN_SCANCODE: u8 = 0x50;
pub const ARROW_UP: u8 = 0x11; // DC1
pub const ARROW_DOWN: u8 = 0x12; // DC2

// Таблица скан-код -> ASCII для стандартной раскладки US QWERTY (без Shift).
const SCANCODE_TO_ASCII: [u8; 128] = build_ascii_table();
const SCANCODE_TO_ASCII_SHIFT: [u8; 128] = build_ascii_table_shift();

const fn build_ascii_table() -> [u8; 128] {
    let mut table = [0u8; 128];
    let pairs: &[(u8, u8)] = &[
        (0x02, b'1'), (0x03, b'2'), (0x04, b'3'), (0x05, b'4'), (0x06, b'5'),
        (0x07, b'6'), (0x08, b'7'), (0x09, b'8'), (0x0A, b'9'), (0x0B, b'0'),
        (0x0C, b'-'), (0x0D, b'='),
        (0x10, b'q'), (0x11, b'w'), (0x12, b'e'), (0x13, b'r'), (0x14, b't'),
        (0x15, b'y'), (0x16, b'u'), (0x17, b'i'), (0x18, b'o'), (0x19, b'p'),
        (0x1A, b'['), (0x1B, b']'),
        (0x1E, b'a'), (0x1F, b's'), (0x20, b'd'), (0x21, b'f'), (0x22, b'g'),
        (0x23, b'h'), (0x24, b'j'), (0x25, b'k'), (0x26, b'l'),
        (0x27, b';'), (0x28, b'\''), (0x29, b'`'),
        (0x2B, b'\\'),
        (0x2C, b'z'), (0x2D, b'x'), (0x2E, b'c'), (0x2F, b'v'), (0x30, b'b'),
        (0x31, b'n'), (0x32, b'm'),
        (0x33, b','), (0x34, b'.'), (0x35, b'/'),
        (0x39, b' '),
    ];
    let mut i = 0;
    while i < pairs.len() {
        let (code, ch) = pairs[i];
        table[code as usize] = ch;
        i += 1;
    }
    table
}

const fn build_ascii_table_shift() -> [u8; 128] {
    let mut table = [0u8; 128];
    let pairs: &[(u8, u8)] = &[
        (0x02, b'!'), (0x03, b'@'), (0x04, b'#'), (0x05, b'$'), (0x06, b'%'),
        (0x07, b'^'), (0x08, b'&'), (0x09, b'*'), (0x0A, b'('), (0x0B, b')'),
        (0x0C, b'_'), (0x0D, b'+'),
        (0x10, b'Q'), (0x11, b'W'), (0x12, b'E'), (0x13, b'R'), (0x14, b'T'),
        (0x15, b'Y'), (0x16, b'U'), (0x17, b'I'), (0x18, b'O'), (0x19, b'P'),
        (0x1A, b'{'), (0x1B, b'}'),
        (0x1E, b'A'), (0x1F, b'S'), (0x20, b'D'), (0x21, b'F'), (0x22, b'G'),
        (0x23, b'H'), (0x24, b'J'), (0x25, b'K'), (0x26, b'L'),
        (0x27, b':'), (0x28, b'"'), (0x29, b'~'),
        (0x2B, b'|'),
        (0x2C, b'Z'), (0x2D, b'X'), (0x2E, b'C'), (0x2F, b'V'), (0x30, b'B'),
        (0x31, b'N'), (0x32, b'M'),
        (0x33, b'<'), (0x34, b'>'), (0x35, b'?'),
        (0x39, b' '),
    ];
    let mut i = 0;
    while i < pairs.len() {
        let (code, ch) = pairs[i];
        table[code as usize] = ch;
        i += 1;
    }
    table
}

/// Вызывается из обработчика прерывания IRQ1 (см. interrupts.rs).
pub fn on_scancode(scancode: u8) {
    // Расширенные клавиши (стрелки, Home/End и т.д.) шлют префикс 0xE0
    // перед основным скан-кодом — запоминаем это на один следующий байт.
    if scancode == EXTENDED_PREFIX_BYTE {
        EXTENDED_PREFIX.store(true, Ordering::Relaxed);
        return;
    }
    let extended = EXTENDED_PREFIX.swap(false, Ordering::Relaxed);

    if extended {
        match scancode {
            ARROW_UP_SCANCODE => QUEUE.lock().push(ARROW_UP),
            ARROW_DOWN_SCANCODE => QUEUE.lock().push(ARROW_DOWN),
            _ => {}
        }
        return;
    }

    match scancode {
        ESCAPE_SCANCODE => {
            ESCAPE_PRESSED.store(true, Ordering::Relaxed);
        }
        LEFT_SHIFT_MAKE | RIGHT_SHIFT_MAKE => {
            SHIFT_PRESSED.store(true, Ordering::Relaxed);
        }
        LEFT_SHIFT_BREAK | RIGHT_SHIFT_BREAK => {
            SHIFT_PRESSED.store(false, Ordering::Relaxed);
        }
        BACKSPACE => {
            QUEUE.lock().push(0x08);
        }
        ENTER => {
            QUEUE.lock().push(b'\n');
        }
        code if code < 0x80 => {
            let shift = SHIFT_PRESSED.load(Ordering::Relaxed);
            let ascii = if shift {
                SCANCODE_TO_ASCII_SHIFT[code as usize]
            } else {
                SCANCODE_TO_ASCII[code as usize]
            };
            if ascii != 0 {
                QUEUE.lock().push(ascii);
            }
        }
        _ => {
            // отпускание прочих клавиш — игнорируем
        }
    }
}

/// Неблокирующее чтение одного символа из очереди (для CLI).
///
/// Вызывается из обычного кода ядра (не из обработчика прерывания), поэтому
/// на время работы со спинлоком нужно отключать прерывания — иначе, если
/// IRQ клавиатуры сработает ровно в этот момент, обработчик прерывания
/// (on_scancode) начнёт крутиться в ожидании того же лока и система
/// зависнет намертво (см. подробности в sync.rs).
pub fn try_read_char() -> Option<u8> {
    without_interrupts(|| QUEUE.lock().pop())
}

/// Неблокирующая проверка "была ли нажата клавиша Esc с прошлого вызова"
/// (сбрасывает флаг после чтения) — используется UI-циклом (ui/mod.rs)
/// как способ выйти из графического режима обратно в текстовый CLI.
pub fn try_read_escape() -> bool {
    ESCAPE_PRESSED.swap(false, Ordering::Relaxed)
}

/// Блокирующее чтение — крутимся в hlt, пока не появится символ
/// (hlt пробуждается по любому прерыванию, включая таймер, так что
/// энергоэффективно и отзывчиво одновременно).
pub fn read_char() -> u8 {
    loop {
        if let Some(c) = try_read_char() {
            return c;
        }
        unsafe { core::arch::asm!("hlt") };
    }
}

/// Читает строку до Enter (используется DS REPL и другими интерактивными
/// инструментами). Показывает вводимые символы на экране.
pub fn read_line() -> alloc::string::String {
    #[allow(unused_imports)]
    let mut buf = [0u8; 256];
    let mut len = 0usize;
    loop {
        let c = read_char();
        match c {
            b'\n' => {
                crate::print!("\n");
                break;
            }
            0x08 => {
                if len > 0 {
                    len -= 1;
                    crate::print!("\u{8}");
                }
            }
            byte if byte >= 0x20 && byte < 0x7F => {
                if len < 256 {
                    buf[len] = byte;
                    len += 1;
                    let s = [byte];
                    if let Ok(s) = core::str::from_utf8(&s) {
                        crate::print!("{}", s);
                    }
                }
            }
            _ => {}
        }
    }
    core::str::from_utf8(&buf[..len]).unwrap_or("").into()
}
