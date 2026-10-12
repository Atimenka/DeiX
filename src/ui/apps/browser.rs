//! Отрисовка приложения Веб-Браузер

use crate::renderer::{Color, IconType, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::{BrowserTab, Window};
use alloc::string::String;

pub fn draw_browser(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    tabs: &[BrowserTab],
    active_tab: usize,
    address_input: &str,
    bookmarks: &[String],
    _status_msg: Option<&String>,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    // Ширина глифа — 8 px на байт (см. Renderer::draw_text): всё, что
    // рисуем, обрезаем по ширине окна, иначе длинные строки уходят
    // за правый край прямо на рабочий стол.
    let max_line_bytes = ((w.width as i32 - 24) / 8).max(4) as usize;

    let mut tx = w.x;
    for (i, tab) in tabs.iter().enumerate() {
        let tab_w = 120i32;
        let is_sel = i == active_tab;
        let bg = if is_sel { theme.window_bg } else { theme.titlebar_inactive };
        r.fill_rounded_rect(tx, content_y, tab_w as u32, 24, 4, bg);
        let tab_title = clip_bytes(&tab.title, 10);
        r.draw_text(tx + 8, content_y + 4, tab_title, theme.text_primary, None);
        tx += tab_w + 2;
    }
    r.fill_rounded_rect(w.x + w.width as i32 - 28, content_y + 2, 22, 20, 4, theme.accent);
    r.draw_text(w.x + w.width as i32 - 21, content_y + 4, "+", Color::WHITE, None);

    let nav_y = content_y + 26;
    r.fill_rect(w.x, nav_y, w.width, 28, theme.titlebar_active);
    r.draw_icon(w.x + 8, nav_y + 6, IconType::ArrowLeft, theme.text_primary);
    r.draw_icon(w.x + 28, nav_y + 6, IconType::ArrowRight, theme.text_primary);
    r.draw_icon(w.x + 48, nav_y + 6, IconType::Reload, theme.text_primary);
    r.draw_icon(w.x + 68, nav_y + 6, IconType::Home, theme.text_primary);
    r.draw_icon(w.x + 88, nav_y + 6, IconType::Bookmark, theme.accent);
    // Адрес показываем как введён (это может быть и deix://), с обрезкой
    // по ширине адресной строки.
    let addr_max = ((w.width as i32 - 116) / 8).max(4) as usize;
    r.draw_text(w.x + 108, nav_y + 6, clip_bytes(address_input, addr_max), theme.text_primary, None);

    let bk_y = nav_y + 28;
    r.fill_rect(w.x, bk_y, w.width, 20, theme.titlebar_inactive);
    let mut bx = w.x + 8;
    for bm in bookmarks.iter() {
        let bm_label = clip_bytes(bm, 12);
        r.draw_text(bx, bk_y + 2, bm_label, theme.accent, None);
        bx += 110;
    }

    let page_y = bk_y + 22;
    let current_tab = tabs.get(active_tab);
    if let Some(tab) = current_tab {
        let visible_lines = ((w.height as i32 - 80) / 18).max(1) as usize;
        let mut py = page_y;
        for line in tab.content.iter().skip(tab.scroll).take(visible_lines) {
            if line.starts_with("# ") {
                r.draw_text(w.x + 12, py, clip_bytes(&line[2..], max_line_bytes), theme.accent, None);
            } else if line.starts_with("## ") {
                r.draw_text(w.x + 12, py, clip_bytes(&line[3..], max_line_bytes), Color::CYAN, None);
            } else if line.starts_with("* ") || line.starts_with("- ") {
                r.draw_text(w.x + 20, py, clip_bytes(line, max_line_bytes.saturating_sub(1)), theme.text_primary, None);
            } else {
                r.draw_text(w.x + 12, py, clip_bytes(line, max_line_bytes), theme.text_primary, None);
            }
            py += 18;
        }
    }
}

/// Обрезает строку до max байт по границе UTF-8 символа (глиф — 8 px
/// на байт, поэтому лимит байтовый). Срез `&s[..n]` без проверки
/// границы паниковал бы на кириллице.
fn clip_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
