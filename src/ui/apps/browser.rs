//! Отрисовка приложения Веб-Браузер
//!
//! Геометрия вкладок, кнопки новой вкладки и закладок описана
//! функциями ниже (координаты относительно области содержимого);
//! обработчик кликов в `desktop.rs` использует те же прямоугольники.

use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::{BrowserTab, Window};
use alloc::string::String;

/// Вкладка `i` в полосе вкладок.
pub fn tab_rect(i: usize) -> Rect {
    Rect::new(i as i32 * 122, 0, 120, 24)
}

/// Кнопка «+» (новая вкладка), прижата к правому краю.
pub fn new_tab_btn_rect(win_w: u32) -> Rect {
    Rect::new(win_w as i32 - 28, 2, 22, 20)
}

/// Сколько вкладок помещается до кнопки «+».
pub fn visible_tabs(win_w: u32, n: usize) -> usize {
    let cap = ((win_w as i32 - 32) / 122).max(1) as usize;
    n.min(cap)
}

/// Закладка `i` в панели закладок.
pub fn bookmark_rect(i: usize) -> Rect {
    Rect::new(8 + i as i32 * 110, 54, 104, 20)
}

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

    // Вкладки: столько, сколько помещается до «+» — тем же расчётом
    // пользуется обработчик кликов, невидимых вкладок-хитбоксов нет.
    for (i, tab) in tabs.iter().enumerate().take(visible_tabs(w.width, tabs.len())) {
        let tr = tab_rect(i);
        let is_sel = i == active_tab;
        let bg = if is_sel { theme.window_bg } else { theme.titlebar_inactive };
        r.fill_rounded_rect(w.x + tr.x, content_y + tr.y, tr.w, tr.h, 4, bg);
        let tab_title = clip_bytes(&tab.title, 10);
        r.draw_text(w.x + tr.x + 8, content_y + tr.y + 4, tab_title, theme.text_primary, None);
    }
    let ntb = new_tab_btn_rect(w.width);
    r.fill_rounded_rect(w.x + ntb.x, content_y + ntb.y, ntb.w, ntb.h, 4, theme.accent);
    r.draw_text(w.x + ntb.x + 7, content_y + ntb.y + 2, "+", Color::WHITE, None);

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
    for (i, bm) in bookmarks.iter().enumerate() {
        let br = bookmark_rect(i);
        if br.x + br.w as i32 > w.width as i32 {
            break;
        }
        let bm_label = clip_bytes(bm, 12);
        r.draw_text(w.x + br.x, content_y + br.y + 2, bm_label, theme.accent, None);
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
