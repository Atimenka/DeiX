//! Глобальный доступ к VGA writer'у, чтобы макросы print!/println! можно
//! было вызывать из любого модуля ядра (включая обработчики прерываний).
//!
//! Дополнительно реализован механизм "перехвата" вывода в строковый буфер
//! (см. begin_capture/end_capture) — он нужен графическому терминалу
//! (ui/mod.rs), чтобы можно было выполнять РЕАЛЬНЫЕ команды полного CLI
//! (cli::execute) внутри окна на рабочем столе: пока перехват активен,
//! весь текст, который команда напечатала бы в VGA text buffer, вместо
//! этого складывается в обычную String, которую окно потом построчно
//! добавляет в свою прокручиваемую историю.

use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;
use crate::vga::{Color, Writer};
use alloc::string::String;

pub static WRITER: SpinLock<Writer> = SpinLock::new(Writer::new(Color::LightGreen, Color::Black));

/// Пока это Some(...), весь вывод print!/println! уходит сюда вместо
/// экрана. Не поддерживает вложенный перехват (второй begin_capture
/// внутри первого молча заменит буфер) — этого пока достаточно, так как
/// единственный потребитель (графический терминал) не запускает
/// перехват рекурсивно (см. защиту от повторного входа в gpu.rs::cmd_gpu_mode
/// через IN_GRAPHICAL_SESSION).
static CAPTURE_BUFFER: SpinLock<Option<String>> = SpinLock::new(None);

/// Начинает перехват вывода в буфер (см. модульную документацию выше).
pub fn begin_capture() {
    without_interrupts(|| {
        *CAPTURE_BUFFER.lock() = Some(String::new());
    });
}

/// Останавливает перехват и возвращает всё, что было "напечатано" за это
/// время (пустая строка, если перехват не был активен).
pub fn end_capture() -> String {
    without_interrupts(|| CAPTURE_BUFFER.lock().take().unwrap_or_default())
}

#[doc(hidden)]
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    // Отключаем прерывания на время печати, чтобы обработчик клавиатуры
    // не мог вклиниться посередине и не словить дедлок на спинлоке
    // (см. подробное объяснение в sync.rs).
    without_interrupts(|| {
        let mut capture = CAPTURE_BUFFER.lock();
        if let Some(buf) = capture.as_mut() {
            let _ = buf.write_fmt(args);
        } else {
            drop(capture);
            WRITER.lock().write_fmt(args).unwrap();
        }
    });
}

/// Безопасно выполнить операцию с WRITER, временно отключив прерывания.
/// Используй эту функцию вместо прямого `WRITER.lock()` в обычном коде
/// ядра (не в обработчиках прерываний) — иначе есть риск дедлока.
pub fn with_writer<F: FnOnce(&mut Writer) -> R, R>(f: F) -> R {
    without_interrupts(|| f(&mut WRITER.lock()))
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vgaglobal::_print(core::format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", core::format_args!($($arg)*)));
}

