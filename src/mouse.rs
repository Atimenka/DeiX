//! Драйвер мыши: стандартная процедура инициализации через контроллер
//! клавиатуры (порты 0x60/0x64), приём 3-байтовых пакетов через IRQ12,
//! декодирование в смещение (dx, dy) и состояние кнопок.
//!
//! Дополнительно, если хост — QEMU (или VMware), при старте пытаемся
//! включить "VMware backdoor" абсолютное позиционирование (см. vmmouse.rs).
//! В этом режиме курсор ОС буквально совпадает с положением курсора мыши
//! хоста в окне QEMU — не нужно "захватывать" мышь и следить за
//! рассинхронизацией относительных смещений. Байты по-прежнему приходят по
//! IRQ12 от PS/2-контроллера (это единственный способ вообще узнать, что
//! мышь шевельнулась), но при активном абсолютном режиме их содержимое не
//! разбираем как обычный PS/2-пакет — вместо этого опрашиваем бэкдор.

use crate::port::{inb, outb};
use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;
use crate::vmmouse;
use core::sync::atomic::{AtomicBool, Ordering};

const PS2_DATA_PORT: u16 = 0x60;
const PS2_STATUS_PORT: u16 = 0x64;
const PS2_COMMAND_PORT: u16 = 0x64;

/// true, если бэкдор VMware/QEMU обнаружен и абсолютный режим включён.
static ABSOLUTE_MODE: AtomicBool = AtomicBool::new(false);
static PS2_MOVEMENT_LOGGED: AtomicBool = AtomicBool::new(false);

struct MouseState {
    x: i32,
    y: i32,
    left_button: bool,
    packet: [u8; 3],
    packet_index: usize,
    screen_width: i32,
    screen_height: i32,
    speed: crate::mouse_speed::MotionScaler,
}

static STATE: SpinLock<MouseState> = SpinLock::new(MouseState {
    x: 0,
    y: 0,
    left_button: false,
    packet: [0; 3],
    packet_index: 0,
    screen_width: 640,
    screen_height: 480,
    speed: crate::mouse_speed::MotionScaler::new(),
});

// Controller commands and mouse ACKs share the same port as the keyboard.
// Never write after a timeout and never consume a keyboard byte as a mouse ACK.
fn wait_for_write() -> bool {
    for _ in 0..100_000 {
        let status = unsafe { inb(PS2_STATUS_PORT) };
        if status == 0xff { return false; }
        if status & 0x02 == 0 { return true; }
        core::hint::spin_loop();
    }
    false
}
fn write_command(cmd: u8) -> bool {
    if !wait_for_write() { return false; }
    unsafe { outb(PS2_COMMAND_PORT, cmd); }
    true
}
fn write_data(data: u8) -> bool {
    if !wait_for_write() { return false; }
    unsafe { outb(PS2_DATA_PORT, data); }
    true
}
fn read_response(aux: bool) -> Option<u8> {
    for _ in 0..100_000 {
        let status = unsafe { inb(PS2_STATUS_PORT) };
        if status == 0xff { return None; }
        if status & 1 == 0 { core::hint::spin_loop(); continue; }
        let byte = unsafe { inb(PS2_DATA_PORT) };
        if status & 0xc0 != 0 { continue; }
        if (status & 0x20 != 0) == aux { return Some(byte); }
        if status & 0x20 != 0 { on_data_byte(byte); }
        else { crate::keyboard::on_scancode(byte); }
    }
    None
}
fn mouse_command(data: u8) -> bool {
    for _ in 0..3 {
        if !write_command(0xd4) || !write_data(data) { return false; }
        match read_response(true) {
            Some(0xfa) => return true,
            Some(0xfe) => continue,
            _ => return false,
        }
    }
    false
}

pub fn init(screen_width: u32, screen_height: u32) {
    without_interrupts(|| {
        let mut state = STATE.lock();
        state.screen_width = screen_width as i32;
        state.screen_height = screen_height as i32;
        state.x = screen_width as i32 / 2;
        state.y = screen_height as i32 / 2;
    });

    let configured = if write_command(0xa8) && write_command(0x20) {
        if let Some(config) = read_response(false) {
            write_command(0x60) && write_data((config | 0x02) & !0x20)
        } else { false }
    } else { false };
    // Always attempt to re-enable streaming, even if the defaults command
    // fails. Both operations are bounded and ACKed by the auxiliary device.
    let defaults = configured && mouse_command(0xf6);
    let enabled = configured && mouse_command(0xf4);
    crate::serial_println!("[mouse] PS/2 config {} defaults {} reporting {}",
        configured, defaults, enabled);

    // Пытаемся включить абсолютное позиционирование через VMware/QEMU
    // backdoor. Это НЕ реальное железо и НЕ часть PS/2-протокола — это
    // отдельный, полностью программный интерфейс, который QEMU эмулирует
    // по умолчанию на машине "pc" (устройства vmmouse+vmport на ISA-шине).
    // Если бэкдора нет (например, кто-то соберёт образ и запустит на
    // Bochs без этого расширения, или мы когда-нибудь попадём на реальное
    // железо через USB-эмуляцию BIOS) — просто остаёмся в обычном
    // относительном PS/2-режиме, никакой деградации функциональности.
    if vmmouse::is_present() {
        vmmouse::enable_absolute();
        ABSOLUTE_MODE.store(true, Ordering::Relaxed);
        crate::serial_println!("[mouse] VMware/QEMU absolute pointer backdoor: enabled");
    } else {
        crate::serial_println!("[mouse] VMware/QEMU backdoor not present, using relative PS/2 mode");
    }
}

/// Вызывается из обработчика прерывания IRQ12 (см. interrupts.rs) с
/// каждым новым байтом от мыши. Собирает 3-байтовые пакеты и обновляет
/// состояние курсора.
///
/// Если активен абсолютный режим (VMware/QEMU backdoor), сам байт нас не
/// интересует — он лишь сигнализирует "что-то произошло", реальные
/// координаты вычитываем прямым опросом бэкдора (см. vmmouse::poll).
pub fn on_data_byte(byte: u8) {
    if ABSOLUTE_MODE.load(Ordering::Relaxed) {
        on_absolute_event();
        return;
    }

    let mut state = STATE.lock();

    // Первый байт пакета должен иметь установленный бит 3 (always-1 bit) —
    // используем это как простую защиту от рассинхронизации потока.
    if state.packet_index == 0 && byte & 0x08 == 0 {
        return;
    }

    let idx = state.packet_index;
    state.packet[idx] = byte;
    state.packet_index += 1;

    if state.packet_index == 3 {
        state.packet_index = 0;

        let flags = state.packet[0];
        let mut dx = state.packet[1] as i32;
        let mut dy = state.packet[2] as i32;

        // Знаковое расширение: биты 4 и 5 первого байта — знак dx/dy.
        if flags & 0x10 != 0 {
            dx -= 256;
        }
        if flags & 0x20 != 0 {
            dy -= 256;
        }

        if (dx != 0 || dy != 0) && !PS2_MOVEMENT_LOGGED.swap(true, Ordering::Relaxed) {
            crate::serial_println!("[mouse] PS/2 movement received");
        }

        // Биты 0x02 (правая) и 0x04 (средняя) не сохраняются:
        // интерфейс пока реагирует только на левую кнопку.
        state.left_button = flags & 0x01 != 0;

        // PS/2 мышь считает Y вверх положительным — экранные координаты
        // растут вниз, поэтому инвертируем.
        let (dx, dy) = state.speed.scale(dx, -dy);
        state.x = state.x.saturating_add(dx).clamp(0, state.screen_width.saturating_sub(1).max(0));
        state.y = state.y.saturating_add(dy).clamp(0, state.screen_height.saturating_sub(1).max(0));
    }
}

/// Опрашивает VMware/QEMU backdoor и масштабирует абсолютные координаты
/// (диапазон бэкдора 0..=0xFFFF независимо от разрешения) в пиксели
/// текущего экрана.
fn on_absolute_event() {
    if let Some(packet) = vmmouse::poll() {
        let mut state = STATE.lock();

        let scaled_x = (packet.x as u64 * (state.screen_width.max(1) - 1) as u64) / 0xFFFF;
        let scaled_y = (packet.y as u64 * (state.screen_height.max(1) - 1) as u64) / 0xFFFF;

        state.x = scaled_x as i32;
        state.y = scaled_y as i32;
        state.left_button = packet.buttons & vmmouse::LEFT_BUTTON != 0;
    }
}


/// Обновляет границы экрана (вызывается при входе в графический режим —
/// см. cli.rs::cmd_gpu_mode), чтобы масштабирование абсолютных координат
/// соответствовало реальному разрешению framebuffer, а не значению по
/// умолчанию 800x600, использованному при старте ядра в текстовом режиме.
pub fn set_screen_size(width: u32, height: u32) {
    without_interrupts(|| {
        let mut state = STATE.lock();
        state.screen_width = width as i32;
        state.screen_height = height as i32;
    });
}

#[derive(Clone, Copy)]
pub struct MouseSnapshot {
    pub x: i32,
    pub y: i32,
    pub left_button: bool,
}

/// Apply a relative HID boot-mouse report (USB Y increases downwards).
pub fn usb_report(dx: i32, dy: i32, buttons: u32) {
    without_interrupts(|| {
        let mut state = STATE.lock();
        let (dx, dy) = state.speed.scale(dx, dy);
        state.x = state.x.saturating_add(dx).clamp(0, state.screen_width.saturating_sub(1).max(0));
        state.y = state.y.saturating_add(dy).clamp(0, state.screen_height.saturating_sub(1).max(0));
        state.left_button = buttons & 1 != 0;
    });
}

/// Sensitivity affects relative PS/2 and USB reports, not absolute VM pointers.
pub fn set_speed(percent: u16) {
    without_interrupts(|| STATE.lock().speed.set_percent(percent));
}
pub fn speed() -> u16 {
    without_interrupts(|| STATE.lock().speed.percent())
}

pub fn snapshot() -> MouseSnapshot {
    without_interrupts(|| {
        let state = STATE.lock();
        MouseSnapshot {
            x: state.x,
            y: state.y,
            left_button: state.left_button,
        }
    })
}
