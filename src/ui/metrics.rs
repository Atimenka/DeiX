//! Система метрик, отступов и сетки дизайна DeiX Aurora (Design Tokens)

pub const SPACE_3: i32 = 12;
pub const SPACE_6: i32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiMetrics {
    pub window_radius: i32,
    pub window_padding: i32,
    pub titlebar_height: i32,
    pub button_diameter: i32,
    /// Высота верхней панели (часы, статус, кнопка меню).
    pub top_panel_height: i32,
    /// Высота плавающего Dock внизу экрана.
    pub dock_height: i32,
    /// Сторона плитки-иконки в Dock.
    pub dock_icon: i32,
    /// Зазор между плитками Dock.
    pub dock_gap: i32,
    /// Отступ Dock от нижнего края экрана.
    pub dock_margin: i32,
}

impl UiMetrics {
    pub const fn fluent() -> Self {
        UiMetrics {
            window_radius: 10,
            window_padding: SPACE_3,
            titlebar_height: SPACE_6,
            button_diameter: 12,
            top_panel_height: 24,
            dock_height: 48,
            dock_icon: 36,
            dock_gap: 6,
            dock_margin: 6,
        }
    }
}
