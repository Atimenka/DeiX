//! Панель задач (Taskbar), Меню Пуск (Start Menu), Центр управления и курсор

use crate::mouse;
use crate::renderer::{Color, IconType, Renderer};
use crate::timer;
use crate::ui::desktop::Desktop;
use crate::ui::metrics::UiMetrics;
use crate::ui::theme::UiTheme;
use alloc::vec::Vec;

pub enum StartMenuAction {
    OpenTerminal,
    OpenFiles,
    OpenBrowser,
    OpenTaskManager,
    OpenThemeSettings,
    OpenAbout,
    OpenDisplaySettings,
    Restart,
    Shutdown,
}

pub fn start_menu_items() -> Vec<(&'static str, StartMenuAction, IconType)> {
    alloc::vec![
        ("Terminal", StartMenuAction::OpenTerminal, IconType::Terminal),
        ("Files Manager", StartMenuAction::OpenFiles, IconType::Files),
        ("Web Browser", StartMenuAction::OpenBrowser, IconType::Browser),
        ("Task Manager", StartMenuAction::OpenTaskManager, IconType::TaskManager),
        ("Personalization", StartMenuAction::OpenThemeSettings, IconType::Theme),
        ("Display Settings", StartMenuAction::OpenDisplaySettings, IconType::Display),
        ("About System", StartMenuAction::OpenAbout, IconType::About),
        ("Restart OS", StartMenuAction::Restart, IconType::Restart),
        ("Shutdown", StartMenuAction::Shutdown, IconType::Power),
    ]
}

pub fn draw_taskbar(r: &mut Renderer, theme: &UiTheme, desktop: &Desktop, screen_w: i32, screen_h: i32) {
    let ui_m = UiMetrics::fluent();
    let taskbar_h = ui_m.taskbar_height as i32;
    let start_w = ui_m.start_button_width;
    let item_w = ui_m.taskbar_item_width;
    let y = screen_h - taskbar_h;

    r.fill_rect_gradient_v(0, y, screen_w as u32, ui_m.taskbar_height, theme.titlebar_active, theme.bg_top);
    r.draw_hline(0, y, screen_w as u32, theme.accent);

    let start_bg = if desktop.start_menu_open { theme.accent } else { theme.titlebar_active };
    r.fill_rounded_rect(6, y + 5, (start_w - 12) as u32, ui_m.taskbar_height - 10, 6, start_bg);
    r.draw_text(24, y + (taskbar_h - 16) / 2, "Start", Color::WHITE, None);

    for (i, w) in desktop.windows.iter().enumerate() {
        let item_x = start_w + 8 + i as i32 * item_w;
        if item_x + item_w > screen_w - 120 {
            break;
        }
        let is_focused = desktop.focused_window == Some(i) && !w.minimized;
        let bg = if is_focused { theme.titlebar_active } else { theme.window_bg };

        r.fill_rounded_rect(item_x, y + 6, (item_w - 6) as u32, ui_m.taskbar_height - 12, 6, bg);

        if is_focused {
            r.draw_hline(item_x + 12, y + taskbar_h - 4, (item_w - 30) as u32, theme.accent);
        }

        let label_color = if w.minimized { Color::GRAY } else { theme.text_primary };
        r.draw_text(item_x + 8, y + (taskbar_h - 16) / 2, truncate(&w.title, 14), label_color, None);
    }

    r.draw_icon(screen_w - 110, y + 12, IconType::Wifi, theme.text_primary);
    r.draw_icon(screen_w - 85, y + 12, IconType::Volume, theme.text_primary);

    let secs = timer::uptime_ms() / 1000;
    let clock = alloc::format!("{:02}:{:02}", (secs / 60) % 100, secs % 60);
    r.draw_text(screen_w - 60, y + (taskbar_h - 16) / 2, &clock, theme.text_primary, None);
}

pub fn draw_start_menu(r: &mut Renderer, theme: &UiTheme, _screen_w: i32, screen_h: i32) {
    let ui_m = UiMetrics::fluent();
    let taskbar_y = screen_h - ui_m.taskbar_height as i32;
    let items = start_menu_items();
    let menu_w = 260i32;
    let menu_h = 240i32;
    let menu_y = taskbar_y - menu_h - 4;

    r.fill_rounded_rect(4, menu_y, menu_w as u32, menu_h as u32, theme.corner_radius, theme.window_bg);
    r.draw_rect_outline_alpha(4, menu_y, menu_w as u32, menu_h as u32, theme.titlebar_active, 180);

    r.fill_rounded_rect(12, menu_y + 10, (menu_w - 24) as u32, 24, 4, theme.titlebar_active);
    r.draw_icon(18, menu_y + 14, IconType::Search, theme.text_secondary);
    r.draw_text(38, menu_y + 14, "Search apps...", theme.text_secondary, None);

    let col_w = (menu_w - 24) / 2;
    for (i, (label, _, icon)) in items.iter().enumerate() {
        if i >= 6 { break; }
        let col = i % 2;
        let row = i / 2;
        let ix = 12 + col as i32 * col_w;
        let iy = menu_y + 44 + row as i32 * 36;

        r.fill_rounded_rect(ix, iy, (col_w - 4) as u32, 32, 4, theme.titlebar_inactive);
        r.draw_icon(ix + 6, iy + 8, *icon, theme.accent);
        r.draw_text(ix + 26, iy + 6, truncate(label, 9), theme.text_primary, None);
    }

    r.draw_hline(12, menu_y + menu_h - 36, (menu_w - 24) as u32, theme.titlebar_active);
    r.draw_text(16, menu_y + menu_h - 26, "root @ DeiX", theme.text_secondary, None);
    r.fill_rounded_rect(menu_w - 60, menu_y + menu_h - 30, 48, 22, 4, Color::RED);
    r.draw_text(menu_w - 54, menu_y + menu_h - 26, "OFF", Color::WHITE, None);
}

pub fn draw_control_center(r: &mut Renderer, theme: &UiTheme, screen_w: i32, screen_h: i32) {
    let ui_m = UiMetrics::fluent();
    let taskbar_y = screen_h - ui_m.taskbar_height as i32;
    let cc_w = 200i32;
    let cc_h = 160i32;
    let cc_x = screen_w - cc_w - 8;
    let cc_y = taskbar_y - cc_h - 4;

    r.fill_rounded_rect(cc_x, cc_y, cc_w as u32, cc_h as u32, theme.corner_radius, theme.window_bg);
    r.draw_rect_outline_alpha(cc_x, cc_y, cc_w as u32, cc_h as u32, theme.titlebar_active, 180);

    r.draw_text(cc_x + 12, cc_y + 10, "Control Center", theme.accent, None);
    r.draw_hline(cc_x + 12, cc_y + 32, (cc_w - 24) as u32, theme.titlebar_active);

    r.draw_icon(cc_x + 12, cc_y + 44, IconType::Wifi, Color::GREEN);
    r.draw_text(cc_x + 36, cc_y + 44, "eth0: 10.0.2.15", theme.text_primary, None);

    r.draw_icon(cc_x + 12, cc_y + 70, IconType::Volume, theme.accent);
    r.draw_text(cc_x + 36, cc_y + 70, "Audio: 80% [HDA]", theme.text_primary, None);

    r.draw_icon(cc_x + 12, cc_y + 96, IconType::Theme, Color::YELLOW);
    r.draw_text(cc_x + 36, cc_y + 96, "GFX.KMOD Active", theme.text_primary, None);

    r.fill_rounded_rect(cc_x + 12, cc_y + 124, (cc_w - 24) as u32, 24, 4, theme.titlebar_active);
    r.draw_text(cc_x + 32, cc_y + 128, "System Running", Color::GREEN, None);
}

pub fn draw_cursor(r: &mut Renderer) {
    let m = mouse::snapshot();
    let (x, y) = (m.x, m.y);

    let color = if m.left_button { Color::RED } else { Color::WHITE };
    let points: [(i32, i32); 7] = [
        (0, 0), (0, 14), (4, 11), (6, 16), (8, 15), (6, 10), (11, 10),
    ];
    for i in 0..points.len() {
        let (x0, y0) = points[i];
        let (x1, y1) = points[(i + 1) % points.len()];
        r.draw_line(x + x0, y + y0, x + x1, y + y1, Color::BLACK);
    }
    r.fill_rect(x + 1, y + 1, 4, 8, color);
}

fn truncate(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        s
    } else {
        let mut char_indices = s.char_indices();
        if let Some((idx, _)) = char_indices.nth(max_chars) {
            &s[..idx]
        } else {
            s
        }
    }
}
