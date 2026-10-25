//! Отрисовка настроек персонализации и экрана.
//!
//! Геометрия каждого контрола задана ОДИН раз (`theme_card_rect`,
//! `accent_rect`, `wallpaper_rect`) в координатах относительно области
//! содержимого окна. И отрисовка здесь, и обработка кликов в
//! `desktop.rs` используют эти же прямоугольники — расхождение
//! видимой карточки и её hitbox исключено по построению.

use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::ui::effects::PerfProfile;
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

/// Свотч-превью стиля обоев `i`: 6 в ряд, 2 ряда (до 12 стилей).
pub fn wallpaper_rect(i: usize) -> Rect {
    Rect::new(
        12 + (i % 6) as i32 * 76,
        232 + (i / 6) as i32 * 46,
        72,
        40,
    )
}

/// Профили производительности. Порядок = порядок отрисовки.
pub const PROFILE_CHOICES: [(&str, PerfProfile); 3] = [
    ("Quality", PerfProfile::Quality),
    ("Balanced", PerfProfile::Balanced),
    ("Performance", PerfProfile::Performance),
];

/// Прямоугольник кнопки профиля производительности `i`.
pub fn profile_rect(i: usize) -> Rect {
    Rect::new(12 + i as i32 * 152, 346, 144, 26)
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

    // Стиль обоев — живые миниатюры: каждый свотч рисуется теми же
    // опорными цветами, что и настоящий генератор обоев этого стиля.
    r.draw_text(w.x + 12, content_y + 214, "WALLPAPER", theme.text_secondary, None);
    for (i, name) in WALLPAPER_STYLES.iter().enumerate() {
        let rect = wallpaper_rect(i);
        let (cx, cy) = (w.x + rect.x, content_y + rect.y);
        draw_wallpaper_swatch(r, theme, i, cx, cy, rect.w, rect.h);
        r.draw_text(cx + 7, cy + rect.h as i32 - 19, name, Color::BLACK, None);
        r.draw_text(cx + 6, cy + rect.h as i32 - 20, name, Color::WHITE, None);
        if theme.wallpaper_style == i as u8 {
            r.draw_rounded_rect_outline(cx, cy, rect.w, rect.h, 8, theme.accent, 235);
            r.draw_rounded_rect_outline(cx - 1, cy - 1, rect.w + 2, rect.h + 2, 9, theme.accent, 80);
        }
    }

    // Профиль производительности: каждый пункт реально меняет путь
    // отрисовки (blur/тени/прозрачность — см. ui::effects).
    r.draw_text(w.x + 12, content_y + 328, "EFFECTS", theme.text_secondary, None);
    let current_profile = crate::ui::effects::current();
    for (i, (name, profile)) in PROFILE_CHOICES.iter().enumerate() {
        let rect = profile_rect(i);
        let (cx, cy) = (w.x + rect.x, content_y + rect.y);
        let is_current = current_profile == *profile;
        let bg = if is_current { theme.accent } else { theme.surface_alt };
        let fg = if is_current { Color::WHITE } else { theme.text_primary };
        r.fill_rounded_rect(cx, cy, rect.w, rect.h, 4, bg);
        r.draw_text(cx + 8, cy + 6, name, fg, None);
    }
    let hint = match current_profile {
        PerfProfile::Quality => "Blur, shadows and translucency enabled.",
        PerfProfile::Balanced => "Blur disabled; shadows and translucency kept.",
        PerfProfile::Performance => "Blur and shadows off, surfaces opaque.",
    };
    r.draw_text(w.x + 12, content_y + 380, hint, theme.text_secondary, None);

    r.draw_text(
        w.x + 12,
        content_y + 398,
        "Custom wallpaper: /userdata/wallpaper.bmp (24/32-bit, <= 268 KB)",
        theme.text_secondary,
        None,
    );

    // Честный статус звука: регулятора громкости в этом релизе нет,
    // поэтому ползунок не рисуется — только реальный backend.
    let audio = if crate::hda::is_ready() {
        "Sound: Intel HDA 48 kHz (no volume control yet)"
    } else {
        "Sound: PC speaker (no volume control)"
    };
    r.draw_text(w.x + 12, content_y + 416, audio, theme.text_secondary, None);
    r.draw_text(
        w.x + 12,
        content_y + 434,
        "Display resolution: Start menu > Display Settings",
        theme.text_secondary,
        None,
    );
}

/// Миниатюра стиля обоев: та же палитра и мотив, что у настоящего
/// генератора (desktop::generate_wallpaper), в масштабе кнопки 72x40.
fn draw_wallpaper_swatch(
    r: &mut Renderer,
    theme: &UiTheme,
    style: usize,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
) {
    let (wi, hi) = (w as i32, h as i32);
    match style {
        // Midnight: тёмное небо, луна, звёзды.
        1 => {
            r.fill_rounded_rect(x, y, w, h, 8, theme.bg_top.lerp(Color::BLACK, 120));
            r.fill_circle(x + wi - 16, y + 11, 5, Color::rgb(226, 232, 240));
            for (sx, sy) in [(10, 8), (28, 18), (46, 7), (60, 24)] {
                r.put_pixel(x + sx, y + sy, theme.text_primary);
            }
        }
        // Plasma: встречный градиент и круги.
        2 => {
            r.fill_rounded_rect(x, y, w, h, 8, theme.bg_bottom);
            r.fill_circle(x + 22, y + 12, 9, theme.bg_top.lerp(theme.accent, 90));
            r.fill_circle(x + wi - 22, y + hi - 12, 10, theme.bg_top.lerp(theme.accent_hover, 70));
        }
        // Minimal: светлая поверхность с акцентной полосой.
        3 => {
            r.fill_rounded_rect(x, y, w, h, 8, theme.window_bg.lerp(Color::WHITE, 120));
            r.fill_rect(x + 2, y + hi * 2 / 3, w - 4, 3, theme.window_bg.lerp(theme.accent, 60));
        }
        // Custom: пользовательский BMP.
        4 => {
            r.fill_rounded_rect(x, y, w, h, 8, theme.surface_alt);
            r.draw_icon(x + wi / 2 - 8, y + hi / 2 - 8, IconType::Files, theme.accent);
        }
        // Nebula: тёмный космос с фиолетово-голубым кольцом газа.
        5 => {
            let space = Color::rgb(14, 16, 44);
            r.fill_rounded_rect(x, y, w, h, 8, space);
            r.fill_circle(x + wi / 2, y + hi / 2, 15, Color::rgb(96, 70, 190));
            r.fill_circle(x + wi / 2 + 3, y + hi / 2 + 2, 13, Color::rgb(52, 150, 190));
            r.fill_circle(x + wi / 2, y + hi / 2, 10, space);
            for (sx, sy) in [(8, 8), (20, 30), (60, 10), (64, 28)] {
                r.put_pixel(x + sx, y + sy, Color::WHITE);
            }
        }
        // Horizon: сумеречное небо и отражение в воде.
        6 => {
            r.fill_rounded_rect(x, y, w, h, 8, Color::rgb(52, 44, 104));
            r.fill_rect(x + 3, y + 17, w - 6, 5, Color::rgb(214, 126, 118));
            r.fill_rect(x + 3, y + 22, w - 6, 2, Color::rgb(250, 170, 110));
            r.fill_rect(x + 8, y + 26, w - 16, 5, Color::rgb(120, 76, 118));
            r.fill_rect(x + 12, y + 31, w - 24, 4, Color::rgb(34, 30, 72));
        }
        // Crystal: стеклянные грани на тёмно-синем фоне.
        7 => {
            r.fill_rounded_rect(x, y, w, h, 8, Color::rgb(8, 16, 34));
            r.fill_circle(x + wi - 20, y + 15, 8, Color::rgb(46, 150, 160));
            r.fill_circle(x + wi - 20, y + 15, 4, Color::rgb(140, 230, 230));
            r.fill_circle(x + 18, y + 26, 5, Color::rgb(60, 170, 180));
            r.draw_line(x + 10, y + 33, x + 38, y + 12, Color::rgb(120, 210, 220));
            r.draw_line(x + 11, y + 33, x + 39, y + 12, Color::rgb(70, 160, 180));
        }
        // Forest: туманная ель на рассвете.
        8 => {
            r.fill_rounded_rect(x, y, w, h, 8, Color::rgb(14, 34, 44));
            r.fill_circle(x + wi / 2, y + 20, 12, Color::rgb(70, 92, 96));
            r.fill_circle(x + wi / 2, y + 20, 6, Color::rgb(226, 170, 112));
            let pine = Color::rgb(8, 22, 26);
            for (tx, top) in [(12, 14), (26, 8), (46, 10), (60, 15)] {
                let (ax, ay) = (x + tx, y + top);
                let base = y + hi - 3;
                r.draw_line(ax, ay, ax - 6, base, pine);
                r.draw_line(ax, ay, ax + 6, base, pine);
                r.draw_line(ax + 1, ay, ax - 5, base, pine);
                r.draw_line(ax - 1, ay, ax + 5, base, pine);
                r.draw_hline(ax - 6, base, 13, pine);
                r.draw_vline(ax, ay, (base - ay) as u32, pine);
            }
        }
        // Dunes: закат над барханами.
        9 => {
            r.fill_rounded_rect(x, y, w, h, 8, Color::rgb(176, 70, 112));
            r.fill_circle(x + wi / 2, y + 22, 9, Color::rgb(255, 214, 128));
            r.fill_rect(x + 3, y + 24, w - 6, 12, Color::rgb(222, 116, 74));
            r.fill_rect(x + 7, y + 36, w - 14, 2, Color::rgb(150, 66, 90));
            r.draw_line(x + 3, y + 33, x + 28, y + 25, Color::rgb(255, 170, 110));
            r.draw_line(x + 28, y + 25, x + wi - 4, y + 32, Color::rgb(150, 66, 90));
        }
        // Neon: ночной город и отражение.
        10 => {
            r.fill_rounded_rect(x, y, w, h, 8, Color::rgb(10, 12, 34));
            let body = Color::rgb(30, 24, 74);
            let tops = [
                Color::rgb(236, 72, 200),
                Color::rgb(60, 220, 240),
                Color::rgb(150, 100, 255),
            ];
            for (i, (bx, bh)) in [(8, 12), (16, 20), (24, 9), (32, 24), (40, 15), (48, 21), (56, 11)]
                .iter()
                .enumerate()
            {
                r.fill_rect(x + bx, y + 28 - bh, 6, *bh as u32, body);
                r.draw_hline(x + bx, y + 28 - bh, 6, tops[i % 3]);
            }
            r.draw_hline(x + 4, y + 28, w - 8, Color::rgb(60, 220, 240));
            for (i, (bx, bh)) in [(16, 6), (32, 8), (48, 7)].iter().enumerate() {
                r.fill_rect(x + bx, y + 30, 6, *bh as u32, tops[(i + 1) % 3].lerp(Color::BLACK, 150));
            }
        }
        // Aurora: графит и две ленты сияния.
        _ => {
            r.fill_rounded_rect(x, y, w, h, 8, theme.bg_top);
            let violet = Color::rgb(138, 123, 240);
            r.draw_line(x + 12, y + hi - 4, x + 34, y + 4, theme.bg_top.lerp(theme.accent, 120));
            r.draw_line(x + 13, y + hi - 4, x + 35, y + 4, theme.bg_top.lerp(theme.accent, 90));
            r.draw_line(x + 44, y + 3, x + 64, y + hi - 5, theme.bg_top.lerp(violet, 110));
            r.draw_line(x + 45, y + 3, x + 65, y + hi - 5, theme.bg_top.lerp(violet, 80));
            for (sx, sy) in [(38, 8), (66, 12), (8, 6)] {
                r.put_pixel(x + sx, y + sy, theme.text_primary);
            }
        }
    }
}

pub fn draw_display_settings(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    content_y: i32,
    status: Option<&str>,
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
        r.draw_icon(w.x + 20, ry + 4, IconType::Display, fg);
        r.draw_text(w.x + 44, ry + 4, &res_str, fg, None);

        if is_current {
            r.draw_text(w.x + w.width as i32 - 70, ry + 4, "[Active]", Color::GREEN, None);
        }

        ry += 28;
    }

    // Причина отказа последней попытки (нехватка памяти ядра) — реальное
    // сообщение из обработчика клика, а не декорация.
    if let Some(msg) = status {
        let max_cells = ((w.width as i32 - 24) / 8).max(8) as usize;
        r.draw_text(
            w.x + 12,
            ry + 6,
            crate::ui::text::truncate_to_cells(msg, max_cells).0,
            Color::RED,
            None,
        );
    }
}
