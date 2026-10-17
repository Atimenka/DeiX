//! Сохранение настроек рабочего стола в `/userdata/config/desktop.cfg`.
//!
//! Формат — строки `ключ=значение` (ASCII). Файл читается один раз при
//! старте графической сессии и перезаписывается при каждом изменении
//! настроек кликом (не чаще). Отсутствующий, пустой или повреждённый
//! файл — штатная ситуация: применяются значения по умолчанию, ошибок
//! не создаётся. Запись идёт только на /userdata; /system (EROFS,
//! read-only) не затрагивается.

use alloc::format;

use crate::ext2;
use crate::ui::effects::{self, PerfProfile};
use crate::ui::theme::{self, ThemePreset};

const CONFIG_DIR: &str = "/config";
const CONFIG_PATH: &str = "/config/desktop.cfg";

fn preset_token(p: ThemePreset) -> &'static str {
    match p {
        ThemePreset::DarkCatppuccin => "catppuccin",
        ThemePreset::NordLight => "nord_light",
        ThemePreset::CyberpunkNeon => "cyberpunk",
        ThemePreset::AeroGlass => "aero_glass",
        ThemePreset::EmeraldForest => "emerald",
        ThemePreset::SunsetGold => "sunset_gold",
    }
}

fn preset_from_token(s: &str) -> Option<ThemePreset> {
    match s {
        "catppuccin" => Some(ThemePreset::DarkCatppuccin),
        "nord_light" => Some(ThemePreset::NordLight),
        "cyberpunk" => Some(ThemePreset::CyberpunkNeon),
        "aero_glass" => Some(ThemePreset::AeroGlass),
        "emerald" => Some(ThemePreset::EmeraldForest),
        "sunset_gold" => Some(ThemePreset::SunsetGold),
        _ => None,
    }
}

/// Сохраняет текущие настройки. Вызывается из обработчиков кликов
/// страницы Personalization — не из цикла отрисовки.
pub fn save() {
    if !ext2::is_formatted() {
        return; // /userdata недоступен — сохранять некуда, это не ошибка
    }
    let t = theme::get_theme();
    // Color хранит 0x00RRGGBB одним словом.
    let (ar, ag, ab) = (
        (t.accent.0 >> 16) & 0xFF,
        (t.accent.0 >> 8) & 0xFF,
        t.accent.0 & 0xFF,
    );
    let data = format!(
        "theme={}\naccent={},{},{}\nwallpaper={}\nprofile={}\n",
        preset_token(t.preset),
        ar,
        ag,
        ab,
        t.wallpaper_style,
        effects::current().token(),
    );
    // Каталог может уже существовать — это не ошибка записи.
    let _ = ext2::mkdir_p(CONFIG_DIR);
    if let Err(e) = ext2::write_file_path(CONFIG_PATH, data.as_bytes()) {
        crate::diag::warn(
            crate::diag::ErrorCode::new(crate::diag::Subsystem::Vfs, 11),
            &format!("настройки, запись {}: {:?}", CONFIG_PATH, e),
        );
    }
}

/// Загружает настройки при старте графической сессии (один раз за
/// загрузку: повторные входы в GUI, в т.ч. после смены разрешения,
/// не затирают изменения пользователя значениями из файла).
pub fn load_once() {
    static mut LOADED: bool = false;
    unsafe {
        if LOADED {
            return;
        }
        LOADED = true;
    }
    if !ext2::is_formatted() {
        return;
    }
    let Ok(data) = ext2::read_file_path(CONFIG_PATH) else {
        return; // файла нет — значения по умолчанию
    };
    let Ok(text) = core::str::from_utf8(&data) else {
        return; // повреждённый файл — игнорируется целиком
    };

    let mut t = theme::get_theme();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "theme" => {
                if let Some(p) = preset_from_token(value.trim()) {
                    // Пресет задаёт базу; accent/wallpaper из файла
                    // применяются поверх ниже по списку.
                    t = crate::ui::apps::settings::theme_for_preset(p);
                }
            }
            "accent" => {
                if let Some(rgb) = parse_rgb(value.trim()) {
                    t.accent = crate::renderer::Color::rgb(rgb.0, rgb.1, rgb.2);
                }
            }
            "wallpaper" => {
                if let Ok(i) = value.trim().parse::<u8>() {
                    if (i as usize) < theme::WALLPAPER_STYLES.len() {
                        t.wallpaper_style = i;
                    }
                }
            }
            "profile" => {
                if let Some(p) = PerfProfile::from_token(value.trim()) {
                    effects::set(p);
                }
            }
            _ => {}
        }
    }
    theme::set_theme(t);
}

fn parse_rgb(s: &str) -> Option<(u8, u8, u8)> {
    let mut it = s.split(',');
    let r = it.next()?.trim().parse::<u8>().ok()?;
    let g = it.next()?.trim().parse::<u8>().ok()?;
    let b = it.next()?.trim().parse::<u8>().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((r, g, b))
}
