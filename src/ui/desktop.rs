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
use crate::ui::taskbar::{
    clock_key, control_center_rect, draw_control_center, draw_cursor, draw_start_menu,
    draw_taskbar, start_menu_layout, StartMenuAction, START_MENU_APPS,
};
use crate::ui::theme::{get_theme, set_theme, UiTheme};
use crate::ui::window::{
    draw_window, fetch_and_render_web_page, load_partition_entries, maximized_geometry,
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
    pub control_center_open: bool,
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
            control_center_open: false,
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

    /// Окно по индексу в фокусе? (для отрисовки панели задач и рамок)
    pub fn is_focused(&self, idx: usize) -> bool {
        match (self.focused_id, self.windows.get(idx)) {
            (Some(id), Some(w)) => w.id == id,
            _ => false,
        }
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
        self.open_placed(480, 320, screen_w, screen_h, Window::new_theme_settings);
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

    pub fn close_window(&mut self, idx: usize) {
        if idx < self.windows.len() {
            // Редактор сохраняет изменённый файл при закрытии: клавиатура
            // не передаёт Ctrl-сочетания, поэтому отдельной горячей
            // клавиши сохранения нет.
            if let WindowContent::FileEditor {
                full_path,
                lines,
                modified: true,
                read_only: false,
                ..
            } = &self.windows[idx].content
            {
                let mut data = lines.join("\n");
                data.push('\n');
                if let Err(e) = crate::vfs::write_file(full_path, data.as_bytes()) {
                    crate::diag::error(
                        crate::diag::ErrorCode::new(crate::diag::Subsystem::Vfs, 10),
                        &format!("редактор, сохранение {}: {}", full_path, e.message()),
                    );
                }
            }
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
        let taskbar_y = screen_h - m.taskbar_height as i32;
        let work_h = (taskbar_y - m.titlebar_height).max(MIN_WINDOW_H as i32) as u32;

        let Some(idx) = self.index_of(id) else { return };
        let w = &mut self.windows[idx];
        if w.maximized {
            return;
        }

        if my <= 1 {
            // Развернуть; Restore вернёт окно туда, откуда его тащили.
            w.restore_geometry = (orig_x, orig_y, w.width, w.height);
            let (gx, gy, gw, gh) = maximized_geometry(screen_w, screen_h, &m);
            w.x = gx;
            w.y = gy;
            w.width = gw;
            w.height = gh;
            w.maximized = true;
        } else if mx <= 1 {
            w.x = 0;
            w.y = 0;
            w.width = (screen_w / 2).max(MIN_WINDOW_W as i32) as u32;
            w.height = work_h;
        } else if mx >= screen_w - 2 {
            let half = (screen_w / 2).max(MIN_WINDOW_W as i32);
            w.x = screen_w - half;
            w.y = 0;
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

    pub fn handle_input(&mut self, screen_w: i32, screen_h: i32) {
        let m = mouse::snapshot();
        let ui_m = UiMetrics::fluent();
        let taskbar_y = screen_h - ui_m.taskbar_height as i32;

        if m.x != self.last_cursor_pos.0 || m.y != self.last_cursor_pos.1 {
            // Движение мыши само по себе сцену не меняет: вместо полной
            // перерисовки (все окна + blur + панель на каждый пиксель
            // движения) курсор обновляется быстрым путём save-under
            // в run_event_loop (update_cursor_only).
            self.last_cursor_pos = (m.x, m.y);
            self.cursor_moved = true;
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
                        // экрана, не под панелью задач, и минимум 60px
                        // окна видно по горизонтали.
                        let nx = (orig_x + (m.x - mx0))
                            .clamp(-(w.width as i32) + 60, screen_w - 60);
                        let ny = (orig_y + (m.y - my0))
                            .clamp(0, taskbar_y - ui_m.titlebar_height);
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

            // Открытое меню Пуск: клик внутри обрабатывается меню, клик
            // вне — только закрывает его. Клик не «проваливается» сквозь
            // меню на окна и рабочий стол.
            if self.start_menu_open {
                let l = start_menu_layout(screen_h);
                if l.menu.contains(m.x, m.y) {
                    self.handle_start_menu_click(m.x, m.y, screen_w, screen_h);
                } else if m.y >= taskbar_y && m.x > ui_m.start_button_width {
                    // Клик по панели вне кнопки Start: меню закрывается,
                    // клик обрабатывается панелью.
                    self.start_menu_open = false;
                    self.handle_taskbar_click(m.x, screen_w);
                } else {
                    // Клик мимо меню (включая повторный клик по Start) —
                    // просто закрыть, без «проваливания» на окна.
                    self.start_menu_open = false;
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

            if m.y >= taskbar_y {
                self.handle_taskbar_click(m.x, screen_w);
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
                    self.close_window(i);
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

        self.handle_keyboard_for_focused_window();
    }

    fn handle_keyboard_for_focused_window(&mut self) {
        if let Some(focused_idx) = self.focused_index() {
            if let Some(w) = self.windows.get_mut(focused_idx) {
                if w.minimized {
                    return;
                }
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
                        while let Some(byte) = keyboard::try_read_char() {
                            self.dirty = true;
                            match byte {
                                b'\n' => {
                                    let new_url = address_input.clone();
                                    if let Some(tab) = tabs.get_mut(*active_tab) {
                                        tab.url = new_url.clone();
                                        tab.title = format!("Page: {}", new_url);
                                        tab.content = fetch_and_render_web_page(&new_url);
                                    }
                                    *status_msg = Some(format!("Navigated to {}", new_url));
                                }
                                0x08 => {
                                    address_input.pop();
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
                        status_msg: _,
                        ..
                    } => {
                        while let Some(byte) = keyboard::try_read_char() {
                            self.dirty = true;
                            match byte {
                                keyboard::ARROW_UP => {
                                    *cursor_row = cursor_row.saturating_sub(1);
                                    if *cursor_row < *scroll {
                                        *scroll = *cursor_row;
                                    }
                                }
                                keyboard::ARROW_DOWN => {
                                    if *cursor_row + 1 < lines.len() {
                                        *cursor_row += 1;
                                    }
                                }
                                keyboard::ARROW_LEFT => {
                                    *cursor_col = cursor_col.saturating_sub(1);
                                }
                                keyboard::ARROW_RIGHT => {
                                    if let Some(line) = lines.get(*cursor_row) {
                                        if *cursor_col < line.len() {
                                            *cursor_col += 1;
                                        }
                                    }
                                }
                                0x08 => {
                                    if !*read_only && *cursor_col > 0 {
                                        if let Some(line) = lines.get_mut(*cursor_row) {
                                            line.remove(*cursor_col - 1);
                                            *cursor_col -= 1;
                                            *modified = true;
                                        }
                                    }
                                }
                                b'\n' | b'\r' => {
                                    if !*read_only {
                                        if let Some(line) = lines.get_mut(*cursor_row) {
                                            let rest = line.split_off((*cursor_col).min(line.len()));
                                            lines.insert(*cursor_row + 1, rest);
                                            *cursor_row += 1;
                                            *cursor_col = 0;
                                            *modified = true;
                                        }
                                    }
                                }
                                b if b >= 0x20 && b < 0x7F => {
                                    if !*read_only {
                                        if let Some(line) = lines.get_mut(*cursor_row) {
                                            let idx = (*cursor_col).min(line.len());
                                            line.insert(idx, b as char);
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

    fn handle_start_menu_click(&mut self, x: i32, y: i32, screen_w: i32, screen_h: i32) {
        // Геометрия кликов — ровно те же прямоугольники, по которым
        // draw_start_menu рисует пункты.
        let l = start_menu_layout(screen_h);

        for (i, rect) in l.app_items.iter().enumerate() {
            if rect.contains(x, y) {
                match START_MENU_APPS[i].1 {
                    StartMenuAction::OpenTerminal => self.open_terminal(screen_w, screen_h),
                    StartMenuAction::OpenFiles => self.open_files(screen_w, screen_h),
                    StartMenuAction::OpenBrowser => self.open_browser(screen_w, screen_h),
                    StartMenuAction::OpenTaskManager => self.open_task_manager(screen_w, screen_h),
                    StartMenuAction::OpenErrorCenter => self.open_error_center(screen_w, screen_h),
                    StartMenuAction::OpenThemeSettings => self.open_theme_settings(screen_w, screen_h),
                    StartMenuAction::OpenAbout => self.open_about(screen_w, screen_h),
                    StartMenuAction::OpenDisplaySettings => {
                        self.open_display_settings(screen_w, screen_h)
                    }
                }
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

    fn handle_taskbar_click(&mut self, x: i32, screen_w: i32) {
        let m = UiMetrics::fluent();
        if x <= m.start_button_width {
            self.start_menu_open = !self.start_menu_open;
            self.control_center_open = false;
            self.dirty = true;
            return;
        }

        if x >= screen_w - 120 {
            self.control_center_open = !self.control_center_open;
            self.start_menu_open = false;
            self.dirty = true;
            return;
        }

        // Элементы окон: индексация и граница обрыва совпадают с
        // draw_taskbar (item_x + item_w должен помещаться до трея).
        let rel = x - m.start_button_width - 8;
        if rel < 0 {
            return;
        }
        let idx = (rel / m.taskbar_item_width) as usize;
        let item_x = m.start_button_width + 8 + idx as i32 * m.taskbar_item_width;
        if idx >= self.windows.len() || item_x + m.taskbar_item_width > screen_w - 120 {
            return;
        }

        // Поведение: свёрнутое окно — восстановить и активировать;
        // неактивное — активировать; уже активное — свернуть.
        if self.windows[idx].minimized {
            self.toggle_minimize(idx); // восстановление + фокус
        } else if !self.is_focused(idx) {
            self.focus_window(idx);
        } else {
            self.toggle_minimize(idx); // сворачивание активного
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
        _screen_w: i32,
        _screen_h: i32,
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
                if rel_y <= 24 {
                    let tab_w = 120;
                    let clicked_tab = (rel_x - 8) / tab_w;
                    if clicked_tab >= 0 && (clicked_tab as usize) < tabs.len() {
                        *active_tab = clicked_tab as usize;
                        *address_input = tabs[*active_tab].url.clone();
                        self.dirty = true;
                    }
                    if rel_x >= win.width as i32 - 30 {
                        let new_tab = BrowserTab {
                            title: String::from("New Tab"),
                            url: String::from("deix://home"),
                            content: fetch_and_render_web_page("deix://home"),
                            scroll: 0,
                        };
                        tabs.push(new_tab);
                        *active_tab = tabs.len() - 1;
                        *address_input = String::from("deix://home");
                        self.dirty = true;
                    }
                    return;
                }
                if rel_y >= 54 && rel_y <= 74 {
                    let bk_w = 110;
                    let clicked_bk = (rel_x - 8) / bk_w;
                    if clicked_bk >= 0 && (clicked_bk as usize) < bookmarks.len() {
                        let target_url = bookmarks[clicked_bk as usize].clone();
                        *address_input = target_url.clone();
                        if let Some(tab) = tabs.get_mut(*active_tab) {
                            tab.url = target_url.clone();
                            tab.title = format!("Page: {}", target_url);
                            tab.content = fetch_and_render_web_page(&target_url);
                        }
                        *status_msg = Some(format!("Navigated to {}", target_url));
                        self.dirty = true;
                    }
                }
            }
            WindowContent::DisplaySettings => {
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
                    self.pending_resolution = Some((res_w, res_h));
                    self.dirty = true;
                }
            }
            _ => {}
        }

        if let Some((x, y, name, full, read_only)) = open_editor {
            self.push_focused(Window::new_file_editor(x, y, &name, &full, read_only));
        }
    }

    pub fn render(&mut self, r: &mut Renderer) {
        let theme = get_theme();
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

        draw_taskbar(r, &theme, self, screen_w, screen_h);

        if self.start_menu_open {
            draw_start_menu(r, &theme, screen_w, screen_h);
        }

        if self.control_center_open {
            draw_control_center(r, &theme, screen_w, screen_h);
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
            l.menu.x - 2,
            l.menu.y - 2,
            l.menu.w + 4,
            l.menu.h + 4,
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
        let cc_rect = Rect::new(cc.x - 2, cc.y - 2, cc.w + 4, cc.h + 4);
        if self.control_center_open {
            if self.prev_control_center_rect.is_none() {
                r.add_damage(cc_rect);
            }
            self.prev_control_center_rect = Some(cc_rect);
        } else if let Some(old) = self.prev_control_center_rect.take() {
            r.add_damage(old);
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

        r.add_damage(crate::renderer::Rect::new(0, 0, screen_w as u32, screen_h as u32));
        self.render(r);
        self.dirty = false;

        loop {
            if keyboard::try_read_escape() {
                // Escape сначала закрывает открытое меню/панель и только
                // при их отсутствии завершает графическую сессию.
                if self.start_menu_open || self.control_center_open {
                    self.start_menu_open = false;
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
    let cache_invalid = !desktop.wallpaper_cache.valid
        || desktop.wallpaper_cache.width != w as u32
        || desktop.wallpaper_cache.height != h as u32;

    if cache_invalid {
        // Обои генерируются ОДИН раз (в кеш) при смене темы/стиля/
        // разрешения; движение курсора и перетаскивание окон кеш
        // не трогают — кадр только копирует готовые пиксели.
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

/// Целочисленный квадратный корень (для заливки кругов Plasma Flow).
fn isqrt(v: i64) -> i32 {
    if v <= 0 {
        return 0;
    }
    let mut x = v;
    let mut last = 0i64;
    while x != last {
        last = x;
        x = (x + v / x) / 2;
    }
    x as i32
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
fn generate_wallpaper(pixels: &mut [u32], w: i32, h: i32, theme: &UiTheme) {
    let (w, h) = (w.max(1), h.max(1));
    let wu = w as usize;

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
        // Midnight: графит + тонкая сетка 64 px и редкие акцентные точки.
        1 => {
            let grid = theme.text_secondary;
            for y in 0..h {
                let row = y as usize * wu;
                if y % 64 == 0 {
                    for x in 0..w {
                        blend_into(pixels, row + x as usize, grid, 22);
                    }
                } else {
                    let mut x = 0;
                    while x < w {
                        blend_into(pixels, row + x as usize, grid, 22);
                        x += 64;
                    }
                }
            }
            for i in 0..18 {
                let px = (i * 211 + 37) % w;
                let py = (i * 149 + 83) % h;
                blend_into(pixels, py as usize * wu + px as usize, theme.accent, 150);
            }
        }
        // Plasma Flow: крупные полупрозрачные круги из цветов темы.
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
        }
        // Minimal: светлые поверхности + одна мягкая акцентная полоса.
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
        }
        // Aurora: две диагональные «полярные» ленты + звёзды.
        _ => {
            for y in 0..h {
                let row = y as usize * wu;
                let xc1 = w / 5 + y / 3;
                for dx in -70..=70i32 {
                    let x = xc1 + dx;
                    if x >= 0 && x < w {
                        let alpha = (40 * (70 - dx.abs()) / 70) as u8;
                        blend_into(pixels, row + x as usize, theme.accent, alpha);
                    }
                }
                let xc2 = w * 3 / 4 - y / 4;
                for dx in -90..=90i32 {
                    let x = xc2 + dx;
                    if x >= 0 && x < w {
                        let alpha = (28 * (90 - dx.abs()) / 90) as u8;
                        blend_into(pixels, row + x as usize, theme.accent_hover, alpha);
                    }
                }
            }
            for i in 0..36 {
                let px = (i * 137 + 42) % w;
                let py = (i * 269 + 17) % h;
                if let Some(p) = pixels.get_mut(py as usize * wu + px as usize) {
                    *p = theme.text_primary.0;
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
    let taskbar_y = screen_h - m.taskbar_height as i32;

    let max_w = (screen_w - 8).max(MIN_WINDOW_W as i32) as u32;
    let max_h = (taskbar_y - m.titlebar_height - 8).max(MIN_WINDOW_H as i32) as u32;
    let w = w.clamp(MIN_WINDOW_W, max_w);
    let h = h.clamp(MIN_WINDOW_H, max_h);

    let x = x.clamp(0, (screen_w - w as i32).max(0));
    // Заголовок (titlebar + содержимое начинается ниже) не должен уходить
    // ни выше экрана, ни под панель задач.
    let y = y.clamp(0, (taskbar_y - m.titlebar_height - h as i32).max(0));

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
