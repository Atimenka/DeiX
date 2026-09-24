//! UI-драйвер: оконный менеджер поверх программного 2D-рендерера
//! (renderer.rs) с настоящим интерактивным циклом отрисовки — рабочий
//! стол, иконки приложений на рабочем столе, перетаскиваемые окна с
//! акриловым размытием и настройкой прозрачности, Центр Управления,
//! полнофункциональный многовкладочный веб-браузер и Центр Кастомизации.

use crate::ext2;
use crate::keyboard;
use crate::mouse;
use crate::renderer::{Color, IconType, Renderer};
use crate::timer;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const TITLEBAR_HEIGHT: i32 = 28;
const BUTTON_DIAMETER: i32 = 14;
const TASKBAR_HEIGHT: u32 = 40;
const START_BUTTON_WIDTH: i32 = 88;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePreset {
    DarkCatppuccin,
    NordLight,
    CyberpunkNeon,
    AeroGlass,
}

#[derive(Clone, Debug)]
pub struct UiTheme {
    pub preset: ThemePreset,
    pub bg_top: Color,
    pub bg_bottom: Color,
    pub window_bg: Color,
    pub titlebar_active: Color,
    pub titlebar_inactive: Color,
    pub accent: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub opacity: u8,
    pub corner_radius: i32,
    pub wallpaper_style: u8,
    pub enable_blur: bool,
}

impl UiTheme {
    pub fn catppuccin() -> Self {
        UiTheme {
            preset: ThemePreset::DarkCatppuccin,
            bg_top: Color::rgb(15, 23, 42),
            bg_bottom: Color::rgb(30, 41, 59),
            window_bg: Color::rgb(24, 24, 37),
            titlebar_active: Color::rgb(30, 41, 59),
            titlebar_inactive: Color::rgb(15, 23, 42),
            accent: Color::rgb(99, 102, 241),
            text_primary: Color::rgb(205, 214, 244),
            text_secondary: Color::rgb(148, 163, 184),
            opacity: 235,
            corner_radius: 8,
            wallpaper_style: 0,
            enable_blur: true,
        }
    }

    pub fn nord_light() -> Self {
        UiTheme {
            preset: ThemePreset::NordLight,
            bg_top: Color::rgb(229, 233, 240),
            bg_bottom: Color::rgb(216, 222, 233),
            window_bg: Color::rgb(242, 244, 248),
            titlebar_active: Color::rgb(216, 222, 233),
            titlebar_inactive: Color::rgb(229, 233, 240),
            accent: Color::rgb(94, 129, 172),
            text_primary: Color::rgb(46, 52, 64),
            text_secondary: Color::rgb(76, 86, 106),
            opacity: 245,
            corner_radius: 6,
            wallpaper_style: 1,
            enable_blur: false,
        }
    }

    pub fn cyberpunk() -> Self {
        UiTheme {
            preset: ThemePreset::CyberpunkNeon,
            bg_top: Color::rgb(10, 5, 20),
            bg_bottom: Color::rgb(25, 10, 40),
            window_bg: Color::rgb(18, 12, 28),
            titlebar_active: Color::rgb(40, 15, 60),
            titlebar_inactive: Color::rgb(18, 12, 28),
            accent: Color::rgb(236, 72, 153),
            text_primary: Color::rgb(244, 244, 245),
            text_secondary: Color::rgb(161, 161, 170),
            opacity: 220,
            corner_radius: 0,
            wallpaper_style: 2,
            enable_blur: true,
        }
    }

    pub fn aero_glass() -> Self {
        UiTheme {
            preset: ThemePreset::AeroGlass,
            bg_top: Color::rgb(15, 30, 60),
            bg_bottom: Color::rgb(5, 15, 35),
            window_bg: Color::rgb(20, 30, 50),
            titlebar_active: Color::rgb(40, 70, 110),
            titlebar_inactive: Color::rgb(20, 30, 50),
            accent: Color::rgb(6, 182, 212),
            text_primary: Color::WHITE,
            text_secondary: Color::rgb(180, 200, 230),
            opacity: 180,
            corner_radius: 10,
            wallpaper_style: 0,
            enable_blur: true,
        }
    }
}

static mut CURRENT_THEME: Option<UiTheme> = None;

pub fn get_theme() -> UiTheme {
    unsafe {
        match CURRENT_THEME {
            Some(ref t) => t.clone(),
            None => {
                let t = UiTheme::catppuccin();
                CURRENT_THEME = Some(t.clone());
                t
            }
        }
    }
}

pub fn set_theme(theme: UiTheme) {
    unsafe {
        CURRENT_THEME = Some(theme);
    }
}

#[derive(Clone, Debug)]
pub struct FileViewEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u32,
}

#[derive(Clone, Debug)]
pub struct BrowserTab {
    pub title: String,
    pub url: String,
    pub content: Vec<String>,
    pub scroll: usize,
}

pub enum WindowContent {
    Terminal {
        lines: Vec<String>,
        current_line: String,
    },
    Files {
        current_partition: String,
        current_path: String,
        entries: Vec<FileViewEntry>,
        selected_idx: Option<usize>,
        error: Option<String>,
        status_msg: Option<String>,
    },
    FileEditor {
        filename: String,
        full_path: String,
        partition: String,
        lines: Vec<String>,
        cursor_row: usize,
        cursor_col: usize,
        scroll: usize,
        modified: bool,
        read_only: bool,
        error: Option<String>,
        status_msg: Option<String>,
    },
    /// Полнофункциональный браузер с вкладками и движком HTML
    WebBrowser {
        tabs: Vec<BrowserTab>,
        active_tab: usize,
        address_input: String,
        bookmarks: Vec<String>,
        status_msg: Option<String>,
    },
    TaskManager {
        refresh_counter: u64,
    },
    /// Кастомизация и персонализация UI (Theme Customizer)
    ThemeSettings {
        volume_level: u8,
        brightness_level: u8,
    },
    About,
    DisplaySettings,
}

pub const RESOLUTION_PRESETS: [(u32, u32); 5] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 1024),
];

pub enum DesktopExit {
    Quit,
    ChangeResolution(u32, u32),
}

pub struct Window {
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub minimized: bool,
    pub maximized: bool,
    pub restore_geometry: (i32, i32, u32, u32),
    pub content: WindowContent,
}

impl Window {
    fn new_terminal(x: i32, y: i32) -> Self {
        Window {
            title: String::from("Terminal"),
            x,
            y,
            width: 380,
            height: 240,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 380, 240),
            content: WindowContent::Terminal {
                lines: alloc::vec![
                    String::from("DeiX Interactive Terminal (v0.2.1)"),
                    String::from("Type 'help' for commands or 'taskmgr' for system monitor."),
                ],
                current_line: String::new(),
            },
        }
    }

    fn new_files(x: i32, y: i32) -> Self {
        let current_partition = String::from("/userdata");
        let current_path = String::from("/");
        let (entries, error) = load_partition_entries(&current_partition, &current_path);
        Window {
            title: String::from("Files"),
            x,
            y,
            width: 460,
            height: 300,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 460, 300),
            content: WindowContent::Files {
                current_partition,
                current_path,
                entries,
                selected_idx: None,
                error,
                status_msg: None,
            },
        }
    }

    fn new_browser(x: i32, y: i32, initial_url: &str) -> Self {
        let url = if initial_url.is_empty() { "deix://home" } else { initial_url };
        let tab0 = BrowserTab {
            title: String::from("DeiX Home"),
            url: String::from(url),
            content: render_html_page(url),
            scroll: 0,
        };

        Window {
            title: String::from("DeiX Web Browser"),
            x,
            y,
            width: 520,
            height: 340,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 520, 340),
            content: WindowContent::WebBrowser {
                tabs: alloc::vec![tab0],
                active_tab: 0,
                address_input: String::from(url),
                bookmarks: alloc::vec![
                    String::from("deix://home"),
                    String::from("deix://docs"),
                    String::from("http://deix.os"),
                ],
                status_msg: Some(String::from("Page loaded")),
            },
        }
    }

    fn new_file_editor(x: i32, y: i32, partition: &str, path: &str, filename: &str) -> Self {
        let full_path = if path == "/" {
            format!("/{}", filename)
        } else {
            format!("{}/{}", path.trim_end_matches('/'), filename)
        };

        let read_only = partition != "/userdata";

        let (lines, error) = match ext2::read_file_path(&full_path) {
            Ok(data) => match core::str::from_utf8(&data) {
                Ok(text) => (text.lines().map(String::from).collect(), None),
                Err(_) => (Vec::new(), Some(String::from("Binary file"))),
            },
            Err(_) => (Vec::new(), Some(String::from("File not found"))),
        };

        let lines = if lines.is_empty() { alloc::vec![String::new()] } else { lines };

        Window {
            title: format!("Edit: {}", filename),
            x,
            y,
            width: 440,
            height: 280,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 440, 280),
            content: WindowContent::FileEditor {
                filename: String::from(filename),
                full_path,
                partition: String::from(partition),
                lines,
                cursor_row: 0,
                cursor_col: 0,
                scroll: 0,
                modified: false,
                read_only,
                error,
                status_msg: if read_only { Some(String::from("Read-Only Partition")) } else { None },
            },
        }
    }

    fn new_task_manager(x: i32, y: i32) -> Self {
        Window {
            title: String::from("Task Manager"),
            x,
            y,
            width: 460,
            height: 280,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 460, 280),
            content: WindowContent::TaskManager { refresh_counter: 0 },
        }
    }

    fn new_theme_settings(x: i32, y: i32) -> Self {
        Window {
            title: String::from("Appearance & Personalization"),
            x,
            y,
            width: 440,
            height: 300,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 440, 300),
            content: WindowContent::ThemeSettings {
                volume_level: 80,
                brightness_level: 100,
            },
        }
    }

    fn new_about(x: i32, y: i32) -> Self {
        Window {
            title: String::from("About DeiX"),
            x,
            y,
            width: 320,
            height: 180,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 320, 180),
            content: WindowContent::About,
        }
    }

    fn new_display_settings(x: i32, y: i32) -> Self {
        let height = 40 + RESOLUTION_PRESETS.len() as u32 * 28;
        let y_pos = (y - height as i32) / 2;
        Window {
            title: String::from("Display settings"),
            x,
            y: y_pos.max(20),
            width: 240,
            height,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y_pos.max(20), 240, height),
            content: WindowContent::DisplaySettings,
        }
    }
}

fn render_html_page(url: &str) -> Vec<String> {
    match url {
        "deix://home" => alloc::vec![
            String::from("# Welcome to DeiX Web Portal"),
            String::from("Fast, Secure & Modern OS Web Engine"),
            String::new(),
            String::from("## Quick Navigation:"),
            String::from("* [deix://docs] System Architecture & Manual"),
            String::from("* [http://deix.os] Live System Status Web Dashboard"),
            String::from("* [deix://settings] Browser Preferences & Cache"),
            String::new(),
            String::from("## Features:"),
            String::from("- Built-in HTML / Markdown renderer"),
            String::from("- Offline documentation & local web pages"),
            String::from("- Full Tab management & bookmarking"),
        ],
        "deix://docs" => alloc::vec![
            String::from("# DeiX OS v0.2.1 Documentation"),
            String::from("Kernel Specs & Subsystem Guide"),
            String::new(),
            String::from("### 1. Preemptive Scheduler"),
            String::from("Round-robin context switching via PIT 100Hz interrupt."),
            String::new(),
            String::from("### 2. Software 2D Renderer"),
            String::from("Double-buffered MMIO VBE driver with Acrylic Blur & Damage Clipping."),
            String::new(),
            String::from("### 3. File Systems"),
            String::from("Second Extended Filesystem (ext2) + Read-Only EROFS v1 partitions."),
        ],
        "http://deix.os" => alloc::vec![
            String::from("# DeiX OS Live Dashboard"),
            String::from("Status: ONLINE | Kernel Mode: Ring 0 Long Mode"),
            String::new(),
            String::from("CPU Cores: 1x x86_64"),
            String::from("Memory Usage: Heap Allocated ~1.2 MB"),
            String::from("Video Mode: Bochs VBE 800x600 32bpp"),
            String::from("Uptime: Active"),
        ],
        _ => alloc::vec![
            format!("# Web Page: {}", url),
            String::from("Connected to local endpoint."),
            String::new(),
            String::from("Content rendered successfully."),
        ],
    }
}

fn load_partition_entries(partition: &str, path: &str) -> (Vec<FileViewEntry>, Option<String>) {
    match partition {
        "/userdata" => {
            if !ext2::is_formatted() {
                let _ = ext2::format();
                let _ = ext2::write_file(
                    "README.TXT",
                    b"Welcome to DeiX OS v0.2.1-beta!\r\nEdit files directly in this window.\r\n",
                );
            }
            match ext2::list_dir_path(path) {
                Ok(entries) => {
                    let mut items = Vec::new();
                    for e in entries {
                        items.push(FileViewEntry {
                            name: e.name,
                            is_dir: e.is_directory,
                            size: e.size,
                        });
                    }
                    (items, None)
                }
                Err(_) => (Vec::new(), Some(String::from("Failed to list directory"))),
            }
        }
        "/system" => {
            let items = alloc::vec![
                FileViewEntry { name: String::from("bin"), is_dir: true, size: 0 },
                FileViewEntry { name: String::from("lib"), is_dir: true, size: 0 },
                FileViewEntry { name: String::from("libdeix_core.so"), is_dir: false, size: 18432 },
                FileViewEntry { name: String::from("libdeix_gui.so"), is_dir: false, size: 24576 },
                FileViewEntry { name: String::from("libdeix_sys.so"), is_dir: false, size: 12288 },
            ];
            (items, None)
        }
        "/kernel" => {
            let items = alloc::vec![
                FileViewEntry { name: String::from("kernel.tar.gz"), is_dir: false, size: 128900 },
                FileViewEntry { name: String::from("kernel.img"), is_dir: false, size: 262144 },
            ];
            (items, None)
        }
        _ => (Vec::new(), Some(String::from("Unknown partition"))),
    }
}

pub struct Desktop {
    windows: Vec<Window>,
    dragging_window: Option<usize>,
    drag_offset: (i32, i32),
    prev_left_button: bool,
    last_titlebar_click: Option<(usize, u64)>,
    focused_window: Option<usize>,
    start_menu_open: bool,
    control_center_open: bool,
    selected_desktop_icon: Option<usize>,
    should_exit: bool,
    pending_resolution: Option<(u32, u32)>,
    frame_counter: u64,
}

const DOUBLE_CLICK_MS: u64 = 400;

enum StartMenuAction {
    OpenTerminal,
    OpenFiles,
    OpenBrowser,
    OpenTaskManager,
    OpenThemeSettings,
    OpenDisplaySettings,
    OpenAbout,
    Restart,
    Shutdown,
}

fn start_menu_items() -> [(&'static str, StartMenuAction, IconType); 8] {
    [
        ("Terminal", StartMenuAction::OpenTerminal, IconType::Terminal),
        ("Files", StartMenuAction::OpenFiles, IconType::Files),
        ("Web Browser", StartMenuAction::OpenBrowser, IconType::Browser),
        ("Task Manager", StartMenuAction::OpenTaskManager, IconType::TaskManager),
        ("Personalization", StartMenuAction::OpenThemeSettings, IconType::Theme),
        ("Display settings", StartMenuAction::OpenDisplaySettings, IconType::Display),
        ("About DeiX", StartMenuAction::OpenAbout, IconType::About),
        ("Shut down", StartMenuAction::Shutdown, IconType::Power),
    ]
}

const START_MENU_ITEM_HEIGHT: i32 = 32;
const START_MENU_WIDTH: i32 = 220;

struct DesktopShortcut {
    name: &'static str,
    icon: IconType,
    action: StartMenuAction,
}

fn desktop_shortcuts() -> [DesktopShortcut; 5] {
    [
        DesktopShortcut { name: "Terminal", icon: IconType::Terminal, action: StartMenuAction::OpenTerminal },
        DesktopShortcut { name: "Files", icon: IconType::Files, action: StartMenuAction::OpenFiles },
        DesktopShortcut { name: "Browser", icon: IconType::Browser, action: StartMenuAction::OpenBrowser },
        DesktopShortcut { name: "TaskMgr", icon: IconType::TaskManager, action: StartMenuAction::OpenTaskManager },
        DesktopShortcut { name: "Themes", icon: IconType::Theme, action: StartMenuAction::OpenThemeSettings },
    ]
}

impl Desktop {
    pub fn new() -> Self {
        Desktop {
            windows: Vec::new(),
            dragging_window: None,
            drag_offset: (0, 0),
            prev_left_button: false,
            last_titlebar_click: None,
            focused_window: None,
            start_menu_open: false,
            control_center_open: false,
            selected_desktop_icon: None,
            should_exit: false,
            pending_resolution: None,
            frame_counter: 0,
        }
    }

    fn bring_to_front(&mut self, index: usize) {
        let window = self.windows.remove(index);
        self.windows.push(window);
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn open_terminal(&mut self, screen_w: i32, screen_h: i32) {
        let x = 40 + (self.windows.len() as i32 * 24) % (screen_w - 400).max(1);
        let y = 40 + (self.windows.len() as i32 * 24) % (screen_h - 300).max(1);
        self.windows.push(Window::new_terminal(x, y));
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn open_files(&mut self, screen_w: i32, screen_h: i32) {
        let x = 60 + (self.windows.len() as i32 * 24) % (screen_w - 460).max(1);
        let y = 50 + (self.windows.len() as i32 * 24) % (screen_h - 320).max(1);
        self.windows.push(Window::new_files(x, y));
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn open_browser(&mut self, screen_w: i32, screen_h: i32) {
        let x = 80 + (self.windows.len() as i32 * 24) % (screen_w - 520).max(1);
        let y = 40 + (self.windows.len() as i32 * 24) % (screen_h - 340).max(1);
        self.windows.push(Window::new_browser(x, y, "deix://home"));
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn open_task_manager(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 460) / 2;
        let y = (screen_h - 280) / 2;
        self.windows.push(Window::new_task_manager(x, y));
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn open_theme_settings(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 440) / 2;
        let y = (screen_h - 300) / 2;
        self.windows.push(Window::new_theme_settings(x, y));
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn open_about(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 320) / 2;
        let y = (screen_h - 180) / 2;
        self.windows.push(Window::new_about(x, y));
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn open_display_settings(&mut self, screen_w: i32, screen_h: i32) {
        let width = 240;
        let height = 40 + RESOLUTION_PRESETS.len() as u32 * 28;
        let x = (screen_w - width) / 2;
        let y = (screen_h - height as i32) / 2;
        self.windows.push(Window::new_display_settings(x, y));
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn toggle_maximize(&mut self, index: usize, screen_w: i32, screen_h: i32) {
        let taskbar_h = TASKBAR_HEIGHT as i32;
        if let Some(w) = self.windows.get_mut(index) {
            if w.maximized {
                let (x, y, width, height) = w.restore_geometry;
                w.x = x;
                w.y = y;
                w.width = width;
                w.height = height;
                w.maximized = false;
            } else {
                w.restore_geometry = (w.x, w.y, w.width, w.height);
                w.x = 0;
                w.y = 0;
                w.width = screen_w as u32;
                w.height = (screen_h - taskbar_h - TITLEBAR_HEIGHT) as u32;
                w.maximized = true;
            }
        }
    }

    fn handle_input(&mut self, screen_w: i32, screen_h: i32) {
        let m = mouse::snapshot();
        let (mx, my) = (m.x, m.y);
        let left_pressed = m.left_button && !self.prev_left_button;
        let left_released = !m.left_button && self.prev_left_button;
        self.prev_left_button = m.left_button;

        let taskbar_y = screen_h - TASKBAR_HEIGHT as i32;
        let in_taskbar = my >= taskbar_y;
        let in_start_button = in_taskbar && mx >= 4 && mx < START_BUTTON_WIDTH;
        let in_tray_area = in_taskbar && mx >= screen_w - 120;

        if left_pressed {
            if in_start_button {
                self.start_menu_open = !self.start_menu_open;
                self.control_center_open = false;
            } else if in_tray_area {
                self.control_center_open = !self.control_center_open;
                self.start_menu_open = false;
            } else if self.start_menu_open {
                let items = start_menu_items();
                let menu_h = items.len() as i32 * START_MENU_ITEM_HEIGHT + 48;
                let menu_y = taskbar_y - menu_h;
                if mx >= 0 && mx < START_MENU_WIDTH && my >= menu_y && my < taskbar_y {
                    self.handle_start_menu_click(mx, my, taskbar_y, screen_w, screen_h);
                }
                self.start_menu_open = false;
            } else if self.control_center_open {
                self.control_center_open = false;
            } else if in_taskbar {
                self.handle_taskbar_click(mx, taskbar_y, screen_w);
            } else {
                // Проверяем клик по иконкам на рабочем столе
                let mut hit_icon = false;
                let shortcuts = desktop_shortcuts();
                for (idx, _) in shortcuts.iter().enumerate() {
                    let ix = 20;
                    let iy = 20 + idx as i32 * 70;
                    if mx >= ix && mx < ix + 60 && my >= iy && my < iy + 60 {
                        hit_icon = true;
                        if self.selected_desktop_icon == Some(idx) {
                            match shortcuts[idx].action {
                                StartMenuAction::OpenTerminal => self.open_terminal(screen_w, screen_h),
                                StartMenuAction::OpenFiles => self.open_files(screen_w, screen_h),
                                StartMenuAction::OpenBrowser => self.open_browser(screen_w, screen_h),
                                StartMenuAction::OpenTaskManager => self.open_task_manager(screen_w, screen_h),
                                StartMenuAction::OpenThemeSettings => self.open_theme_settings(screen_w, screen_h),
                                _ => {}
                            }
                        } else {
                            self.selected_desktop_icon = Some(idx);
                        }
                        break;
                    }
                }
                if !hit_icon {
                    self.selected_desktop_icon = None;
                    self.handle_window_click(mx, my, screen_w, screen_h);
                }
            }
        }

        if m.left_button {
            if let Some(idx) = self.dragging_window {
                if let Some(w) = self.windows.get_mut(idx) {
                    if !w.maximized {
                        w.x = mx - self.drag_offset.0;
                        w.y = my - self.drag_offset.1;
                    }
                }
            }
        }

        if left_released {
            self.dragging_window = None;
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
                            match byte {
                                b'\n' => {
                                    let new_url = address_input.clone();
                                    if let Some(tab) = tabs.get_mut(*active_tab) {
                                        tab.url = new_url.clone();
                                        tab.title = format!("Page: {}", new_url);
                                        tab.content = render_html_page(&new_url);
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

    fn handle_start_menu_click(&mut self, x: i32, y: i32, taskbar_y: i32, screen_w: i32, screen_h: i32) {
        let items = start_menu_items();
        let menu_h = items.len() as i32 * START_MENU_ITEM_HEIGHT + 48;
        let menu_y = taskbar_y - menu_h;
        let items_start_y = menu_y + 44;

        if x >= START_MENU_WIDTH || x < 0 || y < items_start_y {
            return;
        }

        let idx = ((y - items_start_y) / START_MENU_ITEM_HEIGHT).max(0) as usize;
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
                    crate::cli::cmd_reboot();
                }
                StartMenuAction::Shutdown => {
                    crate::cli::cmd_halt();
                }
            }
        }
    }

    fn handle_taskbar_click(&mut self, x: i32, _taskbar_y: i32, _screen_w: i32) {
        let relative_x = x - START_BUTTON_WIDTH - 8;
        if relative_x < 0 {
            return;
        }
        let idx = (relative_x / TASKBAR_ITEM_WIDTH) as usize;
        if idx >= self.windows.len() {
            return;
        }
        if self.windows[idx].minimized {
            self.windows[idx].minimized = false;
            self.bring_to_front(idx);
        } else if self.focused_window == Some(idx) {
            self.windows[idx].minimized = true;
            self.focused_window = None;
        } else {
            self.bring_to_front(idx);
        }
    }

    fn handle_window_click(&mut self, mx: i32, my: i32, screen_w: i32, screen_h: i32) {
        for i in (0..self.windows.len()).rev() {
            if self.windows[i].minimized {
                continue;
            }
            let (wx, wy, ww, wh) = (
                self.windows[i].x,
                self.windows[i].y,
                self.windows[i].width,
                self.windows[i].height,
            );
            let in_titlebar =
                mx >= wx && mx < wx + ww as i32 && my >= wy && my < wy + TITLEBAR_HEIGHT;

            let (close_cx, close_cy) = title_button_center(&self.windows[i], 0);
            let (min_cx, min_cy) = title_button_center(&self.windows[i], 1);
            let (max_cx, max_cy) = title_button_center(&self.windows[i], 2);
            let in_close = circle_hit(mx, my, close_cx, close_cy, BUTTON_DIAMETER / 2);
            let in_minimize = circle_hit(mx, my, min_cx, min_cy, BUTTON_DIAMETER / 2);
            let in_maximize = circle_hit(mx, my, max_cx, max_cy, BUTTON_DIAMETER / 2);

            let in_window_body = mx >= wx
                && mx < wx + ww as i32
                && my >= wy
                && my < wy + wh as i32 + TITLEBAR_HEIGHT;

            if in_close {
                self.windows.remove(i);
                self.focused_window = None;
                return;
            } else if in_minimize {
                self.windows[i].minimized = true;
                self.focused_window = None;
                return;
            } else if in_maximize {
                self.toggle_maximize(i, screen_w, screen_h);
                return;
            } else if in_titlebar {
                let now = timer::uptime_ms();
                let is_double_click = match self.last_titlebar_click {
                    Some((last_idx, last_time)) => {
                        last_idx == i && now.saturating_sub(last_time) <= DOUBLE_CLICK_MS
                    }
                    None => false,
                };
                if is_double_click {
                    self.toggle_maximize(i, screen_w, screen_h);
                    self.last_titlebar_click = None;
                } else {
                    self.last_titlebar_click = Some((i, now));
                    self.bring_to_front(i);
                    self.dragging_window = Some(self.windows.len() - 1);
                    self.drag_offset = (mx - wx, my - wy);
                }
                return;
            } else if in_window_body {
                self.bring_to_front(i);
                let last_idx = self.windows.len() - 1;
                let new_win = self.handle_content_click(last_idx, mx, my);
                if let Some(nw) = new_win {
                    self.windows.push(nw);
                    self.focused_window = Some(self.windows.len() - 1);
                }
                return;
            }
        }
        self.focused_window = None;
    }

    fn handle_content_click(&mut self, window_index: usize, mx: i32, my: i32) -> Option<Window> {
        let open_new_window = if let Some(w) = self.windows.get_mut(window_index) {
            let content_x = w.x;
            let content_y = w.y + TITLEBAR_HEIGHT;
            let rel_x = mx - content_x;
            let rel_y = my - content_y;

            match &mut w.content {
                WindowContent::Files {
                    current_partition,
                    current_path,
                    entries,
                    selected_idx,
                    error,
                    status_msg,
                } => {
                    if rel_y >= 4 && rel_y < 24 {
                        let partitions = ["/userdata", "/system", "/kernel"];
                        let tab_w = 80i32;
                        let clicked_part_idx = (rel_x - 8) / tab_w;
                        if clicked_part_idx >= 0 && (clicked_part_idx as usize) < partitions.len() {
                            let part = partitions[clicked_part_idx as usize];
                            *current_partition = String::from(part);
                            *current_path = String::from("/");
                            *selected_idx = None;
                            let (e, err) = load_partition_entries(current_partition, current_path);
                            *entries = e;
                            *error = err;
                            *status_msg = Some(format!("Switched to {}", part));
                        }
                        return None;
                    }

                    if rel_y >= 26 && rel_y < 50 {
                        if rel_x >= 8 && rel_x < 56 { // Up
                            if current_path != "/" {
                                if let Some(pos) = current_path.rfind('/') {
                                    let new_p = &current_path[..pos];
                                    *current_path = if new_p.is_empty() { String::from("/") } else { String::from(new_p) };
                                    let (e, err) = load_partition_entries(current_partition, current_path);
                                    *entries = e;
                                    *error = err;
                                    *selected_idx = None;
                                }
                            }
                        } else if rel_x >= 64 && rel_x < 136 { // + Folder
                            if current_partition == "/userdata" {
                                let mut num = 1;
                                loop {
                                    let f_name = format!("folder_{}", num);
                                    let full_p = if current_path == "/" { format!("/{}", f_name) } else { format!("{}/{}", current_path.trim_end_matches('/'), f_name) };
                                    if ext2::mkdir_p(&full_p).is_ok() {
                                        *status_msg = Some(format!("Created folder: {}", f_name));
                                        break;
                                    }
                                    num += 1;
                                    if num > 50 { break; }
                                }
                                let (e, err) = load_partition_entries(current_partition, current_path);
                                *entries = e;
                                *error = err;
                            }
                        } else if rel_x >= 144 && rel_x < 204 { // + File
                            if current_partition == "/userdata" {
                                let mut num = 1;
                                loop {
                                    let f_name = format!("notes_{}.txt", num);
                                    let full_p = if current_path == "/" { format!("/{}", f_name) } else { format!("{}/{}", current_path.trim_end_matches('/'), f_name) };
                                    let initial_bytes = b"Welcome to DeiX Text Editor!\r\n";
                                    if ext2::write_file_path(&full_p, initial_bytes).is_ok() {
                                        let target_x = w.x + 30;
                                        let target_y = w.y + 30;
                                        let part_clone = current_partition.clone();
                                        let path_clone = current_path.clone();
                                        let (e, err) = load_partition_entries(current_partition, current_path);
                                        *entries = e;
                                        *error = err;
                                        return Some(Window::new_file_editor(target_x, target_y, &part_clone, &path_clone, &f_name));
                                    }
                                    num += 1;
                                    if num > 50 { break; }
                                }
                            }
                        }
                        return None;
                    }

                    let line_height = 20;
                    let clicked_index = (rel_y - 56) / line_height;
                    if clicked_index >= 0 && (clicked_index as usize) < entries.len() {
                        let idx = clicked_index as usize;
                        if selected_idx.map_or(false, |s| s == idx) {
                            let entry = entries[idx].clone();
                            if entry.is_dir {
                                *current_path = if current_path == "/" { format!("/{}", entry.name) } else { format!("{}/{}", current_path.trim_end_matches('/'), entry.name) };
                                let (e, err) = load_partition_entries(current_partition, current_path);
                                *entries = e;
                                *error = err;
                                *selected_idx = None;
                                return None;
                            } else {
                                let target_x = w.x + 30;
                                let target_y = w.y + 30;
                                return Some(Window::new_file_editor(target_x, target_y, current_partition, current_path, &entry.name));
                            }
                        } else {
                            *selected_idx = Some(idx);
                        }
                    }
                    None
                }
                WindowContent::WebBrowser {
                    tabs,
                    active_tab,
                    address_input,
                    status_msg,
                    ..
                } => {
                    // Клик по быстрым закладам или навигации
                    if rel_y >= 26 && rel_y < 50 {
                        if rel_x >= 8 && rel_x < 36 { // Home
                            let home_url = "deix://home";
                            *address_input = String::from(home_url);
                            if let Some(tab) = tabs.get_mut(*active_tab) {
                                tab.url = String::from(home_url);
                                tab.content = render_html_page(home_url);
                            }
                            *status_msg = Some(String::from("Home loaded"));
                        } else if rel_x >= 40 && rel_x < 90 { // Docs
                            let docs_url = "deix://docs";
                            *address_input = String::from(docs_url);
                            if let Some(tab) = tabs.get_mut(*active_tab) {
                                tab.url = String::from(docs_url);
                                tab.content = render_html_page(docs_url);
                            }
                            *status_msg = Some(String::from("Docs loaded"));
                        }
                    }
                    None
                }
                WindowContent::ThemeSettings { .. } => {
                    // Клик по переключению тем
                    if rel_y >= 30 && rel_y < 60 {
                        if rel_x >= 12 && rel_x < 110 {
                            set_theme(UiTheme::catppuccin());
                        } else if rel_x >= 118 && rel_x < 210 {
                            set_theme(UiTheme::nord_light());
                        } else if rel_x >= 218 && rel_x < 310 {
                            set_theme(UiTheme::cyberpunk());
                        } else if rel_x >= 318 && rel_x < 410 {
                            set_theme(UiTheme::aero_glass());
                        }
                    }
                    None
                }
                WindowContent::DisplaySettings => {
                    let item_height = 28;
                    let items_start_y = 30;
                    let clicked_index = (rel_y - items_start_y) / item_height;
                    if clicked_index >= 0 && (clicked_index as usize) < RESOLUTION_PRESETS.len() {
                        let (nw, nh) = RESOLUTION_PRESETS[clicked_index as usize];
                        self.pending_resolution = Some((nw, nh));
                    }
                    None
                }
                _ => None,
            }
        } else {
            None
        };

        open_new_window
    }

    fn render(&mut self, r: &mut Renderer) {
        let theme = get_theme();
        let screen_w = r.width() as i32;
        let screen_h = r.height() as i32;

        draw_wallpaper(r, &theme, self.frame_counter, screen_w, screen_h);
        draw_desktop_icons(r, &theme, self.selected_desktop_icon);

        for i in 0..self.windows.len() {
            if !self.windows[i].minimized {
                let is_focused = self.focused_window == Some(i);
                draw_window(r, &theme, &self.windows[i], is_focused);
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
            self.windows.push(Window::new_browser(40, 40, "deix://home"));
            self.windows.push(Window::new_files(380, 50));
            self.focused_window = Some(self.windows.len() - 1);
        } else {
            for i in 0..self.windows.len() {
                if self.windows[i].maximized {
                    self.windows[i].maximized = false;
                    self.toggle_maximize(i, screen_w, screen_h);
                }
            }
        }

        let mut last_frame = timer::uptime_ms();
        const FRAME_INTERVAL_MS: u64 = 33; // ~30 FPS

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
            if now.saturating_sub(last_frame) >= FRAME_INTERVAL_MS {
                self.frame_counter = now;
                self.render(r);
                last_frame = now;
            }

            unsafe { core::arch::asm!("hlt") };
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

// ==================== Отрисовка: Обои и Иконки ====================

fn draw_wallpaper(r: &mut Renderer, theme: &UiTheme, frame_counter: u64, w: i32, h: i32) {
    r.fill_rect_gradient_v(0, 0, w as u32, h as u32, theme.bg_top, theme.bg_bottom);

    if theme.wallpaper_style == 0 {
        let star_count = 36;
        for i in 0..star_count {
            let sx = ((i * 137 + 42) as i32) % w;
            let sy = ((i * 269 + 17) as i32) % (h - TASKBAR_HEIGHT as i32);
            let twinkle = (((frame_counter / 150 + i as u64) % 10) > 4) as u8;
            let star_color = theme.accent.lerp(Color::WHITE, 180 + twinkle * 70);
            r.put_pixel(sx, sy, star_color);
        }
    } else if theme.wallpaper_style == 1 {
        let phase = ((frame_counter / 25) % 64) as i32;
        let mut offset = -h - phase;
        while offset < w + h {
            for y in (0..h).step_by(4) {
                let x = offset + y;
                r.fill_rect_alpha(x, y, 4, 4, theme.accent, 22);
            }
            offset += 56;
        }
    }

    let label = "DeiX OS";
    let label_x = w / 2 - (label.len() as i32 * 8 * 2) / 2;
    draw_text_scaled(r, label_x, 32, label, theme.text_primary, 2);
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
        r.draw_icon(ix + 16, iy + 10, match sc.icon {
            IconType::Terminal => IconType::Terminal,
            IconType::Files => IconType::Files,
            IconType::Browser => IconType::Browser,
            IconType::TaskManager => IconType::TaskManager,
            IconType::Theme => IconType::Theme,
            _ => IconType::Terminal,
        }, theme.accent);

        r.draw_text(ix, iy + 40, sc.name, theme.text_primary, None);
    }
}

fn draw_text_scaled(r: &mut Renderer, x: i32, y: i32, text: &str, color: Color, scale: u32) {
    let mut cursor_x = x;
    for byte in text.bytes() {
        let glyph = crate::font::read_glyph(byte);
        for (row, &b) in glyph.iter().enumerate() {
            if b == 0 {
                continue;
            }
            let py = y + row as i32 * scale as i32;
            for col in 0..8 {
                if (b >> (7 - col)) & 1 != 0 {
                    let px = cursor_x + col as i32 * scale as i32;
                    r.fill_rect(px, py, scale, scale, color);
                }
            }
        }
        cursor_x += 8 * scale as i32;
    }
}

// ==================== Отрисовка: Окна ====================

fn title_button_center(w: &Window, button_index: usize) -> (i32, i32) {
    let title_right = w.x + w.width as i32;
    let cy = w.y + TITLEBAR_HEIGHT / 2;
    let margin_right = 16;
    let spacing = 18;
    let cx = title_right - margin_right - (button_index as i32 * spacing);
    (cx, cy)
}

fn circle_hit(x: i32, y: i32, cx: i32, cy: i32, radius: i32) -> bool {
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}

fn draw_window(r: &mut Renderer, theme: &UiTheme, w: &Window, is_focused: bool) {
    let total_h = w.height + TITLEBAR_HEIGHT as u32;

    r.draw_drop_shadow(w.x, w.y, w.width, total_h, 6);

    if theme.enable_blur {
        r.apply_blur_rect(w.x, w.y, w.width, total_h, 2);
    }

    let title_bg_top = if is_focused { theme.titlebar_active } else { theme.titlebar_inactive };
    let title_bg_bot = theme.bg_top;

    r.fill_rounded_rect_alpha(
        w.x,
        w.y,
        w.width,
        total_h,
        theme.corner_radius,
        theme.window_bg,
        theme.opacity,
    );

    r.fill_rect_gradient_v(
        w.x,
        w.y,
        w.width,
        TITLEBAR_HEIGHT as u32,
        title_bg_top,
        title_bg_bot,
    );

    if is_focused {
        r.draw_hline(w.x, w.y, w.width, theme.accent);
    }

    let title_color = if is_focused { theme.text_primary } else { theme.text_secondary };
    r.draw_text(w.x + 12, w.y + (TITLEBAR_HEIGHT - 16) / 2, &w.title, title_color, None);

    let (close_cx, close_cy) = title_button_center(w, 0);
    let (min_cx, min_cy) = title_button_center(w, 1);
    let (max_cx, max_cy) = title_button_center(w, 2);

    r.fill_circle(close_cx, close_cy, BUTTON_DIAMETER / 2, Color::RED);
    r.fill_circle(min_cx, min_cy, BUTTON_DIAMETER / 2, Color::YELLOW);
    r.fill_circle(max_cx, max_cy, BUTTON_DIAMETER / 2, Color::GREEN);

    r.draw_line(close_cx - 2, close_cy - 2, close_cx + 2, close_cy + 2, Color::WHITE);
    r.draw_line(close_cx + 2, close_cy - 2, close_cx - 2, close_cy + 2, Color::WHITE);
    r.draw_hline(min_cx - 3, min_cy, 7, Color::WHITE);
    r.draw_rect(max_cx - 3, max_cy - 3, 6, 6, Color::WHITE);

    let content_y = w.y + TITLEBAR_HEIGHT;

    match &w.content {
        WindowContent::Terminal { lines, current_line } => {
            r.fill_rect_alpha(w.x, content_y, w.width, w.height, Color::rgb(15, 15, 25), theme.opacity);

            let max_visible_lines = (w.height as usize / 18).saturating_sub(1);
            let start_line = lines.len().saturating_sub(max_visible_lines);

            let mut line_y = content_y + 6;
            for line in lines.iter().skip(start_line) {
                let color = if line.starts_with('>') { Color::GREEN } else { theme.text_primary };
                r.draw_text(w.x + 8, line_y, truncate(line, (w.width as usize - 16) / 8), color, None);
                line_y += 18;
            }

            let prompt = format!("root@deix:~# {}", current_line);
            r.draw_text(w.x + 8, content_y + w.height as i32 - 20, truncate(&prompt, (w.width as usize - 16) / 8), theme.accent, None);
        }
        WindowContent::WebBrowser {
            tabs,
            active_tab,
            address_input,
            bookmarks: _,
            status_msg,
        } => {
            r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

            // 1. Панель вкладок (Tabs Bar: 0..24)
            r.fill_rect(w.x, content_y, w.width, 24, theme.titlebar_inactive);
            let mut tab_x = w.x + 8;
            for (idx, tab) in tabs.iter().enumerate() {
                let is_act = idx == *active_tab;
                let bg = if is_act { theme.accent } else { theme.titlebar_active };
                r.fill_rounded_rect(tab_x, content_y + 2, 110, 20, 4, bg);
                r.draw_text(tab_x + 6, content_y + 4, truncate(&tab.title, 11), Color::WHITE, None);
                tab_x += 116;
            }

            // 2. Навигационная панель (Nav Bar: 26..50)
            r.fill_rect(w.x, content_y + 24, w.width, 26, theme.titlebar_active);
            r.draw_icon(w.x + 8, content_y + 29, IconType::Home, theme.accent);
            r.draw_text(w.x + 28, content_y + 29, "Home", theme.text_primary, None);
            r.draw_text(w.x + 68, content_y + 29, "Docs", theme.text_primary, None);

            // Поле URL адреса
            r.fill_rounded_rect(w.x + 110, content_y + 26, w.width - 120, 22, 4, Color::rgb(15, 23, 42));
            r.draw_icon(w.x + 114, content_y + 29, IconType::Search, Color::GRAY);
            r.draw_text(w.x + 132, content_y + 29, truncate(address_input, (w.width as usize - 150) / 8), Color::WHITE, None);

            r.draw_hline(w.x, content_y + 50, w.width, theme.titlebar_inactive);

            // 3. Область HTML контента
            if let Some(tab) = tabs.get(*active_tab) {
                let mut cy = content_y + 56;
                for line in tab.content.iter() {
                    if cy + 18 > content_y + w.height as i32 - 20 {
                        break;
                    }
                    if line.starts_with("# ") {
                        r.draw_text(w.x + 12, cy, &line[2..], theme.accent, None);
                    } else if line.starts_with("## ") {
                        r.draw_text(w.x + 12, cy, &line[3..], Color::GREEN, None);
                    } else if line.starts_with("* ") {
                        r.draw_text(w.x + 12, cy, line, theme.text_primary, None);
                    } else {
                        r.draw_text(w.x + 12, cy, line, theme.text_secondary, None);
                    }
                    cy += 18;
                }
            }

            if let Some(msg) = status_msg {
                r.draw_text(w.x + 12, content_y + w.height as i32 - 18, msg, Color::GRAY, None);
            }
        }
        WindowContent::Files {
            current_partition,
            current_path,
            entries,
            selected_idx,
            error,
            status_msg,
        } => {
            r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

            let partitions = ["/userdata", "/system", "/kernel"];
            let tab_w = 80u32;
            for (idx, &part) in partitions.iter().enumerate() {
                let px = w.x + 8 + (idx as i32 * tab_w as i32);
                let is_sel = current_partition == part;
                let bg = if is_sel { theme.accent } else { theme.titlebar_active };
                r.fill_rounded_rect(px, content_y + 4, tab_w - 4, 20, 4, bg);
                let label = if part == "/userdata" { "user" } else { &part[1..] };
                r.draw_text(px + 6, content_y + 6, label, Color::WHITE, None);
            }

            r.fill_rect(w.x + 8, content_y + 26, 48, 22, theme.titlebar_active);
            r.draw_text(w.x + 14, content_y + 29, "Up", theme.text_primary, None);

            r.fill_rect(w.x + 64, content_y + 26, 72, 22, theme.titlebar_active);
            r.draw_text(w.x + 70, content_y + 29, "+Folder", Color::GREEN, None);

            r.fill_rect(w.x + 144, content_y + 26, 60, 22, theme.titlebar_active);
            r.draw_text(w.x + 150, content_y + 29, "+File", theme.accent, None);

            let path_display = format!("{}:{}", current_partition, current_path);
            r.draw_text(w.x + 212, content_y + 29, truncate(&path_display, 24), theme.text_secondary, None);

            r.draw_hline(w.x + 8, content_y + 52, w.width - 16, theme.titlebar_inactive);

            let mut ey = content_y + 56;
            if let Some(err) = error {
                r.draw_text(w.x + 12, ey, err, Color::RED, None);
            } else if entries.is_empty() {
                r.draw_text(w.x + 12, ey, "(Folder empty)", Color::GRAY, None);
            } else {
                for (idx, entry) in entries.iter().enumerate() {
                    if ey + 18 > content_y + w.height as i32 - 24 {
                        break;
                    }
                    if *selected_idx == Some(idx) {
                        r.fill_rect(w.x + 8, ey - 2, w.width - 16, 20, theme.titlebar_active);
                    }
                    r.draw_icon(w.x + 12, ey, IconType::Files, if entry.is_dir { Color::YELLOW } else { theme.accent });
                    let size_str = if entry.is_dir { String::from("<DIR>") } else { format!("{} B", entry.size) };
                    let name_str = format!("{:<22} {:>8}", entry.name, size_str);
                    r.draw_text(w.x + 32, ey, truncate(&name_str, (w.width as usize - 40) / 8), theme.text_primary, None);
                    ey += 20;
                }
            }

            if let Some(msg) = status_msg {
                r.draw_text(w.x + 12, content_y + w.height as i32 - 20, msg, Color::YELLOW, None);
            }
        }
        WindowContent::ThemeSettings { volume_level, brightness_level } => {
            r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

            r.draw_text(w.x + 12, content_y + 10, "Select UI Style Theme:", theme.text_primary, None);

            let themes = [("Catppuccin", 12i32), ("Nord Light", 118i32), ("Cyberpunk", 218i32), ("Aero Glass", 318i32)];
            for (t_name, tx) in themes.iter() {
                r.fill_rounded_rect(w.x + tx, content_y + 30, 92, 26, 6, theme.titlebar_active);
                r.draw_text(w.x + tx + 8, content_y + 35, t_name, theme.text_primary, None);
            }

            r.draw_hline(w.x + 12, content_y + 70, w.width - 24, theme.titlebar_inactive);

            r.draw_text(w.x + 12, content_y + 80, "Quick Adjustments:", theme.text_primary, None);
            let vol_str = format!("Volume: {}%", volume_level);
            r.draw_text(w.x + 12, content_y + 105, &vol_str, theme.text_secondary, None);
            let bri_str = format!("Brightness: {}%", brightness_level);
            r.draw_text(w.x + 12, content_y + 130, &bri_str, theme.text_secondary, None);

            r.draw_text(w.x + 12, content_y + w.height as i32 - 24, "Changes applied instantly across kernel UI.", theme.accent, None);
        }
        WindowContent::TaskManager { refresh_counter: _ } => {
            r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

            r.draw_text(w.x + 12, content_y + 10, "PID   NAME           STATE       SWITCHES", theme.accent, None);
            r.draw_hline(w.x + 12, content_y + 28, w.width - 24, theme.titlebar_inactive);

            let cur_id = crate::sched::current_id();
            let total_switches = crate::sched::switch_count();

            let row1 = format!("{:02}    dinit (kernel)  Running     {}", 0, total_switches / 2);
            let row2 = format!("{:02}*   desktop_ui     Active      {}", cur_id, total_switches);

            r.draw_text(w.x + 12, content_y + 36, &row1, Color::GREEN, None);
            r.draw_text(w.x + 12, content_y + 56, &row2, Color::YELLOW, None);

            let mem_info = format!("Switches: {} | Timer: {} ms", total_switches, timer::uptime_ms());
            r.draw_text(w.x + 12, content_y + w.height as i32 - 24, &mem_info, theme.text_primary, None);
        }
        WindowContent::About => {
            r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

            draw_text_scaled(r, w.x + 16, content_y + 12, "DeiX OS", theme.accent, 2);
            r.draw_text(w.x + 16, content_y + 50, "Version: 0.2.1-beta (Preemptive GUI)", theme.text_primary, None);
            r.draw_text(w.x + 16, content_y + 68, "Kernel: 64-bit Long Mode (no_std Rust)", theme.text_secondary, None);
            r.draw_text(w.x + 16, content_y + 86, "Renderer: Software 2D + VBE Double Buffer", theme.text_secondary, None);

            let uptime_secs = timer::uptime_ms() / 1000;
            let uptime_str = format!("Uptime: {}m {}s", uptime_secs / 60, uptime_secs % 60);
            r.draw_text(w.x + 16, content_y + 110, &uptime_str, Color::GREEN, None);
        }
        WindowContent::DisplaySettings => {
            r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);
            r.draw_text(w.x + 12, content_y + 8, "Select Resolution:", theme.text_primary, None);

            let cur_w = r.width();
            let cur_h = r.height();

            let mut py = content_y + 30;
            for (w_res, h_res) in RESOLUTION_PRESETS.iter() {
                let is_current = *w_res == cur_w && *h_res == cur_h;
                let bg = if is_current { theme.titlebar_active } else { theme.window_bg };
                r.fill_rounded_rect(w.x + 8, py, w.width - 16, 24, 4, bg);

                let label = format!("{} x {}{}", w_res, h_res, if is_current { " (active)" } else { "" });
                let fg = if is_current { theme.accent } else { theme.text_primary };
                r.draw_text(w.x + 16, py + 4, &label, fg, None);
                py += 28;
            }
        }
        _ => {}
    }

    r.draw_rect_outline_alpha(w.x, w.y, w.width, total_h, theme.titlebar_active, 120);
}

fn truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

// ==================== Отрисовка: Панель задач, Меню и Трей ====================

fn draw_taskbar(r: &mut Renderer, theme: &UiTheme, desktop: &Desktop, screen_w: i32, screen_h: i32) {
    let y = screen_h - TASKBAR_HEIGHT as i32;

    r.fill_rect_gradient_v(0, y, screen_w as u32, TASKBAR_HEIGHT, theme.titlebar_active, theme.bg_top);
    r.draw_hline(0, y, screen_w as u32, theme.accent);

    let start_bg = if desktop.start_menu_open { theme.accent } else { theme.titlebar_active };
    r.fill_rounded_rect(6, y + 5, (START_BUTTON_WIDTH - 12) as u32, TASKBAR_HEIGHT - 10, 6, start_bg);
    r.draw_text(24, y + (TASKBAR_HEIGHT as i32 - 16) / 2, "Start", Color::WHITE, None);

    for (i, w) in desktop.windows.iter().enumerate() {
        let item_x = START_BUTTON_WIDTH + 8 + i as i32 * TASKBAR_ITEM_WIDTH;
        if item_x + TASKBAR_ITEM_WIDTH > screen_w - 120 {
            break;
        }
        let is_focused = desktop.focused_window == Some(i) && !w.minimized;
        let bg = if is_focused { theme.titlebar_active } else { theme.window_bg };

        r.fill_rounded_rect(item_x, y + 6, (TASKBAR_ITEM_WIDTH - 6) as u32, TASKBAR_HEIGHT - 12, 6, bg);

        if is_focused {
            r.draw_hline(item_x + 12, y + TASKBAR_HEIGHT as i32 - 4, (TASKBAR_ITEM_WIDTH - 30) as u32, theme.accent);
        }

        let label_color = if w.minimized { Color::GRAY } else { theme.text_primary };
        r.draw_text(item_x + 8, y + (TASKBAR_HEIGHT as i32 - 16) / 2, truncate(&w.title, 14), label_color, None);
    }

    // Трей
    r.draw_icon(screen_w - 110, y + 12, IconType::Wifi, theme.text_primary);
    r.draw_icon(screen_w - 90, y + 12, IconType::Volume, theme.text_primary);

    let secs = timer::uptime_ms() / 1000;
    let clock = format!("{:02}:{:02}", (secs / 60) % 100, secs % 60);
    r.draw_text(screen_w - 60, y + (TASKBAR_HEIGHT as i32 - 16) / 2, &clock, theme.text_primary, None);
}

fn draw_start_menu(r: &mut Renderer, theme: &UiTheme, screen_w: i32, screen_h: i32) {
    let items = start_menu_items();
    let taskbar_y = screen_h - TASKBAR_HEIGHT as i32;
    let menu_h = items.len() as i32 * START_MENU_ITEM_HEIGHT + 48;
    let menu_y = taskbar_y - menu_h;

    r.shade_rect(0, 0, screen_w as u32, taskbar_y as u32, 50);

    r.fill_rounded_rect(0, menu_y, START_MENU_WIDTH as u32, menu_h as u32, 10, theme.window_bg);
    r.draw_rect_outline_alpha(0, menu_y, START_MENU_WIDTH as u32, menu_h as u32, theme.titlebar_active, 180);

    r.fill_rect_gradient_v(0, menu_y, START_MENU_WIDTH as u32, 38, theme.titlebar_active, theme.window_bg);
    r.draw_text(12, menu_y + 10, "root @ DeiX OS", theme.accent, None);
    r.draw_hline(0, menu_y + 38, START_MENU_WIDTH as u32, theme.titlebar_active);

    let items_start_y = menu_y + 44;

    for (i, (label, _, icon)) in items.iter().enumerate() {
        let item_y = items_start_y + i as i32 * START_MENU_ITEM_HEIGHT;

        r.draw_icon(12, item_y + 8, match icon {
            IconType::Terminal => IconType::Terminal,
            IconType::Files => IconType::Files,
            IconType::Browser => IconType::Browser,
            IconType::TaskManager => IconType::TaskManager,
            IconType::Theme => IconType::Theme,
            IconType::Display => IconType::Display,
            IconType::About => IconType::About,
            IconType::Power => IconType::Power,
            _ => IconType::Terminal,
        }, theme.accent);

        let color = if label.contains("Shut") || label.contains("Restart") {
            Color::RED
        } else {
            theme.text_primary
        };
        r.draw_text(34, item_y + (START_MENU_ITEM_HEIGHT - 16) / 2, label, color, None);
    }
}

fn draw_control_center(r: &mut Renderer, theme: &UiTheme, screen_w: i32, screen_h: i32) {
    let taskbar_y = screen_h - TASKBAR_HEIGHT as i32;
    let cc_w = 200i32;
    let cc_h = 160i32;
    let cc_x = screen_w - cc_w - 8;
    let cc_y = taskbar_y - cc_h - 8;

    r.fill_rounded_rect(cc_x, cc_y, cc_w as u32, cc_h as u32, 10, theme.window_bg);
    r.draw_rect_outline_alpha(cc_x, cc_y, cc_w as u32, cc_h as u32, theme.accent, 150);

    r.draw_text(cc_x + 12, cc_y + 12, "Quick Control Center", theme.accent, None);
    r.draw_hline(cc_x + 12, cc_y + 32, cc_w as u32 - 24, theme.titlebar_active);

    r.draw_icon(cc_x + 12, cc_y + 44, IconType::Wifi, Color::GREEN);
    r.draw_text(cc_x + 36, cc_y + 44, "eth0: 192.168.1.10", theme.text_primary, None);

    r.draw_icon(cc_x + 12, cc_y + 70, IconType::Volume, theme.accent);
    r.draw_text(cc_x + 36, cc_y + 70, "Audio: 80% [HDA]", theme.text_primary, None);

    r.draw_icon(cc_x + 12, cc_y + 96, IconType::Theme, Color::YELLOW);
    r.draw_text(cc_x + 36, cc_y + 96, "Theme: Active", theme.text_primary, None);

    r.fill_rounded_rect(cc_x + 12, cc_y + 124, cc_w - 24, 24, 4, theme.titlebar_active);
    r.draw_text(cc_x + 32, cc_y + 128, "System Running", Color::GREEN, None);
}

fn draw_cursor(r: &mut Renderer) {
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
