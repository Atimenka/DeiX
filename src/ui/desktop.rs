//! Диспетчер рабочего стола DeiX OS (Compositor & Desktop Manager)

use crate::ext2;
use crate::keyboard;
use crate::mouse;
use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::timer;
use crate::ui::apps::files as files_ui;
use crate::ui::apps::settings as settings_ui;
use crate::ui::metrics::UiMetrics;
use crate::ui::surface::WallpaperSurface;
use crate::ui::text as text_util;
use crate::ui::taskbar::{
    clock_key, control_center_rect, dock_layout, draw_control_center, draw_cursor, draw_dock,
    draw_overview, draw_start_menu, draw_top_panel, filtered_app_indices, launch_matches,
    overview_card_rect, panel_menu_button_rect, panel_rect, panel_status_rect, start_menu_layout,
    work_area, DockAction, StartMenuAction, START_MENU_APPS,
};
use crate::ui::theme::{get_theme, set_theme, UiTheme};
use crate::ui::window::{
    draw_window, load_partition_entries, maximized_geometry,
    BrowserTab, Window, WindowContent, WindowFrame, MIN_WINDOW_H, MIN_WINDOW_W,
    RESOLUTION_PRESETS,
};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub struct DesktopShortcut {
    pub name: &'static str,
    pub icon: IconType,
    pub action: DesktopAction,
}

pub enum DesktopAction {
    OpenTerminal,
    OpenFiles,
    OpenBrowser,
    OpenTaskManager,
    OpenThemeSettings,
}

pub fn desktop_shortcuts() -> Vec<DesktopShortcut> {
    alloc::vec![
        DesktopShortcut {
            name: "Terminal",
            icon: IconType::Terminal,
            action: DesktopAction::OpenTerminal,
        },
        DesktopShortcut {
            name: "Files",
            icon: IconType::Files,
            action: DesktopAction::OpenFiles,
        },
        DesktopShortcut {
            name: "Browser",
            icon: IconType::Browser,
            action: DesktopAction::OpenBrowser,
        },
        DesktopShortcut {
            name: "Task Mgr",
            icon: IconType::TaskManager,
            action: DesktopAction::OpenTaskManager,
        },
        DesktopShortcut {
            name: "Personalize",
            icon: IconType::Theme,
            action: DesktopAction::OpenThemeSettings,
        },
    ]
}

pub enum DesktopExit {
    Quit,
    ChangeResolution(u32, u32),
}

pub struct Desktop {
    /// Окна в порядке z (последнее — верхнее). Индексы меняются при
    /// каждой смене порядка, поэтому всё долгоживущее (фокус, drag,
    /// resize) привязано к Window::id, а не к индексу.
    pub windows: Vec<Window>,
    /// id окна в фокусе.
    pub focused_id: Option<u32>,
    pub start_menu_open: bool,
    /// Поисковый запрос меню Пуск (набирается с клавиатуры).
    pub menu_search: String,
    /// Выбранный стрелками пункт меню (индекс в отфильтрованном списке).
    pub menu_sel: usize,
    /// Пункт Dock под курсором (индекс в DockLayout::items); hover
    /// перерисовывает сцену только в момент смены пункта.
    pub dock_hover: Option<usize>,
    pub control_center_open: bool,
    /// Режим обзора окон (Overview).
    pub overview_open: bool,
    /// Состояние Overview в прошлом кадре — для damage на весь экран
    /// при открытии/закрытии (обзор затемняет всю сцену).
    prev_overview_open: bool,
    /// Область Dock прошлого кадра: ширина Dock зависит от числа окон,
    /// при его сжатии освободившуюся полосу надо перерисовать.
    prev_dock_rect: Option<Rect>,
    pub selected_icon: Option<usize>,
    /// (id, исходный x, исходный y, x мыши при захвате, y мыши при захвате)
    pub dragging_window: Option<(u32, i32, i32, i32, i32)>,
    /// (id, x мыши при захвате, y мыши при захвате, исходная ширина, исходная высота)
    pub resizing_window: Option<(u32, i32, i32, u32, u32)>,
    /// Следующий свободный Window::id (монотонный счётчик).
    next_window_id: u32,
    pub should_exit: bool,
    pub pending_resolution: Option<(u32, u32)>,
    pub frame_counter: u64,
    pub wallpaper_cache: WallpaperSurface,
    pub dirty: bool,
    pub last_cursor_pos: (i32, i32),
    /// Видимые области окон ПРОШЛОГО кадра (с полями под тень/blur).
    /// Diff с текущим кадром даёт damage для областей, освободившихся
    /// после закрытия/перетаскивания/сворачивания окна — без него обои
    /// там не восстанавливались и оставались артефакты.
    prev_window_rects: Vec<crate::renderer::Rect>,
    /// Область меню Пуск прошлого кадра (если было открыто).
    prev_start_menu_rect: Option<crate::renderer::Rect>,
    /// Область Центра управления прошлого кадра (если был открыт).
    prev_control_center_rect: Option<crate::renderer::Rect>,
    /// Курсор сдвинулся, но сцена не менялась — быстрый путь без
    /// полной перерисовки (save-under, см. update_cursor_only).
    cursor_moved: bool,
    /// Состояние левой кнопки в прошлом кадре: клики обрабатываются
    /// по фронту нажатия, а не каждый кадр, пока кнопка зажата.
    prev_left_button: bool,
    /// Пиксели back buffer под курсором (сохраняются перед его отрисовкой).
    cursor_saved_pixels: Vec<u32>,
    /// Фактическая (обрезанная по экрану) область сохранённых пикселей.
    cursor_saved_rect: crate::renderer::Rect,
    cursor_saved_valid: bool,
    /// Ключ последних показанных часов (час*60+минута или минуты uptime):
    /// панель перерисовывается при его смене, а не каждую секунду.
    last_clock_key: u32,
    /// Когда в следующий раз проверять часы (RTC читается ~раз в секунду,
    /// а не в каждом проходе цикла событий).
    next_clock_check_ms: u64,
    /// Последнее увиденное поколение обоев (theme::wallpaper_epoch):
    /// при смене — кеш обоев сброшен (обои скачаны браузером/CLI).
    wallpaper_epoch_seen: u32,
}

/// Запас вокруг окна под мягкую тень (4 px), blur-ореол (4 px) и
/// смещённую подложку тени (+4..+6 px вправо/вниз у draw_window).
const WINDOW_VISUAL_MARGIN: i32 = 12;

/// Ширина и высота области курсора (глиф 11x16 + запас на линии контура).
const CURSOR_W: u32 = 16;
const CURSOR_H: u32 = 20;

/// Полная видимая область окна: рамка + заголовок + тень + blur.
fn window_visual_rect(w: &Window, titlebar_h: i32) -> crate::renderer::Rect {
    let total_h = w.height as i32 + titlebar_h;
    crate::renderer::Rect::new(
        w.x - WINDOW_VISUAL_MARGIN,
        w.y - WINDOW_VISUAL_MARGIN,
        (w.width as i32 + WINDOW_VISUAL_MARGIN * 2).max(0) as u32,
        (total_h + WINDOW_VISUAL_MARGIN * 2).max(0) as u32,
    )
}

impl Desktop {
    pub fn new() -> Self {
        Desktop {
            windows: Vec::new(),
            focused_id: None,
            next_window_id: 1,
            start_menu_open: false,
            menu_search: String::new(),
            menu_sel: 0,
            dock_hover: None,
            control_center_open: false,
            overview_open: false,
            prev_overview_open: false,
            prev_dock_rect: None,
            selected_icon: None,
            dragging_window: None,
            resizing_window: None,
            should_exit: false,
            pending_resolution: None,
            frame_counter: 0,
            wallpaper_cache: WallpaperSurface::new(800, 600),
            dirty: true,
            last_cursor_pos: (0, 0),
            prev_window_rects: Vec::new(),
            prev_start_menu_rect: None,
            prev_control_center_rect: None,
            cursor_moved: false,
            prev_left_button: false,
            cursor_saved_pixels: Vec::new(),
            cursor_saved_rect: crate::renderer::Rect::new(0, 0, 0, 0),
            cursor_saved_valid: false,
            last_clock_key: u32::MAX,
            next_clock_check_ms: 0,
            wallpaper_epoch_seen: crate::ui::theme::wallpaper_epoch(),
        }
    }

    /// Размещение нового окна: каскадное смещение от числа открытых окон,
    /// затем жёсткая привязка к доступной области экрана. Окно не может
    /// открыться за пределами экрана или под панелью задач — даже на
    /// 640x480 заголовок и кнопки остаются доступными.
    fn place_new_window(&self, dw: u32, dh: u32, screen_w: i32, screen_h: i32) -> (i32, i32, u32, u32) {
        let cascade = (self.windows.len() % 5) as i32 * 26;
        let x = (screen_w - dw as i32) / 2 - 52 + cascade;
        let y = (screen_h - dh as i32) / 2 - 40 + cascade;
        clamp_window_geometry(x, y, dw, dh, screen_w, screen_h)
    }

    /// Общий путь открытия окна: разместить, подогнать под экран,
    /// сфокусировать.
    fn open_placed(
        &mut self,
        dw: u32,
        dh: u32,
        screen_w: i32,
        screen_h: i32,
        make: impl FnOnce(i32, i32) -> Window,
    ) {
        let (x, y, w, h) = self.place_new_window(dw, dh, screen_w, screen_h);
        let mut win = make(x, y);
        win.width = w;
        win.height = h;
        win.restore_geometry = (x, y, w, h);
        self.push_focused(win);
        self.start_menu_open = false;
        self.overview_open = false;
    }

    /// Единственный путь добавления окна: выдаёт стабильный id,
    /// кладёт окно наверх z-порядка и фокусирует.
    fn push_focused(&mut self, mut win: Window) {
        win.id = self.next_window_id;
        self.next_window_id += 1;
        self.focused_id = Some(win.id);
        self.windows.push(win);
        self.dirty = true;
    }

    /// Текущий индекс окна с данным id (меняется при смене z-порядка).
    fn index_of(&self, id: u32) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    /// Индекс окна в фокусе.
    fn focused_index(&self) -> Option<usize> {
        self.focused_id.and_then(|id| self.index_of(id))
    }

    /// Окно по индексу в фокусе? (для отрисовки Dock и рамок)
    pub fn is_focused(&self, idx: usize) -> bool {
        match (self.focused_id, self.windows.get(idx)) {
            (Some(id), Some(w)) => w.id == id,
            _ => false,
        }
    }

    /// Заголовок активного окна — для верхней панели.
    pub fn focused_title(&self) -> Option<&str> {
        self.focused_index().map(|i| self.windows[i].title.as_str())
    }

    pub fn open_terminal(&mut self, screen_w: i32, screen_h: i32) {
        self.open_placed(380, 240, screen_w, screen_h, Window::new_terminal);
    }

    pub fn open_files(&mut self, screen_w: i32, screen_h: i32) {
        self.open_placed(520, 340, screen_w, screen_h, Window::new_files);
    }

    pub fn open_browser(&mut self, screen_w: i32, screen_h: i32) {
        self.open_placed(560, 360, screen_w, screen_h, |x, y| {
            Window::new_browser(x, y, "deix://home")
        });
    }

    pub fn open_task_manager(&mut self, screen_w: i32, screen_h: i32) {
        self.open_placed(500, 320, screen_w, screen_h, Window::new_task_manager);
    }

    pub fn open_error_center(&mut self, screen_w: i32, screen_h: i32) {
        self.open_placed(520, 360, screen_w, screen_h, Window::new_error_center);
    }

    pub fn open_theme_settings(&mut self, screen_w: i32, screen_h: i32) {
        self.open_placed(480, 460, screen_w, screen_h, Window::new_theme_settings);
    }

    pub fn open_about(&mut self, screen_w: i32, screen_h: i32) {
        self.open_placed(380, 220, screen_w, screen_h, Window::new_about);
    }

    pub fn open_display_settings(&mut self, screen_w: i32, screen_h: i32) {
        let dh = RESOLUTION_PRESETS.len() as u32 * 28 + 40;
        self.open_placed(240, dh, screen_w, screen_h, |x, y| {
            let mut w = Window::new_display_settings(x, 0, screen_h as u32);
            w.y = y;
            w
        });
    }

    /// Закрытие по кнопке окна. Редактор с несохранёнными изменениями
    /// не закрывается молча: открывается диалог Save / Discard / Cancel,
    /// решение принимает пользователь (см. клики по кнопкам диалога).
    pub fn request_close_window(&mut self, idx: usize) {
        if idx >= self.windows.len() {
            return;
        }
        if let WindowContent::FileEditor {
            modified: true,
            read_only: false,
            confirm_close,
            ..
        } = &mut self.windows[idx].content
        {
            *confirm_close = true;
            self.dirty = true;
            return;
        }
        self.close_window(idx);
    }

    pub fn close_window(&mut self, idx: usize) {
        if idx < self.windows.len() {
            let closed = self.windows.remove(idx);
            self.dirty = true;
            if self.focused_id == Some(closed.id) {
                self.focus_topmost_visible();
            }
        }
    }

    /// Передаёт фокус верхнему не свёрнутому окну (после закрытия или
    /// сворачивания активного). Если таких нет — фокус никому.
    fn focus_topmost_visible(&mut self) {
        self.focused_id = self
            .windows
            .iter()
            .rev()
            .find(|w| !w.minimized)
            .map(|w| w.id);
    }

    pub fn focus_window(&mut self, idx: usize) {
        if idx < self.windows.len() {
            let win = self.windows.remove(idx);
            self.focused_id = Some(win.id);
            self.windows.push(win);
            self.dirty = true;
        }
    }

    pub fn toggle_minimize(&mut self, idx: usize) {
        if idx < self.windows.len() {
            self.windows[idx].minimized = !self.windows[idx].minimized;
            self.dirty = true;
            if self.windows[idx].minimized {
                if self.focused_id == Some(self.windows[idx].id) {
                    self.focus_topmost_visible();
                }
            } else {
                self.focus_window(idx);
            }
        }
    }

    /// Снап при отпускании перетаскивания у края экрана: верхний край —
    /// развернуть, левый/правый — половина рабочей области. Окно должно
    /// быть реально дотащено до края (курсор в 2px от него).
    fn snap_on_drag_release(
        &mut self,
        id: u32,
        orig_x: i32,
        orig_y: i32,
        mx: i32,
        my: i32,
        screen_w: i32,
        screen_h: i32,
    ) {
        let m = UiMetrics::fluent();
        let wa = work_area(screen_w, screen_h);
        let work_h = (wa.h as i32 - m.titlebar_height).max(MIN_WINDOW_H as i32) as u32;

        let Some(idx) = self.index_of(id) else { return };
        let w = &mut self.windows[idx];
        if w.maximized {
            return;
        }

        if my <= m.top_panel_height {
            // Дотащили до верхней панели — развернуть; Restore вернёт
            // окно туда, откуда его тащили.
            w.restore_geometry = (orig_x, orig_y, w.width, w.height);
            let (gx, gy, gw, gh) = maximized_geometry(screen_w, screen_h, &m);
            w.x = gx;
            w.y = gy;
            w.width = gw;
            w.height = gh;
            w.maximized = true;
        } else if mx <= 1 {
            w.x = 0;
            w.y = wa.y;
            w.width = (screen_w / 2).max(MIN_WINDOW_W as i32) as u32;
            w.height = work_h;
        } else if mx >= screen_w - 2 {
            let half = (screen_w / 2).max(MIN_WINDOW_W as i32);
            w.x = screen_w - half;
            w.y = wa.y;
            w.width = half as u32;
            w.height = work_h;
        }
    }

    pub fn toggle_maximize(&mut self, idx: usize, screen_w: i32, screen_h: i32) {
        let m = UiMetrics::fluent();
        if idx < self.windows.len() {
            let w = &mut self.windows[idx];
            self.dirty = true;
            if w.maximized {
                // Возврат к сохранённой геометрии, обрезанной по текущему
                // экрану (разрешение могло смениться, пока окно было
                // развёрнуто).
                let (rx, ry, rw, rh) = w.restore_geometry;
                let (cx, cy, cw, ch) = clamp_window_geometry(rx, ry, rw, rh, screen_w, screen_h);
                w.x = cx;
                w.y = cy;
                w.width = cw;
                w.height = ch;
                w.maximized = false;
            } else {
                w.restore_geometry = (w.x, w.y, w.width, w.height);
                let (mx, my, mw, mh) = maximized_geometry(screen_w, screen_h, &m);
                w.x = mx;
                w.y = my;
                w.width = mw;
                w.height = mh;
                w.maximized = true;
            }
        }
    }

    /// Идёт ли сейчас перетаскивание или изменение размера окна —
    /// на это время дорогие эффекты (blur под Dock) отключаются,
    /// как и blur самих окон в draw_window.
    pub fn is_interacting(&self) -> bool {
        self.dragging_window.is_some() || self.resizing_window.is_some()
    }

    /// Подсветка под курсором: пункт Dock и пункт меню Пуск. Сцена
    /// перерисовывается только при смене пункта, а не на каждый пиксель
    /// движения мыши.
    fn update_hover(&mut self, x: i32, y: i32, screen_w: i32, screen_h: i32) {
        // Вне горизонтальной полосы Dock раскладка (с её Vec) не
        // считается вовсе — обычное движение мыши по рабочей области
        // не делает аллокаций.
        let m = UiMetrics::fluent();
        let dock_top = screen_h - m.dock_height - m.dock_margin;
        let mut hover = None;
        if y >= dock_top && y < dock_top + m.dock_height {
            let dl = dock_layout(self, screen_w, screen_h);
            if dl.dock.contains(x, y) {
                for (i, item) in dl.items.iter().enumerate() {
                    if item.rect.contains_pad(x, y, 3) {
                        hover = Some(i);
                        break;
                    }
                }
            }
        }
        if hover != self.dock_hover {
            self.dock_hover = hover;
            self.dirty = true;
        }

        // Наведение в меню Пуск двигает тот же выбор, что и стрелки:
        // Enter всегда запускает подсвеченный пункт.
        if self.start_menu_open {
            let l = start_menu_layout(screen_h);
            let mut idx = [0usize; START_MENU_APPS.len()];
            let n = filtered_app_indices(&self.menu_search, &mut idx);
            for k in 0..n {
                if l.app_items[k].contains(x, y) {
                    if self.menu_sel != k {
                        self.menu_sel = k;
                        self.dirty = true;
                    }
                    break;
                }
            }
        }
    }

    pub fn handle_input(&mut self, screen_w: i32, screen_h: i32) {
        let m = mouse::snapshot();
        let ui_m = UiMetrics::fluent();
        let wa = work_area(screen_w, screen_h);
        let wa_bottom = wa.y + wa.h as i32;

        if m.x != self.last_cursor_pos.0 || m.y != self.last_cursor_pos.1 {
            // Движение мыши само по себе сцену не меняет: вместо полной
            // перерисовки (все окна + blur + панель на каждый пиксель
            // движения) курсор обновляется быстрым путём save-under
            // в run_event_loop (update_cursor_only).
            self.last_cursor_pos = (m.x, m.y);
            self.cursor_moved = true;
            self.update_hover(m.x, m.y, screen_w, screen_h);
        }

        let pressed = m.left_button && !self.prev_left_button;
        self.prev_left_button = m.left_button;

        if m.left_button {
            // Перетаскивание/изменение размера продолжается, пока кнопка
            // зажата; сцена перерисовывается только при реальном сдвиге.
            if let Some((id, orig_x, orig_y, mx0, my0)) = self.dragging_window {
                if let Some(idx) = self.index_of(id) {
                    let w = &mut self.windows[idx];
                    if !w.maximized {
                        // Заголовок всегда остаётся достижим: не выше
                        // рабочей области, не под Dock, и минимум 60px
                        // окна видно по горизонтали.
                        let nx = (orig_x + (m.x - mx0))
                            .clamp(-(w.width as i32) + 60, screen_w - 60);
                        let ny = (orig_y + (m.y - my0))
                            .clamp(wa.y, wa_bottom - ui_m.titlebar_height);
                        if w.x != nx || w.y != ny {
                            w.x = nx;
                            w.y = ny;
                            self.dirty = true;
                        }
                    }
                }
                return;
            }

            if let Some((id, mx0, my0, orig_w, orig_h)) = self.resizing_window {
                if let Some(idx) = self.index_of(id) {
                    let w = &mut self.windows[idx];
                    if !w.maximized {
                        let new_w =
                            (orig_w as i32 + (m.x - mx0)).max(MIN_WINDOW_W as i32) as u32;
                        let new_h =
                            (orig_h as i32 + (m.y - my0)).max(MIN_WINDOW_H as i32) as u32;
                        if w.width != new_w || w.height != new_h {
                            w.width = new_w;
                            w.height = new_h;
                            self.dirty = true;
                        }
                    }
                }
                return;
            }

            // Клик обрабатывается один раз — по фронту нажатия. Удержание
            // кнопки не должно каждый кадр заново «нажимать» интерфейс
            // и гонять полную перерисовку сцены.
            if !pressed {
                return;
            }
            self.dirty = true;

            // Overview поверх всего: клик по карточке активирует окно,
            // мимо карточек — закрывает обзор. Вниз клик не проваливается.
            if self.overview_open {
                let n = self.windows.len();
                for i in 0..n {
                    if overview_card_rect(i, n, screen_w, screen_h).contains(m.x, m.y) {
                        if self.windows[i].minimized {
                            self.windows[i].minimized = false;
                        }
                        self.focus_window(i);
                        break;
                    }
                }
                self.overview_open = false;
                return;
            }

            // Открытое меню Пуск: клик внутри обрабатывается меню, клик
            // вне — только закрывает его. Клик не «проваливается» сквозь
            // меню на окна и рабочий стол.
            if self.start_menu_open {
                let l = start_menu_layout(screen_h);
                if l.menu.contains(m.x, m.y) {
                    self.handle_start_menu_click(m.x, m.y, screen_w, screen_h);
                } else {
                    // Клик мимо меню (включая повторный клик по кнопке
                    // DeiX — toggle) — закрыть, без «проваливания» вниз.
                    self.close_start_menu();
                }
                return;
            }

            // Центр управления: информационная панель; клик внутри не
            // проваливается вниз, клик вне — закрывает.
            if self.control_center_open {
                let cc = control_center_rect(screen_w, screen_h);
                if !cc.contains(m.x, m.y) {
                    self.control_center_open = false;
                }
                return;
            }

            // Верхняя панель.
            if panel_rect(screen_w).contains(m.x, m.y) {
                if panel_menu_button_rect().contains(m.x, m.y) {
                    self.open_start_menu();
                } else if panel_status_rect(screen_w).contains(m.x, m.y) {
                    self.control_center_open = true;
                }
                return;
            }

            // Dock рисуется поверх окон — и кликается раньше них.
            let dl = dock_layout(self, screen_w, screen_h);
            if dl.dock.contains(m.x, m.y) {
                for item in dl.items.iter() {
                    if item.rect.contains(m.x, m.y) {
                        let action = item.action;
                        self.handle_dock_action(action, screen_w, screen_h);
                        break;
                    }
                }
                return;
            }

            for i in (0..self.windows.len()).rev() {
                if self.windows[i].minimized {
                    continue;
                }

                let win_maximized = self.windows[i].maximized;
                let f = WindowFrame::of(&self.windows[i], &ui_m);
                let pad = WindowFrame::BTN_HIT_PAD;

                if f.close.contains_pad(m.x, m.y, pad) {
                    self.request_close_window(i);
                    return;
                }
                if f.maximize.contains_pad(m.x, m.y, pad) {
                    self.toggle_maximize(i, screen_w, screen_h);
                    return;
                }
                if f.minimize.contains_pad(m.x, m.y, pad) {
                    self.toggle_minimize(i);
                    return;
                }
                if f.resize.contains(m.x, m.y) && !win_maximized {
                    let (id, win_w, win_h) =
                        (self.windows[i].id, self.windows[i].width, self.windows[i].height);
                    self.focus_window(i);
                    self.resizing_window = Some((id, m.x, m.y, win_w, win_h));
                    return;
                }
                if f.drag.contains(m.x, m.y) {
                    let (id, cur_x, cur_y) =
                        (self.windows[i].id, self.windows[i].x, self.windows[i].y);
                    self.focus_window(i);
                    self.dragging_window = Some((id, cur_x, cur_y, m.x, m.y));
                    return;
                }
                if f.titlebar.contains(m.x, m.y) {
                    // Заголовок рядом с кнопками: фокус без перетаскивания,
                    // чтобы промах мимо Close не таскал окно.
                    self.focus_window(i);
                    return;
                }

                if f.outer.contains(m.x, m.y) {
                    self.focus_window(i);
                    self.handle_window_content_click(self.windows.len() - 1, m.x, m.y, screen_w, screen_h);
                    return;
                }
            }

            self.handle_desktop_click(m.x, m.y, screen_w, screen_h);
        } else {
            // Конец перетаскивания/изменения размера: один полный кадр,
            // чтобы вернуть blur (на время drag он отключён ради скорости).
            if let Some((id, orig_x, orig_y, mx0, my0)) = self.dragging_window.take() {
                // Снап — только если окно реально тащили: клик по
                // заголовку без движения не должен разворачивать окно,
                // даже когда оно стоит у края экрана.
                if (m.x, m.y) != (mx0, my0) {
                    self.snap_on_drag_release(id, orig_x, orig_y, m.x, m.y, screen_w, screen_h);
                }
                self.dirty = true;
            }
            if self.resizing_window.take().is_some() {
                self.dirty = true;
            }
        }

        // При открытом меню Пуск клавиатура идёт в строку поиска,
        // а не в приложение под меню.
        if self.start_menu_open {
            self.handle_menu_search_keys(screen_w, screen_h);
        } else {
            self.handle_keyboard_for_focused_window();
        }
    }

    fn handle_keyboard_for_focused_window(&mut self) {
        if let Some(focused_idx) = self.focused_index() {
            if let Some(w) = self.windows.get_mut(focused_idx) {
                if w.minimized {
                    return;
                }
                // Высота окна нужна редактору для прокрутки вслед за
                // курсором; берётся до заёма содержимого.
                let win_h = w.height;
                match &mut w.content {
                    WindowContent::Terminal { lines, current_line } => {
                        while let Some(byte) = keyboard::try_read_char() {
                            self.dirty = true;
                            match byte {
                                b'\n' => {
                                    let cmd = current_line.clone();
                                    lines.push(format!("> {}", cmd));
                                    run_mini_terminal_command(&cmd, lines);
                                    current_line.clear();
                                }
                                0x08 => {
                                    current_line.pop();
                                }
                                b if b >= 0x20 && b < 0x7F => {
                                    current_line.push(b as char);
                                }
                                _ => {}
                            }
                        }
                    }
                    WindowContent::WebBrowser {
                        tabs,
                        active_tab,
                        address_input,
                        status_msg,
                        ..
                    } => {
                        let visible = crate::ui::apps::browser::visible_lines(win_h);
                        while let Some(byte) = keyboard::try_read_char() {
                            self.dirty = true;
                            match byte {
                                b'\n' => {
                                    let new_url = address_input.clone();
                                    if let Some(tab) = tabs.get_mut(*active_tab) {
                                        tab.navigate(&new_url, true);
                                        *address_input = tab.url.clone();
                                    }
                                    *status_msg = Some(format!("Loaded {}", new_url));
                                }
                                0x08 => {
                                    address_input.pop();
                                }
                                // Стрелки/прокрутка страницы: вверх-вниз по
                                // строкам, влево-вправо — Back/Forward.
                                keyboard::ARROW_UP => {
                                    if let Some(tab) = tabs.get_mut(*active_tab) {
                                        tab.scroll = tab.scroll.saturating_sub(3);
                                    }
                                }
                                keyboard::ARROW_DOWN => {
                                    if let Some(tab) = tabs.get_mut(*active_tab) {
                                        let max = tab.content.len().saturating_sub(visible / 2);
                                        tab.scroll = (tab.scroll + 3).min(max);
                                    }
                                }
                                keyboard::ARROW_LEFT => {
                                    if let Some(tab) = tabs.get_mut(*active_tab) {
                                        if tab.go_back() {
                                            *address_input = tab.url.clone();
                                            *status_msg = Some(String::from("Back"));
                                        }
                                    }
                                }
                                keyboard::ARROW_RIGHT => {
                                    if let Some(tab) = tabs.get_mut(*active_tab) {
                                        if tab.go_forward() {
                                            *address_input = tab.url.clone();
                                            *status_msg = Some(String::from("Forward"));
                                        }
                                    }
                                }
                                b if b >= 0x20 && b < 0x7F => {
                                    address_input.push(b as char);
                                }
                                _ => {}
                            }
                        }
                    }
                    WindowContent::Files {
                        search_query,
                        current_partition,
                        current_path,
                        entries,
                        error,
                        ..
                    } => {
                        let mut changed = false;
                        while let Some(byte) = keyboard::try_read_char() {
                            self.dirty = true;
                            match byte {
                                0x08 => {
                                    search_query.pop();
                                    changed = true;
                                }
                                b if b >= 0x20 && b < 0x7F => {
                                    search_query.push(b as char);
                                    changed = true;
                                }
                                _ => {}
                            }
                        }
                        if changed {
                            let (all_entries, err) =
                                load_partition_entries(current_partition, current_path);
                            *error = err;
                            if search_query.is_empty() {
                                *entries = all_entries;
                            } else {
                                let query_lower = search_query.to_lowercase();
                                *entries = all_entries
                                    .into_iter()
                                    .filter(|e| e.name.to_lowercase().contains(&query_lower))
                                    .collect();
                            }
                        }
                    }
                    WindowContent::FileEditor {
                        filename: _,
                        full_path: _,
                        lines,
                        cursor_row,
                        cursor_col,
                        scroll,
                        modified,
                        read_only,
                        confirm_close,
                        status_msg: _,
                        ..
                    } => {
                        if *confirm_close {
                            // Диалог закрытия модален: ввод в текст не
                            // идёт, накопленные нажатия сбрасываются.
                            while keyboard::try_read_char().is_some() {}
                            return;
                        }
                        // Видимых строк столько же, сколько рисует
                        // draw_file_editor (шаг 18, поля 60).
                        let visible_lines =
                            ((win_h as i32 - 60) / 18).max(1) as usize;
                        while let Some(byte) = keyboard::try_read_char() {
                            self.dirty = true;
                            match byte {
                                keyboard::ARROW_UP => {
                                    *cursor_row = cursor_row.saturating_sub(1);
                                    if *cursor_row < *scroll {
                                        *scroll = *cursor_row;
                                    }
                                    let max_col = lines
                                        .get(*cursor_row)
                                        .map(|l| l.chars().count())
                                        .unwrap_or(0);
                                    *cursor_col = (*cursor_col).min(max_col);
                                }
                                keyboard::ARROW_DOWN => {
                                    if *cursor_row + 1 < lines.len() {
                                        *cursor_row += 1;
                                    }
                                    // Прокрутка следует за курсором вниз,
                                    // как и вверх.
                                    if *cursor_row >= *scroll + visible_lines {
                                        *scroll = *cursor_row + 1 - visible_lines;
                                    }
                                    let max_col = lines
                                        .get(*cursor_row)
                                        .map(|l| l.chars().count())
                                        .unwrap_or(0);
                                    *cursor_col = (*cursor_col).min(max_col);
                                }
                                keyboard::ARROW_LEFT => {
                                    *cursor_col = cursor_col.saturating_sub(1);
                                }
                                keyboard::ARROW_RIGHT => {
                                    if let Some(line) = lines.get(*cursor_row) {
                                        if *cursor_col < line.chars().count() {
                                            *cursor_col += 1;
                                        }
                                    }
                                }
                                // cursor_col — номер СИМВОЛА. String::remove/
                                // insert/split_off принимают байтовые индексы,
                                // и байтовый индекс внутри UTF-8-последовательности
                                // (кириллица в файле) — это паника ядра, поэтому
                                // границу даёт char_byte_index.
                                0x08 => {
                                    if !*read_only && *cursor_col > 0 {
                                        if let Some(line) = lines.get_mut(*cursor_row) {
                                            if *cursor_col <= line.chars().count() {
                                                let bi = text_util::char_byte_index(line, *cursor_col - 1);
                                                line.remove(bi);
                                                *cursor_col -= 1;
                                                *modified = true;
                                            }
                                        }
                                    }
                                }
                                b'\n' | b'\r' => {
                                    if !*read_only {
                                        if let Some(line) = lines.get_mut(*cursor_row) {
                                            let bi = text_util::char_byte_index(line, *cursor_col);
                                            let rest = line.split_off(bi);
                                            lines.insert(*cursor_row + 1, rest);
                                            *cursor_row += 1;
                                            *cursor_col = 0;
                                            *modified = true;
                                            if *cursor_row >= *scroll + visible_lines {
                                                *scroll = *cursor_row + 1 - visible_lines;
                                            }
                                        }
                                    }
                                }
                                b if b >= 0x20 && b < 0x7F => {
                                    if !*read_only {
                                        if let Some(line) = lines.get_mut(*cursor_row) {
                                            let bi = text_util::char_byte_index(line, *cursor_col);
                                            line.insert(bi, b as char);
                                            *cursor_col += 1;
                                            *modified = true;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {
                        while keyboard::try_read_char().is_some() {}
                    }
                }
            }
        } else {
            while keyboard::try_read_char().is_some() {}
        }
    }

    /// Закрывает меню Пуск, очищая поисковый запрос.
    fn close_start_menu(&mut self) {
        self.start_menu_open = false;
        self.menu_search.clear();
        self.menu_sel = 0;
    }

    /// Открывает меню Пуск с чистым поиском (кнопка панели / Win).
    fn open_start_menu(&mut self) {
        self.start_menu_open = true;
        self.menu_search.clear();
        self.menu_sel = 0;
        self.control_center_open = false;
        self.overview_open = false;
    }

    /// Запускает приложение из меню/Dock. Открытие окна закрывает меню.
    fn launch_app(&mut self, action: StartMenuAction, screen_w: i32, screen_h: i32) {
        match action {
            StartMenuAction::OpenTerminal => self.open_terminal(screen_w, screen_h),
            StartMenuAction::OpenFiles => self.open_files(screen_w, screen_h),
            StartMenuAction::OpenBrowser => self.open_browser(screen_w, screen_h),
            StartMenuAction::OpenTaskManager => self.open_task_manager(screen_w, screen_h),
            StartMenuAction::OpenErrorCenter => self.open_error_center(screen_w, screen_h),
            StartMenuAction::OpenThemeSettings => self.open_theme_settings(screen_w, screen_h),
            StartMenuAction::OpenAbout => self.open_about(screen_w, screen_h),
            StartMenuAction::OpenDisplaySettings => self.open_display_settings(screen_w, screen_h),
        }
        self.menu_search.clear();
    }

    fn handle_start_menu_click(&mut self, x: i32, y: i32, screen_w: i32, screen_h: i32) {
        // Геометрия кликов — ровно те же прямоугольники, по которым
        // draw_start_menu рисует пункты; порядок отфильтрованных
        // приложений — тот же filtered_app_indices, что и у отрисовки.
        let l = start_menu_layout(screen_h);

        if l.search.contains(x, y) {
            return; // ввод и так идёт в поле поиска
        }

        let mut idx = [0usize; START_MENU_APPS.len()];
        let n = filtered_app_indices(&self.menu_search, &mut idx);
        for k in 0..n {
            if l.app_items[k].contains(x, y) {
                self.launch_app(START_MENU_APPS[idx[k]].1, screen_w, screen_h);
                return;
            }
        }

        if l.restart.contains(x, y) {
            crate::println!("  [sys] Rebooting system...");
            unsafe { crate::port::outb(0x64, 0xFE) };
            return;
        }
        if l.shutdown.contains(x, y) {
            crate::println!("  [sys] System shutdown.");
            unsafe { crate::port::outw(0x604, 0x2000) };
            self.should_exit = true;
        }
    }

    /// Клик по элементу Dock.
    fn handle_dock_action(&mut self, action: DockAction, screen_w: i32, screen_h: i32) {
        self.dirty = true;
        match action {
            DockAction::Overview => {
                self.overview_open = !self.overview_open;
                self.close_start_menu();
                self.control_center_open = false;
            }
            DockAction::Launch(i) => {
                let act = START_MENU_APPS[i].1;
                // Если окно такого приложения уже открыто — активировать
                // верхнее из них (и восстановить, если свёрнуто), а не
                // плодить копии; новое окно всегда можно открыть из меню.
                let existing = self
                    .windows
                    .iter()
                    .rposition(|w| launch_matches(act, &w.content));
                match existing {
                    Some(idx) => {
                        if self.windows[idx].minimized {
                            self.windows[idx].minimized = false;
                        }
                        self.focus_window(idx);
                    }
                    None => self.launch_app(act, screen_w, screen_h),
                }
            }
            DockAction::Window(id) => {
                if let Some(idx) = self.index_of(id) {
                    // Свёрнутое — восстановить; неактивное — активировать;
                    // активное — свернуть.
                    if self.windows[idx].minimized {
                        self.toggle_minimize(idx);
                    } else if !self.is_focused(idx) {
                        self.focus_window(idx);
                    } else {
                        self.toggle_minimize(idx);
                    }
                }
            }
        }
    }

    /// Клавиатура при открытом меню Пуск: набор уходит в строку поиска,
    /// стрелки двигают выбор по отфильтрованным пунктам, Enter запускает
    /// выбранный, Backspace стирает.
    fn handle_menu_search_keys(&mut self, screen_w: i32, screen_h: i32) {
        while let Some(b) = keyboard::try_read_char() {
            self.dirty = true;
            let mut idx = [0usize; START_MENU_APPS.len()];
            let n = filtered_app_indices(&self.menu_search, &mut idx);
            match b {
                0x08 => {
                    self.menu_search.pop();
                    self.menu_sel = 0;
                }
                b'\n' | b'\r' => {
                    if n > 0 {
                        let k = self.menu_sel.min(n - 1);
                        self.launch_app(START_MENU_APPS[idx[k]].1, screen_w, screen_h);
                    }
                    return;
                }
                keyboard::ARROW_DOWN | keyboard::ARROW_RIGHT => {
                    if n > 0 {
                        self.menu_sel = (self.menu_sel + 1) % n;
                    }
                }
                keyboard::ARROW_UP | keyboard::ARROW_LEFT => {
                    if n > 0 {
                        self.menu_sel = self.menu_sel.min(n - 1).checked_sub(1).unwrap_or(n - 1);
                    }
                }
                b if b >= 0x20 && b < 0x7F => {
                    if self.menu_search.chars().count() < 24 {
                        self.menu_search.push(b as char);
                        self.menu_sel = 0;
                    }
                }
                _ => {}
            }
        }
    }



    fn handle_desktop_click(&mut self, x: i32, y: i32, screen_w: i32, screen_h: i32) {
        let shortcuts = desktop_shortcuts();
        for (idx, _) in shortcuts.iter().enumerate() {
            // Та же область, что подсветка в draw_desktop_icons:
            // (ix-6, iy-4, 64, 60).
            let ix = 20;
            let iy = 20 + idx as i32 * 70;
            if x >= ix - 6 && x < ix - 6 + 64 && y >= iy - 4 && y < iy - 4 + 60 {
                self.selected_icon = Some(idx);
                self.dirty = true;
                match shortcuts[idx].action {
                    DesktopAction::OpenTerminal => self.open_terminal(screen_w, screen_h),
                    DesktopAction::OpenFiles => self.open_files(screen_w, screen_h),
                    DesktopAction::OpenBrowser => self.open_browser(screen_w, screen_h),
                    DesktopAction::OpenTaskManager => self.open_task_manager(screen_w, screen_h),
                    DesktopAction::OpenThemeSettings => self.open_theme_settings(screen_w, screen_h),
                }
                return;
            }
        }
        self.selected_icon = None;
    }

    fn handle_window_content_click(
        &mut self,
        win_idx: usize,
        mx: i32,
        my: i32,
        screen_w: i32,
        screen_h: i32,
    ) {
        let ui_m = UiMetrics::fluent();
        let content = WindowFrame::of(&self.windows[win_idx], &ui_m).content;
        let win = &mut self.windows[win_idx];
        let rel_x = mx - content.x;
        let rel_y = my - content.y;

        if rel_y < 0 {
            return;
        }

        // Запрос на открытие редактора: окно нельзя добавить, пока
        // `win` держит заём на список окон, поэтому арм кладёт параметры
        // сюда, а push происходит после match.
        let mut open_editor: Option<(i32, i32, String, String, bool)> = None;
        // Аналогично для закрытия текущего окна из диалога редактора.
        let mut close_after = false;

        match &mut win.content {
            WindowContent::TaskManager {
                selected_pid,
                status_msg,
            } => {
                // Таблица задач: строки начинаются на 90 px ниже контента
                // (заголовок + метрики + шапка таблицы), шаг 20 px —
                // ровно так их кладёт draw_task_manager.
                let rows_start = 90;
                let tasks = crate::sched::list();
                if rel_y >= rows_start {
                    let row = ((rel_y - rows_start) / 20) as usize;
                    if let Some(t) = tasks.get(row) {
                        *selected_pid = Some(t.id);
                        self.dirty = true;
                    }
                }

                if rel_x >= win.width as i32 - 110
                    && rel_x <= win.width as i32 - 10
                    && rel_y >= win.height as i32 - 32
                    && rel_y <= win.height as i32 - 8
                {
                    self.dirty = true;
                    match *selected_pid {
                        Some(task_id) => {
                            match tasks.iter().find(|t| t.id == task_id) {
                                Some(t) if t.pid == 0 => {
                                    // Задача ядра: у неё нет процесса,
                                    // завершать её из UI запрещено.
                                    *status_msg = Some(format!(
                                        "'{}' is a kernel task and cannot be terminated.",
                                        t.name
                                    ));
                                }
                                Some(t) => match crate::process::kill(t.pid, -9) {
                                    Ok(()) => {
                                        *status_msg =
                                            Some(format!("Process {} (pid {}) terminated.", t.name, t.pid));
                                        *selected_pid = None;
                                    }
                                    Err(e) => {
                                        *status_msg = Some(e.message());
                                    }
                                },
                                None => {
                                    *status_msg = Some(String::from("Task already exited."));
                                    *selected_pid = None;
                                }
                            }
                        }
                        None => {
                            *status_msg = Some(String::from("Select a process to terminate."));
                        }
                    }
                }
            }
            WindowContent::ErrorCenter { selected, scroll } => {
                use crate::ui::apps::error_center::{LIST_ROWS, LIST_START, ROW_H};
                let records = crate::ui::apps::error_center::visible_records();
                if rel_y >= LIST_START {
                    let row = ((rel_y - LIST_START) / (ROW_H + 2)) as usize;
                    if row < LIST_ROWS {
                        let idx = *scroll + row;
                        if idx < records.len() {
                            *selected = Some(idx);
                            self.dirty = true;
                        }
                    }
                }
                // Шапка списка листает: левая половина — вверх, правая — вниз.
                if rel_y >= LIST_START - 22 && rel_y < LIST_START {
                    if rel_x < win.width as i32 / 2 {
                        *scroll = scroll.saturating_sub(LIST_ROWS);
                    } else if *scroll + LIST_ROWS < records.len() {
                        *scroll += LIST_ROWS;
                    }
                    self.dirty = true;
                }
            }
            WindowContent::ThemeSettings => {
                // Hitbox = те же Rect, по которым settings.rs рисует
                // карточки/образцы. Невидимых активных областей нет.
                let current = get_theme();

                for (i, (_, preset)) in settings_ui::THEME_CHOICES.iter().enumerate() {
                    if settings_ui::theme_card_rect(i).contains(rel_x, rel_y) {
                        set_theme(settings_ui::theme_for_preset(*preset));
                        self.wallpaper_cache.invalidate();
                        self.dirty = true;
                        crate::ui::config::save();
                        return;
                    }
                }

                for (i, (cr, cg, cb)) in settings_ui::ACCENT_CHOICES.iter().enumerate() {
                    if settings_ui::accent_rect(i).contains(rel_x, rel_y) {
                        let mut updated = current;
                        updated.accent = Color::rgb(*cr, *cg, *cb);
                        set_theme(updated);
                        // Обои могут использовать акцент — кеш устарел.
                        self.wallpaper_cache.invalidate();
                        self.dirty = true;
                        crate::ui::config::save();
                        return;
                    }
                }

                for i in 0..crate::ui::theme::WALLPAPER_STYLES.len() {
                    if settings_ui::wallpaper_rect(i).contains(rel_x, rel_y) {
                        let mut updated = current;
                        updated.wallpaper_style = i as u8;
                        set_theme(updated);
                        self.wallpaper_cache.invalidate();
                        self.dirty = true;
                        crate::ui::config::save();
                        return;
                    }
                }

                for (i, (_, profile)) in settings_ui::PROFILE_CHOICES.iter().enumerate() {
                    if settings_ui::profile_rect(i).contains(rel_x, rel_y) {
                        crate::ui::effects::set(*profile);
                        // Тени/blur меняются у всех окон сразу — сцену
                        // нужно собрать заново целиком (инвалидация кеша
                        // обоев даёт полноэкранный damage).
                        self.wallpaper_cache.invalidate();
                        self.dirty = true;
                        crate::ui::config::save();
                        return;
                    }
                }
            }
            WindowContent::Files {
                current_partition,
                current_path,
                entries,
                selected_idx,
                search_query: _,
                error,
                status_msg,
            } => {
                if rel_x < files_ui::SIDEBAR_W {
                    // Боковая панель Files: два реальных раздела, hitbox =
                    // прямоугольники, по которым они нарисованы.
                    let partitions = ["/userdata", "/system"];
                    for (i, p) in partitions.iter().enumerate() {
                        if files_ui::sidebar_item_rect(i).contains(rel_x, rel_y) {
                            *current_partition = String::from(*p);
                            *current_path = String::from("/");
                            let (new_entries, err) =
                                load_partition_entries(current_partition, current_path);
                            *entries = new_entries;
                            *selected_idx = None;
                            *error = err;
                            *status_msg = None;
                            self.dirty = true;
                            break;
                        }
                    }
                    return;
                }

                let writable = current_partition == "/userdata";
                if writable && files_ui::new_file_btn_rect(win.width).contains(rel_x, rel_y) {
                    // Свободное имя вместо повторяющихся по modulo uptime:
                    // существующий файл не перезаписывается.
                    match free_name(entries, "NEW_FILE_", ".TXT") {
                        Some(fname) => {
                            let path = join_fs_path(current_path, &fname);
                            match ext2::write_file_path(&path, b"New file created in DeiX Files.\r\n") {
                                Ok(()) => *status_msg = Some(format!("Created {}", fname)),
                                Err(e) => {
                                    *status_msg = None;
                                    *error = Some(format!("Create failed: {:?}", e));
                                }
                            }
                        }
                        None => {
                            *status_msg = None;
                            *error = Some(String::from("Create failed: too many NEW_FILE_* here"));
                        }
                    }
                    let (new_entries, err) = load_partition_entries(current_partition, current_path);
                    *entries = new_entries;
                    if err.is_some() {
                        *error = err;
                    }
                    self.dirty = true;
                    return;
                }
                if writable && files_ui::new_folder_btn_rect(win.width).contains(rel_x, rel_y) {
                    match free_name(entries, "FOLDER_", "") {
                        Some(dname) => {
                            let path = join_fs_path(current_path, &dname);
                            match ext2::mkdir(&path) {
                                Ok(_) => *status_msg = Some(format!("Created folder {}", dname)),
                                Err(e) => {
                                    *status_msg = None;
                                    *error = Some(format!("Create failed: {:?}", e));
                                }
                            }
                        }
                        None => {
                            *status_msg = None;
                            *error = Some(String::from("Create failed: too many FOLDER_* here"));
                        }
                    }
                    let (new_entries, err) = load_partition_entries(current_partition, current_path);
                    *entries = new_entries;
                    if err.is_some() {
                        *error = err;
                    }
                    self.dirty = true;
                    return;
                }

                // Хлебные крошки: переход в любой сегмент текущего пути.
                for (rect, _, target) in
                    files_ui::breadcrumbs(current_partition, current_path, win.width)
                {
                    if rect.contains(rel_x, rel_y) {
                        let (new_entries, err) =
                            load_partition_entries(current_partition, &target);
                        match err {
                            Some(e) => *error = Some(e),
                            None => {
                                *current_path = target;
                                *entries = new_entries;
                                *selected_idx = None;
                                *error = None;
                                *status_msg = None;
                            }
                        }
                        self.dirty = true;
                        return;
                    }
                }

                if rel_y >= files_ui::LIST_START_Y {
                    let idx = ((rel_y - files_ui::LIST_START_Y) / files_ui::ROW_H) as usize;
                    if idx < entries.len() {
                        // Повторный клик по уже выбранному элементу:
                        // каталог — переход внутрь, файл — редактор.
                        // /system смонтирован только для чтения.
                        if *selected_idx == Some(idx) {
                            let name = entries[idx].name.clone();
                            if entries[idx].is_dir {
                                if current_partition == "/userdata" {
                                    let new_path = match name.as_str() {
                                        "." => current_path.clone(),
                                        ".." => parent_path(current_path),
                                        _ => join_fs_path(current_path, &name),
                                    };
                                    let (new_entries, err) =
                                        load_partition_entries(current_partition, &new_path);
                                    match err {
                                        Some(e) => *error = Some(e),
                                        None => {
                                            *current_path = new_path;
                                            *entries = new_entries;
                                            *selected_idx = None;
                                            *error = None;
                                            *status_msg = None;
                                        }
                                    }
                                }
                                // /system: readdir поддерживает только
                                // корень раздела — вход внутрь не
                                // изображается работающим.
                            } else {
                                let base = current_partition.trim_end_matches('/');
                                let dir = current_path.trim_matches('/');
                                let full = if dir.is_empty() {
                                    format!("{}/{}", base, name)
                                } else {
                                    format!("{}/{}/{}", base, dir, name)
                                };
                                let read_only = current_partition != "/userdata";
                                open_editor = Some((win.x + 28, win.y + 28, name, full, read_only));
                            }
                        } else {
                            *selected_idx = Some(idx);
                        }
                        self.dirty = true;
                    }
                }
            }
            WindowContent::WebBrowser {
                tabs,
                active_tab,
                address_input,
                bookmarks,
                status_msg,
            } => {
                use crate::ui::apps::browser as browser_ui;
                if rel_y <= 24 {
                    // Hitbox = те же Rect, по которым draw_browser рисует
                    // вкладки и кнопку «+».
                    if browser_ui::new_tab_btn_rect(win.width).contains(rel_x, rel_y) {
                        tabs.push(BrowserTab::open("deix://home"));
                        *active_tab = tabs.len() - 1;
                        *address_input = String::from("deix://home");
                        self.dirty = true;
                        return;
                    }
                    for i in 0..browser_ui::visible_tabs(win.width, tabs.len()) {
                        if !browser_ui::tab_rect(i).contains(rel_x, rel_y) {
                            continue;
                        }
                        // «×» активной вкладки: закрытие (последняя вкладка
                        // не закрывается, а возвращается на домашнюю).
                        if i == *active_tab
                            && browser_ui::tab_close_rect(i).contains(rel_x, rel_y)
                        {
                            if tabs.len() > 1 {
                                tabs.remove(i);
                                *active_tab = i.min(tabs.len() - 1);
                            } else if let Some(tab) = tabs.get_mut(0) {
                                tab.navigate("deix://home", true);
                            }
                        } else {
                            *active_tab = i;
                        }
                        if let Some(tab) = tabs.get(*active_tab) {
                            *address_input = tab.url.clone();
                        }
                        self.dirty = true;
                        break;
                    }
                    return;
                }
                // Панель навигации: Back / Forward / Reload / Home / Star.
                // Hitbox = те же Rect, по которым рисуются кнопки.
                if rel_y > 24 && rel_y < 54 {
                    let cur_url = tabs.get(*active_tab).map(|t| t.url.clone()).unwrap_or_default();
                    match browser_ui::nav_button_at(rel_x, rel_y) {
                        Some(browser_ui::NavButton::Back) => {
                            if let Some(tab) = tabs.get_mut(*active_tab) {
                                if tab.go_back() {
                                    *address_input = tab.url.clone();
                                    *status_msg = Some(String::from("Back"));
                                }
                            }
                        }
                        Some(browser_ui::NavButton::Forward) => {
                            if let Some(tab) = tabs.get_mut(*active_tab) {
                                if tab.go_forward() {
                                    *address_input = tab.url.clone();
                                    *status_msg = Some(String::from("Forward"));
                                }
                            }
                        }
                        Some(browser_ui::NavButton::Reload) => {
                            if let Some(tab) = tabs.get_mut(*active_tab) {
                                tab.navigate(&cur_url, false);
                                *status_msg = Some(String::from("Reloaded"));
                            }
                        }
                        Some(browser_ui::NavButton::Home) => {
                            if let Some(tab) = tabs.get_mut(*active_tab) {
                                tab.navigate("deix://home", true);
                                *address_input = tab.url.clone();
                            }
                        }
                        Some(browser_ui::NavButton::Star) => {
                            // Закладка текущей страницы: повторный клик снимает.
                            if let Some(pos) = bookmarks.iter().position(|b| *b == cur_url) {
                                bookmarks.remove(pos);
                                *status_msg = Some(String::from("Bookmark removed"));
                            } else if !cur_url.is_empty() {
                                bookmarks.push(cur_url);
                                *status_msg = Some(String::from("Bookmark added"));
                            }
                        }
                        None => {}
                    }
                    self.dirty = true;
                    return;
                }
                if rel_y >= 54 && rel_y < 74 {
                    let mut clicked: Option<usize> = None;
                    for i in 0..bookmarks.len() {
                        let br = browser_ui::bookmark_rect(i);
                        if br.x + br.w as i32 > win.width as i32 {
                            break;
                        }
                        if br.contains(rel_x, rel_y) {
                            clicked = Some(i);
                            break;
                        }
                    }
                    if let Some(i) = clicked {
                        let target_url = bookmarks[i].clone();
                        *address_input = target_url.clone();
                        if let Some(tab) = tabs.get_mut(*active_tab) {
                            tab.navigate(&target_url, true);
                        }
                        *status_msg = Some(format!("Loaded {}", target_url));
                        self.dirty = true;
                    }
                    return;
                }
                // Содержимое: клик по строке `[N] ...` открывает links[N-1]
                // (геометрия строк — та же, что в draw_browser: старт 76,
                // шаг 18).
                if rel_y >= 76 {
                    let row = ((rel_y - 76) / 18) as usize;
                    if row >= browser_ui::visible_lines(win.height) {
                        return; // строка статуса / полоса под страницей
                    }
                    let target = tabs.get(*active_tab).and_then(|tab| {
                        let line = tab.content.get(tab.scroll + row)?;
                        let rest = line.strip_prefix('[')?;
                        let close = rest.find(']')?;
                        let n: usize = rest[..close].parse().ok()?;
                        tab.links.get(n.checked_sub(1)?).cloned()
                    });
                    if let Some(url) = target {
                        if let Some(tab) = tabs.get_mut(*active_tab) {
                            tab.navigate(&url, true);
                            *address_input = tab.url.clone();
                            *status_msg = Some(format!("Loaded {}", url));
                            self.dirty = true;
                        }
                    }
                }
            }
            WindowContent::DisplaySettings { status } => {
                // Строки рисуются с content_y + 32 шагом 28 (см.
                // draw_display_settings); клики выше первой строки
                // (заголовок) не должны выбирать режим.
                let py_start = 32;
                if rel_y < py_start {
                    return;
                }
                let idx = ((rel_y - py_start) / 28) as usize;
                if idx < RESOLUTION_PRESETS.len() {
                    let (res_w, res_h) = RESOLUTION_PRESETS[idx];
                    // Preflight по памяти: новому режиму нужны back buffer
                    // и кеш обоев (w*h*4 каждый); буферы текущего режима
                    // при переключении освобождаются. Если места всё равно
                    // не хватает — честный отказ вместо alloc error и
                    // чёрного экрана посреди переключения.
                    let needed = (res_w as usize) * (res_h as usize) * 4 * 2;
                    let reclaimed = (screen_w as usize) * (screen_h as usize) * 4 * 2;
                    let margin = 2 * 1024 * 1024;
                    let available = crate::allocator::stats().free + reclaimed;
                    if needed + margin > available {
                        *status = Some(format!(
                            "Not enough kernel memory for {}x{} ({} MiB short)",
                            res_w,
                            res_h,
                            (needed + margin - available).div_ceil(1024 * 1024)
                        ));
                    } else {
                        *status = None;
                        self.pending_resolution = Some((res_w, res_h));
                    }
                    self.dirty = true;
                }
            }
            WindowContent::FileEditor {
                full_path,
                lines,
                modified,
                confirm_close,
                error,
                status_msg,
                ..
            } => {
                // Диалог закрытия модален: пока он открыт, клики по
                // содержимому обрабатываются только кнопками диалога.
                if *confirm_close {
                    let (ww, wh) = (win.width, win.height);
                    if files_ui::editor_dialog_btn(ww, wh, 0).contains(rel_x, rel_y) {
                        // Save: при успехе окно закрывается, при ошибке
                        // остаётся открытым с текстом ошибки.
                        let mut data = lines.join("\n");
                        data.push('\n');
                        match crate::vfs::write_file(full_path, data.as_bytes()) {
                            Ok(()) => {
                                *modified = false;
                                close_after = true;
                            }
                            Err(e) => {
                                *confirm_close = false;
                                *status_msg = None;
                                *error = Some(format!("Save failed: {}", e.message()));
                            }
                        }
                        self.dirty = true;
                    } else if files_ui::editor_dialog_btn(ww, wh, 1).contains(rel_x, rel_y) {
                        // Discard: закрыть без записи.
                        close_after = true;
                        self.dirty = true;
                    } else if files_ui::editor_dialog_btn(ww, wh, 2).contains(rel_x, rel_y) {
                        // Cancel: вернуться к редактированию.
                        *confirm_close = false;
                        self.dirty = true;
                    }
                }
            }
            _ => {}
        }

        if close_after {
            self.close_window(win_idx);
            return;
        }
        if let Some((x, y, name, full, read_only)) = open_editor {
            self.push_focused(Window::new_file_editor(x, y, &name, &full, read_only));
        }
    }

    pub fn render(&mut self, r: &mut Renderer) {
        // Профиль производительности применяется к локальной копии темы:
        // Performance делает поверхности непрозрачными (fill_*_alpha при
        // 255 идёт быстрым путём без смешивания), Balanced/Performance
        // отключают blur. Сохранённая тема при этом не изменяется.
        let fx = crate::ui::effects::current();
        let mut theme = get_theme();
        theme.opacity = fx.effective_opacity(theme.opacity);
        theme.enable_blur = theme.enable_blur && fx.blur_enabled();
        let screen_w = r.width() as i32;
        let screen_h = r.height() as i32;

        // Снимаем курсор прошлого кадра: без этого при полной перерисовке
        // (например, с зажатой кнопкой мыши) его пиксели оставались в back
        // buffer вне damage-областей и тянулись следом за мышью.
        self.restore_under_cursor(r);
        self.diff_exposed_regions(r, screen_w, screen_h);

        draw_wallpaper(r, &theme, self, screen_w, screen_h);
        draw_desktop_icons(r, &theme, self.selected_icon);

        let is_dragging = self.dragging_window.is_some();

        for i in 0..self.windows.len() {
            if !self.windows[i].minimized {
                let is_focused = self.is_focused(i);
                draw_window(r, &theme, &self.windows[i], is_focused, is_dragging);
            }
        }

        draw_dock(r, &theme, self, screen_w, screen_h);
        draw_top_panel(r, &theme, self, screen_w);

        if self.start_menu_open {
            draw_start_menu(r, &theme, self, screen_h);
        }

        if self.control_center_open {
            draw_control_center(r, &theme, screen_w, screen_h);
        }

        if self.overview_open {
            draw_overview(r, &theme, self, screen_w, screen_h);
        }

        let m = mouse::snapshot();
        self.save_under_cursor(r, m.x, m.y);
        draw_cursor(r);
        r.add_damage(crate::renderer::Rect::new(m.x, m.y, CURSOR_W, CURSOR_H));
        r.present();
    }

    /// Diff видимых областей между кадрами. Всё, что исчезло
    /// (закрытое/свёрнутое/перетащенное окно, закрытое меню) или
    /// появилось, добавляется в damage ДО draw_wallpaper — иначе обои
    /// под освободившейся областью не восстанавливаются и на экране
    /// остаются куски старых окон.
    fn diff_exposed_regions(&mut self, r: &mut Renderer, screen_w: i32, screen_h: i32) {
        let ui_m = UiMetrics::fluent();
        let titlebar_h = ui_m.titlebar_height;

        let mut current: Vec<crate::renderer::Rect> = Vec::with_capacity(self.windows.len());
        for w in self.windows.iter() {
            if !w.minimized {
                current.push(window_visual_rect(w, titlebar_h));
            }
        }
        for old in self.prev_window_rects.iter() {
            if !current.iter().any(|c| c == old) {
                r.add_damage(*old);
            }
        }
        for new in current.iter() {
            if !self.prev_window_rects.iter().any(|o| o == new) {
                r.add_damage(*new);
            }
        }
        self.prev_window_rects = current;

        // Меню Пуск: геометрия из start_menu_layout + запас на рамку.
        let l = start_menu_layout(screen_h);
        let menu_rect = Rect::new(
            l.menu.x - 8,
            l.menu.y - 8,
            l.menu.w + 16,
            l.menu.h + 16,
        );
        if self.start_menu_open {
            if self.prev_start_menu_rect.is_none() {
                r.add_damage(menu_rect);
            }
            self.prev_start_menu_rect = Some(menu_rect);
        } else if let Some(old) = self.prev_start_menu_rect.take() {
            r.add_damage(old);
        }

        // Центр управления: геометрия из control_center_rect + запас.
        let cc = control_center_rect(screen_w, screen_h);
        let cc_rect = Rect::new(cc.x - 8, cc.y - 8, cc.w + 16, cc.h + 16);
        if self.control_center_open {
            if self.prev_control_center_rect.is_none() {
                r.add_damage(cc_rect);
            }
            self.prev_control_center_rect = Some(cc_rect);
        } else if let Some(old) = self.prev_control_center_rect.take() {
            r.add_damage(old);
        }

        // Dock: его ширина зависит от числа окон. Если область Dock
        // изменилась (окно открыли/закрыли), старую полосу надо
        // перерисовать, иначе по краям остаются куски прежнего Dock.
        let dock = dock_layout(self, screen_w, screen_h).dock;
        let dock_rect = Rect::new(dock.x - 8, dock.y - 8, dock.w + 16, dock.h + 16);
        if self.prev_dock_rect != Some(dock_rect) {
            if let Some(old) = self.prev_dock_rect {
                r.add_damage(old);
            }
            r.add_damage(dock_rect);
            self.prev_dock_rect = Some(dock_rect);
        }

        // Overview затемняет весь экран — его открытие/закрытие требует
        // полной перерисовки сцены.
        if self.overview_open != self.prev_overview_open {
            r.add_damage(Rect::new(0, 0, screen_w as u32, screen_h as u32));
            self.prev_overview_open = self.overview_open;
        }
    }

    /// Сохраняет пиксели back buffer под курсором (вызывается перед
    /// draw_cursor), чтобы быстрый путь мог восстановить фон без
    /// полной перерисовки сцены.
    fn save_under_cursor(&mut self, r: &mut Renderer, x: i32, y: i32) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + CURSOR_W as i32).min(r.width() as i32);
        let y1 = (y + CURSOR_H as i32).min(r.height() as i32);
        if x0 >= x1 || y0 >= y1 {
            self.cursor_saved_valid = false;
            return;
        }
        let w = (x1 - x0) as usize;
        let stride = r.width() as usize;
        self.cursor_saved_pixels.clear();
        for row in y0..y1 {
            let start = row as usize * stride + x0 as usize;
            self.cursor_saved_pixels.extend_from_slice(&r.back_buffer[start..start + w]);
        }
        self.cursor_saved_rect =
            crate::renderer::Rect::new(x0, y0, w as u32, (y1 - y0) as u32);
        self.cursor_saved_valid = true;
    }

    /// Возвращает в back buffer пиксели, сохранённые save_under_cursor,
    /// и помечает эту область как damage.
    fn restore_under_cursor(&mut self, r: &mut Renderer) {
        if !self.cursor_saved_valid {
            return;
        }
        let rect = self.cursor_saved_rect;
        if rect.x < 0
            || rect.y < 0
            || rect.x + rect.w as i32 > r.width() as i32
            || rect.y + rect.h as i32 > r.height() as i32
        {
            // Область вышла за пределы экрана (например, после смены
            // разрешения) — сохранённые пиксели неактуальны.
            self.cursor_saved_valid = false;
            return;
        }
        let w = rect.w as usize;
        let stride = r.width() as usize;
        for (i, row) in (rect.y..rect.y + rect.h as i32).enumerate() {
            let start = row as usize * stride + rect.x as usize;
            r.back_buffer[start..start + w]
                .copy_from_slice(&self.cursor_saved_pixels[i * w..(i + 1) * w]);
        }
        r.add_damage(rect);
        self.cursor_saved_valid = false;
    }

    /// Быстрый путь для кадров, где сдвинулся только курсор: восстановить
    /// фон на старом месте, сохранить фон на новом, нарисовать курсор и
    /// вывести два маленьких прямоугольника. Полная перерисовка сцены
    /// (все окна + blur + панель) на каждое движение мыши не нужна.
    fn update_cursor_only(&mut self, r: &mut Renderer) {
        if !self.cursor_saved_valid {
            // Нет сохранённого фона (например, первый кадр) — полный рендер.
            self.render(r);
            return;
        }
        let m = mouse::snapshot();
        self.restore_under_cursor(r);
        self.save_under_cursor(r, m.x, m.y);
        draw_cursor(r);
        r.add_damage(crate::renderer::Rect::new(m.x, m.y, CURSOR_W, CURSOR_H));
        r.present();
    }

    pub fn run_event_loop(&mut self, r: &mut Renderer, preserve_windows: bool) -> DesktopExit {
        let screen_w = r.width() as i32;
        let screen_h = r.height() as i32;

        // Настройки рабочего стола (тема/акцент/обои/профиль) из
        // /userdata/config/desktop.cfg — один раз за загрузку.
        crate::ui::config::load_once();

        if !preserve_windows {
            self.push_focused(Window::new_browser(40, 40, "deix://home"));
            self.push_focused(Window::new_files(380, 50));
        } else {
            // Смена разрешения: у развёрнутых окон пересчитывается только
            // текущая (развёрнутая) геометрия — restore_geometry не
            // перезаписывается, иначе Restore возвращал бы полноэкранный
            // размер старого режима. Обычные окна прижимаются к экрану.
            let m = UiMetrics::fluent();
            for w in self.windows.iter_mut() {
                if w.maximized {
                    let (mx, my, mw, mh) = maximized_geometry(screen_w, screen_h, &m);
                    w.x = mx;
                    w.y = my;
                    w.width = mw;
                    w.height = mh;
                } else {
                    let (cx, cy, cw, ch) =
                        clamp_window_geometry(w.x, w.y, w.width, w.height, screen_w, screen_h);
                    w.x = cx;
                    w.y = cy;
                    w.width = cw;
                    w.height = ch;
                }
            }
            self.dirty = true;
        }

        // Маяки первого кадра: по serial-логу видно, дошла ли сессия до
        // отрисовки и завершилась ли она (диагностика чёрного экрана).
        crate::serial_println!("[desktop] session start {}x{}", screen_w, screen_h);
        r.add_damage(crate::renderer::Rect::new(0, 0, screen_w as u32, screen_h as u32));
        self.render(r);
        self.dirty = false;
        crate::serial_println!("[desktop] first frame presented");

        loop {
            // Win/Super открывает и закрывает меню Пуск.
            if keyboard::try_read_super() {
                if self.start_menu_open {
                    self.close_start_menu();
                } else {
                    self.open_start_menu();
                }
                self.dirty = true;
            }

            if keyboard::try_read_escape() {
                // Escape сначала закрывает обзор/меню/панель и только
                // при их отсутствии завершает графическую сессию.
                if self.overview_open {
                    self.overview_open = false;
                    self.dirty = true;
                } else if self.start_menu_open || self.control_center_open {
                    self.close_start_menu();
                    self.control_center_open = false;
                    self.dirty = true;
                } else {
                    self.should_exit = true;
                }
            }

            // Сетевой рабочий поток: ISR карты только складывает кадры
            // в кольцо, разбор стеком идёт здесь, вне прерывания.
            crate::rtl8139::poll_deferred();

            // Квант супервизора: детект упавших служб и политика
            // перезапуска. Без этого вызова RestartPolicy не работает.
            crate::dinit::tick();
            crate::diag::persist::flush();

            self.handle_input(screen_w, screen_h);

            // Часы панели: RTC опрашивается ~раз в секунду, перерисовка —
            // только когда реально сменилась минута (а не каждую секунду).
            let now_ms = timer::uptime_ms();
            if now_ms >= self.next_clock_check_ms {
                self.next_clock_check_ms = now_ms + 1000;
                let key = clock_key();
                if key != self.last_clock_key {
                    self.last_clock_key = key;
                    self.dirty = true;
                }
            }

            if let Some((w, h)) = self.pending_resolution.take() {
                return DesktopExit::ChangeResolution(w, h);
            }

            if self.should_exit {
                return DesktopExit::Quit;
            }

            if self.dirty {
                self.frame_counter = timer::uptime_ms();
                self.render(r);
                self.dirty = false;
                self.cursor_moved = false;
            } else if self.cursor_moved {
                self.update_cursor_only(r);
                self.cursor_moved = false;
            }

            crate::sched::idle_wait();
        }
    }
}

pub fn run_desktop_session(mut on_resolution_change: impl FnMut(u32, u32) -> bool) {
    // В графическом режиме печать ядра на текстовую консоль бессмысленна
    // и вредна — события остаются в журнале и видны в Error Center.
    crate::diag::log::finish_boot();

    let mut desktop = Desktop::new();
    let mut preserve_windows = false;

    // Если прошлая сессия завершилась отказом ядра, Error Center
    // открывается сразу: пользователь должен узнать о сбое, даже если
    // пропустил текстовое уведомление при загрузке.
    if crate::diag::panic::has_previous_failure() {
        crate::renderer::with_renderer(|r| {
            let (w, h) = (r.width() as i32, r.height() as i32);
            desktop.open_error_center(w, h);
        });
    }

    loop {
        let exit = crate::renderer::with_renderer_long(|r| desktop.run_event_loop(r, preserve_windows));

        match exit {
            Some(DesktopExit::Quit) | None => return,
            Some(DesktopExit::ChangeResolution(w, h)) => {
                preserve_windows = true;
                // Кеш обоев старого разрешения освобождается ДО смены
                // режима: вместе со сбросом рендерера в enter_graphics_mode
                // это убирает пиковое удвоение памяти (старые + новые
                // буферы), из-за которого переключение на большие
                // разрешения падало по alloc error в чёрный экран.
                desktop.wallpaper_cache.release();
                if !on_resolution_change(w, h) {
                    return;
                }
            }
        }
    }
}

fn run_mini_terminal_command(cmd: &str, lines: &mut Vec<String>) {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return;
    }

    if cmd == "clear" {
        lines.clear();
        return;
    }

    crate::vgaglobal::begin_capture();
    crate::cli::IN_GRAPHICAL_TERMINAL.store(true, core::sync::atomic::Ordering::Relaxed);
    crate::cli::execute(cmd);
    crate::cli::IN_GRAPHICAL_TERMINAL.store(false, core::sync::atomic::Ordering::Relaxed);
    let output = crate::vgaglobal::end_capture();

    for line in output.lines() {
        lines.push(String::from(line));
    }
}

fn draw_wallpaper(r: &mut Renderer, theme: &UiTheme, desktop: &mut Desktop, w: i32, h: i32) {
    // Новые обои могли записать браузер или CLI прямо во время сессии.
    let epoch = crate::ui::theme::wallpaper_epoch();
    if epoch != desktop.wallpaper_epoch_seen {
        desktop.wallpaper_epoch_seen = epoch;
        desktop.wallpaper_cache.invalidate();
    }
    let cache_invalid = !desktop.wallpaper_cache.valid
        || desktop.wallpaper_cache.width != w as u32
        || desktop.wallpaper_cache.height != h as u32;

    if cache_invalid {
        // Обои генерируются ОДИН раз (в кеш) при смене темы/стиля/
        // разрешения; движение курсора и перетаскивание окон кеш
        // не трогают — кадр только копирует готовые пиксели.
        // Старые пиксели возвращаются куче до выделения новых — иначе
        // на большом разрешении кеш временно существует в двух копиях.
        desktop.wallpaper_cache.release();
        desktop.wallpaper_cache = WallpaperSurface::new(w as u32, h as u32);
        generate_wallpaper(&mut desktop.wallpaper_cache.pixels, w, h, theme);
        desktop.wallpaper_cache.valid = true;
        r.add_damage(crate::renderer::Rect::new(0, 0, w as u32, h as u32));
    }

    if r.damage.full_redraw || cache_invalid {
        let src = &desktop.wallpaper_cache.pixels;
        let dst = &mut r.back_buffer;
        let copy_len = src.len().min(dst.len());
        dst[..copy_len].copy_from_slice(&src[..copy_len]);
    } else {
        let count = r.damage.count;
        let width = w as usize;
        for i in 0..count {
            let rect = r.damage.rects[i];
            let x0 = rect.x.max(0) as usize;
            let y0 = rect.y.max(0) as usize;
            let x1 = ((rect.x + rect.w as i32).min(w)).max(0) as usize;
            let y1 = ((rect.y + rect.h as i32).min(h)).max(0) as usize;

            for row in y0..y1 {
                let offset = row * width;
                let start = offset + x0;
                let end = offset + x1;
                if end <= desktop.wallpaper_cache.pixels.len() && end <= r.back_buffer.len() {
                    r.back_buffer[start..end].copy_from_slice(&desktop.wallpaper_cache.pixels[start..end]);
                }
            }
        }
    }
}

/// Целочисленный квадратный корень (floor). Монотонно убывающая
/// итерация Ньютона: `y >= x` — гарантированный выход. Прежний вариант
/// с условием `x != last` зацикливался навсегда на значениях, где
/// итерация осциллирует между floor(sqrt(v)) и соседом (v = 3, 8,
/// 399, 5775, ...) — первый же мягкий диск обоев попадал на такое v,
/// и первый кадр рабочего стола зависал чёрным экраном.
fn isqrt(v: i64) -> i32 {
    if v <= 0 {
        return 0;
    }
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x as i32
}

/// Мягкий диск в пиксельный буфер: альфа квадратично спадает от
/// центра (peak) к краю (0). Основа свечений в генераторе обоев.
fn blend_soft_disc(
    pixels: &mut [u32],
    wu: usize,
    w: i32,
    h: i32,
    cx: i32,
    cy: i32,
    rad: i32,
    color: Color,
    peak: u8,
) {
    if rad <= 0 {
        return;
    }
    let r2 = rad as i64 * rad as i64;
    let y0 = (cy - rad).max(0);
    let y1 = (cy + rad).min(h - 1);
    for y in y0..=y1 {
        let dy = (y - cy) as i64;
        let span = isqrt(r2 - dy * dy);
        let x0 = (cx - span).max(0);
        let x1 = (cx + span).min(w - 1);
        let row = y as usize * wu;
        for x in x0..=x1 {
            let dx = (x - cx) as i64;
            let a = ((r2 - (dx * dx + dy * dy)) * peak as i64 / r2) as u8;
            blend_into(pixels, row + x as usize, color, a);
        }
    }
}

/// Смешивает цвет `fg` с альфой поверх пикселя кеша.
#[inline]
fn blend_into(pixels: &mut [u32], idx: usize, fg: Color, alpha: u8) {
    if let Some(p) = pixels.get_mut(idx) {
        *p = fg.alpha_blend(Color::from_u32(*p), alpha).0;
    }
}

/// Генерация обоев выбранного стиля в пиксели кеша. Четыре стиля дают
/// действительно разные картины; все цвета берутся из активной темы.
/// Пытается загрузить пользовательские обои из /userdata/wallpaper.bmp:
/// несжатый BMP 24/32 бит (BI_RGB), любой размер в пределах лимита файла
/// ext2 (268 КБ, например 320x240x24); картинка растягивается на экран
/// ближайшим соседом. false — файла нет или формат не поддержан.
fn load_bmp_wallpaper(pixels: &mut [u32], w: i32, h: i32) -> bool {
    let data = match ext2::read_file_path("/wallpaper.bmp") {
        Ok(d) => d,
        Err(_) => return false,
    };
    if data.len() < 54 || &data[0..2] != b"BM" {
        return false;
    }
    let off = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
    let bw = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
    let bh_raw = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
    let bpp = u16::from_le_bytes([data[28], data[29]]);
    let compression = u32::from_le_bytes([data[30], data[31], data[32], data[33]]);
    if compression != 0 || (bpp != 24 && bpp != 32) || bw <= 0 || bh_raw == 0 {
        return false;
    }
    let top_down = bh_raw < 0;
    let bh = bh_raw.unsigned_abs() as usize;
    let bw = bw as usize;
    let bytes_pp = (bpp / 8) as usize;
    // Строки BMP выровнены на 4 байта.
    let stride = (bw * bytes_pp).div_ceil(4) * 4;
    let needed = match stride.checked_mul(bh).and_then(|v| v.checked_add(off)) {
        Some(v) => v,
        None => return false,
    };
    if needed > data.len() {
        return false;
    }

    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    for y in 0..h {
        let sy = y * bh / h;
        let srow = if top_down { sy } else { bh - 1 - sy };
        let row_off = off + srow * stride;
        for x in 0..w {
            let sx = x * bw / w;
            let p = row_off + sx * bytes_pp;
            let (b, g, r) = (data[p], data[p + 1], data[p + 2]);
            if let Some(dst) = pixels.get_mut(y * w + x) {
                *dst = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
            }
        }
    }
    true
}

fn generate_wallpaper(pixels: &mut [u32], w: i32, h: i32, theme: &UiTheme) {
    let (w, h) = (w.max(1), h.max(1));
    let wu = w as usize;

    // Стиль Custom: картинка пользователя; если её нет или формат не
    // поддержан — ниже рисуется процедурная Aurora (ветки `_`).
    if theme.wallpaper_style == 4 && load_bmp_wallpaper(pixels, w, h) {
        return;
    }

    // База: вертикальный градиент (у каждого стиля свои опорные цвета).
    let (top, bottom) = match theme.wallpaper_style {
        1 => (
            theme.bg_top.lerp(Color::BLACK, 110),
            theme.bg_bottom.lerp(Color::BLACK, 150),
        ),
        2 => (theme.bg_bottom, theme.bg_top),
        3 => (
            theme.window_bg.lerp(Color::WHITE, 150),
            theme.window_bg.lerp(Color::WHITE, 90),
        ),
        _ => (theme.bg_top, theme.bg_bottom),
    };
    for y in 0..h {
        let t = (y * 255 / h) as u8;
        let c = top.lerp(bottom, t).0;
        let row = y as usize * wu;
        for x in 0..wu {
            if let Some(p) = pixels.get_mut(row + x) {
                *p = c;
            }
        }
    }

    match theme.wallpaper_style {
        // Midnight: луна с мягким гало, звёзды трёх яркостей и дымка
        // у горизонта.
        1 => {
            let moon = Color::rgb(226, 232, 240);
            let (mx, my) = (w * 3 / 4, h / 4);
            blend_soft_disc(pixels, wu, w, h, mx, my, h / 5, moon, 26);
            blend_soft_disc(pixels, wu, w, h, mx, my, h / 12, moon, 210);
            for i in 0..90i32 {
                let px = (i * 193 + 31) % w;
                let py = (i * 311 + 59) % h;
                let idx = py as usize * wu + px as usize;
                let a = match i % 3 {
                    0 => 230,
                    1 => 140,
                    _ => 80,
                };
                blend_into(pixels, idx, theme.text_primary, a);
            }
            let band_c = h * 3 / 4;
            for y in (band_c - 50).max(0)..(band_c + 50).min(h) {
                let dist = (y - band_c).abs();
                let alpha = (16 * (50 - dist) / 50) as u8;
                let row = y as usize * wu;
                for x in 0..w {
                    blend_into(pixels, row + x as usize, theme.accent, alpha);
                }
            }
        }
        // Plasma Flow: крупные полупрозрачные круги из цветов темы +
        // мягкие световые пятна, задающие «течение» по диагонали.
        2 => {
            let colors = [
                theme.accent,
                theme.surface_alt,
                theme.accent_hover,
                theme.titlebar_active,
                theme.surface,
            ];
            for (i, color) in colors.iter().enumerate() {
                let cx = w * (2 * i as i32 + 1) / 10;
                let cy = h * ((i as i32 * 37 + 20) % 100) / 100;
                let rad = h / 4 + i as i32 * h / 16;
                let y0 = (cy - rad).max(0);
                let y1 = (cy + rad).min(h - 1);
                for y in y0..=y1 {
                    let dy = (y - cy) as i64;
                    let span = isqrt(rad as i64 * rad as i64 - dy * dy);
                    let x0 = (cx - span).max(0);
                    let x1 = (cx + span).min(w - 1);
                    let row = y as usize * wu;
                    for x in x0..=x1 {
                        blend_into(pixels, row + x as usize, *color, 30);
                    }
                }
            }
            blend_soft_disc(pixels, wu, w, h, w / 4, h / 4, h / 2, theme.accent, 26);
            blend_soft_disc(pixels, wu, w, h, w * 3 / 4, h * 3 / 4, h / 2, theme.accent_hover, 22);
        }
        // Minimal: светлые поверхности, одна мягкая акцентная полоса и
        // лёгкое затемнение краёв для глубины.
        3 => {
            let band_c = h * 2 / 3;
            for y in (band_c - 40).max(0)..(band_c + 40).min(h) {
                let dist = (y - band_c).abs();
                let alpha = (20 * (40 - dist) / 40).max(0) as u8;
                let row = y as usize * wu;
                for x in 0..w {
                    blend_into(pixels, row + x as usize, theme.accent, alpha);
                }
            }
            let edge = (w / 8).max(1);
            for y in 0..h {
                let row = y as usize * wu;
                for x in 0..edge {
                    let a = (22 * (edge - x) / edge) as u8;
                    blend_into(pixels, row + x as usize, Color::BLACK, a);
                    blend_into(pixels, row + (w - 1 - x) as usize, Color::BLACK, a);
                }
            }
        }
        // Aurora: изогнутые ленты сияния (бирюза/фиолет) с квадратичным
        // спадом к краям, мягкие свечения и звёзды — как в концепте.
        _ => {
            let violet = Color::rgb(138, 123, 240);
            blend_soft_disc(pixels, wu, w, h, w / 5, h / 3, h / 2, theme.accent, 16);
            blend_soft_disc(pixels, wu, w, h, w * 3 / 4, h / 2, h / 2, violet, 12);

            let ribbons: [(Color, i32, i32, i32, i32); 3] = [
                (theme.accent, w / 6, 3, 84, 42),
                (violet, w * 11 / 16, -4, 110, 34),
                (theme.accent_hover, w * 7 / 8, 5, 64, 26),
            ];
            for y in 0..h {
                let row = y as usize * wu;
                // Квадратичная добавка даёт лентам лёгкий изгиб.
                let bend = (y * y) / (h * 6).max(1);
                for (color, base, slope, halfw, peak) in ribbons.iter() {
                    let xc = *base + y / *slope + if *slope > 0 { bend } else { -bend };
                    let hw = *halfw;
                    let x0 = (xc - hw).max(0);
                    let x1 = (xc + hw).min(w - 1);
                    for x in x0..=x1 {
                        let dx = x - xc;
                        let a = ((hw * hw - dx * dx) * peak / (hw * hw)) as u8;
                        blend_into(pixels, row + x as usize, *color, a);
                    }
                }
            }

            for i in 0..70i32 {
                let px = (i * 193 + 31) % w;
                let py = (i * 311 + 59) % h;
                let idx = py as usize * wu + px as usize;
                let a = match i % 3 {
                    0 => 255,
                    1 => 150,
                    _ => 90,
                };
                blend_into(pixels, idx, theme.text_primary, a);
                // Редкие звёзды — с короткими лучами.
                if i % 9 == 0 {
                    if px + 1 < w {
                        blend_into(pixels, idx + 1, theme.text_primary, 70);
                    }
                    if px > 0 {
                        blend_into(pixels, idx - 1, theme.text_primary, 70);
                    }
                    if py + 1 < h {
                        blend_into(pixels, idx + wu, theme.text_primary, 70);
                    }
                    if py > 0 {
                        blend_into(pixels, idx - wu, theme.text_primary, 70);
                    }
                }
            }
        }
    }
}

fn draw_desktop_icons(r: &mut Renderer, theme: &UiTheme, selected_idx: Option<usize>) {
    let shortcuts = desktop_shortcuts();
    for (idx, sc) in shortcuts.iter().enumerate() {
        let ix = 20;
        let iy = 20 + idx as i32 * 70;
        let is_sel = selected_idx == Some(idx);

        if is_sel {
            r.fill_rounded_rect_alpha(ix - 6, iy - 4, 64, 60, 6, theme.accent, 100);
        }

        r.fill_rounded_rect(ix, iy, 48, 36, 8, theme.titlebar_active);
        r.draw_icon(
            ix + 16,
            iy + 10,
            match sc.icon {
                IconType::Terminal => IconType::Terminal,
                IconType::Files => IconType::Files,
                IconType::Browser => IconType::Browser,
                IconType::TaskManager => IconType::TaskManager,
                IconType::Theme => IconType::Theme,
                _ => IconType::Terminal,
            },
            theme.accent,
        );

        r.draw_text(ix, iy + 40, sc.name, theme.text_primary, None);
    }
}

/// Прижимает геометрию окна к доступной области экрана: окно не шире
/// экрана, не ниже панели задач, заголовок с кнопками всегда достижим.
fn clamp_window_geometry(
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    screen_w: i32,
    screen_h: i32,
) -> (i32, i32, u32, u32) {
    let m = UiMetrics::fluent();
    let wa = work_area(screen_w, screen_h);
    let wa_bottom = wa.y + wa.h as i32;

    let max_w = (screen_w - 8).max(MIN_WINDOW_W as i32) as u32;
    let max_h = (wa.h as i32 - m.titlebar_height - 8).max(MIN_WINDOW_H as i32) as u32;
    let w = w.clamp(MIN_WINDOW_W, max_w);
    let h = h.clamp(MIN_WINDOW_H, max_h);

    let x = x.clamp(0, (screen_w - w as i32).max(0));
    // Окно целиком в рабочей области: не выше верхней панели и не под
    // Dock (заголовок + содержимое должны помещаться).
    let y = y.clamp(wa.y, (wa_bottom - m.titlebar_height - h as i32).max(wa.y));

    (x, y, w, h)
}

/// Первое свободное имя вида `{prefix}{n}{suffix}` среди текущих записей
/// каталога. Существующие файлы не перезаписываются.
fn free_name(
    entries: &[crate::ui::window::FileViewEntry],
    prefix: &str,
    suffix: &str,
) -> Option<String> {
    for n in 1..=99u32 {
        let candidate = format!("{}{}{}", prefix, n, suffix);
        if !entries.iter().any(|e| e.name.eq_ignore_ascii_case(&candidate)) {
            return Some(candidate);
        }
    }
    None
}

/// Склейка пути внутри раздела: `"/", "A"` -> `"/A"`; `"/A", "B"` -> `"/A/B"`.
fn join_fs_path(dir: &str, name: &str) -> String {
    if dir == "/" || dir.is_empty() {
        format!("/{}", name)
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}

/// Родительский каталог: `"/A/B"` -> `"/A"`, `"/A"` -> `"/"`.
fn parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(pos) => String::from(&trimmed[..pos]),
    }
}
