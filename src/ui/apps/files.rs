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
/// Первая строка списка файлов (от верха содержимого):
/// панель инструментов (36) + строка хлебных крошек (24) + зазор.
pub const LIST_START_Y: i32 = 68;
/// Шаг строки списка.
pub const ROW_H: i32 = 22;

/// Хлебные крошки текущего пути: (hitbox, подпись, целевой путь внутри
/// раздела). Клик по сегменту — переход в этот каталог; те же
/// прямоугольники использует и отрисовка, и обработчик кликов.
/// Сегменты, не влезающие по ширине окна, не показываются и не кликаются.
pub fn breadcrumbs(
    partition: &str,
    path: &str,
    win_w: u32,
) -> alloc::vec::Vec<(Rect, String, String)> {
    let mut out = alloc::vec::Vec::new();
    let y = 42;
    let limit = win_w as i32 - 8;
    let mut x = SIDEBAR_W + 8;

    let w = crate::ui::text::text_cells(partition) as i32 * 8 + 8;
    if x + w > limit {
        return out;
    }
    out.push((
        Rect::new(x, y, w as u32, 20),
        String::from(partition),
        String::from("/"),
    ));
    x += w + 14;

    let mut target = String::new();
    for comp in path.split('/').filter(|c| !c.is_empty()) {
        target.push('/');
        target.push_str(comp);
        let w = crate::ui::text::text_cells(comp) as i32 * 8 + 8;
        if x + w > limit {
            break;
        }
        out.push((
            Rect::new(x, y, w as u32, 20),
            String::from(comp),
            target.clone(),
        ));
        x += w + 14;
    }
    out
}

/// Кнопка «+File» (относительно области содержимого). Правый якорь —
/// как и при отрисовке.
pub fn new_file_btn_rect(win_w: u32) -> Rect {
    Rect::new(win_w as i32 - 120, 6, 50, 24)
}

/// Кнопка «+Folder».
pub fn new_folder_btn_rect(win_w: u32) -> Rect {
    Rect::new(win_w as i32 - 64, 6, 55, 24)
}

/// Кнопка «Delete» (видна, когда на записываемом разделе выбран элемент).
pub fn delete_btn_rect(win_w: u32) -> Rect {
    Rect::new(win_w as i32 - 182, 6, 54, 24)
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

    // Создание файлов возможно только на записываемом разделе: на /system
    // кнопки есть, только если он ext2 и включён dev-режим (vfs::is_writable).
    let writable = crate::vfs::is_writable(current_partition);
    if writable {
        let fb = new_file_btn_rect(w.width);
        r.fill_rounded_rect(w.x + fb.x, content_y + fb.y, fb.w, fb.h, 4, theme.accent);
        r.draw_text(w.x + fb.x + 5, content_y + fb.y + 4, "+File", Color::WHITE, None);

        let db = new_folder_btn_rect(w.width);
        r.fill_rounded_rect(w.x + db.x, content_y + db.y, db.w, db.h, 4, theme.accent);
        r.draw_text(w.x + db.x + 4, content_y + db.y + 4, "+Folder", Color::WHITE, None);

        if selected_idx.is_some() {
            let xb = delete_btn_rect(w.width);
            r.fill_rounded_rect(w.x + xb.x, content_y + xb.y, xb.w, xb.h, 4, Color::RED);
            r.draw_text(w.x + xb.x + 5, content_y + xb.y + 4, "Delete", Color::WHITE, None);
        }
    }

    // Хлебные крошки: раздел и каждый каталог пути — кликабельные
    // сегменты (те же Rect проверяет обработчик кликов).
    let crumbs = breadcrumbs(current_partition, current_path, w.width);
    let last = crumbs.len().saturating_sub(1);
    for (i, (rect, label, _)) in crumbs.iter().enumerate() {
        let (cx, cy) = (w.x + rect.x, content_y + rect.y);
        let is_last = i == last;
        let bg = if is_last { theme.titlebar_active } else { theme.titlebar_inactive };
        let fg = if is_last { theme.accent } else { theme.text_primary };
        r.fill_rounded_rect(cx, cy, rect.w, rect.h, 4, bg);
        r.draw_text(cx + 4, cy + 4, label, fg, None);
        if !is_last {
            r.draw_text(cx + rect.w as i32 + 3, cy + 4, ">", theme.text_secondary, None);
        }
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
        let mut loc = if current_path == "/" {
            String::from(current_partition)
        } else {
            format!("{}{}", current_partition, current_path)
        };
        if !writable {
            if current_partition == "/system" && crate::vfs::system_is_ext2_mounted() {
                loc.push_str("  [read-only: dev mode off]");
            } else {
                loc.push_str("  [read-only]");
            }
        }
        let max_cells = ((w.width as i32 - SIDEBAR_W - 16) / 8).max(8) as usize;
        let shown = crate::ui::text::truncate_to_cells(&loc, max_cells).0;
        r.draw_text(w.x + SIDEBAR_W + 8, bottom_y, shown, theme.text_secondary, None);
    }
}

/// Диалог подтверждения закрытия редактора (координаты относительно
/// области содержимого окна).
pub fn editor_dialog_rect(win_w: u32, win_h: u32) -> Rect {
    let w = 300u32.min(win_w.saturating_sub(16)).max(220);
    let h = 96u32;
    Rect::new(
        (win_w as i32 - w as i32) / 2,
        ((win_h as i32 - h as i32) / 2).max(8),
        w,
        h,
    )
}

/// Кнопка диалога закрытия: 0 = Save, 1 = Discard, 2 = Cancel.
pub fn editor_dialog_btn(win_w: u32, win_h: u32, i: usize) -> Rect {
    let d = editor_dialog_rect(win_w, win_h);
    let btn_w = (d.w as i32 - 32) / 3;
    Rect::new(
        d.x + 8 + i as i32 * (btn_w + 8),
        d.y + d.h as i32 - 32,
        btn_w as u32,
        24,
    )
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
    confirm_close: bool,
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

    // Диалог закрытия с несохранёнными изменениями: Save / Discard /
    // Cancel. Кнопки — те же Rect, что проверяет обработчик кликов.
    if confirm_close {
        r.shade_rect(w.x, content_y, w.width, w.height, 90);
        let d = editor_dialog_rect(w.width, w.height);
        let (dx, dy) = (w.x + d.x, content_y + d.y);
        r.fill_rounded_rect(dx, dy, d.w, d.h, 6, theme.window_bg);
        r.draw_rect(dx, dy, d.w, d.h, theme.accent);

        let max_cells = ((d.w as i32 - 24) / 8).max(8) as usize;
        let prompt = format!("Save changes to {}?", filename);
        let shown = crate::ui::text::truncate_to_cells(&prompt, max_cells).0;
        r.draw_text(dx + 12, dy + 12, shown, theme.text_primary, None);

        let labels = ["Save", "Discard", "Cancel"];
        for (i, label) in labels.iter().enumerate() {
            let b = editor_dialog_btn(w.width, w.height, i);
            let (bx, by) = (w.x + b.x, content_y + b.y);
            let bg = match i {
                0 => theme.accent,
                1 => Color::RED,
                _ => theme.titlebar_inactive,
            };
            let fg = if i == 2 { theme.text_primary } else { Color::WHITE };
            r.fill_rounded_rect(bx, by, b.w, b.h, 4, bg);
            let tw = label.len() as i32 * 8;
            r.draw_text(bx + (b.w as i32 - tw) / 2, by + 5, label, fg, None);
        }
    }
}
