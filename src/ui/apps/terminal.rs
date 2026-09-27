//! Отрисовка приложения DUIL Графический Терминал с поддержкой прозрачности,
//! акрилового размытия (Acrylic Blur), подсветки синтаксиса и настройки цветов.

use crate::renderer::{Color, Renderer};
use crate::spinlock::SpinLock;
use crate::ui::theme::UiTheme;
use crate::ui::window::Window;
use alloc::string::String;

#[derive(Clone, Copy, Debug)]
pub struct TerminalConfig {
    pub bg_color: Color,
    pub opacity: u8,
    pub blur_enabled: bool,
    pub valid_cmd_color: Color,
    pub invalid_cmd_color: Color,
    pub arg_color: Color,
    pub text_color: Color,
    pub prompt_color: Color,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            bg_color: Color::rgb(30, 30, 46),        // Catppuccin Mocha / Dark Glass
            opacity: 190,                              // ~75% Aero Translucency
            blur_enabled: true,                        // Acrylic Blur
            valid_cmd_color: Color::rgb(0, 255, 136),   // Neon Green (Valid command)
            invalid_cmd_color: Color::rgb(255, 85, 85), // Neon Red (Invalid command)
            arg_color: Color::rgb(136, 204, 255),     // Light Cyan (Arguments)
            text_color: Color::rgb(205, 214, 244),    // Off-white text
            prompt_color: Color::rgb(99, 102, 241),   // Indigo prompt
        }
    }
}

pub static TERMINAL_CONFIG: SpinLock<TerminalConfig> = SpinLock::new(TerminalConfig {
    bg_color: Color::rgb(30, 30, 46),
    opacity: 190,
    blur_enabled: true,
    valid_cmd_color: Color::rgb(0, 255, 136),
    invalid_cmd_color: Color::rgb(255, 85, 85),
    arg_color: Color::rgb(136, 204, 255),
    text_color: Color::rgb(205, 214, 244),
    prompt_color: Color::rgb(99, 102, 241),
});

pub fn parse_color_hex(val: &str) -> Option<Color> {
    let clean = val.trim().trim_start_matches("0x").trim_start_matches('#');
    if let Ok(num) = u32::from_str_radix(clean, 16) {
        Some(Color::from_u32(num))
    } else {
        None
    }
}

pub fn cmd_terminal_color(arg: &str) {
    let mut parts = arg.trim().split_whitespace();
    let sub = parts.next().unwrap_or("");
    let val = parts.next().unwrap_or("");

    let mut cfg = TERMINAL_CONFIG.lock();

    match sub {
        "opacity" | "transparency" | "alpha" => {
            if let Ok(num) = val.parse::<u8>() {
                cfg.opacity = num;
                crate::println!("  [DUIL Terminal] Прозрачность изменена: {}", num);
            } else {
                crate::println!("  Использование: color opacity <0-255>");
            }
        }
        "blur" => {
            match val {
                "on" | "true" | "1" => {
                    cfg.blur_enabled = true;
                    crate::println!("  [DUIL Terminal] Размытие фона (Acrylic Blur): ВКЛ");
                }
                "off" | "false" | "0" => {
                    cfg.blur_enabled = false;
                    crate::println!("  [DUIL Terminal] Размытие фона (Acrylic Blur): ВЫКЛ");
                }
                _ => crate::println!("  Использование: color blur <on|off>"),
            }
        }
        "bg" | "background" => {
            if let Some(c) = parse_color_hex(val) {
                cfg.bg_color = c;
                crate::println!("  [DUIL Terminal] Цвет фона изменён: {:#08x}", c.0);
            } else {
                crate::println!("  Использование: color bg <0xRRGGBB>");
            }
        }
        "valid" => {
            if let Some(c) = parse_color_hex(val) {
                cfg.valid_cmd_color = c;
                crate::println!("  [DUIL Terminal] Цвет валидной команды изменён: {:#08x}", c.0);
            } else {
                crate::println!("  Использование: color valid <0xRRGGBB>");
            }
        }
        "invalid" => {
            if let Some(c) = parse_color_hex(val) {
                cfg.invalid_cmd_color = c;
                crate::println!("  [DUIL Terminal] Цвет неверной команды изменён: {:#08x}", c.0);
            } else {
                crate::println!("  Использование: color invalid <0xRRGGBB>");
            }
        }
        "text" => {
            if let Some(c) = parse_color_hex(val) {
                cfg.text_color = c;
                crate::println!("  [DUIL Terminal] Цвет текста изменён: {:#08x}", c.0);
            } else {
                crate::println!("  Использование: color text <0xRRGGBB>");
            }
        }
        "preset" => {
            match val {
                "aero" | "glass" => {
                    cfg.bg_color = Color::rgb(15, 23, 42);
                    cfg.opacity = 160;
                    cfg.blur_enabled = true;
                    cfg.valid_cmd_color = Color::rgb(52, 211, 153);
                    cfg.invalid_cmd_color = Color::rgb(248, 113, 113);
                    cfg.arg_color = Color::rgb(125, 211, 252);
                    cfg.text_color = Color::WHITE;
                    crate::println!("  [DUIL Terminal] Применён пресет: Aero Glass");
                }
                "cyberpunk" | "cyber" => {
                    cfg.bg_color = Color::rgb(13, 2, 33);
                    cfg.opacity = 220;
                    cfg.blur_enabled = true;
                    cfg.valid_cmd_color = Color::rgb(0, 255, 204);
                    cfg.invalid_cmd_color = Color::rgb(255, 0, 128);
                    cfg.arg_color = Color::rgb(255, 230, 0);
                    cfg.text_color = Color::rgb(240, 240, 255);
                    crate::println!("  [DUIL Terminal] Применён пресет: Cyberpunk Neon");
                }
                "catppuccin" | "dark" => {
                    *cfg = TerminalConfig::default();
                    crate::println!("  [DUIL Terminal] Применён пресет: Catppuccin Mocha");
                }
                _ => crate::println!("  Доступные пресеты: aero, cyberpunk, catppuccin"),
            }
        }
        "reset" => {
            *cfg = TerminalConfig::default();
            crate::println!("  [DUIL Terminal] Сброс настроек цветов и прозрачности по умолчанию.");
        }
        _ => {
            crate::println!("  Настройки терминала DUIL:");
            crate::println!("    color opacity <0-255>   - прозрачность окна");
            crate::println!("    color blur <on|off>     - акриловое размытие фона");
            crate::println!("    color bg <0xHEX>        - цвет фона");
            crate::println!("    color valid <0xHEX>     - цвет верной команды");
            crate::println!("    color invalid <0xHEX>   - цвет неверной команды");
            crate::println!("    color preset <aero|cyber|dark>");
            crate::println!("    color reset             - сброс по умолчанию");
        }
    }
}

pub fn draw_terminal(
    r: &mut Renderer,
    _theme: &UiTheme,
    w: &Window,
    lines: &[String],
    current_line: &str,
    content_y: i32,
) {
    let cfg = *TERMINAL_CONFIG.lock();

    // 1. Акриловое размытие фона под окном терминала (Acrylic Blur)
    if cfg.blur_enabled {
        r.apply_blur_rect(w.x, content_y, w.width, w.height, 2);
    }

    // 2. Полупрозрачный стеклянный фон терминала (Aero Translucency)
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, cfg.bg_color, cfg.opacity);

    let max_visible_lines = (w.height as usize - 24) / 16;
    let start_idx = lines.len().saturating_sub(max_visible_lines);

    let mut ty = content_y + 8;
    for line in lines.iter().skip(start_idx) {
        r.draw_text(w.x + 8, ty, line, cfg.text_color, None);
        ty += 16;
    }

    // 3. Строка ввода с подсветкой синтаксиса DUIL (Зелёный = Валидная команда, Красный = Неизвестная)
    let prompt_prefix = "deix [DUIL]> ";
    r.draw_text(w.x + 8, ty, prompt_prefix, cfg.prompt_color, None);
    let mut cx = w.x + 8 + (prompt_prefix.len() as i32 * 8);

    let trimmed = current_line.trim_start();
    let leading_spaces = current_line.len() - trimmed.len();
    if leading_spaces > 0 {
        cx += leading_spaces as i32 * 8;
    }

    let first_word = trimmed.split_whitespace().next().unwrap_or("");
    if first_word.is_empty() {
        r.draw_text(cx, ty, current_line, cfg.text_color, None);
        cx += current_line.len() as i32 * 8;
    } else {
        let is_valid = crate::cli::is_valid_command(first_word);
        let cmd_color = if is_valid { cfg.valid_cmd_color } else { cfg.invalid_cmd_color };
        r.draw_text(cx, ty, first_word, cmd_color, None);
        cx += first_word.len() as i32 * 8;

        if trimmed.len() > first_word.len() {
            let rest = &trimmed[first_word.len()..];
            r.draw_text(cx, ty, rest, cfg.arg_color, None);
            cx += rest.len() as i32 * 8;
        }
    }

    // Курсор ввода
    r.draw_text(cx, ty, "_", cfg.valid_cmd_color, None);
}
