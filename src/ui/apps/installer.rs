//! Отрисовка мастера установки (Install DeiX).
//!
//! Геометрия элементов (кнопки, списки, поля) задаётся здесь ОДНИМИ
//! функциями в координатах ОТНОСИТЕЛЬНО области содержимого окна —
//! и отрисовка, и обработчик кликов в desktop.rs используют одни и те
//! же прямоугольники.

use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::{InstallerPage, InstallerState, Window};
use alloc::format;
use alloc::string::String;

/// Высота шапки мастера (заголовок + разделитель).
const HEADER_H: i32 = 56;
/// Начало тела страницы.
const BODY_Y: i32 = 64;

/// Кнопка Back (координаты относительно области содержимого).
pub fn back_btn_rect(win_w: u32, win_h: u32) -> Rect {
    Rect::new(win_w as i32 - 196, win_h as i32 - 38, 88, 28)
}

/// Кнопка Next / Install.
pub fn next_btn_rect(win_w: u32, win_h: u32) -> Rect {
    Rect::new(win_w as i32 - 100, win_h as i32 - 38, 88, 28)
}

/// Строка списка (диски, схема, размеры).
pub fn list_item_rect(win_w: u32, i: usize) -> Rect {
    Rect::new(16, BODY_Y + 22 + i as i32 * 34, win_w.saturating_sub(32), 30)
}

/// Поле ввода страницы Account (0 = username, 1 = password, 2 = confirm).
pub fn field_rect(win_w: u32, i: usize) -> Rect {
    Rect::new(16, BODY_Y + 40 + i as i32 * 54, win_w.saturating_sub(32), 26)
}

/// Строка-переключатель «Migrate all your files» (страница Confirm,
/// этап 2 переноса данных).
pub fn migrate_row_rect(win_w: u32) -> Rect {
    Rect::new(16, BODY_Y + 116, win_w.saturating_sub(32), 22)
}

/// Кнопка «Restart now» страницы Done (этап 3 — немедленная
/// перезагрузка с нового диска). Стоит слева внизу: невидимые на Done
/// click-зоны Back/Next остаются справа и не перехватывают клик.
pub fn done_reboot_rect(win_h: u32) -> Rect {
    Rect::new(16, win_h as i32 - 38, 150, 28)
}

/// Есть ли у страницы кнопка Back.
fn has_back(page: InstallerPage) -> bool {
    matches!(
        page,
        InstallerPage::Scheme | InstallerPage::Size | InstallerPage::Account | InstallerPage::Confirm
    )
}

/// Есть ли у страницы кнопка Next и её подпись.
fn next_label(page: InstallerPage) -> Option<&'static str> {
    match page {
        InstallerPage::Disk | InstallerPage::Scheme | InstallerPage::Size | InstallerPage::Account => {
            Some("Next")
        }
        InstallerPage::Confirm => Some("Install"),
        InstallerPage::Progress | InstallerPage::Done | InstallerPage::Failed => None,
    }
}

fn page_title(page: InstallerPage) -> (&'static str, &'static str) {
    match page {
        InstallerPage::Disk => ("Step 1 of 5", "Select the target disk"),
        InstallerPage::Scheme => ("Step 2 of 5", "Select the boot scheme"),
        InstallerPage::Size => ("Step 3 of 5", "Choose /userdata size"),
        InstallerPage::Account => ("Step 4 of 5", "Create your account"),
        InstallerPage::Confirm => ("Step 5 of 5", "Ready to install"),
        InstallerPage::Progress => ("Installing", "Do not power off the machine"),
        InstallerPage::Done => ("Finished", "DeiX has been installed"),
        InstallerPage::Failed => ("Failed", "Installation did not complete"),
    }
}

fn draw_button(r: &mut Renderer, theme: &UiTheme, x: i32, y: i32, rect: Rect, label: &str, primary: bool) {
    let bg = if primary { theme.accent } else { theme.titlebar_inactive };
    let fg = if primary { Color::WHITE } else { theme.text_primary };
    crate::ui::widgets::icon_button(
        r,
        x + rect.x,
        y + rect.y,
        rect.w,
        rect.h,
        6,
        bg,
        fg,
        crate::ui::widgets::icon_for_label(label),
        label,
    );
}

/// Строка списка: выделение + текст (и приглушённый текст для
/// недоступных пунктов).
fn draw_list_item(
    r: &mut Renderer,
    theme: &UiTheme,
    x: i32,
    y: i32,
    rect: Rect,
    text: &str,
    selected: bool,
    enabled: bool,
) {
    if selected && enabled {
        r.fill_rounded_rect_alpha(x + rect.x, y + rect.y, rect.w, rect.h, 5, theme.accent, 90);
        r.draw_rect(x + rect.x, y + rect.y, rect.w, rect.h, theme.accent);
    }
    let fg = if enabled { theme.text_primary } else { theme.text_secondary };
    r.draw_text(x + rect.x + 10, y + rect.y + 8, text, fg, None);
}

pub fn draw_installer(r: &mut Renderer, theme: &UiTheme, w: &Window, st: &InstallerState, content_y: i32) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);
    let x = w.x;
    let y = content_y;
    let ww = w.width;
    let wh = w.height;

    // Шапка.
    let (step, title) = page_title(st.page);
    r.draw_icon(x + 16, y + 14, IconType::Install, theme.accent);
    r.draw_text(x + 44, y + 10, "Install DeiX", theme.accent, None);
    r.draw_text(x + 44, y + 28, step, theme.text_secondary, None);
    r.draw_hline(x + 12, y + HEADER_H, ww.saturating_sub(24), theme.titlebar_active);
    r.draw_text(x + 16, y + BODY_Y, title, theme.text_primary, None);

    match st.page {
        InstallerPage::Disk => {
            if st.disks.is_empty() {
                r.draw_text(x + 16, y + BODY_Y + 26, "No ATA disks detected.", theme.text_secondary, None);
            }
            for (i, d) in st.disks.iter().enumerate() {
                let line = match d.sectors {
                    Some(s) => format!("{} - {} MiB", d.label, s / 2048),
                    None => format!("{} - size unknown", d.label),
                };
                draw_list_item(r, theme, x, y, list_item_rect(ww, i), &line, i == st.disk_idx, true);
            }
            r.draw_text(
                x + 16,
                y + wh as i32 - 78,
                "Everything on the selected disk will be erased.",
                theme.text_secondary,
                None,
            );
        }
        InstallerPage::Scheme => {
            draw_list_item(r, theme, x, y, list_item_rect(ww, 0), "BIOS / MBR (recommended)", true, true);
            draw_list_item(
                r,
                theme,
                x,
                y,
                list_item_rect(ww, 1),
                "UEFI (GPT + EFI loader) - not available yet",
                false,
                false,
            );
            r.draw_text(
                x + 16,
                y + wh as i32 - 78,
                "UEFI support is planned for the next update.",
                theme.text_secondary,
                None,
            );
        }
        InstallerPage::Size => {
            let options = st.size_options_mib();
            let min_mib = crate::install::USERDATA_MIN_SECTORS / 2048;
            for (i, mib) in options.iter().enumerate() {
                let line = if *mib == min_mib {
                    format!("{} MiB (minimum, stock layout)", mib)
                } else {
                    format!("{} MiB", mib)
                };
                draw_list_item(r, theme, x, y, list_item_rect(ww, i), &line, i == st.size_idx, true);
            }
            if options.is_empty() {
                r.draw_text(x + 16, y + BODY_Y + 26, "Disk has no room for /userdata.", theme.text_secondary, None);
            }
        }
        InstallerPage::Account => {
            let labels = ["Username", "Password", "Confirm password"];
            for (i, label) in labels.iter().enumerate() {
                let rect = field_rect(ww, i);
                r.draw_text(x + rect.x, y + rect.y - 16, label, theme.text_secondary, None);
                let focused = st.field == i;
                let border = if focused { theme.accent } else { theme.titlebar_inactive };
                r.fill_rect_alpha(x + rect.x, y + rect.y, rect.w, rect.h, theme.window_bg, 230);
                r.draw_rect(x + rect.x, y + rect.y, rect.w, rect.h, border);
                let value: String = match i {
                    0 => st.username.clone(),
                    1 => "*".repeat(st.password.len()),
                    _ => "*".repeat(st.confirm.len()),
                };
                let shown = if focused { format!("{}_", value) } else { value };
                r.draw_text(x + rect.x + 8, y + rect.y + 8, &shown, theme.text_primary, None);
            }
            r.draw_text(
                x + 16,
                y + wh as i32 - 78,
                "The password is also the disk encryption key.",
                theme.text_secondary,
                None,
            );
        }
        InstallerPage::Confirm => {
            let disk = st
                .disks
                .get(st.disk_idx)
                .map(|d| d.label)
                .unwrap_or("(no disk)");
            let mib = st.size_options_mib().get(st.size_idx).copied().unwrap_or(8);
            let mut ly = y + BODY_Y + 24;
            r.draw_text(x + 16, ly, &format!("Target disk:    {}", disk), theme.text_primary, None);
            ly += 20;
            r.draw_text(x + 16, ly, "Boot scheme:    BIOS / MBR", theme.text_primary, None);
            ly += 20;
            r.draw_text(x + 16, ly, &format!("/userdata size: {} MiB (ext2)", mib), theme.text_primary, None);
            ly += 20;
            r.draw_text(x + 16, ly, &format!("User:           {}", st.username.trim()), theme.text_primary, None);
            // Этап 2: строка-переключатель переноса всех файлов пользователя.
            let mrect = migrate_row_rect(ww);
            r.draw_rect(x + mrect.x, y + mrect.y + 3, 14, 14, theme.titlebar_inactive);
            if st.migrate {
                r.fill_rect(x + mrect.x + 3, y + mrect.y + 6, 8, 8, theme.accent);
            }
            r.draw_text(
                x + mrect.x + 24,
                y + mrect.y + 4,
                "Migrate all your files from this disk",
                theme.text_primary,
                None,
            );
            r.draw_text(
                x + 16,
                y + mrect.y + mrect.h as i32 + 10,
                "WARNING: ALL DATA ON THE TARGET DISK WILL BE ERASED!",
                Color::rgb(220, 60, 60),
                None,
            );
        }
        InstallerPage::Progress => {
            let (label, percent) = match st.job.as_ref() {
                Some(job) => (job.stage_label(), job.progress_percent()),
                None => ("Preparing...", 0),
            };
            r.draw_text(x + 16, y + BODY_Y + 28, label, theme.text_primary, None);
            let bar = Rect::new(16, BODY_Y + 54, ww.saturating_sub(32), 18);
            r.draw_rect(x + bar.x, y + bar.y, bar.w, bar.h, theme.titlebar_active);
            let fill_w = (bar.w.saturating_sub(4)) as u64 * percent as u64 / 100;
            r.fill_rect(x + bar.x + 2, y + bar.y + 2, fill_w as u32, bar.h - 4, theme.accent);
            r.draw_text(
                x + 16,
                y + BODY_Y + 82,
                &format!("{}%", percent),
                theme.text_secondary,
                None,
            );
        }
        InstallerPage::Done => {
            r.draw_icon(x + 16, y + BODY_Y + 24, IconType::Check, Color::rgb(80, 200, 120));
            r.draw_text(x + 40, y + BODY_Y + 26, "Install complete.", theme.text_primary, None);
            let mut ly = y + BODY_Y + 50;
            for note in st.result_notes.iter().take(7) {
                r.draw_text(x + 16, ly, note, theme.text_secondary, None);
                ly += 18;
            }
            ly += 6;
            r.draw_text(x + 16, ly, "Restart to begin using your new disk.", theme.text_primary, None);
            // Этап 3: кнопка немедленной перезагрузки.
            draw_button(r, theme, x, y, done_reboot_rect(wh), "Restart now", true);
        }
        InstallerPage::Failed => {
            r.draw_text(x + 16, y + BODY_Y + 26, "Installation failed:", Color::rgb(220, 60, 60), None);
            let err = st.error.as_deref().unwrap_or("unknown error");
            // Длинная ошибка переносится по ширине окна.
            let chars_per_line = ((ww.saturating_sub(32)) / 8).max(16) as usize;
            let mut ly = y + BODY_Y + 46;
            let mut rest = err;
            for _ in 0..4 {
                if rest.is_empty() {
                    break;
                }
                let take = rest.len().min(chars_per_line);
                // Не рвём UTF-8 (ошибки у нас ASCII, но не полагаемся).
                let mut cut = take;
                while cut > 0 && !rest.is_char_boundary(cut) {
                    cut -= 1;
                }
                r.draw_text(x + 16, ly, &rest[..cut], theme.text_secondary, None);
                rest = &rest[cut..];
                ly += 18;
            }
            r.draw_text(x + 16, ly + 6, "The target disk may be unbootable. Try again.", theme.text_secondary, None);
        }
    }

    // Сообщение валидации текущей страницы.
    if let Some(note) = st.note.as_deref() {
        r.draw_text(x + 16, y + wh as i32 - 58, note, Color::rgb(230, 160, 60), None);
    }

    // Кнопки навигации.
    if has_back(st.page) {
        draw_button(r, theme, x, y, back_btn_rect(ww, wh), "Back", false);
    }
    if let Some(label) = next_label(st.page) {
        draw_button(r, theme, x, y, next_btn_rect(ww, wh), label, true);
    }
}
