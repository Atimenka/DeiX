//! Драйвер PS/2-клавиатуры: переводит скан-коды (набор 1, как отдаёт
//! стандартный контроллер, эмулируемый QEMU) в ASCII-символы и складывает
//! их в маленькую кольцевую очередь, откуда их читает CLI.

use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// Diagnostic counters only; never retain or log the scan codes or text.
static INIT_ACK: AtomicBool = AtomicBool::new(false);
static INPUT_BYTES: AtomicU32 = AtomicU32::new(0);
static AUX_BYTES: AtomicU32 = AtomicU32::new(0);
static SET2_ACK: AtomicBool = AtomicBool::new(false);
static RAW_SET2: AtomicBool = AtomicBool::new(false);
static SET2_DECODER: SpinLock<crate::ps2_set2::Decoder> = SpinLock::new(crate::ps2_set2::Decoder::new());
pub fn diagnostic_status() -> (bool, u32, u32, bool) {
    (INIT_ACK.load(Ordering::Relaxed), INPUT_BYTES.load(Ordering::Relaxed),
     AUX_BYTES.load(Ordering::Relaxed), SET2_ACK.load(Ordering::Relaxed))
}

const ESCAPE_SCANCODE: u8 = 0x01;
static ESCAPE_PRESSED: AtomicBool = AtomicBool::new(false);

/// Win/Super (E0 5B левая, E0 5C правая). Как и Escape, отдаётся
/// отдельным флагом, а не через очередь символов: клавиша глобальная
/// (открыть/закрыть меню Пуск) и не должна попадать в поле ввода.
const LEFT_SUPER_SCANCODE: u8 = 0x5B;
const RIGHT_SUPER_SCANCODE: u8 = 0x5C;
static SUPER_PRESSED: AtomicBool = AtomicBool::new(false);

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

const LEFT_SHIFT_MAKE: u8 = 0x2A;
const RIGHT_SHIFT_MAKE: u8 = 0x36;
const LEFT_SHIFT_BREAK: u8 = LEFT_SHIFT_MAKE | 0x80;
const RIGHT_SHIFT_BREAK: u8 = RIGHT_SHIFT_MAKE | 0x80;
const BACKSPACE: u8 = 0x0E;
const ENTER: u8 = 0x1C;
const EXTENDED_PREFIX_BYTE: u8 = 0xE0;

const ARROW_UP_SCANCODE: u8 = 0x48;
const ARROW_DOWN_SCANCODE: u8 = 0x50;
const ARROW_LEFT_SCANCODE: u8 = 0x4B;
const ARROW_RIGHT_SCANCODE: u8 = 0x4D;

pub const ARROW_UP: u8 = 0x11; // DC1
pub const ARROW_DOWN: u8 = 0x12; // DC2
pub const ARROW_LEFT: u8 = 0x13; // DC3
pub const ARROW_RIGHT: u8 = 0x14; // DC4

const SCANCODE_TO_ASCII: [u8; 128] = build_ascii_table();
const SCANCODE_TO_ASCII_SHIFT: [u8; 128] = build_ascii_table_shift();

const fn build_ascii_table() -> [u8; 128] {
    let mut table = [0u8; 128];
    let pairs: &[(u8, u8)] = &[
        (0x02, b'1'), (0x03, b'2'), (0x04, b'3'), (0x05, b'4'), (0x06, b'5'),
        (0x07, b'6'), (0x08, b'7'), (0x09, b'8'), (0x0A, b'9'), (0x0B, b'0'),
        (0x0C, b'-'), (0x0D, b'='),
        (0x0F, b'\t'),
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
        (0x0F, b'\t'),
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

// i8042 can be left disabled or in a non-translated scan set by firmware.
// Bound every wait: machines without a PS/2 controller must still boot.
fn wait_input_empty() -> bool {
    for _ in 0..100_000 {
        let status = unsafe { crate::port::inb(0x64) };
        if status == 0xff { return false; }
        if status & 2 == 0 { return true; }
        core::hint::spin_loop();
    }
    false
}

fn read_controller_response() -> Option<u8> {
    for _ in 0..100_000 {
        let status = unsafe { crate::port::inb(0x64) };
        if status == 0xff { return None; }
        if status & 1 != 0 {
            let byte = unsafe { crate::port::inb(0x60) };
            if status & 0x20 != 0 { crate::mouse::on_data_byte(byte); continue; }
            return Some(byte);
        }
        core::hint::spin_loop();
    }
    None
}

// Send keyboard commands with bounded RESEND handling. The keyboard must
// emit set 2 when controller translation is enabled; firmware may leave a
// different scan set selected.
fn keyboard_command(byte: u8) -> bool {
    for _ in 0..3 {
        if !wait_input_empty() { return false; }
        unsafe { crate::port::outb(0x60, byte); }
        match read_controller_response() {
            Some(0xfa) => return true,
            Some(0xfe) => continue,
            _ => return false,
        }
    }
    false
}

/// Enable the first PS/2 port, IRQ1, and set-1 translation. Do not reset the
/// controller or touch firmware/SATA settings; preserve the mouse config.
pub fn init() -> bool {
    let ok = without_interrupts(|| {
        if !wait_input_empty() { return false; }
        unsafe { crate::port::outb(0x64, 0xae); }
        if !wait_input_empty() { return false; }
        unsafe { crate::port::outb(0x64, 0x20); }
        let Some(config) = read_controller_response() else { return false; };
        if !wait_input_empty() { return false; }
        unsafe { crate::port::outb(0x64, 0x60); }
        if !wait_input_empty() { return false; }
        unsafe { crate::port::outb(0x60, (config | 0x41) & !0x10); }
        // Some real i8042 implementations do not implement translation.
        // Read the config back rather than assuming the write took effect.
        let readback = if wait_input_empty() {
            unsafe { crate::port::outb(0x64, 0x20); }
            read_controller_response()
        } else { None };
        // Make the assumed scan set explicit. Always re-enable scanning even
        // when changing the set is unsupported or an ACK is missing.
        let disabled = keyboard_command(0xf5);
        let set2 = if disabled { keyboard_command(0xf0) && keyboard_command(0x02) } else { false };
        let enabled = keyboard_command(0xf4);
        SET2_ACK.store(set2, Ordering::Relaxed);
        RAW_SET2.store(set2 && readback.is_some_and(|v| v != 0xff && v & 0x40 == 0), Ordering::Relaxed);
        if RAW_SET2.load(Ordering::Relaxed) { crate::serial_println!("[keyboard] using raw set-2 decoder"); }
        if !set2 { crate::serial_println!("[keyboard] scan set 2 not confirmed"); }
        enabled
    });
    INIT_ACK.store(ok, Ordering::Relaxed);
    crate::serial_println!("[keyboard] i8042 init {} (IRQ + polling)", if ok { "OK" } else { "unconfirmed" });
    ok
}

/// Drain one pending byte using the status AUX bit rather than assuming the
/// IRQ line identifies its source. Used both by IRQ1/IRQ12 and as a fallback
/// on machines whose firmware does not deliver an IRQ for every key.
pub fn poll_controller() {
    let status = unsafe { crate::port::inb(0x64) };
    if status == 0xff || status & 1 == 0 { return; }
    let byte = unsafe { crate::port::inb(0x60) };
    if status & 0xc0 != 0 { return; } // parity or timeout error
    if status & 0x20 != 0 {
        AUX_BYTES.fetch_add(1, Ordering::Relaxed);
        crate::mouse::on_data_byte(byte);
    }
    else {
        INPUT_BYTES.fetch_add(1, Ordering::Relaxed);
        if RAW_SET2.load(Ordering::Relaxed) {
            let translated = SET2_DECODER.lock().feed(byte);
            if let Some((extended, code)) = translated {
                if extended { on_scancode(0xe0); }
                on_scancode(code);
            }
        } else {
            on_scancode(byte);
        }
    }
}

pub fn on_scancode(scancode: u8) {
    if scancode == EXTENDED_PREFIX_BYTE {
        EXTENDED_PREFIX.store(true, Ordering::Relaxed);
        return;
    }
    let extended = EXTENDED_PREFIX.swap(false, Ordering::Relaxed);

    if extended {
        match scancode {
            ARROW_UP_SCANCODE => QUEUE.lock().push(ARROW_UP),
            ARROW_DOWN_SCANCODE => QUEUE.lock().push(ARROW_DOWN),
            ARROW_LEFT_SCANCODE => QUEUE.lock().push(ARROW_LEFT),
            ARROW_RIGHT_SCANCODE => QUEUE.lock().push(ARROW_RIGHT),
            LEFT_SUPER_SCANCODE | RIGHT_SUPER_SCANCODE => {
                SUPER_PRESSED.store(true, Ordering::Relaxed);
            }
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
        _ => {}
    }
}

pub fn try_read_char() -> Option<u8> {
    without_interrupts(|| {
        poll_controller();
        QUEUE.lock().pop()
    })
}

pub fn try_read_escape() -> bool {
    ESCAPE_PRESSED.swap(false, Ordering::Relaxed)
}

/// Было ли нажатие Win/Super с прошлого опроса (флаг сбрасывается).
pub fn try_read_super() -> bool {
    SUPER_PRESSED.swap(false, Ordering::Relaxed)
}

pub fn read_char() -> u8 {
    loop {
        if let Some(c) = try_read_char() {
            return c;
        }
        crate::sched::idle_wait();
    }
}
