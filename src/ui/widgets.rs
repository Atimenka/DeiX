//! Общие элементы интерфейса: кнопка действия с иконкой.

use crate::renderer::{Color, IconType, Renderer};
use crate::ui::text::text_cells;

/// Сторона глифа `Renderer::draw_icon`.
const ICON: i32 = 16;
/// Зазор между иконкой и подписью.
const GAP: i32 = 6;

/// Скруглённая кнопка: иконка и подпись по центру. Если обе не помещаются в
/// ширину кнопки, остаётся только подпись — текст важнее картинки.
pub fn icon_button(
    r: &mut Renderer,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    radius: i32,
    bg: Color,
    fg: Color,
    icon: IconType,
    label: &str,
) {
    r.fill_rounded_rect(x, y, w, h, radius, bg);
    let text_w = text_cells(label) as i32 * 8;
    let text_y = y + (h as i32 - 16) / 2;
    let full = ICON + GAP + text_w;
    if full + 8 <= w as i32 {
        let left = x + (w as i32 - full) / 2;
        r.draw_icon(left, y + (h as i32 - ICON) / 2, icon, fg);
        r.draw_text(left + ICON + GAP, text_y, label, fg, None);
    } else {
        r.draw_text(x + (w as i32 - text_w) / 2, text_y, label, fg, None);
    }
}

/// Иконка для подписи кнопки центра обновлений и установщика.
pub fn icon_for_label(label: &str) -> IconType {
    match label {
        "Back" => IconType::ArrowLeft,
        "Next" => IconType::ArrowRight,
        "Cancel" => IconType::Cross,
        "Discard" => IconType::Trash,
        "Download" => IconType::Download,
        "Check now" | "Check again" => IconType::Reload,
        "Restart now" => IconType::Restart,
        "Install and restart" | "Install" => IconType::Install,
        "Confirm and restart" => IconType::Check,
        _ => IconType::ArrowRight,
    }
}
