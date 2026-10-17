//! Отрисовка приложения Веб-Браузер
//!
//! Геометрия вкладок, кнопок навигации, закладок и строк страницы
//! описана функциями ниже (координаты относительно области
//! содержимого); обработчик кликов в `desktop.rs` использует те же
//! прямоугольники — невидимых хитбоксов нет.
//!
//! Раскладка (относительно content_y):
//!   0..24   — полоса вкладок (+ кнопка новой вкладки, × на активной)
//!   26..54  — панель навигации (Back/Forward/Reload/Home/Star + адрес)
//!   54..74  — панель закладок
//!   76..    — страница, строки шагом 18; справа — полоса прокрутки
//!   низ 18  — строка статуса (сообщение + позиция в документе)

use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::{BrowserTab, Window};
use alloc::string::String;

/// Вкладка `i` в полосе вкладок.
pub fn tab_rect(i: usize) -> Rect {
    Rect::new(i as i32 * 122, 0, 120, 24)
}

/// Зона «×» (закрыть) внутри вкладки `i` — правые 18 px.
pub fn tab_close_rect(i: usize) -> Rect {
    let t = tab_rect(i);
    Rect::new(t.x + t.w as i32 - 18, t.y + 3, 16, 18)
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

/// Кнопки панели навигации.
#[derive(Clone, Copy, PartialEq)]
pub enum NavButton {
    Back,
    Forward,
    Reload,
    Home,
    Star,
}

const NAV_BUTTONS: [NavButton; 5] = [
    NavButton::Back,
    NavButton::Forward,
    NavButton::Reload,
    NavButton::Home,
    NavButton::Star,
];

/// Прямоугольник кнопки навигации `i` (0..5).
pub fn nav_btn_rect(i: usize) -> Rect {
    Rect::new(6 + i as i32 * 22, 28, 22, 24)
}

/// Какая кнопка навигации лежит под точкой (той же геометрией
/// пользуется draw_browser).
pub fn nav_button_at(rel_x: i32, rel_y: i32) -> Option<NavButton> {
    for (i, b) in NAV_BUTTONS.iter().enumerate() {
        if nav_btn_rect(i).contains(rel_x, rel_y) {
            return Some(*b);
        }
    }
    None
}

/// Закладка `i` в панели закладок.
pub fn bookmark_rect(i: usize) -> Rect {
    Rect::new(8 + i as i32 * 110, 54, 104, 20)
}

/// Сколько строк страницы помещается в окно высотой win_h
/// (верх 76 px — вкладки/навигация/закладки, низ 20 px — статус).
pub fn visible_lines(win_h: u32) -> usize {
    ((win_h as i32 - 96) / 18).max(1) as usize
}

pub fn draw_browser(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    tabs: &[BrowserTab],
    active_tab: usize,
    address_input: &str,
    bookmarks: &[String],
    status_msg: Option<&String>,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    // Ширина глифа — 8 px на байт (см. Renderer::draw_text): всё, что
    // рисуем, обрезаем по ширине окна, иначе длинные строки уходят
    // за правый край прямо на рабочий стол.
    let max_line_bytes = ((w.width as i32 - 28) / 8).max(4) as usize;

    // ---- Полоса вкладок: тёмная подложка, активная вкладка светлее
    // с акцентной чертой снизу и «×».
    r.fill_rect(w.x, content_y, w.width, 24, theme.titlebar_inactive.lerp(Color::BLACK, 60));
    for (i, tab) in tabs.iter().enumerate().take(visible_tabs(w.width, tabs.len())) {
        let tr = tab_rect(i);
        let is_sel = i == active_tab;
        let bg = if is_sel {
            theme.window_bg
        } else {
            theme.titlebar_inactive
        };
        r.fill_rounded_rect(w.x + tr.x + 1, content_y + tr.y + 2, tr.w - 2, tr.h - 2, 6, bg);
        if is_sel {
            r.fill_rect(w.x + tr.x + 6, content_y + tr.y + 22, tr.w - 12, 2, theme.accent);
        }
        let title_cap = if is_sel { 10 } else { 12 };
        let tab_title = clip_bytes(&tab.title, title_cap);
        let tcol = if is_sel { theme.text_primary } else { theme.text_secondary };
        r.draw_text(w.x + tr.x + 8, content_y + tr.y + 6, tab_title, tcol, None);
        if is_sel {
            let cr = tab_close_rect(i);
            r.draw_text(w.x + cr.x + 4, content_y + cr.y + 3, "x", theme.text_secondary, None);
        }
    }
    let ntb = new_tab_btn_rect(w.width);
    r.fill_rounded_rect(w.x + ntb.x, content_y + ntb.y, ntb.w, ntb.h, 6, theme.accent);
    r.draw_text(w.x + ntb.x + 7, content_y + ntb.y + 3, "+", Color::WHITE, None);

    // ---- Панель навигации: круглые кнопки + скруглённое поле адреса.
    let nav_y = content_y + 26;
    r.fill_rect(w.x, nav_y, w.width, 28, theme.titlebar_active);
    let cur_tab = tabs.get(active_tab);
    let can_back = cur_tab.map(|t| t.hist_pos > 0).unwrap_or(false);
    let can_fwd = cur_tab
        .map(|t| t.hist_pos + 1 < t.history.len())
        .unwrap_or(false);
    let bookmarked = cur_tab
        .map(|t| bookmarks.iter().any(|b| *b == t.url))
        .unwrap_or(false);
    for (i, b) in NAV_BUTTONS.iter().enumerate() {
        let br = nav_btn_rect(i);
        let (cx, cy) = (w.x + br.x + br.w as i32 / 2, nav_y + 14);
        r.fill_circle(cx, cy, 10, theme.window_bg.lerp(theme.titlebar_active, 128));
        let (icon, col) = match b {
            NavButton::Back => (
                IconType::ArrowLeft,
                if can_back { theme.text_primary } else { theme.text_secondary },
            ),
            NavButton::Forward => (
                IconType::ArrowRight,
                if can_fwd { theme.text_primary } else { theme.text_secondary },
            ),
            NavButton::Reload => (IconType::Reload, theme.text_primary),
            NavButton::Home => (IconType::Home, theme.text_primary),
            NavButton::Star => (
                IconType::Bookmark,
                if bookmarked { theme.accent } else { theme.text_secondary },
            ),
        };
        r.draw_icon(cx - 8, cy - 8, icon, col);
    }
    // Поле адреса: скруглённый фон + рамка; https подсвечивается акцентом.
    let addr_x = w.x + 118;
    let addr_w = (w.width as i32 - 118 - 8).max(40) as u32;
    r.fill_rounded_rect(addr_x, nav_y + 3, addr_w, 22, 10, theme.window_bg.lerp(Color::BLACK, 30));
    r.draw_rounded_rect_outline(addr_x, nav_y + 3, addr_w, 22, 10, theme.accent, 90);
    let addr_max = ((addr_w as i32 - 16) / 8).max(4) as usize;
    let addr_col = if address_input.starts_with("https://") {
        theme.accent
    } else {
        theme.text_primary
    };
    r.draw_text(addr_x + 8, nav_y + 8, clip_bytes(address_input, addr_max), addr_col, None);

    // ---- Панель закладок.
    let bk_y = nav_y + 28;
    r.fill_rect(w.x, bk_y, w.width, 20, theme.titlebar_inactive);
    for (i, bm) in bookmarks.iter().enumerate() {
        let br = bookmark_rect(i);
        if br.x + br.w as i32 > w.width as i32 {
            break;
        }
        let label = bm
            .strip_prefix("https://")
            .or_else(|| bm.strip_prefix("http://"))
            .unwrap_or(bm);
        r.draw_text(w.x + br.x, content_y + br.y + 4, clip_bytes(label, 12), theme.accent, None);
    }

    // ---- Страница.
    let page_y = bk_y + 22;
    let vis = visible_lines(w.height);
    if let Some(tab) = cur_tab {
        let mut py = page_y;
        for line in tab.content.iter().skip(tab.scroll).take(vis) {
            if line.starts_with("# ") {
                r.draw_text(w.x + 12, py, clip_bytes(&line[2..], max_line_bytes), theme.accent, None);
            } else if line.starts_with("## ") {
                r.draw_text(w.x + 12, py, clip_bytes(&line[3..], max_line_bytes), Color::CYAN, None);
            } else if is_link_line(line) {
                // Кликабельная строка `[N] url`: акцентный цвет + подчёркивание.
                let shown = clip_bytes(line, max_line_bytes);
                r.draw_text(w.x + 12, py, shown, theme.accent, None);
                r.draw_hline(w.x + 12, py + 13, (shown.len() * 8) as u32, theme.accent.lerp(theme.window_bg, 110));
            } else if line.starts_with("* ") || line.starts_with("- ") {
                r.draw_text(w.x + 20, py, clip_bytes(line, max_line_bytes.saturating_sub(1)), theme.text_primary, None);
            } else {
                r.draw_text(w.x + 12, py, clip_bytes(line, max_line_bytes), theme.text_primary, None);
            }
            py += 18;
        }

        // Полоса прокрутки: только когда контент длиннее окна; размеры
        // пропорциональны видимой доле.
        let total = tab.content.len();
        if total > vis {
            let track_h = (vis * 18) as i32;
            let track_x = w.x + w.width as i32 - 7;
            r.fill_rect(track_x, page_y, 4, track_h as u32, theme.titlebar_inactive);
            let thumb_h = ((track_h as usize * vis) / total).max(12) as i32;
            let denom = (total - vis).max(1);
            let thumb_y = page_y + ((track_h - thumb_h) as usize * tab.scroll.min(denom) / denom) as i32;
            r.fill_rounded_rect(track_x, thumb_y, 4, thumb_h as u32, 2, theme.accent);
        }

        // ---- Строка статуса: сообщение + позиция.
        let st_y = content_y + w.height as i32 - 18;
        r.fill_rect(w.x, st_y, w.width, 18, theme.titlebar_inactive.lerp(Color::BLACK, 40));
        let pos = alloc::format!(
            "{}/{}",
            (tab.scroll + vis).min(total),
            total
        );
        let pos_x = w.x + w.width as i32 - (pos.len() as i32 * 8) - 10;
        r.draw_text(pos_x, st_y + 3, &pos, theme.text_secondary, None);
        if let Some(msg) = status_msg {
            let cap = (((pos_x - w.x - 16) / 8).max(4)) as usize;
            r.draw_text(w.x + 8, st_y + 3, clip_bytes(msg, cap), theme.text_secondary, None);
        }
    }
}

/// Строка-ссылка вида `[N] ...` (N — число): той же проверкой
/// пользуется обработчик кликов в desktop.rs.
fn is_link_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('[') else {
        return false;
    };
    match rest.find(']') {
        Some(close) if close > 0 => rest[..close].bytes().all(|b| b.is_ascii_digit()),
        _ => false,
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
