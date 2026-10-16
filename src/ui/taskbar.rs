//! Панель задач (Taskbar), Меню Пуск (Start Menu), Центр управления и курсор.
//!
//! Геометрия меню Пуск и Центра управления описана один раз
//! (`start_menu_layout`, `control_center_rect`) и используется и для
//! отрисовки, и для обработки кликов в `desktop.rs`, и для damage-diff.

use crate::mouse;
use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::timer;
use crate::ui::desktop::Desktop;
use crate::ui::metrics::UiMetrics;
use crate::ui::theme::UiTheme;

#[derive(Clone, Copy)]
pub enum StartMenuAction {
    OpenTerminal,
    OpenFiles,
    OpenBrowser,
    OpenTaskManager,
    OpenErrorCenter,
    OpenThemeSettings,
    OpenAbout,
    OpenDisplaySettings,
}

/// Приложения меню Пуск (сетка 2 x 4). Restart/Shutdown — отдельные
/// кнопки в нижнем ряду меню (см. `StartMenuLayout`), не пункты сетки:
/// раньше массив содержал 10 пунктов, а рисовались только 8, и два
/// действия были доступны лишь кликом в невидимую область.
pub const START_MENU_APPS: [(&str, StartMenuAction, IconType); 8] = [
    ("Terminal", StartMenuAction::OpenTerminal, IconType::Terminal),
    ("Files", StartMenuAction::OpenFiles, IconType::Files),
    ("Browser", StartMenuAction::OpenBrowser, IconType::Browser),
    ("Task Mgr", StartMenuAction::OpenTaskManager, IconType::TaskManager),
    ("Errors", StartMenuAction::OpenErrorCenter, IconType::Settings),
    ("Personalize", StartMenuAction::OpenThemeSettings, IconType::Theme),
    ("Display", StartMenuAction::OpenDisplaySettings, IconType::Display),
    ("About", StartMenuAction::OpenAbout, IconType::About),
];

/// Все интерактивные области меню Пуск.
pub struct StartMenuLayout {
    pub menu: Rect,
    pub app_items: [Rect; START_MENU_APPS.len()],
    pub restart: Rect,
    pub shutdown: Rect,
}

pub fn start_menu_layout(screen_h: i32) -> StartMenuLayout {
    let m = UiMetrics::fluent();
    let taskbar_y = screen_h - m.taskbar_height as i32;
    let menu_w = 260i32;
    let menu_h = 200i32;
    let mx = 4i32;
    let my = taskbar_y - menu_h - 4;

    let col_w = (menu_w - 24) / 2;
    let mut app_items = [Rect::new(0, 0, 0, 0); START_MENU_APPS.len()];
    for (i, slot) in app_items.iter_mut().enumerate() {
        let col = (i % 2) as i32;
        let row = (i / 2) as i32;
        *slot = Rect::new(mx + 8 + col * col_w, my + 10 + row * 36, (col_w - 4) as u32, 32);
    }

    StartMenuLayout {
        menu: Rect::new(mx, my, menu_w as u32, menu_h as u32),
        app_items,
        restart: Rect::new(mx + menu_w - 156, my + menu_h - 30, 72, 22),
        shutdown: Rect::new(mx + menu_w - 78, my + menu_h - 30, 72, 22),
    }
}

/// Область Центра управления (для отрисовки, кликов и damage-diff).
pub fn control_center_rect(screen_w: i32, screen_h: i32) -> Rect {
    let m = UiMetrics::fluent();
    let taskbar_y = screen_h - m.taskbar_height as i32;
    let cc_w = 220i32;
    let cc_h = 150i32;
    Rect::new(screen_w - cc_w - 8, taskbar_y - cc_h - 4, cc_w as u32, cc_h as u32)
}

/// Строка часов панели: реальное время из CMOS RTC. Если RTC выдаёт
/// неправдоподобные значения, uptime показывается ЯВНО как uptime,
/// а не маскируется под время суток.
pub fn clock_text() -> alloc::string::String {
    if crate::rtc::is_plausible() {
        let t = crate::rtc::now();
        alloc::format!("{:02}:{:02}", t.hour, t.minute)
    } else {
        let mins = timer::uptime_ms() / 60_000;
        if mins < 100 {
            alloc::format!("up {}m", mins)
        } else {
            alloc::format!("up {}h", mins / 60)
        }
    }
}

/// Ключ смены показаний часов: пока он не изменился, панель не
/// перерисовывается ради времени (см. run_event_loop).
pub fn clock_key() -> u32 {
    if crate::rtc::is_plausible() {
        let t = crate::rtc::now();
        t.hour as u32 * 60 + t.minute as u32
    } else {
        0x8000_0000 | (timer::uptime_ms() / 60_000) as u32
    }
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

    // Сеть: иконка отражает реальное наличие устройства, а не всегда
    // «подключено». Детали — в Центре управления.
    let net_color = if crate::rtl8139::is_present() {
        theme.text_primary
    } else {
        Color::GRAY
    };
    r.draw_icon(screen_w - 110, y + 12, IconType::Wifi, net_color);
    r.draw_icon(screen_w - 85, y + 12, IconType::Volume, theme.text_primary);

    let clock = clock_text();
    r.draw_text(screen_w - 60, y + (taskbar_h - 16) / 2, &clock, theme.text_primary, None);
}

pub fn draw_start_menu(r: &mut Renderer, theme: &UiTheme, _screen_w: i32, screen_h: i32) {
    let l = start_menu_layout(screen_h);

    r.fill_rounded_rect(l.menu.x, l.menu.y, l.menu.w, l.menu.h, theme.corner_radius, theme.window_bg);
    r.draw_rect_outline_alpha(l.menu.x, l.menu.y, l.menu.w, l.menu.h, theme.titlebar_active, 180);

    for (i, (label, _, icon)) in START_MENU_APPS.iter().enumerate() {
        let item = l.app_items[i];
        r.fill_rounded_rect(item.x, item.y, item.w, item.h, 4, theme.titlebar_inactive);
        r.draw_icon(item.x + 6, item.y + 8, *icon, theme.accent);
        r.draw_text(item.x + 26, item.y + 6, truncate(label, 11), theme.text_primary, None);
    }

    let sep_y = l.menu.y + l.menu.h as i32 - 38;
    r.draw_hline(l.menu.x + 8, sep_y, l.menu.w - 16, theme.titlebar_active);
    r.draw_text(l.menu.x + 12, sep_y + 10, "root @ DeiX", theme.text_secondary, None);

    r.fill_rounded_rect(l.restart.x, l.restart.y, l.restart.w, l.restart.h, 4, theme.titlebar_active);
    r.draw_icon(l.restart.x + 4, l.restart.y + 3, IconType::Restart, theme.text_primary);
    r.draw_text(l.restart.x + 22, l.restart.y + 4, "Reboot", theme.text_primary, None);

    r.fill_rounded_rect(l.shutdown.x, l.shutdown.y, l.shutdown.w, l.shutdown.h, 4, Color::RED);
    r.draw_icon(l.shutdown.x + 4, l.shutdown.y + 3, IconType::Power, Color::WHITE);
    r.draw_text(l.shutdown.x + 22, l.shutdown.y + 4, "Power", Color::WHITE, None);
}

pub fn draw_control_center(r: &mut Renderer, theme: &UiTheme, screen_w: i32, screen_h: i32) {
    let cc = control_center_rect(screen_w, screen_h);
    let (cc_x, cc_y) = (cc.x, cc.y);

    r.fill_rounded_rect(cc_x, cc_y, cc.w, cc.h, theme.corner_radius, theme.window_bg);
    r.draw_rect_outline_alpha(cc_x, cc_y, cc.w, cc.h, theme.titlebar_active, 180);

    r.draw_text(cc_x + 12, cc_y + 10, "Control Center", theme.accent, None);
    r.draw_hline(cc_x + 12, cc_y + 32, cc.w - 24, theme.titlebar_active);

    // Сеть: реальное наличие устройства + настроенный IP. Зелёная иконка
    // не рисуется, когда карты нет.
    if crate::rtl8139::is_present() {
        let ip = crate::net::my_ip();
        r.draw_icon(cc_x + 12, cc_y + 44, IconType::Wifi, Color::GREEN);
        let line = alloc::format!("eth0: {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
        r.draw_text(cc_x + 36, cc_y + 44, &line, theme.text_primary, None);
    } else {
        r.draw_icon(cc_x + 12, cc_y + 44, IconType::Wifi, Color::GRAY);
        r.draw_text(cc_x + 36, cc_y + 44, "No network device", theme.text_secondary, None);
    }

    // Звук: реально инициализированный backend, без фиктивных процентов.
    let (audio_color, audio_line) = if crate::hda::is_ready() {
        (theme.accent, "Audio: Intel HDA ready")
    } else {
        (theme.text_secondary, "Audio: PC speaker only")
    };
    r.draw_icon(cc_x + 12, cc_y + 70, IconType::Volume, audio_color);
    r.draw_text(cc_x + 36, cc_y + 70, audio_line, theme.text_primary, None);

    // Дисплей: фактический режим framebuffer.
    let disp = alloc::format!("Display: {}x{} 32bpp", r.width(), r.height());
    r.draw_icon(cc_x + 12, cc_y + 96, IconType::Display, theme.text_secondary);
    r.draw_text(cc_x + 36, cc_y + 96, &disp, theme.text_primary, None);

    // Реальное время работы системы.
    let secs = timer::uptime_ms() / 1000;
    let up = alloc::format!("Uptime: {}m {:02}s", secs / 60, secs % 60);
    r.draw_text(cc_x + 36, cc_y + 122, &up, theme.text_secondary, None);
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
