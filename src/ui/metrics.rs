//! Система метрик, отступов и сетки дизайна DeiX Fluent (Design Tokens)

pub const SPACE_1: i32 = 4;
pub const SPACE_2: i32 = 8;
pub const SPACE_3: i32 = 12;
pub const SPACE_4: i32 = 16;
pub const SPACE_5: i32 = 24;
pub const SPACE_6: i32 = 32;
pub const SPACE_7: i32 = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiMetrics {
    pub window_radius: i32,
    pub window_padding: i32,
    pub titlebar_height: i32,
    pub button_diameter: i32,
    pub taskbar_height: u32,
    pub start_button_width: i32,
    pub taskbar_item_width: i32,
}

impl UiMetrics {
    pub const fn fluent() -> Self {
        UiMetrics {
            window_radius: 10,
            window_padding: SPACE_3,
            titlebar_height: SPACE_6,
            button_diameter: 12,
            taskbar_height: 38,
            start_button_width: 80,
            taskbar_item_width: 140,
        }
    }
}
