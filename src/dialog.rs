// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// dialog — ГРАФИЧЕСКИЕ ДИАЛОГИ для оболочек ядра (recovery/fastbootd/DSM).
//
// Проблема, которую решает модуль: раньше подтверждения опасных действий и
// ввод пароля делались БЛОКИРУЮЩИМ циклом чтения с консоли БЕЗ отрисовки —
// экран замирал на меню, пользователь думал, что система зависла. Теперь:
//   * confirm(title)        — диалог с рамкой поверх меню, Enter/Y = да, Esc/N = нет;
//   * prompt_masked(title)  — ввод пароля с маской '*';
//   * prompt_line(title)    — ввод строки (номер пакета, размер и т.п.).
// Диалог перерисовывается при каждом введённом символе; после закрытия
// вызывающий код перерисовывает меню (render). Ввод — с PS/2 и COM1 (serial),
// с таймаутом (15 с для confirm, 30 с для ввода) — зависание исключено.
// no_std-совместимо: alloc (String), рендер через crate::renderer.


use alloc::format;
use alloc::string::{String, ToString};

const BG: u32 = 0x1E2430;
const BORDER: u32 = 0x3A4A5E;
const TEXT: u32 = 0xE8F0F8;
const ACCENT: u32 = 0xFFD24A;
const ERR: u32 = 0xFF6B6B;

/// Рисует диалоговое окно по центру экрана (поверх меню).
fn draw_dialog(title: &str, body: &str, hint: &str, accent: u32) {
    crate::renderer::with_renderer(|r| {
        let w = r.width();
        let h = r.height();
        let dw = 600u32;
        let dh = 140u32;
        let dx = ((w as i32 - dw as i32) / 2).max(8);
        let dy = ((h as i32 - dh as i32) / 2).max(8);
        // Затемнение фона + рамка.
        r.fill_rect(dx, dy, dw, dh, crate::renderer::Color::from_u32(BG));
        r.draw_rect(dx, dy, dw, dh, crate::renderer::Color::from_u32(BORDER));
        r.draw_hline(dx, dy, dw, crate::renderer::Color::from_u32(accent));
        // Заголовок.
        r.draw_text(dx + 16, dy + 16, title, crate::renderer::Color::from_u32(accent), None);
        // Тело (строка ввода).
        r.fill_rect(dx + 16, dy + 52, dw - 32, 26, crate::renderer::Color::from_u32(0x0F141C));
        r.draw_text(dx + 20, dy + 58, body, crate::renderer::Color::from_u32(TEXT), None);
        // Подсказка.
        r.draw_text(dx + 16, dy + 96, hint, crate::renderer::Color::from_u32(TEXT), None);
        r.present();
    });
}

/// Опрос ввода: COM1 (serial) или PS/2.
fn poll_key() -> Option<u8> {
    if crate::serial::is_data_ready() {
        return Some(crate::serial::read_byte());
    }
    crate::keyboard::try_read_char()
}

/// Диалог подтверждения. Enter/Y — да, Esc/N/Q — нет. Таймаут 15 с -> нет.
pub fn confirm(title: &str) -> bool {
    crate::println!("  [dialog] {}? (Enter=Yes, Esc=No)", title);
    draw_dialog(title, " ", "Enter = Yes | Esc = No", ACCENT);
    let start = crate::timer::uptime_ms();
    loop {
        if let Some(k) = poll_key() {
            match k {
                b'\n' | b'\r' | b'y' | b'Y' => return true,
                b'\x1b' | b'n' | b'N' | b'q' | b'Q' => return false,
                _ => {}
            }
        }
        if crate::timer::uptime_ms().saturating_sub(start) > 15_000 {
            return false;
        }
        unsafe { core::arch::asm!("nop"); }
    }
}

/// Диалог ввода строки (без маски). Esc -> пустая строка.
pub fn prompt_line(title: &str) -> String {
    let mut s = String::new();
    crate::println!("  [dialog] {} (Enter=OK, Esc=cancel)", title);
    let start = crate::timer::uptime_ms();
    loop {
        let body: String = if s.is_empty() { " ".to_string() } else { s.clone() };
        draw_dialog(title, &body, "Enter = OK | Esc = cancel | Backspace = delete", ACCENT);
        if let Some(k) = poll_key() {
            match k {
                b'\n' | b'\r' => return s,
                b'\x1b' => return String::new(),
                0x08 | 0x7F => {
                    s.pop();
                }
                b if b >= 0x20 && b < 0x7F => {
                    if s.len() < 128 {
                        s.push(b as char);
                    }
                }
                _ => {}
            }
        }
        if crate::timer::uptime_ms().saturating_sub(start) > 30_000 {
            return s;
        }
        unsafe { core::arch::asm!("nop"); }
    }
}

/// Диалог ввода пароля с маской '*'. Esc -> пустая строка.
pub fn prompt_masked(title: &str) -> String {
    let mut s = String::new();
    crate::println!("  [dialog] {}", title);
    let start = crate::timer::uptime_ms();
    loop {
        let masked: String = "*".repeat(s.len());
        let body: String = if masked.is_empty() {
            " ".to_string()
        } else {
            masked
        };
        draw_dialog(title, &body, "Enter = OK | Esc = cancel", ERR);
        if let Some(k) = poll_key() {
            match k {
                b'\n' | b'\r' => return s,
                b'\x1b' => return String::new(),
                0x08 | 0x7F => {
                    s.pop();
                }
                b if b >= 0x20 && b < 0x7F => {
                    if s.len() < 64 {
                        s.push(b as char);
                    }
                }
                _ => {}
            }
        }
        if crate::timer::uptime_ms().saturating_sub(start) > 30_000 {
            return s;
        }
        unsafe { core::arch::asm!("nop"); }
    }
}

/// Диалог выбора из списка (строки items): стрелки/цифры + Enter. Возвращает
/// индекс или None при Esc. Используется для выбора пакета/бэкапа.
pub fn choose(title: &str, items: &[String]) -> Option<usize> {
    let mut sel: usize = 0;
    crate::println!("  [dialog] {} (UP/DOWN, 1..9, Enter)", title);
    for (i, it) in items.iter().enumerate() {
        crate::println!("    {}. {}", i + 1, it);
    }
    let start = crate::timer::uptime_ms();
    loop {
        let mut body = String::new();
        for (i, it) in items.iter().enumerate().take(10) {
            if i == sel {
                body.push_str("> ");
            } else {
                body.push_str("  ");
            }
            body.push_str(&format!("{}. {}", i + 1, it));
            body.push('\n');
        }
        draw_dialog(title, body.trim_end(), "UP/DOWN or 1..9 + Enter | Esc = cancel", ACCENT);
        if let Some(k) = poll_key() {
            match k {
                b'\n' | b'\r' => return Some(sel),
                b'\x1b' => return None,
                crate::keyboard::ARROW_UP => {
                    sel = if sel == 0 { items.len().saturating_sub(1) } else { sel - 1 };
                }
                crate::keyboard::ARROW_DOWN => {
                    sel = (sel + 1) % items.len();
                }
                b'1'..=b'9' => {
                    let idx = (k - b'1') as usize;
                    if idx < items.len() {
                        sel = idx;
                    }
                }
                _ => {}
            }
        }
        if crate::timer::uptime_ms().saturating_sub(start) > 30_000 {
            return Some(sel);
        }
        unsafe { core::arch::asm!("nop"); }
    }
}
