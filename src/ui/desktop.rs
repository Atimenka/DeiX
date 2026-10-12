//! Диспетчер рабочего стола DeiX OS (Compositor & Desktop Manager)

use crate::ext2;
use crate::keyboard;
use crate::mouse;
use crate::renderer::{Color, IconType, Renderer};
use crate::timer;
use crate::ui::metrics::UiMetrics;
use crate::ui::surface::WallpaperSurface;
use crate::ui::taskbar::{
    draw_control_center, draw_cursor, draw_start_menu, draw_taskbar, start_menu_items,
    StartMenuAction,
};
use crate::ui::theme::{get_theme, set_theme, UiTheme};
use crate::ui::window::{
    draw_window, fetch_and_render_web_page, load_partition_entries, BrowserTab, Window,
    WindowContent, RESOLUTION_PRESETS,
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
    pub windows: Vec<Window>,
    pub focused_window: Option<usize>,
    pub start_menu_open: bool,
    pub control_center_open: bool,
    pub selected_icon: Option<usize>,
    pub dragging_window: Option<(usize, i32, i32, i32, i32)>,
    pub resizing_window: Option<(usize, i32, i32, u32, u32)>,
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
    /// Пиксели back buffer под курсором (сохраняются перед его отрисовкой).
    cursor_saved_pixels: Vec<u32>,
    /// Фактическая (обрезанная по экрану) область сохранённых пикселей.
    cursor_saved_rect: crate::renderer::Rect,
    cursor_saved_valid: bool,
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
            focused_window: None,
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
            cursor_saved_pixels: Vec::new(),
            cursor_saved_rect: crate::renderer::Rect::new(0, 0, 0, 0),
            cursor_saved_valid: false,
        }
    }

    pub fn open_terminal(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 380) / 2 + (self.windows.len() as i32 * 20) % 100;
        let y = (screen_h - 240) / 2 + (self.windows.len() as i32 * 20) % 100;
        self.windows.push(Window::new_terminal(x, y));
        self.focused_window = Some(self.windows.len() - 1);
        self.start_menu_open = false;
        self.dirty = true;
    }

    pub fn open_files(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 520) / 2 + (self.windows.len() as i32 * 20) % 100;
        let y = (screen_h - 340) / 2 + (self.windows.len() as i32 * 20) % 100;
        self.windows.push(Window::new_files(x, y));
        self.focused_window = Some(self.windows.len() - 1);
        self.start_menu_open = false;
        self.dirty = true;
    }

    pub fn open_browser(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 560) / 2 + (self.windows.len() as i32 * 20) % 100;
        let y = (screen_h - 360) / 2 + (self.windows.len() as i32 * 20) % 100;
        self.windows.push(Window::new_browser(x, y, "deix://home"));
        self.focused_window = Some(self.windows.len() - 1);
        self.start_menu_open = false;
        self.dirty = true;
    }

    pub fn open_task_manager(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 500) / 2;
        let y = (screen_h - 320) / 2;
        self.windows.push(Window::new_task_manager(x, y));
        self.focused_window = Some(self.windows.len() - 1);
        self.start_menu_open = false;
        self.dirty = true;
    }

    pub fn open_error_center(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 520) / 2;
        let y = (screen_h - 360) / 2;
        self.windows.push(Window::new_error_center(x, y));
        self.focused_window = Some(self.windows.len() - 1);
        self.start_menu_open = false;
        self.dirty = true;
    }

    pub fn open_theme_settings(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 480) / 2;
        let y = (screen_h - 320) / 2;
        self.windows.push(Window::new_theme_settings(x, y));
        self.focused_window = Some(self.windows.len() - 1);
        self.start_menu_open = false;
        self.dirty = true;
    }

    pub fn open_about(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 380) / 2;
        let y = (screen_h - 220) / 2;
        self.windows.push(Window::new_about(x, y));
        self.focused_window = Some(self.windows.len() - 1);
        self.start_menu_open = false;
        self.dirty = true;
    }

    pub fn open_display_settings(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 240) / 2;
        self.windows.push(Window::new_display_settings(x, 0, screen_h as u32));
        self.focused_window = Some(self.windows.len() - 1);
        self.start_menu_open = false;
        self.dirty = true;
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
            self.windows.remove(idx);
            self.dirty = true;
            if self.windows.is_empty() {
                self.focused_window = None;
            } else if let Some(focused) = self.focused_window {
                if focused == idx {
                    self.focused_window = Some(self.windows.len() - 1);
                } else if focused > idx {
                    self.focused_window = Some(focused - 1);
                }
            }
        }
    }

    pub fn focus_window(&mut self, idx: usize) {
        if idx < self.windows.len() {
            let win = self.windows.remove(idx);
            self.windows.push(win);
            self.focused_window = Some(self.windows.len() - 1);
            self.dirty = true;
        }
    }

    pub fn toggle_minimize(&mut self, idx: usize) {
        if idx < self.windows.len() {
            self.windows[idx].minimized = !self.windows[idx].minimized;
            self.dirty = true;
            if self.windows[idx].minimized {
                if self.focused_window == Some(idx) {
                    self.focused_window = None;
                    for i in (0..self.windows.len()).rev() {
                        if !self.windows[i].minimized {
                            self.focused_window = Some(i);
                            break;
                        }
                    }
                }
            } else {
                self.focus_window(idx);
            }
        }
    }

    pub fn toggle_maximize(&mut self, idx: usize, screen_w: i32, screen_h: i32) {
        let m = UiMetrics::fluent();
        if idx < self.windows.len() {
            let w = &mut self.windows[idx];
            self.dirty = true;
            if w.maximized {
                let (rx, ry, rw, rh) = w.restore_geometry;
                w.x = rx;
                w.y = ry;
                w.width = rw;
                w.height = rh;
                w.maximized = false;
            } else {
                w.restore_geometry = (w.x, w.y, w.width, w.height);
                w.x = 0;
                w.y = 0;
                w.width = screen_w as u32;
                w.height = (screen_h - m.taskbar_height as i32) as u32;
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

        if m.left_button {
            self.dirty = true;
            if self.start_menu_open && m.x < 260 && m.y < taskbar_y {
                self.handle_start_menu_click(m.x, m.y, taskbar_y, screen_w, screen_h);
                return;
            }

            if m.y >= taskbar_y {
                self.handle_taskbar_click(m.x, screen_w);
                return;
            }

            if self.start_menu_open {
                self.start_menu_open = false;
            }
            if self.control_center_open {
                self.control_center_open = false;
            }

            if let Some((idx, orig_x, orig_y, mx0, my0)) = self.dragging_window {
                if let Some(w) = self.windows.get_mut(idx) {
                    if !w.maximized {
                        w.x = orig_x + (m.x - mx0);
                        w.y = (orig_y + (m.y - my0)).max(0);
                    }
                }
                return;
            }

            if let Some((idx, orig_x, orig_y, orig_w, orig_h)) = self.resizing_window {
                if let Some(w) = self.windows.get_mut(idx) {
                    if !w.maximized {
                        let new_w = (orig_w as i32 + (m.x - orig_x)).max(220) as u32;
                        let new_h = (orig_h as i32 + (m.y - orig_y)).max(140) as u32;
                        w.width = new_w;
                        w.height = new_h;
                    }
                }
                return;
            }

            for i in (0..self.windows.len()).rev() {
                if self.windows[i].minimized {
                    continue;
                }

                let win_x = self.windows[i].x;
                let win_y = self.windows[i].y;
                let win_w = self.windows[i].width;
                let win_h = self.windows[i].height;
                let win_maximized = self.windows[i].maximized;

                let titlebar_rect = (win_x, win_y, win_w, ui_m.titlebar_height as u32);
                let close_btn = (win_x + win_w as i32 - 22, win_y + 6, 14, 14);
                let max_btn = (win_x + win_w as i32 - 42, win_y + 6, 14, 14);
                let min_btn = (win_x + win_w as i32 - 62, win_y + 6, 14, 14);
                let resize_handle = (
                    win_x + win_w as i32 - 16,
                    win_y + win_h as i32 + ui_m.titlebar_height - 16,
                    16,
                    16,
                );

                if point_in_rect(m.x, m.y, close_btn) {
                    self.close_window(i);
                    return;
                }
                if point_in_rect(m.x, m.y, max_btn) {
                    self.toggle_maximize(i, screen_w, screen_h);
                    return;
                }
                if point_in_rect(m.x, m.y, min_btn) {
                    self.toggle_minimize(i);
                    return;
                }
                if point_in_rect(m.x, m.y, resize_handle) && !win_maximized {
                    self.focus_window(i);
                    self.resizing_window = Some((self.windows.len() - 1, m.x, m.y, win_w, win_h));
                    return;
                }
                if point_in_rect(m.x, m.y, titlebar_rect) {
                    self.focus_window(i);
                    let last_idx = self.windows.len() - 1;
                    let cur_x = self.windows[last_idx].x;
                    let cur_y = self.windows[last_idx].y;
                    self.dragging_window = Some((last_idx, cur_x, cur_y, m.x, m.y));
                    return;
                }

                let win_rect = (win_x, win_y, win_w, win_h + ui_m.titlebar_height as u32);
                if point_in_rect(m.x, m.y, win_rect) {
                    self.focus_window(i);
                    self.handle_window_content_click(self.windows.len() - 1, m.x, m.y, screen_w, screen_h);
                    return;
                }
            }

            self.handle_desktop_click(m.x, m.y, screen_w, screen_h);
        } else {
            // Конец перетаскивания/изменения размера: один полный кадр,
            // чтобы вернуть blur (на время drag он отключён ради скорости).
            if self.dragging_window.is_some() || self.resizing_window.is_some() {
                self.dirty = true;
            }
            self.dragging_window = None;
            self.resizing_window = None;
        }

        self.handle_keyboard_for_focused_window();
    }

    fn handle_keyboard_for_focused_window(&mut self) {
        if let Some(focused_idx) = self.focused_window {
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

    fn handle_start_menu_click(
        &mut self,
        x: i32,
        y: i32,
        taskbar_y: i32,
        screen_w: i32,
        screen_h: i32,
    ) {
        let items = start_menu_items();
        let col_w = 118i32;
        let menu_y = taskbar_y - 244;

        if x < 12 || x > 248 || y < menu_y + 42 || y > menu_y + 204 {
            return;
        }

        let col = (x - 12) / col_w;
        let row = (y - (menu_y + 42)) / 32;
        let idx = (row * 2 + col) as usize;

        if let Some((_, action, _)) = items.get(idx) {
            match action {
                StartMenuAction::OpenTerminal => self.open_terminal(screen_w, screen_h),
                StartMenuAction::OpenFiles => self.open_files(screen_w, screen_h),
                StartMenuAction::OpenBrowser => self.open_browser(screen_w, screen_h),
                StartMenuAction::OpenTaskManager => self.open_task_manager(screen_w, screen_h),
                StartMenuAction::OpenErrorCenter => self.open_error_center(screen_w, screen_h),
                StartMenuAction::OpenThemeSettings => self.open_theme_settings(screen_w, screen_h),
                StartMenuAction::OpenAbout => self.open_about(screen_w, screen_h),
                StartMenuAction::OpenDisplaySettings => self.open_display_settings(screen_w, screen_h),
                StartMenuAction::Restart => {
                    crate::println!("  [sys] Rebooting system...");
                    unsafe { crate::port::outb(0x64, 0xFE) };
                }
                StartMenuAction::Shutdown => {
                    crate::println!("  [sys] System shutdown.");
                    unsafe { crate::port::outw(0x604, 0x2000) };
                    self.should_exit = true;
                }
            }
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

        let item_idx = (x - m.start_button_width - 8) / m.taskbar_item_width;
        if item_idx >= 0 && (item_idx as usize) < self.windows.len() {
            self.toggle_minimize(item_idx as usize);
        }
    }

    fn handle_desktop_click(&mut self, x: i32, y: i32, screen_w: i32, screen_h: i32) {
        let shortcuts = desktop_shortcuts();
        for (idx, _) in shortcuts.iter().enumerate() {
            let ix = 20;
            let iy = 20 + idx as i32 * 70;
            if x >= ix && x <= ix + 64 && y >= iy && y <= iy + 60 {
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
        let win = &mut self.windows[win_idx];
        let rel_x = mx - win.x;
        let rel_y = my - (win.y + ui_m.titlebar_height);

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
                                        "'{}' — задача ядра, завершение запрещено.",
                                        t.name
                                    ));
                                }
                                Some(t) => match crate::process::kill(t.pid, -9) {
                                    Ok(()) => {
                                        *status_msg =
                                            Some(format!("Процесс {} (pid {}) завершён.", t.name, t.pid));
                                        *selected_pid = None;
                                    }
                                    Err(e) => {
                                        *status_msg = Some(e.message());
                                    }
                                },
                                None => {
                                    *status_msg = Some(String::from("Задача уже завершилась."));
                                    *selected_pid = None;
                                }
                            }
                        }
                        None => {
                            *status_msg = Some(String::from("Выберите процесс для завершения."));
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
            WindowContent::ThemeSettings { .. } => {
                let current = get_theme();
                if rel_y >= 30 && rel_y <= 60 {
                    self.dirty = true;
                    if rel_x >= 12 && rel_x <= 80 {
                        set_theme(UiTheme::catppuccin());
                    } else if rel_x >= 86 && rel_x <= 150 {
                        set_theme(UiTheme::nord_light());
                    } else if rel_x >= 156 && rel_x <= 220 {
                        set_theme(UiTheme::cyberpunk());
                    } else if rel_x >= 226 && rel_x <= 290 {
                        set_theme(UiTheme::aero_glass());
                    } else if rel_x >= 296 && rel_x <= 360 {
                        set_theme(UiTheme::emerald_forest());
                    } else if rel_x >= 366 && rel_x <= 430 {
                        set_theme(UiTheme::sunset_gold());
                    }
                    self.wallpaper_cache.invalidate();
                }
                if rel_y >= 100 && rel_y <= 130 {
                    self.dirty = true;
                    let mut updated = current;
                    if rel_x >= 12 && rel_x <= 40 {
                        updated.accent = Color::rgb(99, 102, 241);
                    } else if rel_x >= 46 && rel_x <= 74 {
                        updated.accent = Color::rgb(6, 182, 212);
                    } else if rel_x >= 80 && rel_x <= 108 {
                        updated.accent = Color::rgb(16, 185, 129);
                    } else if rel_x >= 114 && rel_x <= 142 {
                        updated.accent = Color::rgb(236, 72, 153);
                    } else if rel_x >= 148 && rel_x <= 176 {
                        updated.accent = Color::rgb(245, 158, 11);
                    } else if rel_x >= 182 && rel_x <= 210 {
                        updated.accent = Color::rgb(168, 85, 247);
                    }
                    set_theme(updated);
                }
                if rel_y >= 160 && rel_y <= 190 {
                    self.dirty = true;
                    let mut updated = current;
                    if rel_x >= 12 && rel_x <= 100 {
                        updated.wallpaper_style = 0;
                    } else if rel_x >= 106 && rel_x <= 194 {
                        updated.wallpaper_style = 1;
                    } else if rel_x >= 200 && rel_x <= 288 {
                        updated.wallpaper_style = 2;
                    } else if rel_x >= 294 && rel_x <= 382 {
                        updated.wallpaper_style = 3;
                    }
                    set_theme(updated);
                    self.wallpaper_cache.invalidate();
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
                if rel_x < 110 {
                    let py = rel_y / 36;
                    // Боковая панель Files: два реальных раздела
                    // (см. src/ui/apps/files.rs, массив partitions).
                    let new_part = match py {
                        0 => "/userdata",
                        _ => "/system",
                    };
                    *current_partition = String::from(new_part);
                    *current_path = String::from("/");
                    let (new_entries, err) =
                        load_partition_entries(current_partition, current_path);
                    *entries = new_entries;
                    *selected_idx = None;
                    *error = err;
                    self.dirty = true;
                    return;
                }

                if rel_y >= 6 && rel_y <= 30 {
                    if rel_x >= 120 && rel_x <= 180 {
                        if current_partition == "/userdata" {
                            let fname = format!("NEW_FILE_{}.TXT", timer::uptime_ms() % 1000);
                            let _ = ext2::write_file(&fname, b"New file created in DeiX Files.\r\n");
                            let (new_entries, err) =
                                load_partition_entries(current_partition, current_path);
                            *entries = new_entries;
                            *error = err;
                            *status_msg = Some(format!("Created {}", fname));
                            self.dirty = true;
                        }
                    } else if rel_x >= 186 && rel_x <= 246 {
                        if current_partition == "/userdata" {
                            let dirname = format!("FOLDER_{}", timer::uptime_ms() % 100);
                            let _ = ext2::mkdir(&dirname);
                            let (new_entries, err) =
                                load_partition_entries(current_partition, current_path);
                            *entries = new_entries;
                            *error = err;
                            *status_msg = Some(format!("Created folder {}", dirname));
                            self.dirty = true;
                        }
                    }
                    return;
                }

                let item_start_y = 44;
                if rel_y >= item_start_y {
                    let idx = ((rel_y - item_start_y) / 22) as usize;
                    if idx < entries.len() {
                        // Повторный клик по уже выбранному файлу открывает его
                        // в редакторе. /system смонтирован только для чтения.
                        if *selected_idx == Some(idx) && !entries[idx].is_dir {
                            let name = entries[idx].name.clone();
                            let base = current_partition.trim_end_matches('/');
                            let dir = current_path.trim_matches('/');
                            let full = if dir.is_empty() {
                                format!("{}/{}", base, name)
                            } else {
                                format!("{}/{}/{}", base, dir, name)
                            };
                            let read_only = current_partition != "/userdata";
                            open_editor = Some((win.x + 28, win.y + 28, name, full, read_only));
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
                let py_start = 30;
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
            self.windows
                .push(Window::new_file_editor(x, y, &name, &full, read_only));
            self.focused_window = Some(self.windows.len() - 1);
            self.dirty = true;
        }
    }

    pub fn render(&mut self, r: &mut Renderer) {
        let theme = get_theme();
        let screen_w = r.width() as i32;
        let screen_h = r.height() as i32;

        self.diff_exposed_regions(r, screen_w, screen_h);

        draw_wallpaper(r, &theme, self, screen_w, screen_h);
        draw_desktop_icons(r, &theme, self.selected_icon);

        let is_dragging = self.dragging_window.is_some();

        for i in 0..self.windows.len() {
            if !self.windows[i].minimized {
                let is_focused = self.focused_window == Some(i);
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
        let taskbar_y = screen_h - ui_m.taskbar_height as i32;

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

        // Меню Пуск: геометрия из draw_start_menu (4, taskbar_y-244, 260x240)
        // + запас на контурную рамку.
        let menu_rect = crate::renderer::Rect::new(2, taskbar_y - 246, 264, 244);
        if self.start_menu_open {
            if self.prev_start_menu_rect.is_none() {
                r.add_damage(menu_rect);
            }
            self.prev_start_menu_rect = Some(menu_rect);
        } else if let Some(old) = self.prev_start_menu_rect.take() {
            r.add_damage(old);
        }

        // Центр управления: геометрия из draw_control_center
        // (screen_w-208, taskbar_y-164, 200x160) + запас на рамку.
        let cc_rect = crate::renderer::Rect::new(screen_w - 210, taskbar_y - 166, 204, 164);
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
            self.windows.push(Window::new_browser(40, 40, "deix://home"));
            self.windows.push(Window::new_files(380, 50));
            self.focused_window = Some(self.windows.len() - 1);
            self.dirty = true;
        } else {
            for i in 0..self.windows.len() {
                if self.windows[i].maximized {
                    self.windows[i].maximized = false;
                    self.toggle_maximize(i, screen_w, screen_h);
                }
            }
            self.dirty = true;
        }

        r.add_damage(crate::renderer::Rect::new(0, 0, screen_w as u32, screen_h as u32));
        self.render(r);
        self.dirty = false;

        loop {
            if keyboard::try_read_escape() {
                self.should_exit = true;
            }

            // Сетевой рабочий поток: ISR карты только складывает кадры
            // в кольцо, разбор стеком идёт здесь, вне прерывания.
            crate::rtl8139::poll_deferred();

            // Квант супервизора: детект упавших служб и политика
            // перезапуска. Без этого вызова RestartPolicy не работает.
            crate::dinit::tick();
            crate::diag::persist::flush();

            self.handle_input(screen_w, screen_h);

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

            unsafe { core::arch::asm!("hlt") };
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
    let ui_m = UiMetrics::fluent();
    let taskbar_h = ui_m.taskbar_height as i32;

    let cache_invalid = !desktop.wallpaper_cache.valid
        || desktop.wallpaper_cache.width != w as u32
        || desktop.wallpaper_cache.height != h as u32;

    if cache_invalid {
        desktop.wallpaper_cache = WallpaperSurface::new(w as u32, h as u32);
        let mut idx = 0;
        for y in 0..h {
            let t = (y * 255) / h;
            let c = theme.bg_top.lerp(theme.bg_bottom, t as u8);
            for _x in 0..w {
                if idx < desktop.wallpaper_cache.pixels.len() {
                    desktop.wallpaper_cache.pixels[idx] = c.0;
                }
                idx += 1;
            }
        }
        desktop.wallpaper_cache.valid = true;
        r.add_damage(crate::renderer::Rect::new(0, 0, w as u32, h as u32));
    }

    if r.damage.full_redraw || cache_invalid {
        let src = &desktop.wallpaper_cache.pixels;
        let dst = &mut r.back_buffer;
        let copy_len = src.len().min(dst.len());
        dst[..copy_len].copy_from_slice(&src[..copy_len]);

        if theme.wallpaper_style == 0 {
            let star_count = 36;
            for i in 0..star_count {
                let sx = ((i * 137 + 42) as i32) % w;
                let sy = ((i * 269 + 17) as i32) % (h - taskbar_h);
                r.put_pixel(sx, sy, theme.accent);
            }
        }
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

        if theme.wallpaper_style == 0 {
            let star_count = 36;
            for i in 0..star_count {
                let sx = ((i * 137 + 42) as i32) % w;
                let sy = ((i * 269 + 17) as i32) % (h - taskbar_h);
                for d in 0..count {
                    if r.damage.rects[d].intersects(&crate::renderer::Rect::new(sx, sy, 1, 1)) {
                        r.put_pixel(sx, sy, theme.accent);
                        break;
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

fn point_in_rect(px: i32, py: i32, rect: (i32, i32, u32, u32)) -> bool {
    let (rx, ry, rw, rh) = rect;
    px >= rx && px < rx + rw as i32 && py >= ry && py < ry + rh as i32
}
