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
    draw_window, fetch_and_render_web_page, load_partition_entries, BrowserTab, FileViewEntry,
    Window, WindowContent, RESOLUTION_PRESETS,
};
use alloc::format;
use alloc::string::{String, ToString};
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
}

impl Desktop {
    pub fn new() -> Self {
        crate::module::register_builtin_module("GFX.KMOD", (1, 0), 65536);

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
        self.windows.push(Window::new_browser(x, y, "google.com"));
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

        match &mut win.content {
            WindowContent::TaskManager {
                selected_pid,
                status_msg,
            } => {
                let ty_start = 56;
                if rel_y >= ty_start && rel_y <= ty_start + 120 {
                    let pid_click = (rel_y - ty_start) / 20;
                    if pid_click >= 0 && pid_click < 7 {
                        *selected_pid = Some(pid_click as usize);
                        self.dirty = true;
                    }
                }

                if rel_x >= win.width as i32 - 110
                    && rel_x <= win.width as i32 - 10
                    && rel_y >= win.height as i32 - 32
                    && rel_y <= win.height as i32 - 8
                {
                    self.dirty = true;
                    if let Some(pid) = *selected_pid {
                        if pid == 0 || pid == 1 || pid == 2 {
                            *status_msg = Some(format!("PID {} is SYSTEM PROTECTED!", pid));
                        } else {
                            *status_msg = Some(format!("Task PID {} terminated successfully.", pid));
                            *selected_pid = None;
                        }
                    } else {
                        *status_msg = Some(String::from("Select a process to kill."));
                    }
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
                    let new_part = match py {
                        0 => "/userdata",
                        1 => "/system",
                        2 => "/kernel",
                        _ => "/userdata",
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
                        *selected_idx = Some(idx);
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
                            title: String::from("Google Search"),
                            url: String::from("google.com"),
                            content: fetch_and_render_web_page("google.com"),
                            scroll: 0,
                        };
                        tabs.push(new_tab);
                        *active_tab = tabs.len() - 1;
                        *address_input = String::from("google.com");
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
    }

    pub fn render(&mut self, r: &mut Renderer) {
        let theme = get_theme();
        let screen_w = r.width() as i32;
        let screen_h = r.height() as i32;

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

        draw_cursor(r);
        r.present();
    }

    pub fn run_event_loop(&mut self, r: &mut Renderer, preserve_windows: bool) -> DesktopExit {
        let screen_w = r.width() as i32;
        let screen_h = r.height() as i32;

        if !preserve_windows {
            self.windows.push(Window::new_browser(40, 40, "google.com"));
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

        let mut last_frame = timer::uptime_ms();
        const FRAME_INTERVAL_MS: u64 = 33;

        loop {
            if keyboard::try_read_escape() {
                self.should_exit = true;
            }

            self.handle_input(screen_w, screen_h);

            if let Some((w, h)) = self.pending_resolution.take() {
                return DesktopExit::ChangeResolution(w, h);
            }

            if self.should_exit {
                return DesktopExit::Quit;
            }

            let now = timer::uptime_ms();
            if self.dirty || now.saturating_sub(last_frame) >= FRAME_INTERVAL_MS {
                self.frame_counter = now;
                self.render(r);
                self.dirty = false;
                last_frame = now;
            }

            unsafe { core::arch::asm!("hlt") };
        }
    }
}

pub fn run_desktop_session(mut on_resolution_change: impl FnMut(u32, u32) -> bool) {
    let mut desktop = Desktop::new();
    let mut preserve_windows = false;

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

    if !desktop.wallpaper_cache.valid
        || desktop.wallpaper_cache.width != w as u32
        || desktop.wallpaper_cache.height != h as u32
    {
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
    }

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

    r.add_damage(crate::renderer::Rect::new(0, 0, w as u32, h as u32));
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
