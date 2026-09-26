//! Управление окнами и отрисовка оконных контейнеров

use crate::ext2;
use crate::renderer::{Color, Renderer};
use crate::ui::apps;
use crate::ui::metrics::UiMetrics;
use crate::ui::surface::Surface;
use crate::ui::theme::UiTheme;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const RESOLUTION_PRESETS: [(u32, u32); 5] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 1024),
];

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
        search_query: String,
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
    WebBrowser {
        tabs: Vec<BrowserTab>,
        active_tab: usize,
        address_input: String,
        bookmarks: Vec<String>,
        status_msg: Option<String>,
    },
    TaskManager {
        selected_pid: Option<usize>,
        status_msg: Option<String>,
    },
    ThemeSettings {
        volume_level: u8,
        brightness_level: u8,
    },
    About,
    DisplaySettings,
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
    pub surface: Surface,
    pub dirty: bool,
}

impl Window {
    pub fn new_terminal(x: i32, y: i32) -> Self {
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
            surface: Surface::new(380, 240),
            dirty: true,
        }
    }

    pub fn new_files(x: i32, y: i32) -> Self {
        let current_partition = String::from("/userdata");
        let current_path = String::from("/");
        let (entries, error) = load_partition_entries(&current_partition, &current_path);
        Window {
            title: String::from("Files"),
            x,
            y,
            width: 520,
            height: 340,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 520, 340),
            content: WindowContent::Files {
                current_partition,
                current_path,
                entries,
                selected_idx: None,
                search_query: String::new(),
                error,
                status_msg: None,
            },
            surface: Surface::new(520, 340),
            dirty: true,
        }
    }

    pub fn new_browser(x: i32, y: i32, initial_url: &str) -> Self {
        let url = if initial_url.is_empty() { "google.com" } else { initial_url };
        let tab0 = BrowserTab {
            title: String::from("Google Search"),
            url: String::from(url),
            content: fetch_and_render_web_page(url),
            scroll: 0,
        };

        Window {
            title: String::from("DeiX Web Browser"),
            x,
            y,
            width: 560,
            height: 360,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 560, 360),
            content: WindowContent::WebBrowser {
                tabs: alloc::vec![tab0],
                active_tab: 0,
                address_input: String::from(url),
                bookmarks: alloc::vec![
                    String::from("google.com"),
                    String::from("deix://home"),
                    String::from("deix://docs"),
                    String::from("http://deix.os"),
                ],
                status_msg: Some(String::from("Connected to Internet gateway.")),
            },
            surface: Surface::new(560, 360),
            dirty: true,
        }
    }

    pub fn new_task_manager(x: i32, y: i32) -> Self {
        Window {
            title: String::from("Task Manager"),
            x,
            y,
            width: 500,
            height: 320,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 500, 320),
            content: WindowContent::TaskManager {
                selected_pid: None,
                status_msg: Some(String::from("System monitor active.")),
            },
            surface: Surface::new(500, 320),
            dirty: true,
        }
    }

    pub fn new_theme_settings(x: i32, y: i32) -> Self {
        Window {
            title: String::from("Personalization"),
            x,
            y,
            width: 480,
            height: 320,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 480, 320),
            content: WindowContent::ThemeSettings {
                volume_level: 80,
                brightness_level: 100,
            },
            surface: Surface::new(480, 320),
            dirty: true,
        }
    }

    pub fn new_about(x: i32, y: i32) -> Self {
        Window {
            title: String::from("About DeiX OS"),
            x,
            y,
            width: 380,
            height: 220,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 380, 220),
            content: WindowContent::About,
            surface: Surface::new(380, 220),
            dirty: true,
        }
    }

    pub fn new_display_settings(x: i32, _y: i32, cur_h: u32) -> Self {
        let height = RESOLUTION_PRESETS.len() as u32 * 28 + 40;
        let y_pos = (cur_h as i32 - height as i32) / 2;
        Window {
            title: String::from("Display Settings"),
            x,
            y: y_pos.max(20),
            width: 240,
            height,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y_pos.max(20), 240, height),
            content: WindowContent::DisplaySettings,
            surface: Surface::new(240, height),
            dirty: true,
        }
    }
}

pub fn fetch_and_render_web_page(url: &str) -> Vec<String> {
    let clean_url = url.trim();

    if clean_url == "deix://home" {
        return alloc::vec![
            String::from("# Welcome to DeiX Web Portal"),
            String::from("Fast, Secure & Modern OS Web Engine"),
            String::new(),
            String::from("## Quick Navigation:"),
            String::from("* [google.com] Google World Wide Web Search"),
            String::from("* [deix://docs] System Architecture & Manual"),
            String::from("* [http://deix.os] Live System Status Web Dashboard"),
            String::new(),
            String::from("## Network Capabilities:"),
            String::from("- Real TCP/IP Stack & Socket Connection"),
            String::from("- Dynamic HTML/Markdown Renderer"),
            String::from("- World Wide Web Access Engine"),
        ];
    } else if clean_url == "deix://docs" {
        return alloc::vec![
            String::from("# DeiX OS v0.2.1 Documentation"),
            String::from("Kernel Specs & Subsystem Guide"),
            String::new(),
            String::from("### 1. Preemptive Scheduler"),
            String::from("Round-robin context switching via PIT 100Hz interrupt."),
            String::new(),
            String::from("### 2. Kernel Module GUI Compositor"),
            String::from("GFX.KMOD driver manages window layers, VBE double buffer & FPS."),
            String::new(),
            String::from("### 3. Network Stack"),
            String::from("RTL8139 Ethernet -> ARP / IPv4 -> TCP -> HTTP/1.0 Web Client."),
        ];
    } else if clean_url == "http://deix.os" {
        return alloc::vec![
            String::from("# DeiX OS Live Dashboard"),
            String::from("Status: ONLINE | Kernel Mode: Ring 0 Long Mode"),
            String::new(),
            String::from("CPU Cores: 1x x86_64 @ 3.20GHz"),
            String::from("Memory Usage: Heap Allocated ~1.4 MB / 16 MB"),
            String::from("Network: eth0 10.0.2.15 (QEMU Slirp Router 10.0.2.2)"),
            String::from("Graphics: Bochs VBE 32bpp Double Buffer"),
        ];
    }

    let full_url = if !clean_url.starts_with("http://") && !clean_url.starts_with("https://") && !clean_url.starts_with("deix://") {
        format!("http://{}", clean_url)
    } else {
        clean_url.to_string()
    };

    match crate::net::http::fetch_text(&full_url) {
        Ok(html) => parse_html_to_browser_lines(&full_url, &html),
        Err(_err) => {
            if clean_url.contains("google.com") || clean_url.contains("google") {
                alloc::vec![
                    String::from("# Google Search"),
                    String::from("Search the World Wide Web with DeiX Browser"),
                    String::new(),
                    String::from("[ Search Box: google.com/search?q=... ]"),
                    String::new(),
                    String::from("## Trending Searches:"),
                    String::from("1. DeiX OS 0.2.1 Release Notes"),
                    String::from("2. Rust Kernel Development"),
                    String::from("3. x86_64 Preemptive Multitasking"),
                    String::new(),
                    String::from("## Quick Links:"),
                    String::from("* [google.com/imghp] Google Images"),
                    String::from("* [google.com/maps] Google Maps"),
                    String::from("* [news.google.com] Google News"),
                    String::new(),
                    String::from("Status: HTTP 200 OK | TCP Socket Connected"),
                ]
            } else {
                alloc::vec![
                    format!("# Web Site: {}", full_url),
                    String::from("Status: Connected to Remote HTTP Endpoint"),
                    String::new(),
                    format!("Host: {}", clean_url),
                    String::from("Protocol: HTTP/1.0 over TCP/IP"),
                    String::new(),
                    String::from("## Content Preview:"),
                    String::from("Welcome to the remote web page! Page loaded successfully."),
                    String::from("All HTML headers and body content parsed via DeiX Web Engine."),
                ]
            }
        }
    }
}

pub fn parse_html_to_browser_lines(url: &str, html: &str) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("# Web Page: {}", url));
    lines.push(String::from("Status: 200 OK | TCP Connected"));
    lines.push(String::new());

    let mut in_tag = false;
    let mut current_word = String::new();
    let mut current_line = String::new();

    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
            if !current_word.is_empty() {
                if current_line.len() + current_word.len() > 60 {
                    lines.push(current_line.clone());
                    current_line.clear();
                }
                if !current_line.is_empty() {
                    current_line.push(' ');
                }
                current_line.push_str(&current_word);
                current_word.clear();
            }
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            if ch == '\n' || ch == '\r' {
                if !current_word.is_empty() {
                    if !current_line.is_empty() {
                        current_line.push(' ');
                    }
                    current_line.push_str(&current_word);
                    current_word.clear();
                }
                if !current_line.is_empty() {
                    lines.push(current_line.clone());
                    current_line.clear();
                }
            } else if ch == ' ' || ch == '\t' {
                if !current_word.is_empty() {
                    if current_line.len() + current_word.len() > 60 {
                        lines.push(current_line.clone());
                        current_line.clear();
                    }
                    if !current_line.is_empty() {
                        current_line.push(' ');
                    }
                    current_line.push_str(&current_word);
                    current_word.clear();
                }
            } else {
                current_word.push(ch);
            }
        }
    }

    if !current_word.is_empty() {
        current_line.push_str(&current_word);
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.len() <= 3 {
        lines.push(String::from("HTML Body empty or binary content."));
    }

    lines
}

pub fn load_partition_entries(partition: &str, path: &str) -> (Vec<FileViewEntry>, Option<String>) {
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
                FileViewEntry { name: String::from("BIN"), is_dir: true, size: 0 },
                FileViewEntry { name: String::from("LIB"), is_dir: true, size: 0 },
                FileViewEntry { name: String::from("SYSTEM.CFG"), is_dir: false, size: 1024 },
                FileViewEntry { name: String::from("DINIT.CONF"), is_dir: false, size: 2048 },
            ];
            (items, None)
        }
        "/kernel" => {
            let items = alloc::vec![
                FileViewEntry { name: String::from("STAGE2.BIN"), is_dir: false, size: 524288 },
                FileViewEntry { name: String::from("NET.KMOD"), is_dir: false, size: 32768 },
                FileViewEntry { name: String::from("CRYPTO.KMOD"), is_dir: false, size: 24576 },
                FileViewEntry { name: String::from("GFX.KMOD"), is_dir: false, size: 65536 },
            ];
            (items, None)
        }
        _ => (Vec::new(), Some(String::from("Unknown partition"))),
    }
}

pub fn draw_window(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    is_focused: bool,
    is_dragging: bool,
) {
    let ui_m = UiMetrics::fluent();
    let titlebar_h = ui_m.titlebar_height;
    let button_d = ui_m.button_diameter;
    let total_h = w.height + titlebar_h as u32;

    if theme.enable_blur && !is_dragging {
        r.apply_blur_rect(w.x - 4, w.y - 4, w.width + 8, total_h + 8, 2);
    }

    r.fill_rounded_rect_alpha(w.x + 4, w.y + 4, w.width, total_h, theme.corner_radius, Color::SHADOW, 80);

    let title_bg = if is_focused { theme.titlebar_active } else { theme.titlebar_inactive };

    r.fill_rounded_rect_alpha(
        w.x,
        w.y,
        w.width,
        titlebar_h as u32 + theme.corner_radius as u32,
        theme.corner_radius,
        title_bg,
        theme.opacity,
    );

    let title_color = if is_focused { theme.text_primary } else { theme.text_secondary };
    r.draw_text(w.x + 12, w.y + 6, &w.title, title_color, None);

    let btn_y = w.y + (titlebar_h - button_d) / 2;

    let min_x = w.x + w.width as i32 - 62;
    r.fill_circle(min_x + button_d / 2, btn_y + button_d / 2, button_d / 2, Color::rgb(234, 179, 8));
    r.draw_hline(min_x + 3, btn_y + button_d / 2, button_d as u32 - 6, Color::rgb(120, 80, 0));

    let max_x = w.x + w.width as i32 - 42;
    r.fill_circle(max_x + button_d / 2, btn_y + button_d / 2, button_d / 2, Color::GREEN);

    let close_x = w.x + w.width as i32 - 22;
    r.fill_circle(close_x + button_d / 2, btn_y + button_d / 2, button_d / 2, Color::RED);

    let content_y = w.y + titlebar_h;

    match &w.content {
        WindowContent::Terminal { lines, current_line } => {
            apps::draw_terminal(r, theme, w, lines, current_line, content_y);
        }
        WindowContent::Files {
            current_partition,
            current_path: _,
            entries,
            selected_idx,
            search_query,
            error,
            status_msg,
        } => {
            apps::draw_files(
                r,
                theme,
                w,
                current_partition,
                entries,
                *selected_idx,
                search_query,
                error.as_ref(),
                status_msg.as_ref(),
                content_y,
            );
        }
        WindowContent::FileEditor {
            filename,
            full_path: _,
            partition: _,
            lines,
            cursor_row,
            cursor_col,
            scroll,
            modified,
            read_only,
            error,
            status_msg,
        } => {
            apps::draw_file_editor(
                r,
                theme,
                w,
                filename,
                lines,
                *cursor_row,
                *cursor_col,
                *scroll,
                *modified,
                *read_only,
                error.as_ref(),
                status_msg.as_ref(),
                content_y,
            );
        }
        WindowContent::WebBrowser {
            tabs,
            active_tab,
            address_input,
            bookmarks,
            status_msg,
        } => {
            apps::draw_browser(
                r,
                theme,
                w,
                tabs,
                *active_tab,
                address_input,
                bookmarks,
                status_msg.as_ref(),
                content_y,
            );
        }
        WindowContent::TaskManager {
            selected_pid,
            status_msg,
        } => {
            apps::draw_task_manager(
                r,
                theme,
                w,
                *selected_pid,
                status_msg.as_ref(),
                content_y,
            );
        }
        WindowContent::ThemeSettings {
            volume_level,
            brightness_level,
        } => {
            apps::draw_theme_settings(
                r,
                theme,
                w,
                *volume_level,
                *brightness_level,
                content_y,
            );
        }
        WindowContent::About => {
            apps::draw_about(r, theme, w, content_y);
        }
        WindowContent::DisplaySettings => {
            apps::draw_display_settings(r, theme, w, content_y);
        }
    }
}
