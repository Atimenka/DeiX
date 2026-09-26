//! Отрисовка приложения О системе (About System)

use crate::renderer::{Color, IconType, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::Window;

pub fn draw_about(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    r.draw_icon(w.x + 20, content_y + 16, IconType::About, theme.accent);
    r.draw_text(w.x + 48, content_y + 16, "DeiX Operating System", theme.accent, None);
    r.draw_text(w.x + 48, content_y + 36, "Version 0.2.1-beta (Fluent Edition)", theme.text_primary, None);

    r.draw_hline(w.x + 12, content_y + 60, w.width - 24, theme.titlebar_active);

    r.draw_text(w.x + 16, content_y + 72, "Architecture: x86_64 Long Mode (64-bit)", theme.text_secondary, None);
    r.draw_text(w.x + 16, content_y + 92, "Kernel: Custom Rust No-Std Microkernel", theme.text_secondary, None);
    r.draw_text(w.x + 16, content_y + 112, "Compositor: GFX.KMOD VBE Double Buffer", theme.text_secondary, None);
    r.draw_text(w.x + 16, content_y + 132, "GUI Design: DeiX Fluent System", theme.text_secondary, None);

    r.draw_text(w.x + 16, content_y + 160, "Copyright (c) 2026 DeiX OS Team", Color::GRAY, None);
}
