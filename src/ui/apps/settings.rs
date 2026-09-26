//! Отрисовка настроек персонализации и экрана

use crate::renderer::{Color, Renderer};
use crate::ui::window::{RESOLUTION_PRESETS, Window};
use crate::ui::theme::{UiTheme, ThemePreset};
use alloc::format;

pub fn draw_theme_settings(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    volume: u8,
    brightness: u8,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    r.draw_text(w.x + 12, content_y + 8, "DESKTOP THEMES & COLOR PALETTE", theme.accent, None);

    let themes = [
        ("Catppuccin", ThemePreset::DarkCatppuccin),
        ("Nord Light", ThemePreset::NordLight),
        ("Cyberpunk", ThemePreset::CyberpunkNeon),
        ("Aero Glass", ThemePreset::AeroGlass),
        ("Emerald", ThemePreset::EmeraldForest),
        ("Sunset Gold", ThemePreset::SunsetGold),
    ];

    let mut sx = w.x + 12;
    let mut sy = content_y + 32;

    for (i, (name, preset)) in themes.iter().enumerate() {
        if i == 3 {
            sx = w.x + 12;
            sy += 64;
        }

        let is_current = theme.preset == *preset;
        let bg = if is_current { theme.accent } else { theme.titlebar_inactive };
        r.fill_rounded_rect(sx, sy, 98, 56, 6, bg);
        let fg = if is_current { Color::WHITE } else { theme.text_primary };
        r.draw_text(sx + 8, sy + 20, name, fg, None);
        sx += 102;
    }

    r.draw_text(w.x + 12, content_y + 168, "LIVE THEME PREVIEW:", theme.accent, None);
    r.fill_rounded_rect(w.x + 12, content_y + 186, w.width - 24, 60, 6, theme.titlebar_active);
    r.draw_text(w.x + 24, content_y + 198, "Window Title Bar Preview", theme.text_primary, None);
    r.draw_text(w.x + 24, content_y + 218, "Body Text & Accent Line Preview", theme.accent, None);

    let vol_str = format!("Volume: {}%", volume);
    r.draw_text(w.x + 12, content_y + 256, &vol_str, theme.text_primary, None);

    let bright_str = format!("Brightness: {}%", brightness);
    r.draw_text(w.x + 240, content_y + 256, &bright_str, theme.text_primary, None);
}

pub fn draw_display_settings(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    r.draw_text(w.x + 12, content_y + 8, "DISPLAY RESOLUTION", theme.accent, None);

    let cur_res = (r.width(), r.height());
    let mut ry = content_y + 32;
    for (width, height) in RESOLUTION_PRESETS.iter() {
        let is_current = cur_res == (*width, *height);
        let bg = if is_current { theme.accent } else { theme.titlebar_inactive };
        let fg = if is_current { Color::WHITE } else { theme.text_primary };

        r.fill_rounded_rect(w.x + 12, ry, w.width - 24, 24, 4, bg);
        let res_str = format!("{} x {}", width, height);
        r.draw_text(w.x + 24, ry + 4, &res_str, fg, None);

        if is_current {
            r.draw_text(w.x + w.width as i32 - 70, ry + 4, "[Active]", Color::GREEN, None);
        }

        ry += 28;
    }
}
