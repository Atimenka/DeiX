//! Управление окнами и отрисовка оконных контейнеров

use crate::ext2;
use crate::renderer::{Color, Rect, Renderer};
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
        /// Открыт ли диалог подтверждения закрытия (Save/Discard/Cancel).
        /// Пока он открыт, ввод в текст не идёт, клики — только по кнопкам.
        confirm_close: bool,
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
    ThemeSettings,
    About,
    DisplaySettings {
        /// Причина отказа последней попытки смены разрешения
        /// (например, нехватка памяти ядра) — показывается в окне.
        status: Option<String>,
    },
}

impl WindowContent {
    /// Иконка приложения для Dock и Overview — единое соответствие
    /// «тип содержимого -> пиктограмма».
    pub fn icon(&self) -> crate::renderer::IconType {
        use crate::renderer::IconType;
        match self {
            WindowContent::Terminal { .. } => IconType::Terminal,
            WindowContent::Files { .. } | WindowContent::FileEditor { .. } => IconType::Files,
            WindowContent::WebBrowser { .. } => IconType::Browser,
            WindowContent::TaskManager { .. } => IconType::TaskManager,
            WindowContent::ErrorCenter { .. } => IconType::Settings,
            WindowContent::ThemeSettings => IconType::Theme,
            WindowContent::About => IconType::About,
            WindowContent::DisplaySettings { .. } => IconType::Display,
        }
    }
}

pub struct Window {
    /// Стабильный идентификатор окна. Выдаётся Desktop'ом при открытии
    /// и не меняется до закрытия. Фокус, перетаскивание и изменение
    /// размера привязываются к id, а не к индексу в векторе: индексы
    /// сдвигаются при каждом изменении z-порядка (focus_window
    /// переставляет окно в конец) и при закрытии окон.
    pub id: u32,
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

/// Единая геометрия декораций окна. И отрисовка (`draw_window`), и
/// обработка мыши (`desktop.rs`) обязаны использовать эти прямоугольники:
/// два независимых источника координат уже приводили к несовпадению
/// видимой кнопки и её hitbox.
pub struct WindowFrame {
    /// Рамка окна целиком (заголовок + содержимое).
    pub outer: Rect,
    /// Строка заголовка.
    pub titlebar: Rect,
    /// Видимые кнопки (кликам даётся запас `BTN_HIT_PAD`).
    pub close: Rect,
    pub maximize: Rect,
    pub minimize: Rect,
    /// Часть заголовка, за которую окно перетаскивается (без кнопок).
    pub drag: Rect,
    /// Уголок изменения размера (справа внизу).
    pub resize: Rect,
    /// Область содержимого приложения.
    pub content: Rect,
}

impl WindowFrame {
    /// Запас hitbox вокруг видимой кнопки: 12 + 2*3 = 18 < шаг 20,
    /// зоны соседних кнопок не пересекаются.
    pub const BTN_HIT_PAD: i32 = 3;

    pub fn of(w: &Window, m: &UiMetrics) -> Self {
        let titlebar_h = m.titlebar_height;
        let d = m.button_diameter as u32;
        let total_h = w.height + titlebar_h as u32;
        let btn_y = w.y + (titlebar_h - m.button_diameter) / 2;
        let right = w.x + w.width as i32;

        // Зона drag заканчивается до кнопок (с их hit-запасом), чтобы
        // промах мимо Close не начинал перетаскивание.
        let drag_w = (w.width as i32 - 62 - Self::BTN_HIT_PAD).max(0) as u32;

        WindowFrame {
            outer: Rect::new(w.x, w.y, w.width, total_h),
            titlebar: Rect::new(w.x, w.y, w.width, titlebar_h as u32),
            close: Rect::new(right - 22, btn_y, d, d),
            maximize: Rect::new(right - 42, btn_y, d, d),
            minimize: Rect::new(right - 62, btn_y, d, d),
            drag: Rect::new(w.x, w.y, drag_w, titlebar_h as u32),
            resize: Rect::new(right - 16, w.y + total_h as i32 - 16, 16, 16),
            content: Rect::new(w.x, w.y + titlebar_h, w.width, w.height),
        }
    }
}

/// Минимальный размер содержимого окна. Один источник и для ручного
/// изменения размера, и для прижатия к экрану (clamp_window_geometry):
/// раньше у них были разные минимумы (220x140 против 160x80).
pub const MIN_WINDOW_W: u32 = 220;
pub const MIN_WINDOW_H: u32 = 140;

/// Геометрия развёрнутого окна: занимает рабочую область (между верхней
/// панелью и Dock). `height` — высота СОДЕРЖИМОГО, заголовок добавляется
/// сверху, поэтому он вычитается из высоты рабочей области.
pub fn maximized_geometry(screen_w: i32, screen_h: i32, m: &UiMetrics) -> (i32, i32, u32, u32) {
    let wa = crate::ui::taskbar::work_area(screen_w, screen_h);
    let h = (wa.h as i32 - m.titlebar_height).max(120);
    (wa.x, wa.y, wa.w.max(160), h as u32)
}

impl Window {
    pub fn new_terminal(x: i32, y: i32) -> Self {
        Window {
            id: 0,
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
            id: 0,
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
            id: 0,
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
                    String::from("https://example.com"),
                    String::from("deix://home"),
                    String::from("deix://docs"),
                    String::from("http://deix.os"),
                ],
                status_msg: Some(String::from("HTTP/HTTPS client ready (TLS 1.3, certificate verification on).")),
            },
        }
    }

    pub fn new_task_manager(x: i32, y: i32) -> Self {
        Window {
            id: 0,
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
            id: 0,
            title: String::from("Personalization"),
            x,
            y,
            width: 480,
            height: 460,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 480, 460),
            content: WindowContent::ThemeSettings,
        }
    }

    pub fn new_error_center(x: i32, y: i32) -> Self {
        Window {
            id: 0,
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
            id: 0,
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
            id: 0,
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
                confirm_close: false,
                error,
                status_msg: if read_only {
                    Some(String::from("read-only"))
                } else {
                    None
                },
            },
        }
    }

    pub fn new_display_settings(x: i32, _y: i32, cur_h: u32) -> Self {
        let height = RESOLUTION_PRESETS.len() as u32 * 28 + 40;
        let y_pos = (cur_h as i32 - height as i32) / 2;
        Window {
            id: 0,
            title: String::from("Display Settings"),
            x,
            y: y_pos.max(20),
            width: 240,
            height,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y_pos.max(20), 240, height),
            content: WindowContent::DisplaySettings { status: None },
        }
    }
}

pub fn fetch_and_render_web_page(url: &str) -> Vec<String> {
    let clean_url = url.trim();

    if clean_url == "deix://home" {
        return alloc::vec![
            String::from("# DeiX Browser"),
            String::from("Limited text-oriented HTTP/HTTPS client."),
            String::new(),
            String::from("## Quick Navigation:"),
            String::from("* [https://example.com] HTTPS test page (TLS 1.3)"),
            String::from("* [deix://docs] System Architecture & Manual"),
            String::from("* [http://deix.os] System Status (local page)"),
            String::new(),
            String::from("## Honest limitations:"),
            String::from("- HTTPS: own TLS 1.3 client (X25519 + AES-128-GCM) with X.509 chain verification"),
            String::from("  (23 Mozilla root CAs, expiry via RTC, SAN hostname check; no OCSP/CRL revocation)"),
            String::from("- TLS 1.3 only: servers without it fail with an honest error"),
            String::from("- HTTP/1.1 with chunked transfer and gzip (DEFLATE) decompression; GET only, no keep-alive"),
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
            String::from("RTL8139 Ethernet -> ARP / IPv4 -> ICMP + UDP (DNS) + TCP -> HTTP/1.1 + TLS 1.3 (https)."),
            String::from("Hostnames are resolved with a real DNS client (A records, UDP/53)."),
            String::from("TLS 1.3: X25519 + HKDF-SHA256 + AES-128-GCM; X.509 chains verified against 23 Mozilla roots."),
            String::from("HTTP/1.1: chunked transfer decoding + gzip (RFC 1951/1952) decompression."),
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
            format!("# Failed to load {}", full_url),
            String::new(),
            format!("Error: {}", err),
            String::new(),
            String::from("Possible causes:"),
            String::from("* network card not initialized (no eth0)"),
            String::from("* DNS server unreachable or the name does not exist"),
            String::from("* host unreachable or port closed"),
            String::from("* server does not support TLS 1.3 (older HTTPS sites will not load)"),
            String::from("* server certificate failed verification (expiry, hostname, CA chain)"),
            String::new(),
            String::from("Check in the CLI: `ifconfig`, `dns <host>`, `ping <ip>`."),
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
    if resp.final_url.starts_with("https://") {
        lines.push(String::from(
            "TLS 1.3: encrypted, certificate chain verified (23 Mozilla roots; no OCSP/CRL)",
        ));
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
                    let cut = crate::ui::text::char_byte_index(rest, WRAP);
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
    let frame = WindowFrame::of(w, &ui_m);

    // Профиль производительности реально меняет путь отрисовки:
    // Balanced убирает blur, Performance — ещё и тени (см. ui::effects).
    let fx = crate::ui::effects::current();
    if theme.enable_blur && fx.blur_enabled() && !is_dragging {
        r.apply_blur_rect(w.x - 4, w.y - 4, w.width + 8, total_h + 8, 2);
    }

    if fx.shadows_enabled() {
        r.draw_drop_shadow(w.x, w.y, w.width, total_h, 4);
    }
    if !is_focused {
        r.shade_rect(w.x, w.y, w.width, total_h, 15);
    }

    if fx.shadows_enabled() {
        r.fill_rounded_rect_alpha(w.x + 4, w.y + 4, w.width, total_h, theme.corner_radius, Color::SHADOW, 80);
    }

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
    // Длинный заголовок (имя файла в редакторе) не должен заезжать под
    // кнопки: слева отступ 12, справа зона кнопок начинается на width-62.
    let max_title_cells = ((w.width as i32 - 80) / 8).max(4) as usize;
    let title = crate::ui::text::truncate_to_cells(&w.title, max_title_cells).0;
    r.draw_text(w.x + 12, w.y + 6, title, title_color, None);

    // Кнопки — строго по прямоугольникам WindowFrame (теми же
    // пользуется обработчик кликов в desktop.rs).
    let rad = button_d / 2;
    r.fill_circle(frame.minimize.x + rad, frame.minimize.y + rad, rad, Color::rgb(234, 179, 8));
    r.draw_hline(frame.minimize.x + 3, frame.minimize.y + rad, button_d as u32 - 6, Color::rgb(120, 80, 0));
    r.fill_circle(frame.maximize.x + rad, frame.maximize.y + rad, rad, Color::GREEN);
    r.fill_circle(frame.close.x + rad, frame.close.y + rad, rad, Color::RED);

    let content_y = w.y + titlebar_h;

    match &w.content {
        WindowContent::Terminal { lines, current_line } => {
            apps::draw_terminal(r, theme, w, lines, current_line, content_y);
        }
        WindowContent::Files {
            current_partition,
            current_path,
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
                current_path,
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
            confirm_close,
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
                *confirm_close,
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
        WindowContent::ThemeSettings => {
            apps::draw_theme_settings(r, theme, w, content_y);
        }
        WindowContent::ErrorCenter { selected, scroll } => {
            apps::draw_error_center(r, theme, w, *selected, *scroll, content_y);
        }
        WindowContent::About => {
            apps::draw_about(r, theme, w, content_y);
        }
        WindowContent::DisplaySettings { status } => {
            apps::draw_display_settings(r, theme, w, content_y, status.as_deref());
        }
    }
}
