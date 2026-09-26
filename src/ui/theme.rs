//! Система тем и палитры цветов DeiX Fluent

use crate::renderer::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePreset {
    DarkCatppuccin,
    NordLight,
    CyberpunkNeon,
    AeroGlass,
    EmeraldForest,
    SunsetGold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiTheme {
    pub preset: ThemePreset,
    pub name: &'static str,
    pub bg_top: Color,
    pub bg_bottom: Color,
    pub window_bg: Color,
    pub titlebar_active: Color,
    pub titlebar_inactive: Color,
    pub accent: Color,
    pub accent_hover: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub surface: Color,
    pub surface_alt: Color,
    pub opacity: u8,
    pub corner_radius: i32,
    pub wallpaper_style: u8,
    pub enable_blur: bool,
}

impl UiTheme {
    pub fn catppuccin() -> Self {
        UiTheme {
            preset: ThemePreset::DarkCatppuccin,
            name: "Catppuccin",
            bg_top: Color::rgb(15, 23, 42),
            bg_bottom: Color::rgb(30, 41, 59),
            window_bg: Color::rgb(24, 24, 37),
            titlebar_active: Color::rgb(30, 41, 59),
            titlebar_inactive: Color::rgb(15, 23, 42),
            accent: Color::rgb(99, 102, 241),
            accent_hover: Color::rgb(129, 140, 248),
            text_primary: Color::rgb(205, 214, 244),
            text_secondary: Color::rgb(148, 163, 184),
            surface: Color::rgb(30, 30, 46),
            surface_alt: Color::rgb(40, 40, 60),
            opacity: 235,
            corner_radius: 10,
            wallpaper_style: 0,
            enable_blur: true,
        }
    }

    pub fn nord_light() -> Self {
        UiTheme {
            preset: ThemePreset::NordLight,
            name: "Nord Light",
            bg_top: Color::rgb(229, 233, 240),
            bg_bottom: Color::rgb(216, 222, 233),
            window_bg: Color::rgb(242, 244, 248),
            titlebar_active: Color::rgb(216, 222, 233),
            titlebar_inactive: Color::rgb(229, 233, 240),
            accent: Color::rgb(94, 129, 172),
            accent_hover: Color::rgb(129, 161, 193),
            text_primary: Color::rgb(46, 52, 64),
            text_secondary: Color::rgb(76, 86, 106),
            surface: Color::rgb(229, 233, 240),
            surface_alt: Color::rgb(216, 222, 233),
            opacity: 245,
            corner_radius: 8,
            wallpaper_style: 1,
            enable_blur: false,
        }
    }

    pub fn cyberpunk() -> Self {
        UiTheme {
            preset: ThemePreset::CyberpunkNeon,
            name: "Cyberpunk",
            bg_top: Color::rgb(10, 5, 20),
            bg_bottom: Color::rgb(25, 10, 40),
            window_bg: Color::rgb(18, 12, 28),
            titlebar_active: Color::rgb(40, 15, 60),
            titlebar_inactive: Color::rgb(18, 12, 28),
            accent: Color::rgb(236, 72, 153),
            accent_hover: Color::rgb(244, 114, 182),
            text_primary: Color::rgb(244, 244, 245),
            text_secondary: Color::rgb(161, 161, 170),
            surface: Color::rgb(28, 18, 42),
            surface_alt: Color::rgb(42, 24, 64),
            opacity: 220,
            corner_radius: 0,
            wallpaper_style: 2,
            enable_blur: true,
        }
    }

    pub fn aero_glass() -> Self {
        UiTheme {
            preset: ThemePreset::AeroGlass,
            name: "Aero Glass",
            bg_top: Color::rgb(15, 30, 60),
            bg_bottom: Color::rgb(5, 15, 35),
            window_bg: Color::rgb(20, 30, 50),
            titlebar_active: Color::rgb(40, 70, 110),
            titlebar_inactive: Color::rgb(20, 30, 50),
            accent: Color::rgb(6, 182, 212),
            accent_hover: Color::rgb(34, 211, 238),
            text_primary: Color::WHITE,
            text_secondary: Color::rgb(180, 200, 230),
            surface: Color::rgb(25, 40, 70),
            surface_alt: Color::rgb(35, 55, 90),
            opacity: 180,
            corner_radius: 10,
            wallpaper_style: 3,
            enable_blur: true,
        }
    }

    pub fn emerald_forest() -> Self {
        UiTheme {
            preset: ThemePreset::EmeraldForest,
            name: "Emerald",
            bg_top: Color::rgb(6, 44, 30),
            bg_bottom: Color::rgb(2, 24, 16),
            window_bg: Color::rgb(12, 32, 22),
            titlebar_active: Color::rgb(16, 64, 42),
            titlebar_inactive: Color::rgb(8, 32, 20),
            accent: Color::rgb(16, 185, 129),
            accent_hover: Color::rgb(52, 211, 153),
            text_primary: Color::rgb(209, 250, 229),
            text_secondary: Color::rgb(110, 231, 183),
            surface: Color::rgb(16, 48, 32),
            surface_alt: Color::rgb(24, 64, 44),
            opacity: 230,
            corner_radius: 8,
            wallpaper_style: 1,
            enable_blur: true,
        }
    }

    pub fn sunset_gold() -> Self {
        UiTheme {
            preset: ThemePreset::SunsetGold,
            name: "Sunset Gold",
            bg_top: Color::rgb(45, 20, 10),
            bg_bottom: Color::rgb(20, 8, 4),
            window_bg: Color::rgb(30, 14, 8),
            titlebar_active: Color::rgb(65, 28, 14),
            titlebar_inactive: Color::rgb(30, 14, 8),
            accent: Color::rgb(245, 158, 11),
            accent_hover: Color::rgb(251, 191, 36),
            text_primary: Color::rgb(254, 243, 199),
            text_secondary: Color::rgb(252, 211, 77),
            surface: Color::rgb(40, 20, 12),
            surface_alt: Color::rgb(55, 28, 16),
            opacity: 235,
            corner_radius: 8,
            wallpaper_style: 0,
            enable_blur: true,
        }
    }
}

static mut CURRENT_THEME: Option<UiTheme> = None;

pub fn get_theme() -> UiTheme {
    unsafe {
        match CURRENT_THEME {
            Some(ref t) => *t,
            None => {
                let t = UiTheme::catppuccin();
                CURRENT_THEME = Some(t);
                t
            }
        }
    }
}

pub fn set_theme(theme: UiTheme) {
    unsafe {
        CURRENT_THEME = Some(theme);
    }
}
