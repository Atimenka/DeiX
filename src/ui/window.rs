//! Управление окнами и отрисовка оконных контейнеров

use crate::ext2;
use crate::renderer::{Color, Renderer};
use crate::ui::apps;
use crate::ui::metrics::UiMetrics;
use crate::ui::theme::UiTheme;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
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
    ErrorCenter {
        /// Индекс выбранной записи в списке `error_center::visible_records`.
        selected: Option<usize>,
        /// Прокрутка списка (индекс первой видимой записи).
        scroll: usize,
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
                    alloc::format!("DeiX Interactive Terminal (v{})", crate::KERNEL_VERSION),
                    String::from("Type 'help' for commands or 'taskmgr' for system monitor."),
                ],
                current_line: String::new(),
            },
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
        }
    }

    pub fn new_browser(x: i32, y: i32, initial_url: &str) -> Self {
        let url = if initial_url.is_empty() { "deix://home" } else { initial_url };
        let tab0 = BrowserTab {
            title: String::from("DeiX Browser"),
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
                    String::from("example.com"),
                    String::from("deix://home"),
                    String::from("deix://docs"),
                    String::from("http://deix.os"),
                ],
                status_msg: Some(String::from("HTTP client ready (text-only, no TLS).")),
            },
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
        }
    }

    pub fn new_error_center(x: i32, y: i32) -> Self {
        Window {
            title: String::from("Error Center"),
            x,
            y,
            width: 520,
            height: 360,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 520, 360),
            content: WindowContent::ErrorCenter {
                selected: None,
                scroll: 0,
            },
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
        }
    }

    /// Открывает редактор, читая файл через VFS. Ошибка чтения не рисует
    /// пустой документ как настоящий: редактор открывается только для
    /// чтения и показывает причину в строке состояния.
    pub fn new_file_editor(x: i32, y: i32, filename: &str, path: &str, read_only: bool) -> Self {
        let (lines, error, read_only) = match crate::vfs::read_file(path) {
            Ok(data) => {
                let text = String::from_utf8_lossy(&data);
                let mut lines: Vec<String> =
                    text.lines().map(String::from).collect();
                if lines.is_empty() {
                    lines.push(String::new());
                }
                (lines, None, read_only)
            }
            Err(e) => (alloc::vec![String::new()], Some(e.message()), true),
        };
        Window {
            title: format!("Editor - {}", filename),
            x,
            y,
            width: 440,
            height: 300,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 440, 300),
            content: WindowContent::FileEditor {
                filename: filename.to_string(),
                full_path: path.to_string(),
                lines,
                cursor_row: 0,
                cursor_col: 0,
                scroll: 0,
                modified: false,
                read_only,
                error,
                status_msg: if read_only {
                    Some(String::from("только чтение"))
                } else {
                    Some(String::from("сохранение при закрытии окна"))
                },
            },
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
        }
    }
}

pub fn fetch_and_render_web_page(url: &str) -> Vec<String> {
    let clean_url = url.trim();

    if clean_url == "deix://home" {
        return alloc::vec![
            String::from("# DeiX Browser"),
            String::from("Limited text-oriented HTTP client."),
            String::new(),
            String::from("## Quick Navigation:"),
            String::from("* [example.com] Plain-HTTP test page"),
            String::from("* [deix://docs] System Architecture & Manual"),
            String::from("* [http://deix.os] System Status (local page)"),
            String::new(),
            String::from("## Honest limitations:"),
            String::from("- HTTP/1.0 only: no HTTPS/TLS (most modern sites redirect to HTTPS and will not load)"),
            String::from("- Text rendering only: no CSS, JavaScript or images"),
            String::from("- DNS: real A-record lookups over UDP/53 (no DNSSEC, no IPv6)"),
        ];
    } else if clean_url == "deix://docs" {
        return alloc::vec![
            alloc::format!("# {} Documentation", crate::OS_RELEASE),
            String::from("Kernel Specs & Subsystem Guide"),
            String::new(),
            String::from("### 1. Preemptive Scheduler"),
            String::from("Round-robin context switching via PIT 100Hz interrupt."),
            String::new(),
            String::from("### 2. Kernel Module GUI Compositor"),
            String::from("Built-in compositor manages window layers, VBE double buffer & FPS."),
            String::new(),
            String::from("### 3. Network Stack"),
            String::from("RTL8139 Ethernet -> ARP / IPv4 -> ICMP + UDP (DNS) + TCP -> HTTP/1.0."),
            String::from("Hostnames are resolved with a real DNS client (A records, UDP/53)."),
        ];
    } else if clean_url == "http://deix.os" {
        // Локальная страница с НАСТОЯЩИМИ значениями подсистем — никакой
        // выдуманной телеметрии.
        let heap = crate::allocator::stats();
        let ip = crate::net::my_ip();
        let gw = crate::net::gateway_ip();
        let dns = crate::net::dns_ip();
        let cpu = crate::cpuid::brand_string()
            .unwrap_or_else(|| crate::cpuid::vendor_string());
        return alloc::vec![
            String::from("# DeiX OS Status (local page)"),
            alloc::format!("{} | Ring 0 Long Mode", crate::OS_RELEASE),
            String::new(),
            alloc::format!("CPU: {}", cpu.trim()),
            alloc::format!(
                "Heap: {} KiB used / {} KiB total",
                heap.used / 1024,
                heap.total / 1024
            ),
            alloc::format!(
                "Network: eth0 {}.{}.{}.{} (gw {}.{}.{}.{}, dns {}.{}.{}.{})",
                ip[0], ip[1], ip[2], ip[3], gw[0], gw[1], gw[2], gw[3],
                dns[0], dns[1], dns[2], dns[3]
            ),
        ];
    }

    let full_url = if !clean_url.starts_with("http://") && !clean_url.starts_with("https://") && !clean_url.starts_with("deix://") {
        format!("http://{}", clean_url)
    } else {
        clean_url.to_string()
    };

    match crate::net::http::fetch(&full_url) {
        Ok(resp) => parse_html_to_browser_lines(&resp),
        // Показываем настоящую причину сбоя из net::http. Подменять её
        // «успешной» страницей нельзя: иначе сеть не диагностируется.
        Err(err) => alloc::vec![
            format!("# Не удалось загрузить {}", full_url),
            String::new(),
            format!("Ошибка: {}", err),
            String::new(),
            String::from("Возможные причины:"),
            String::from("* сетевая карта не инициализирована (нет eth0)"),
            String::from("* DNS-сервер недоступен или имя не существует"),
            String::from("* хост недоступен или порт закрыт"),
            String::from("* сайт доступен только по HTTPS (TLS не поддерживается)"),
            String::new(),
            String::from("Проверьте в CLI: `ifconfig`, `dns <host>`, `ping <ip>`."),
        ],
    }
}

pub fn parse_html_to_browser_lines(resp: &crate::net::http::HttpResponse) -> Vec<String> {
    let html = &resp.body;
    let mut lines = Vec::new();
    lines.push(format!("# Web Page: {}", resp.final_url));
    // Настоящая строка статуса из ответа сервера, а не вечное «200 OK».
    if resp.status == 0 {
        lines.push(String::from("Status: unknown (no HTTP status line in reply)"));
    } else {
        lines.push(format!("Status: {} {}", resp.status, resp.reason));
    }
    lines.push(String::new());

    let cs: Vec<char> = html.chars().collect();

    if let Some(title) = extract_title(&cs) {
        if !title.is_empty() {
            lines.push(format!("## {}", title));
            lines.push(String::new());
        }
    }

    // Проход по документу: содержимое <script>/<style> и комментарии
    // пропускаются (иначе минифицированный JS/CSS высыпается на экран
    // «разбросанными буквами»), блочные теги дают перенос строки,
    // HTML-сущности декодируются.
    let mut text = String::new();
    let mut i = 0usize;
    while i < cs.len() {
        if cs[i] == '<' {
            if starts_with_ci(&cs, i, "<!--") {
                i = find_ci(&cs, i, "-->").map(|p| p + 3).unwrap_or(cs.len());
                continue;
            }
            if starts_with_ci(&cs, i, "<script") {
                i = find_ci(&cs, i, "</script").unwrap_or(cs.len());
                i = skip_past_gt(&cs, i);
                text.push('\n');
                continue;
            }
            if starts_with_ci(&cs, i, "<style") {
                i = find_ci(&cs, i, "</style").unwrap_or(cs.len());
                i = skip_past_gt(&cs, i);
                text.push('\n');
                continue;
            }
            let tag_end = skip_past_gt(&cs, i);
            let name = tag_name(&cs, i);
            if name == "li" {
                text.push_str("\n* ");
            } else if is_block_tag(&name) {
                text.push('\n');
            }
            i = tag_end;
        } else if cs[i] == '&' {
            let (ch, next) = decode_entity(&cs, i);
            text.push(ch);
            i = next;
        } else {
            let c = cs[i];
            if c == '\r' {
                // игнорируем, перенос даст \n
            } else if c == '\n' || c == '\t' {
                text.push(' ');
            } else {
                text.push(c);
            }
            i += 1;
        }
    }

    // Сборка строк: схлопывание пробелов, перенос по словам на 72
    // символа, слова длиннее лимита режутся жёстко (длинные URL).
    const WRAP: usize = 72;
    let mut blank_pending = false;
    for para in text.split('\n') {
        if para.split_whitespace().next().is_none() {
            // пустой абзац: максимум одна пустая строка подряд
            blank_pending = true;
            continue;
        }
        if blank_pending && !lines.is_empty() {
            lines.push(String::new());
        }
        blank_pending = false;
        let mut line = String::new();
        for word in para.split_whitespace() {
            let mut rest = word;
            loop {
                if line.is_empty() {
                    if rest.chars().count() <= WRAP {
                        line.push_str(rest);
                        break;
                    }
                    // жёсткий разрез сверхдлинного слова по границе символа
                    let cut = char_prefix_len(rest, WRAP);
                    line.push_str(&rest[..cut]);
                    lines.push(core::mem::take(&mut line));
                    rest = &rest[cut..];
                } else if line.chars().count() + 1 + rest.chars().count() <= WRAP {
                    line.push(' ');
                    line.push_str(rest);
                    break;
                } else {
                    lines.push(core::mem::take(&mut line));
                }
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }

    if lines.len() <= 3 {
        lines.push(String::from("HTML Body empty or binary content."));
    }

    lines
}

/// Регистронезависимое (ASCII) сравнение с образцом на позиции pos.
fn starts_with_ci(cs: &[char], pos: usize, pat: &str) -> bool {
    let pc: Vec<char> = pat.chars().collect();
    if pos + pc.len() > cs.len() {
        return false;
    }
    pc.iter()
        .enumerate()
        .all(|(k, p)| cs[pos + k].to_ascii_lowercase() == *p)
}

/// Поиск образца (ASCII, без регистра) начиная с from. Возвращает позицию.
fn find_ci(cs: &[char], from: usize, pat: &str) -> Option<usize> {
    let pc: Vec<char> = pat.chars().collect();
    if pc.is_empty() || cs.len() < pc.len() {
        return None;
    }
    (from..=cs.len() - pc.len()).find(|&p| {
        pc.iter()
            .enumerate()
            .all(|(k, q)| cs[p + k].to_ascii_lowercase() == *q)
    })
}

/// Позиция сразу за ближайшим '>' (конец тега).
fn skip_past_gt(cs: &[char], from: usize) -> usize {
    let mut i = from;
    while i < cs.len() {
        if cs[i] == '>' {
            return i + 1;
        }
        i += 1;
    }
    cs.len()
}

/// Имя тега в нижнем регистре: "</div class..>" -> "div".
fn tag_name(cs: &[char], pos: usize) -> String {
    let mut i = pos + 1;
    if i < cs.len() && cs[i] == '/' {
        i += 1;
    }
    let mut name = String::new();
    while i < cs.len() && cs[i].is_ascii_alphanumeric() && name.len() < 12 {
        name.push(cs[i].to_ascii_lowercase());
        i += 1;
    }
    name
}

/// Блочные теги, дающие перенос строки.
fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "p" | "div" | "br" | "tr" | "ul" | "ol" | "table" | "section" | "article"
            | "header" | "footer" | "nav" | "form" | "blockquote" | "pre" | "hr"
            | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "title" | "head" | "body"
    )
}

/// Содержимое <title>...</title>, если есть.
fn extract_title(cs: &[char]) -> Option<String> {
    let open = find_ci(cs, 0, "<title")?;
    let start = skip_past_gt(cs, open);
    let close = find_ci(cs, start, "</title")?;
    let mut t = String::new();
    let mut i = start;
    while i < close {
        if cs[i] == '&' {
            let (ch, next) = decode_entity(cs, i);
            t.push(ch);
            i = next;
        } else {
            t.push(cs[i]);
            i += 1;
        }
    }
    let trimmed: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.len() > 120 {
        None
    } else {
        Some(trimmed)
    }
}

/// Декодирует HTML-сущность на позиции pos ('&'): возвращает символ и
/// позицию за сущностью. Нераспознанное — просто '&', позиция +1.
fn decode_entity(cs: &[char], pos: usize) -> (char, usize) {
    // ищем ';' в пределах 10 символов
    let mut end = pos + 1;
    while end < cs.len() && end - pos <= 10 && cs[end] != ';' {
        end += 1;
    }
    if end >= cs.len() || cs[end] != ';' {
        return ('&', pos + 1);
    }
    let name: String = cs[pos + 1..end].iter().collect();
    let decoded = match name.as_str() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "nbsp" => Some(' '),
        "copy" => Some('©'),
        "reg" => Some('®'),
        "deg" => Some('°'),
        "ndash" | "mdash" => Some('-'),
        "laquo" => Some('«'),
        "raquo" => Some('»'),
        "hellip" => Some('…'),
        _ => {
            if let Some(num) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                u32::from_str_radix(num, 16).ok().and_then(char::from_u32)
            } else if let Some(num) = name.strip_prefix('#') {
                num.parse::<u32>().ok().and_then(char::from_u32)
            } else {
                None
            }
        }
    };
    match decoded {
        Some(c) => (c, end + 1),
        None => ('&', pos + 1),
    }
}

/// Длина (в байтах) префикса строки из не более max символов.
fn char_prefix_len(s: &str, max: usize) -> usize {
    match s.char_indices().nth(max) {
        Some((idx, _)) => idx,
        None => s.len(),
    }
}

pub fn load_partition_entries(partition: &str, path: &str) -> (Vec<FileViewEntry>, Option<String>) {
    match partition {
        "/userdata" => {
            if !ext2::is_formatted() {
                let _ = ext2::format();
                let _ = ext2::write_file(
                    "README.TXT",
                    alloc::format!("Welcome to {}!\r\nEdit files directly in this window.\r\n", crate::OS_RELEASE).as_bytes(),
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
        "/system" => match crate::vfs::readdir("/system") {
            Ok(entries) => {
                let mut items = Vec::new();
                for e in entries {
                    items.push(FileViewEntry {
                        name: e.name,
                        is_dir: e.is_dir,
                        size: e.size.min(u32::MAX as u64) as u32,
                    });
                }
                (items, None)
            }
            Err(e) => (Vec::new(), Some(format!("/system: {}", e.message()))),
        },
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

    r.draw_drop_shadow(w.x, w.y, w.width, total_h, 4);
    if !is_focused {
        r.shade_rect(w.x, w.y, w.width, total_h, 15);
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
        WindowContent::ErrorCenter { selected, scroll } => {
            apps::draw_error_center(r, theme, w, *selected, *scroll, content_y);
        }
        WindowContent::About => {
            apps::draw_about(r, theme, w, content_y);
        }
        WindowContent::DisplaySettings => {
            apps::draw_display_settings(r, theme, w, content_y);
        }
    }
}
