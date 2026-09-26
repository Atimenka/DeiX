//! Отрисовка приложения Терминал

use crate::renderer::Renderer;
use crate::ui::theme::UiTheme;
use crate::ui::window::Window;
use alloc::format;
use alloc::string::String;

pub fn draw_terminal(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    lines: &[String],
    current_line: &str,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    let max_visible_lines = (w.height as usize - 24) / 16;
    let start_idx = lines.len().saturating_sub(max_visible_lines);

    let mut ty = content_y + 8;
    for line in lines.iter().skip(start_idx) {
        r.draw_text(w.x + 8, ty, line, theme.text_primary, None);
        ty += 16;
    }

    let prompt = format!("> {}_", current_line);
    r.draw_text(w.x + 8, ty, &prompt, theme.accent, None);
}
