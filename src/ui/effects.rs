//! Профили производительности графики (Quality / Balanced / Performance).
//!
//! Каждый профиль реально меняет путь отрисовки:
//! * Quality — blur под окнами (если тема его включает), тени,
//!   полупрозрачные поверхности;
//! * Balanced — без blur (самая дорогая операция), тени и
//!   полупрозрачность остаются;
//! * Performance — без blur и теней, поверхности непрозрачны
//!   (alpha = 255 уходит в быстрый путь копирования без смешивания).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PerfProfile {
    Quality,
    Balanced,
    Performance,
}

impl PerfProfile {
    /// Разрешён ли blur под окнами (сам blur ещё должна включать тема).
    pub fn blur_enabled(self) -> bool {
        matches!(self, PerfProfile::Quality)
    }

    /// Рисовать ли тени окон.
    pub fn shadows_enabled(self) -> bool {
        !matches!(self, PerfProfile::Performance)
    }

    /// Итоговая непрозрачность поверхностей: Performance форсирует 255,
    /// и fill_*_alpha сводится к быстрому пути без смешивания.
    pub fn effective_opacity(self, theme_opacity: u8) -> u8 {
        match self {
            PerfProfile::Performance => 255,
            _ => theme_opacity,
        }
    }

    /// Токен для конфигурационного файла.
    pub fn token(self) -> &'static str {
        match self {
            PerfProfile::Quality => "quality",
            PerfProfile::Balanced => "balanced",
            PerfProfile::Performance => "performance",
        }
    }

    /// Разбор токена из конфигурационного файла.
    pub fn from_token(s: &str) -> Option<Self> {
        match s {
            "quality" => Some(PerfProfile::Quality),
            "balanced" => Some(PerfProfile::Balanced),
            "performance" => Some(PerfProfile::Performance),
            _ => None,
        }
    }
}

static mut CURRENT: PerfProfile = PerfProfile::Quality;

pub fn current() -> PerfProfile {
    unsafe { CURRENT }
}

pub fn set(p: PerfProfile) {
    unsafe { CURRENT = p };
}
