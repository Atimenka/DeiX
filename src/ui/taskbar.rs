//! Shell DeiX Aurora: верхняя панель, Dock, меню Пуск с поиском,
//! Центр управления, карточки Overview и курсор.
//!
//! Вся геометрия описана функциями-лэйаутами (`panel_*`, `dock_layout`,
//! `start_menu_layout`, `control_center_rect`, `overview_card_rect`) и
//! используется и для отрисовки, и для обработки кликов в `desktop.rs`,
//! и для damage-diff — видимая область и hitbox совпадают по построению.

use crate::mouse;
use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::timer;
use crate::ui::desktop::Desktop;
use crate::ui::metrics::UiMetrics;
use crate::ui::text::truncate_to_cells;
use crate::ui::theme::UiTheme;
use crate::ui::window::WindowContent;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StartMenuAction {
    OpenTerminal,
    OpenFiles,
    OpenBrowser,
    OpenTaskManager,
    OpenErrorCenter,
    OpenThemeSettings,
    OpenAbout,
    OpenDisplaySettings,
    OpenInstaller,
    OpenUpdates,
}

/// Приложения меню Пуск и закреплённая часть Dock.
pub const START_MENU_APPS: [(&str, StartMenuAction, IconType); 10] = [
    ("Terminal", StartMenuAction::OpenTerminal, IconType::Terminal),
    ("Files", StartMenuAction::OpenFiles, IconType::Files),
    ("Browser", StartMenuAction::OpenBrowser, IconType::Browser),
    ("Task Mgr", StartMenuAction::OpenTaskManager, IconType::TaskManager),
    ("Errors", StartMenuAction::OpenErrorCenter, IconType::Settings),
    ("Personalize", StartMenuAction::OpenThemeSettings, IconType::Theme),
    ("Display", StartMenuAction::OpenDisplaySettings, IconType::Display),
    ("Install DeiX", StartMenuAction::OpenInstaller, IconType::Install),
    ("Updates", StartMenuAction::OpenUpdates, IconType::Reload),
    ("About", StartMenuAction::OpenAbout, IconType::About),
];

/// Открытое окно соответствует пункту запуска? (точка «запущено» в Dock)
pub fn launch_matches(action: StartMenuAction, content: &WindowContent) -> bool {
    match (action, content) {
        (StartMenuAction::OpenTerminal, WindowContent::Terminal { .. }) => true,
        (StartMenuAction::OpenFiles, WindowContent::Files { .. }) => true,
        (StartMenuAction::OpenBrowser, WindowContent::WebBrowser { .. }) => true,
        (StartMenuAction::OpenTaskManager, WindowContent::TaskManager { .. }) => true,
        (StartMenuAction::OpenErrorCenter, WindowContent::ErrorCenter { .. }) => true,
        (StartMenuAction::OpenThemeSettings, WindowContent::ThemeSettings) => true,
        (StartMenuAction::OpenAbout, WindowContent::About) => true,
        (StartMenuAction::OpenDisplaySettings, WindowContent::DisplaySettings { .. }) => true,
        (StartMenuAction::OpenInstaller, WindowContent::Installer { .. }) => true,
        (StartMenuAction::OpenUpdates, WindowContent::Updates { .. }) => true,
        _ => false,
    }
}

// ---------------------------------------------------------------- панель

/// Полоса верхней панели.
pub fn panel_rect(screen_w: i32) -> Rect {
    let m = UiMetrics::fluent();
    Rect::new(0, 0, screen_w.max(0) as u32, m.top_panel_height as u32)
}

/// Кнопка «DeiX» слева — открывает меню Пуск.
pub fn panel_menu_button_rect() -> Rect {
    let m = UiMetrics::fluent();
    Rect::new(0, 0, 56, m.top_panel_height as u32)
}

/// Правый блок панели (сеть, звук, часы) — открывает Центр управления.
pub fn panel_status_rect(screen_w: i32) -> Rect {
    let m = UiMetrics::fluent();
    Rect::new(screen_w - 118, 0, 118, m.top_panel_height as u32)
}

/// Рабочая область экрана: между верхней панелью и Dock. Окна
/// размещаются, прижимаются и разворачиваются только внутри неё.
pub fn work_area(screen_w: i32, screen_h: i32) -> Rect {
    let m = UiMetrics::fluent();
    let top = m.top_panel_height;
    let bottom = screen_h - m.dock_height - m.dock_margin - 4;
    Rect::new(0, top, screen_w.max(0) as u32, (bottom - top).max(120) as u32)
}

pub fn draw_top_panel(r: &mut Renderer, theme: &UiTheme, desktop: &Desktop, screen_w: i32) {
    let p = panel_rect(screen_w);
    // Акриловая полоса: в Performance opacity форсируется в 255 и
    // уходит в быстрый путь без смешивания.
    r.fill_rect_alpha(p.x, p.y, p.w, p.h, theme.titlebar_active, theme.opacity);
    r.fill_rect_alpha(p.x, p.y + p.h as i32 - 1, p.w, 1, Color::BLACK, 90);

    // Кнопка меню — чип внутри той же hitbox-области panel_menu_button_rect.
    let btn = panel_menu_button_rect();
    let (chip_x, chip_y) = (btn.x + 4, btn.y + 3);
    let (chip_w, chip_h) = (btn.w - 8, btn.h - 6);
    if desktop.start_menu_open {
        r.fill_rounded_rect(chip_x, chip_y, chip_w, chip_h, 6, theme.accent);
    }
    let btn_fg = if desktop.start_menu_open { Color::WHITE } else { theme.accent };
    r.draw_text(btn.x + 12, btn.y + 4, "DeiX", btn_fg, None);

    // Заголовок активного окна — реальное состояние, не украшение.
    if let Some(title) = desktop.focused_title() {
        let t = truncate_to_cells(title, ((screen_w - 200) / 8).max(6) as usize).0;
        r.draw_text(64, 4, t, theme.text_secondary, None);
    }

    // Статус справа: сеть по факту наличия устройства, звук, часы.
    let net_color = if crate::rtl8139::is_present() {
        theme.text_primary
    } else {
        Color::GRAY
    };
    r.draw_icon(screen_w - 112, 4, IconType::Wifi, net_color);
    r.draw_icon(screen_w - 90, 4, IconType::Volume, theme.text_primary);
    let clock = clock_text();
    r.draw_text(screen_w - 60, 4, &clock, theme.text_primary, None);
}

// ---------------------------------------------------------------- Dock

#[derive(Clone, Copy)]
pub enum DockAction {
    /// Открыть/закрыть обзор окон.
    Overview,
    /// Запустить приложение (индекс в START_MENU_APPS).
    Launch(usize),
    /// Активировать/свернуть окно с данным id.
    Window(u32),
}

pub struct DockItem {
    pub rect: Rect,
    pub action: DockAction,
}

pub struct DockLayout {
    pub dock: Rect,
    pub items: Vec<DockItem>,
}

/// Раскладка Dock: [Overview] [8 приложений] | [открытые окна].
/// Высчитывается одинаково для отрисовки и кликов. Окна, не влезшие
/// по ширине, в Dock не показываются (доступны через Overview).
pub fn dock_layout(desktop: &Desktop, screen_w: i32, screen_h: i32) -> DockLayout {
    let m = UiMetrics::fluent();
    let step = m.dock_icon + m.dock_gap;
    let sep_w = 10i32;

    let fixed = 1 + START_MENU_APPS.len(); // Overview + приложения
    let max_total = (((screen_w - 16) - sep_w) / step).max(fixed as i32) as usize;
    let n_win = desktop.windows.len().min(max_total.saturating_sub(fixed));

    let mut width = m.dock_gap + (fixed as i32 + n_win as i32) * step;
    if n_win > 0 {
        width += sep_w;
    }

    let dock = Rect::new(
        (screen_w - width) / 2,
        screen_h - m.dock_height - m.dock_margin,
        width.max(step) as u32,
        m.dock_height as u32,
    );

    let mut items = Vec::with_capacity(fixed + n_win);
    let icon_y = dock.y + (m.dock_height - m.dock_icon) / 2;
    let mut x = dock.x + m.dock_gap;

    items.push(DockItem {
        rect: Rect::new(x, icon_y, m.dock_icon as u32, m.dock_icon as u32),
        action: DockAction::Overview,
    });
    x += step;

    for i in 0..START_MENU_APPS.len() {
        items.push(DockItem {
            rect: Rect::new(x, icon_y, m.dock_icon as u32, m.dock_icon as u32),
            action: DockAction::Launch(i),
        });
        x += step;
    }

    if n_win > 0 {
        x += sep_w;
        for w in desktop.windows.iter().take(n_win) {
            items.push(DockItem {
                rect: Rect::new(x, icon_y, m.dock_icon as u32, m.dock_icon as u32),
                action: DockAction::Window(w.id),
            });
            x += step;
        }
    }

    DockLayout { dock, items }
}

pub fn draw_dock(r: &mut Renderer, theme: &UiTheme, desktop: &Desktop, screen_w: i32, screen_h: i32) {
    let dl = dock_layout(desktop, screen_w, screen_h);
    let fx = crate::ui::effects::current();

    // Стеклянная подложка: blur обоев (только Quality и не во время
    // перетаскивания окна), мягкая тень, акрил и две кромки.
    if theme.enable_blur && fx.blur_enabled() && !desktop.is_interacting() {
        r.apply_blur_rect(dl.dock.x, dl.dock.y, dl.dock.w, dl.dock.h, 2);
    }
    if fx.shadows_enabled() {
        r.draw_drop_shadow(dl.dock.x, dl.dock.y, dl.dock.w, dl.dock.h, 3);
    }
    r.fill_rounded_rect_alpha(dl.dock.x, dl.dock.y, dl.dock.w, dl.dock.h, 14, theme.window_bg, theme.opacity);
    r.draw_rounded_rect_outline(dl.dock.x, dl.dock.y, dl.dock.w, dl.dock.h, 14, Color::BLACK, 120);
    r.draw_rounded_rect_outline(
        dl.dock.x + 1,
        dl.dock.y + 1,
        dl.dock.w - 2,
        dl.dock.h - 2,
        13,
        Color::WHITE,
        26,
    );

    let dot_y = dl.dock.y + dl.dock.h as i32 - 5;
    let mut seen_window_item = false;

    for (i, item) in dl.items.iter().enumerate() {
        let rc = item.rect;
        let hovered = desktop.dock_hover == Some(i);
        match item.action {
            DockAction::Overview => {
                if desktop.overview_open {
                    r.fill_rounded_rect(rc.x, rc.y, rc.w, rc.h, 10, theme.accent);
                } else if hovered {
                    r.fill_rounded_rect_alpha(rc.x, rc.y, rc.w, rc.h, 10, theme.surface_alt, 230);
                }
                let fg = if desktop.overview_open { Color::WHITE } else { theme.text_primary };
                r.draw_icon(rc.x + 10, rc.y + 10, IconType::Grid, fg);
            }
            DockAction::Launch(i) => {
                if hovered {
                    r.fill_rounded_rect_alpha(rc.x, rc.y, rc.w, rc.h, 10, theme.surface_alt, 230);
                }
                let fg = if hovered { theme.accent_hover } else { theme.accent };
                r.draw_icon(rc.x + 10, rc.y + 10, START_MENU_APPS[i].2, fg);
                // Точка «запущено» — только если такое окно реально открыто.
                let running = desktop
                    .windows
                    .iter()
                    .any(|w| launch_matches(START_MENU_APPS[i].1, &w.content));
                if running {
                    r.fill_circle(rc.x + rc.w as i32 / 2, dot_y, 2, theme.accent);
                }
            }
            DockAction::Window(id) => {
                if !seen_window_item {
                    seen_window_item = true;
                    // Разделитель между закреплёнными и открытыми окнами.
                    r.fill_rect_alpha(rc.x - 6, dl.dock.y + 10, 1, dl.dock.h - 20, Color::WHITE, 46);
                }
                if let Some(w) = desktop.windows.iter().find(|w| w.id == id) {
                    let focused = desktop.focused_id == Some(id);
                    if focused {
                        r.fill_rounded_rect_alpha(rc.x, rc.y, rc.w, rc.h, 10, theme.titlebar_active, 240);
                        r.draw_rounded_rect_outline(rc.x, rc.y, rc.w, rc.h, 10, theme.accent, 150);
                    } else if hovered {
                        r.fill_rounded_rect_alpha(rc.x, rc.y, rc.w, rc.h, 10, theme.surface_alt, 230);
                    }
                    let fg = if w.minimized { Color::GRAY } else { theme.text_primary };
                    r.draw_icon(rc.x + 10, rc.y + 10, w.content.icon(), fg);
                    if focused {
                        r.fill_rounded_rect(rc.x + 10, dot_y - 1, rc.w - 20, 3, 1, theme.accent);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------- меню Пуск

/// Все интерактивные области меню Пуск.
pub struct StartMenuLayout {
    pub menu: Rect,
    pub search: Rect,
    pub app_items: [Rect; START_MENU_APPS.len()],
    pub restart: Rect,
    pub shutdown: Rect,
}

/// Меню Пуск: поиск сверху, одна колонка приложений (как в концепте
/// Aurora), футер с пользователем и кнопками питания. Высота подобрана
/// так, чтобы меню не перекрывало Dock даже на 640x480.
pub fn start_menu_layout(screen_h: i32) -> StartMenuLayout {
    let m = UiMetrics::fluent();
    let menu_w = 288i32;
    let mx = 6i32;
    let my = m.top_panel_height + 6;
    // Шаг строки подбирается под высоту экрана: шапка (66) + список +
    // футер с кнопками питания (44) не должны ложиться на Dock даже на
    // 640x480. На больших экранах шаг — штатные 34 px.
    let n = START_MENU_APPS.len() as i32;
    let avail = screen_h - m.dock_height - m.dock_margin - 2 - my;
    let step = ((avail - 66 - 44) / n).clamp(26, 34);
    let menu_h = 66 + step * n + 44;

    let mut app_items = [Rect::new(0, 0, 0, 0); START_MENU_APPS.len()];
    for (i, slot) in app_items.iter_mut().enumerate() {
        *slot = Rect::new(
            mx + 12,
            my + 66 + i as i32 * step,
            (menu_w - 24) as u32,
            (step - 4) as u32,
        );
    }

    StartMenuLayout {
        menu: Rect::new(mx, my, menu_w as u32, menu_h as u32),
        search: Rect::new(mx + 12, my + 10, (menu_w - 24) as u32, 28),
        app_items,
        restart: Rect::new(mx + menu_w - 88, my + menu_h - 34, 36, 24),
        shutdown: Rect::new(mx + menu_w - 48, my + menu_h - 34, 36, 24),
    }
}

/// Индексы приложений, подходящих под поисковый запрос (регистр не
/// важен). Один и тот же порядок у отрисовки, кликов и Enter в поиске.
pub fn filtered_app_indices(query: &str, out: &mut [usize; START_MENU_APPS.len()]) -> usize {
    let mut n = 0;
    for (i, (label, _, _)) in START_MENU_APPS.iter().enumerate() {
        if query.is_empty() || contains_ascii_ci(label, query) {
            out[n] = i;
            n += 1;
        }
    }
    n
}

fn contains_ascii_ci(hay: &str, needle: &str) -> bool {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || n.len() > h.len() {
        return n.is_empty();
    }
    for start in 0..=(h.len() - n.len()) {
        if h[start..start + n.len()]
            .iter()
            .zip(n.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return true;
        }
    }
    false
}

pub fn draw_start_menu(r: &mut Renderer, theme: &UiTheme, desktop: &Desktop, screen_h: i32) {
    let l = start_menu_layout(screen_h);
    let fx = crate::ui::effects::current();

    // Акриловая панель: blur обоев (Quality), тень, стеклянные кромки.
    if theme.enable_blur && fx.blur_enabled() {
        r.apply_blur_rect(l.menu.x, l.menu.y, l.menu.w, l.menu.h, 2);
    }
    if fx.shadows_enabled() {
        r.draw_drop_shadow(l.menu.x, l.menu.y, l.menu.w, l.menu.h, 3);
    }
    r.fill_rounded_rect_alpha(l.menu.x, l.menu.y, l.menu.w, l.menu.h, 12, theme.window_bg, theme.opacity);
    r.draw_rounded_rect_outline(l.menu.x, l.menu.y, l.menu.w, l.menu.h, 12, Color::BLACK, 120);
    r.draw_rounded_rect_outline(l.menu.x + 1, l.menu.y + 1, l.menu.w - 2, l.menu.h - 2, 11, Color::WHITE, 26);

    // Поиск — реальное поле: ввод идёт с клавиатуры, Enter запускает
    // выбранный пункт.
    r.fill_rounded_rect(l.search.x, l.search.y, l.search.w, l.search.h, 13, theme.titlebar_inactive);
    r.draw_rounded_rect_outline(l.search.x, l.search.y, l.search.w, l.search.h, 13, Color::WHITE, 20);
    r.draw_icon(l.search.x + 8, l.search.y + 6, IconType::Search, theme.text_secondary);
    if desktop.menu_search.is_empty() {
        r.draw_text(l.search.x + 30, l.search.y + 6, "Search applications...", theme.text_secondary, None);
    } else {
        let q = truncate_to_cells(&desktop.menu_search, 26).0;
        r.draw_text(l.search.x + 30, l.search.y + 6, q, theme.text_primary, None);
        r.draw_text(
            l.search.x + 30 + crate::ui::text::text_cells(q) as i32 * 8,
            l.search.y + 6,
            "_",
            theme.accent,
            None,
        );
    }

    r.draw_text(l.menu.x + 14, l.menu.y + 48, "APPLICATIONS", theme.text_secondary, None);

    let mut idx = [0usize; START_MENU_APPS.len()];
    let n = filtered_app_indices(&desktop.menu_search, &mut idx);
    for k in 0..n {
        let (label, _, icon) = START_MENU_APPS[idx[k]];
        let item = l.app_items[k];
        // Пункт, выбранный стрелками или наведением (Enter запустит
        // именно его), подсвечен акцентной рамкой со «свечением».
        let is_sel = n > 0 && k == desktop.menu_sel.min(n - 1);
        if is_sel {
            r.fill_rounded_rect_alpha(item.x, item.y, item.w, item.h, 8, theme.surface_alt, 235);
            r.draw_rounded_rect_outline(item.x, item.y, item.w, item.h, 8, theme.accent, 235);
            r.draw_rounded_rect_outline(item.x - 1, item.y - 1, item.w + 2, item.h + 2, 9, theme.accent, 80);
            r.draw_rounded_rect_outline(item.x - 2, item.y - 2, item.w + 4, item.h + 4, 10, theme.accent, 36);
        }
        // Иконка в собственном чипе, подпись — без обрезки до 1 колонки.
        r.fill_rounded_rect(item.x + 5, item.y + 4, 22, 22, 6, theme.surface);
        r.draw_icon(item.x + 8, item.y + 7, icon, theme.accent);
        let fg = if is_sel { theme.text_primary } else { theme.text_secondary };
        let max_cells = ((item.w as i32 - 44) / 8).max(4) as usize;
        r.draw_text(item.x + 36, item.y + 7, truncate_to_cells(label, max_cells).0, fg, None);
    }
    if n == 0 {
        r.draw_text(l.menu.x + 14, l.menu.y + 72, "No matching applications", theme.text_secondary, None);
    }

    // Футер: пользователь и питание.
    let sep_y = l.menu.y + l.menu.h as i32 - 44;
    r.fill_rect_alpha(l.menu.x + 10, sep_y, l.menu.w - 20, 1, Color::WHITE, 30);
    let (av_x, av_y) = (l.menu.x + 24, sep_y + 22);
    r.fill_circle(av_x, av_y, 10, theme.accent);
    r.draw_text(av_x - 4, av_y - 8, "r", Color::WHITE, None);
    r.draw_text(av_x + 18, av_y - 8, "root @ DeiX", theme.text_secondary, None);

    // Компактные чипы питания: перезагрузка и выключение.
    r.fill_rounded_rect(l.restart.x, l.restart.y, l.restart.w, l.restart.h, 8, theme.titlebar_active);
    r.draw_rounded_rect_outline(l.restart.x, l.restart.y, l.restart.w, l.restart.h, 8, Color::WHITE, 20);
    r.draw_icon(l.restart.x + 10, l.restart.y + 4, IconType::Restart, theme.text_primary);

    r.fill_rounded_rect(l.shutdown.x, l.shutdown.y, l.shutdown.w, l.shutdown.h, 8, Color::RED);
    r.draw_icon(l.shutdown.x + 10, l.shutdown.y + 4, IconType::Power, Color::WHITE);
}

// --------------------------------------------------- Центр управления

/// Область Центра управления (под правым краем верхней панели).
pub fn control_center_rect(screen_w: i32, _screen_h: i32) -> Rect {
    let m = UiMetrics::fluent();
    Rect::new(screen_w - 228, m.top_panel_height + 4, 220, 150)
}

/// Строка часов: реальное время из CMOS RTC. Если RTC неправдоподобен,
/// uptime показывается ЯВНО как uptime, а не маскируется под время суток.
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

pub fn draw_control_center(r: &mut Renderer, theme: &UiTheme, screen_w: i32, screen_h: i32) {
    let cc = control_center_rect(screen_w, screen_h);
    let (cc_x, cc_y) = (cc.x, cc.y);

    let fx = crate::ui::effects::current();
    if theme.enable_blur && fx.blur_enabled() {
        r.apply_blur_rect(cc_x, cc_y, cc.w, cc.h, 2);
    }
    if fx.shadows_enabled() {
        r.draw_drop_shadow(cc_x, cc_y, cc.w, cc.h, 3);
    }
    r.fill_rounded_rect_alpha(cc_x, cc_y, cc.w, cc.h, 12, theme.window_bg, theme.opacity);
    r.draw_rounded_rect_outline(cc_x, cc_y, cc.w, cc.h, 12, Color::BLACK, 120);
    r.draw_rounded_rect_outline(cc_x + 1, cc_y + 1, cc.w - 2, cc.h - 2, 11, Color::WHITE, 26);

    r.draw_text(cc_x + 12, cc_y + 10, "Control Center", theme.accent, None);
    r.draw_hline(cc_x + 12, cc_y + 32, cc.w - 24, theme.titlebar_active);

    // Сеть: реальное наличие устройства + настроенный IP.
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

// ----------------------------------------------------------- Overview

/// Карточка окна `i` из `n` в режиме обзора: сетка до 3 колонок,
/// отцентрована в рабочей области.
pub fn overview_card_rect(i: usize, n: usize, screen_w: i32, screen_h: i32) -> Rect {
    let wa = work_area(screen_w, screen_h);
    let n = n.max(1);
    let cols = n.min(3);
    let rows = n.div_ceil(cols);

    let card_w = (((wa.w as i32 - 40) / cols as i32) - 16).clamp(120, 220);
    let card_h = 84i32;
    let grid_w = cols as i32 * (card_w + 16) - 16;
    let grid_h = rows as i32 * (card_h + 16) - 16;
    let x0 = wa.x + (wa.w as i32 - grid_w) / 2;
    let y0 = wa.y + ((wa.h as i32 - grid_h) / 2).max(8);

    let col = (i % cols) as i32;
    let row = (i / cols) as i32;
    Rect::new(
        x0 + col * (card_w + 16),
        y0 + row * (card_h + 16),
        card_w as u32,
        card_h as u32,
    )
}

pub fn draw_overview(r: &mut Renderer, theme: &UiTheme, desktop: &Desktop, screen_w: i32, screen_h: i32) {
    let wa = work_area(screen_w, screen_h);
    r.shade_rect(0, 0, screen_w as u32, screen_h as u32, 110);

    let n = desktop.windows.len();
    if n == 0 {
        r.draw_text(
            wa.x + (wa.w as i32 - 15 * 8) / 2,
            wa.y + wa.h as i32 / 2,
            "No open windows",
            theme.text_primary,
            None,
        );
        return;
    }

    for (i, w) in desktop.windows.iter().enumerate() {
        let card = overview_card_rect(i, n, screen_w, screen_h);
        r.fill_rounded_rect_alpha(card.x, card.y, card.w, card.h, 10, theme.window_bg, 242);
        if desktop.focused_id == Some(w.id) {
            r.draw_rounded_rect_outline(card.x, card.y, card.w, card.h, 10, theme.accent, 235);
            r.draw_rounded_rect_outline(card.x - 1, card.y - 1, card.w + 2, card.h + 2, 11, theme.accent, 80);
        } else {
            r.draw_rounded_rect_outline(card.x, card.y, card.w, card.h, 10, Color::WHITE, 30);
        }

        r.draw_icon(card.x + 10, card.y + 10, w.content.icon(), theme.accent);
        let title = truncate_to_cells(&w.title, ((card.w as i32 - 40) / 8).max(4) as usize).0;
        r.draw_text(card.x + 32, card.y + 12, title, theme.text_primary, None);
        if w.minimized {
            r.draw_text(card.x + 10, card.y + card.h as i32 - 22, "minimized", Color::GRAY, None);
        }
    }
}

// ------------------------------------------------------------- курсор

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
