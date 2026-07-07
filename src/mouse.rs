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

struct MouseState {
    x: i32,
    y: i32,
    left_button: bool,
    right_button: bool,
    middle_button: bool,
    packet: [u8; 3],
    packet_index: usize,
    screen_width: i32,
    screen_height: i32,
}

static STATE: SpinLock<MouseState> = SpinLock::new(MouseState {
    x: 0,
    y: 0,
    left_button: false,
    right_button: false,
    middle_button: false,
    packet: [0; 3],
    packet_index: 0,
    screen_width: 640,
    screen_height: 480,
});

fn wait_for_write() {
    // Ждём, пока input buffer контроллера освободится (бит 1 статуса == 0).
    for _ in 0..100_000 {
        unsafe {
            if inb(PS2_STATUS_PORT) & 0x02 == 0 {
                return;
            }
        }
    }
}

fn wait_for_read() {
    // Ждём, пока output buffer будет заполнен (бит 0 статуса == 1).
    for _ in 0..100_000 {
        unsafe {
            if inb(PS2_STATUS_PORT) & 0x01 != 0 {
                return;
            }
        }
    }
}

fn write_command(cmd: u8) {
    wait_for_write();
    unsafe { outb(PS2_COMMAND_PORT, cmd) };
}

fn write_data(data: u8) {
    wait_for_write();
    unsafe { outb(PS2_DATA_PORT, data) };
}

fn read_data() -> u8 {
    wait_for_read();
    unsafe { inb(PS2_DATA_PORT) }
}

/// Отправляет байт-команду самой мыши (через "прокси" 0xD4) и ждёт ACK (0xFA).
fn mouse_write(data: u8) {
    write_command(0xD4);
    write_data(data);
}

pub fn init(screen_width: u32, screen_height: u32) {
    without_interrupts(|| {
        let mut state = STATE.lock();
        state.screen_width = screen_width as i32;
        state.screen_height = screen_height as i32;
        state.x = screen_width as i32 / 2;
        state.y = screen_height as i32 / 2;
    });

    // 1. Включаем вспомогательное устройство (aux, т.е. мышь) на контроллере.
    write_command(0xA8);

    // 2. Читаем текущий Compaq Status byte, выставляем бит "Enable IRQ12"
    // и на всякий случай убеждаемся, что бит "Disable Mouse Clock" сброшен.
    write_command(0x20);
    let mut status = read_data();
    status |= 0x02; // Enable IRQ12
    status &= !0x20; // Enable mouse clock
    write_command(0x60);
    write_data(status);

    // 3. Просим мышь использовать настройки по умолчанию и включаем поток
    // данных (Enable Data Reporting).
    mouse_write(0xF6);
    let _ack1 = read_data();

    mouse_write(0xF4);
    let _ack2 = read_data();

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

        state.left_button = flags & 0x01 != 0;
        state.right_button = flags & 0x02 != 0;
        state.middle_button = flags & 0x04 != 0;

        // PS/2 мышь считает Y вверх положительным — экранные координаты
        // растут вниз, поэтому инвертируем.
        state.x = (state.x + dx).clamp(0, state.screen_width - 1);
        state.y = (state.y - dy).clamp(0, state.screen_height - 1);
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
        state.right_button = packet.buttons & vmmouse::RIGHT_BUTTON != 0;
        state.middle_button = packet.buttons & vmmouse::MIDDLE_BUTTON != 0;
    }
}

/// true, если сейчас активен абсолютный режим позиционирования курсора
/// (VMware/QEMU backdoor) — используется UI, чтобы решить, нужно ли вообще
/// нам самим рисовать курсор поверх картинки (см. подробности в
/// ui/mod.rs::draw_cursor).
pub fn is_absolute() -> bool {
    ABSOLUTE_MODE.load(Ordering::Relaxed)
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
    pub right_button: bool,
    pub middle_button: bool,
}

pub fn snapshot() -> MouseSnapshot {
    without_interrupts(|| {
        let state = STATE.lock();
        MouseSnapshot {
            x: state.x,
            y: state.y,
            left_button: state.left_button,
            right_button: state.right_button,
            middle_button: state.middle_button,
        }
    })
}
