//! Отрисовка настроек персонализации и экрана.
//!
//! Геометрия каждого контрола задана ОДИН раз (`theme_card_rect`,
//! `accent_rect`, `wallpaper_rect`) в координатах относительно области
//! содержимого окна. И отрисовка здесь, и обработка кликов в
//! `desktop.rs` используют эти же прямоугольники — расхождение
//! видимой карточки и её hitbox исключено по построению.

use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::ui::theme::{ThemePreset, UiTheme, WALLPAPER_STYLES};
use crate::ui::window::{RESOLUTION_PRESETS, Window};
use alloc::format;

/// Карточки тем: подпись + пресет. Порядок = порядок отрисовки.
pub const THEME_CHOICES: [(&str, ThemePreset); 6] = [
    ("Catppuccin", ThemePreset::DarkCatppuccin),
    ("Nord Light", ThemePreset::NordLight),
    ("Cyberpunk", ThemePreset::CyberpunkNeon),
    ("Aero Glass", ThemePreset::AeroGlass),
    ("Emerald", ThemePreset::EmeraldForest),
    ("Sunset Gold", ThemePreset::SunsetGold),
];

/// Акцентные цвета (RGB).
pub const ACCENT_CHOICES: [(u8, u8, u8); 6] = [
    (99, 102, 241),  // indigo
    (6, 182, 212),   // cyan
    (16, 185, 129),  // emerald
    (236, 72, 153),  // pink
    (245, 158, 11),  // amber
    (168, 85, 247),  // purple
];

/// Прямоугольник карточки темы `i` (относительно левого края окна и
/// верха области содержимого). 3 карточки в ряд, 2 ряда.
pub fn theme_card_rect(i: usize) -> Rect {
    let col = (i % 3) as i32;
    let row = (i / 3) as i32;
    Rect::new(12 + col * 118, 32 + row * 64, 110, 56)
}

/// Прямоугольник образца акцентного цвета `i`.
pub fn accent_rect(i: usize) -> Rect {
    Rect::new(12 + i as i32 * 34, 180, 26, 26)
}

/// Прямоугольник кнопки стиля обоев `i`.
pub fn wallpaper_rect(i: usize) -> Rect {
    Rect::new(12 + i as i32 * 102, 232, 96, 26)
}

/// Тема по пресету — для построения карточек-превью.
pub fn theme_for_preset(preset: ThemePreset) -> UiTheme {
    match preset {
        ThemePreset::DarkCatppuccin => UiTheme::catppuccin(),
        ThemePreset::NordLight => UiTheme::nord_light(),
        ThemePreset::CyberpunkNeon => UiTheme::cyberpunk(),
        ThemePreset::AeroGlass => UiTheme::aero_glass(),
        ThemePreset::EmeraldForest => UiTheme::emerald_forest(),
        ThemePreset::SunsetGold => UiTheme::sunset_gold(),
    }
}

pub fn draw_theme_settings(r: &mut Renderer, theme: &UiTheme, w: &Window, content_y: i32) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    r.draw_icon(w.x + 12, content_y + 8, IconType::Settings, theme.accent);
    r.draw_text(w.x + 32, content_y + 8, "PERSONALIZATION", theme.accent, None);

    // Карточки тем — мини-превью из настоящих цветов пресета.
    for (i, (name, preset)) in THEME_CHOICES.iter().enumerate() {
        let rect = theme_card_rect(i);
        let (cx, cy) = (w.x + rect.x, content_y + rect.y);
        let preview = theme_for_preset(*preset);
        let is_current = theme.preset == *preset;

        r.fill_rounded_rect(cx, cy, rect.w, rect.h, 6, preview.bg_top);
        r.fill_rect(cx + 6, cy + 24, rect.w - 12, 10, preview.window_bg);
        r.fill_rect(cx + 6, cy + 38, (rect.w - 12) / 2, 6, preview.accent);
        r.draw_text(cx + 6, cy + 6, name, preview.text_primary, None);

        if is_current {
            r.draw_rect(cx, cy, rect.w, rect.h, theme.accent);
            r.draw_rect(cx + 1, cy + 1, rect.w - 2, rect.h - 2, theme.accent);
            r.draw_icon(cx + rect.w as i32 - 20, cy + rect.h as i32 - 20, IconType::Check, theme.accent);
        }
    }

    // Акцентный цвет.
    r.draw_text(w.x + 12, content_y + 162, "ACCENT COLOR", theme.text_secondary, None);
    for (i, (cr, cg, cb)) in ACCENT_CHOICES.iter().enumerate() {
        let rect = accent_rect(i);
        let (cx, cy) = (w.x + rect.x, content_y + rect.y);
        let color = Color::rgb(*cr, *cg, *cb);
        r.fill_rounded_rect(cx, cy, rect.w, rect.h, 6, color);
        if theme.accent == color {
            r.draw_rect(cx - 2, cy - 2, rect.w + 4, rect.h + 4, theme.text_primary);
        }
    }

    // Стиль обоев.
    r.draw_text(w.x + 12, content_y + 214, "WALLPAPER", theme.text_secondary, None);
    for (i, name) in WALLPAPER_STYLES.iter().enumerate() {
        let rect = wallpaper_rect(i);
        let (cx, cy) = (w.x + rect.x, content_y + rect.y);
        let is_current = theme.wallpaper_style == i as u8;
        let bg = if is_current { theme.accent } else { theme.surface_alt };
        let fg = if is_current { Color::WHITE } else { theme.text_primary };
        r.fill_rounded_rect(cx, cy, rect.w, rect.h, 4, bg);
        r.draw_text(cx + 6, cy + 6, name, fg, None);
    }

    // Честный статус звука: регулятора громкости в этом релизе нет,
    // поэтому ползунок не рисуется — только реальный backend.
    let audio = if crate::hda::is_ready() {
        "Sound: Intel HDA 48 kHz (no volume control yet)"
    } else {
        "Sound: PC speaker (no volume control)"
    };
    r.draw_text(w.x + 12, content_y + 274, audio, theme.text_secondary, None);
    r.draw_text(
        w.x + 12,
        content_y + 294,
        "Display resolution: Start menu > Display Settings",
        theme.text_secondary,
        None,
    );
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
