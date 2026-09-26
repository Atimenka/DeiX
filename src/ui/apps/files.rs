//! Отрисовка приложения Менеджер файлов и Редактор файлов

use crate::renderer::{Color, IconType, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::{FileViewEntry, Window};
use alloc::format;
use alloc::string::String;

pub fn draw_files(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    current_partition: &str,
    entries: &[FileViewEntry],
    selected_idx: Option<usize>,
    search_query: &str,
    error: Option<&String>,
    status_msg: Option<&String>,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    r.fill_rect(w.x, content_y, 110, w.height, theme.titlebar_inactive);
    let partitions = ["/userdata", "/system", "/kernel"];
    let mut py = content_y + 8;
    for p in partitions.iter() {
        let is_sel = current_partition == *p;
        let bg = if is_sel { theme.titlebar_active } else { theme.titlebar_inactive };
        let fg = if is_sel { theme.accent } else { theme.text_secondary };
        r.fill_rounded_rect(w.x + 4, py, 102, 28, 4, bg);
        r.draw_text(w.x + 12, py + 6, p, fg, None);
        py += 36;
    }

    r.fill_rect(w.x + 110, content_y, w.width - 110, 36, theme.titlebar_active);
    r.draw_text(w.x + 116, content_y + 10, &format!("Search: [{}]", search_query), theme.text_primary, None);

    r.fill_rounded_rect(w.x + w.width as i32 - 120, content_y + 6, 50, 24, 4, theme.accent);
    r.draw_text(w.x + w.width as i32 - 115, content_y + 10, "+File", Color::WHITE, None);

    r.fill_rounded_rect(w.x + w.width as i32 - 64, content_y + 6, 55, 24, 4, theme.accent);
    r.draw_text(w.x + w.width as i32 - 60, content_y + 10, "+Folder", Color::WHITE, None);

    let list_start_y = content_y + 44;
    let mut ey = list_start_y;
    for (idx, entry) in entries.iter().enumerate() {
        if ey + 20 > content_y + w.height as i32 - 30 {
            break;
        }
        let is_selected = selected_idx == Some(idx);
        let bg = if is_selected { theme.titlebar_active } else { theme.window_bg };
        r.fill_rect(w.x + 114, ey - 2, w.width - 120, 20, bg);

        let icon_color = if entry.is_dir { Color::YELLOW } else { theme.accent };
        r.draw_icon(w.x + 118, ey, IconType::Files, icon_color);

        let entry_str = if entry.is_dir {
            format!("{:<18} <DIR>", entry.name)
        } else {
            format!("{:<18} {:>6} B", entry.name, entry.size)
        };
        let text_color = if is_selected { theme.accent } else { theme.text_primary };
        r.draw_text(w.x + 138, ey + 2, &entry_str, text_color, None);
        ey += 22;
    }

    if let Some(msg) = status_msg {
        r.draw_text(w.x + 118, content_y + w.height as i32 - 20, msg, Color::GREEN, None);
    } else if let Some(err) = error {
        r.draw_text(w.x + 118, content_y + w.height as i32 - 20, err, Color::RED, None);
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
