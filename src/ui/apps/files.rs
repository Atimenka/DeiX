//! Отрисовка приложения Менеджер файлов и Редактор файлов.
//!
//! Геометрия интерактивных элементов задана функциями ниже (координаты
//! относительно области содержимого окна); обработчик кликов в
//! `desktop.rs` обязан использовать те же прямоугольники.

use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::{FileViewEntry, Window};
use alloc::format;
use alloc::string::String;

/// Ширина боковой панели разделов.
pub const SIDEBAR_W: i32 = 110;
/// Первая строка списка файлов (от верха содержимого).
pub const LIST_START_Y: i32 = 44;
/// Шаг строки списка.
pub const ROW_H: i32 = 22;

/// Кнопка «+File» (относительно области содержимого). Правый якорь —
/// как и при отрисовке.
pub fn new_file_btn_rect(win_w: u32) -> Rect {
    Rect::new(win_w as i32 - 120, 6, 50, 24)
}

/// Кнопка «+Folder».
pub fn new_folder_btn_rect(win_w: u32) -> Rect {
    Rect::new(win_w as i32 - 64, 6, 55, 24)
}

/// Кнопка раздела в боковой панели.
pub fn sidebar_item_rect(i: usize) -> Rect {
    Rect::new(4, 8 + i as i32 * 36, 102, 28)
}

pub fn draw_files(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    current_partition: &str,
    current_path: &str,
    entries: &[FileViewEntry],
    selected_idx: Option<usize>,
    search_query: &str,
    error: Option<&String>,
    status_msg: Option<&String>,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    r.fill_rect(w.x, content_y, SIDEBAR_W as u32, w.height, theme.titlebar_inactive);
    let partitions = ["/userdata", "/system"];
    for (i, p) in partitions.iter().enumerate() {
        let item = sidebar_item_rect(i);
        let is_sel = current_partition == *p;
        let bg = if is_sel { theme.titlebar_active } else { theme.titlebar_inactive };
        let fg = if is_sel { theme.accent } else { theme.text_secondary };
        r.fill_rounded_rect(w.x + item.x, content_y + item.y, item.w, item.h, 4, bg);
        r.draw_text(w.x + item.x + 8, content_y + item.y + 6, p, fg, None);
    }

    r.fill_rect(w.x + SIDEBAR_W, content_y, w.width - SIDEBAR_W as u32, 36, theme.titlebar_active);
    r.draw_icon(w.x + SIDEBAR_W + 6, content_y + 10, IconType::Search, theme.text_secondary);
    r.draw_text(
        w.x + SIDEBAR_W + 26,
        content_y + 10,
        &format!("[{}]", search_query),
        theme.text_primary,
        None,
    );

    // Создание файлов возможно только на записываемом разделе: на
    // /system (EROFS, read-only) кнопки не рисуются и не кликаются.
    let writable = current_partition == "/userdata";
    if writable {
        let fb = new_file_btn_rect(w.width);
        r.fill_rounded_rect(w.x + fb.x, content_y + fb.y, fb.w, fb.h, 4, theme.accent);
        r.draw_text(w.x + fb.x + 5, content_y + fb.y + 4, "+File", Color::WHITE, None);

        let db = new_folder_btn_rect(w.width);
        r.fill_rounded_rect(w.x + db.x, content_y + db.y, db.w, db.h, 4, theme.accent);
        r.draw_text(w.x + db.x + 4, content_y + db.y + 4, "+Folder", Color::WHITE, None);
    }

    let list_start_y = content_y + LIST_START_Y;
    let mut ey = list_start_y;
    for (idx, entry) in entries.iter().enumerate() {
        if ey + 20 > content_y + w.height as i32 - 30 {
            break;
        }
        let is_selected = selected_idx == Some(idx);
        let bg = if is_selected { theme.titlebar_active } else { theme.window_bg };
        r.fill_rect(w.x + SIDEBAR_W + 4, ey - 2, w.width - (SIDEBAR_W as u32 + 10), 20, bg);

        let icon_color = if entry.is_dir { Color::YELLOW } else { theme.accent };
        r.draw_icon(w.x + SIDEBAR_W + 8, ey, IconType::Files, icon_color);

        let entry_str = if entry.is_dir {
            format!("{:<18} <DIR>", entry.name)
        } else {
            format!("{:<18} {:>6} B", entry.name, entry.size)
        };
        let text_color = if is_selected { theme.accent } else { theme.text_primary };
        r.draw_text(w.x + SIDEBAR_W + 28, ey + 2, &entry_str, text_color, None);
        ey += ROW_H;
    }

    // Нижняя строка: сообщение об операции, ошибка или текущий путь.
    let bottom_y = content_y + w.height as i32 - 20;
    if let Some(msg) = status_msg {
        r.draw_text(w.x + SIDEBAR_W + 8, bottom_y, msg, Color::GREEN, None);
    } else if let Some(err) = error {
        r.draw_text(w.x + SIDEBAR_W + 8, bottom_y, err, Color::RED, None);
    } else {
        let loc = if current_path == "/" {
            String::from(current_partition)
        } else {
            format!("{}{}", current_partition, current_path)
        };
        r.draw_text(w.x + SIDEBAR_W + 8, bottom_y, &loc, theme.text_secondary, None);
    }
}

pub fn draw_file_editor(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    filename: &str,
    lines: &[String],
    cursor_row: usize,
    cursor_col: usize,
    scroll: usize,
    modified: bool,
    read_only: bool,
    error: Option<&String>,
    status_msg: Option<&String>,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    let mod_flag = if modified { " *" } else { "" };
    let ro_flag = if read_only { " [RO]" } else { "" };
    let title_str = format!("File: {}{}{}", filename, mod_flag, ro_flag);
    r.draw_text(w.x + 12, content_y + 8, &title_str, theme.accent, None);

    let edit_area_y = content_y + 32;
    let line_height = 18;
    let visible_lines = ((w.height as i32 - 60) / line_height).max(1) as usize;

    for (i, line) in lines.iter().skip(scroll).take(visible_lines).enumerate() {
        let line_no = scroll + i;
        let ly = edit_area_y + (i as i32 * line_height);

        let num_str = format!("{:>3} |", line_no + 1);
        r.draw_text(w.x + 8, ly, &num_str, theme.text_secondary, None);
        r.draw_text(w.x + 50, ly, line, theme.text_primary, None);

        if line_no == cursor_row {
            let cx = w.x + 50 + (cursor_col as i32 * 8);
            r.fill_rect(cx, ly, 2, line_height as u32, theme.accent);
        }
    }

    let status_y = content_y + w.height as i32 - 22;
    r.draw_hline(w.x, status_y - 2, w.width, theme.titlebar_active);
    let pos_str = format!("Ln {}, Col {}", cursor_row + 1, cursor_col + 1);
    r.draw_text(w.x + 8, status_y, &pos_str, theme.text_secondary, None);

    if let Some(msg) = status_msg {
        r.draw_text(w.x + 160, status_y, msg, Color::GREEN, None);
    } else if let Some(err) = error {
        r.draw_text(w.x + 160, status_y, err, Color::RED, None);
    }
}
