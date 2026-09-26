//! DUIL — Declarative UI Language & UI Framework DeiX OS.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetKind {
    Window,
    VBox,
    HBox,
    Grid,
    Button,
    Label,
    Input,
    ProgressBar,
    CheckBox,
    Spacer,
    TextBox,
    GroupBox,
    Slider,
    Badge,
}

#[derive(Debug, Clone)]
pub struct Widget {
    pub id: u32,
    pub kind: WidgetKind,
    pub title: String,
    pub text: String,
    pub value: i32,
    pub max_value: i32,
    pub checked: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub bg_color: u32,
    pub fg_color: u32,
    pub border_color: u32,
    pub grid_rows: u32,
    pub grid_cols: u32,
    pub padding: u32,
    pub spacing: u32,
    pub children: Vec<Widget>,
    pub onclick: Option<String>,
}

impl Widget {
    pub fn new(kind: WidgetKind, text: &str) -> Self {
        let (bg, fg, border) = match kind {
            WidgetKind::Window => (0xFF1E1E2E, 0xFFFFFFFF, 0xFF313244),
            WidgetKind::Button => (0xFF313244, 0xFFCDD6F4, 0xFF45475A),
            WidgetKind::Label => (0x00000000, 0xFFCDD6F4, 0x00000000),
            WidgetKind::Input => (0xFF11111B, 0xFFA6ADC8, 0xFF45475A),
            WidgetKind::ProgressBar => (0xFF181825, 0xFF89B4FA, 0xFF45475A),
            WidgetKind::CheckBox => (0x00000000, 0xFFCDD6F4, 0xFF89B4FA),
            WidgetKind::GroupBox => (0xFF181825, 0xFFCDD6F4, 0xFF313244),
            WidgetKind::Slider => (0xFF181825, 0xFF89B4FA, 0xFF45475A),
            WidgetKind::Badge => (0xFF89B4FA, 0xFF11111B, 0x00000000),
            _ => (0x00000000, 0xFFCDD6F4, 0x00000000),
        };

        Self {
            id: 0,
            kind,
            title: String::new(),
            text: text.to_string(),
            value: 0,
            max_value: 100,
            checked: false,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            bg_color: bg,
            fg_color: fg,
            border_color: border,
            grid_rows: 1,
            grid_cols: 1,
            padding: 4,
            spacing: 4,
            children: Vec::new(),
            onclick: None,
        }
    }

    pub fn set_attr(&mut self, key: &str, val: &str) {
        match key {
            "id" => {
                if let Ok(num) = val.parse::<u32>() {
                    self.id = num;
                }
            }
            "text" | "label" => self.text = val.to_string(),
            "title" => self.title = val.to_string(),
            "val" | "value" => {
                if let Ok(num) = val.parse::<i32>() {
                    self.value = num;
                }
            }
            "max" => {
                if let Ok(num) = val.parse::<i32>() {
                    self.max_value = num;
                }
            }
            "checked" => self.checked = val == "true" || val == "1",
            "w" | "width" => {
                if let Ok(num) = val.parse::<u32>() {
                    self.width = num;
                }
            }
            "h" | "height" => {
                if let Ok(num) = val.parse::<u32>() {
                    self.height = num;
                }
            }
            "bg" | "bg_color" => {
                if let Ok(num) = u32::from_str_radix(val.trim_start_matches("0x"), 16) {
                    self.bg_color = num | 0xFF000000;
                }
            }
            "fg" | "fg_color" => {
                if let Ok(num) = u32::from_str_radix(val.trim_start_matches("0x"), 16) {
                    self.fg_color = num | 0xFF000000;
                }
            }
            "grid" => {
                if let Some((r, c)) = val.split_once('x') {
                    if let (Ok(r_num), Ok(c_num)) = (r.parse::<u32>(), c.parse::<u32>()) {
                        self.grid_rows = r_num;
                        self.grid_cols = c_num;
                    }
                }
            }
            "padding" => {
                if let Ok(num) = val.parse::<u32>() {
                    self.padding = num;
                }
            }
            "spacing" => {
                if let Ok(num) = val.parse::<u32>() {
                    self.spacing = num;
                }
            }
            "onclick" => self.onclick = Some(val.to_string()),
            _ => {}
        }
    }
}

// ---------------- Отрисовка в буфер кадра ----------------

fn draw_char_buf(buf: &mut [u32], buf_w: u32, buf_h: u32, x: i32, y: i32, ch: u8, color: u32) {
    let glyph = crate::font::read_glyph(ch);
    for (row, &byte) in glyph.iter().enumerate() {
        if byte == 0 {
            continue;
        }
        let py = y + row as i32;
        if py < 0 || py >= buf_h as i32 {
            continue;
        }
        for col in 0..8 {
            if (byte >> (7 - col)) & 1 != 0 {
                let px = x + col as i32;
                if px >= 0 && px < buf_w as i32 {
                    buf[(py as u32 * buf_w + px as u32) as usize] = color;
                }
            }
        }
    }
}

fn draw_text_buf(buf: &mut [u32], buf_w: u32, buf_h: u32, x: i32, y: i32, text: &str, color: u32) {
    let mut cx = x;
    for b in text.bytes() {
        draw_char_buf(buf, buf_w, buf_h, cx, y, b, color);
        cx += 8;
    }
}

fn fill_rect_buf(buf: &mut [u32], buf_w: u32, buf_h: u32, rx: i32, ry: i32, rw: u32, rh: u32, color: u32) {
    for y in ry..(ry + rh as i32) {
        if y < 0 || y >= buf_h as i32 {
            continue;
        }
        for x in rx..(rx + rw as i32) {
            if x >= 0 && x < buf_w as i32 {
                buf[(y as u32 * buf_w + x as u32) as usize] = color;
            }
        }
    }
}

fn draw_rect_border_buf(buf: &mut [u32], buf_w: u32, buf_h: u32, rx: i32, ry: i32, rw: u32, rh: u32, color: u32) {
    for x in rx..(rx + rw as i32) {
        if x >= 0 && x < buf_w as i32 {
            if ry >= 0 && ry < buf_h as i32 {
                buf[(ry as u32 * buf_w + x as u32) as usize] = color;
            }
            let bottom = ry + rh as i32 - 1;
            if bottom >= 0 && bottom < buf_h as i32 {
                buf[(bottom as u32 * buf_w + x as u32) as usize] = color;
            }
        }
    }
    for y in ry..(ry + rh as i32) {
        if y >= 0 && y < buf_h as i32 {
            if rx >= 0 && rx < buf_w as i32 {
                buf[(y as u32 * buf_w + rx as u32) as usize] = color;
            }
            let right = rx + rw as i32 - 1;
            if right >= 0 && right < buf_w as i32 {
                buf[(y as u32 * buf_w + right as u32) as usize] = color;
            }
        }
    }
}

// ---------------- Расчёт геометрии layout ----------------

pub fn layout_widget_tree(w: &mut Widget, x: i32, y: i32, avail_w: u32, avail_h: u32) {
    w.x = x;
    w.y = y;
    if w.width == 0 {
        w.width = avail_w;
    }
    if w.height == 0 {
        w.height = avail_h;
    }

    match w.kind {
        WidgetKind::Window => {
            let title_h = 24u32;
            let content_x = x + w.padding as i32;
            let content_y = y + title_h as i32 + w.padding as i32;
            let content_w = w.width.saturating_sub(w.padding * 2);
            let content_h = w.height.saturating_sub(title_h + w.padding * 2);

            for child in w.children.iter_mut() {
                layout_widget_tree(child, content_x, content_y, content_w, content_h);
            }
        }
        WidgetKind::VBox | WidgetKind::GroupBox => {
            let pad = w.padding as i32;
            let sp = w.spacing as i32;
            let mut curr_y = y + pad;
            let child_w = w.width.saturating_sub(w.padding * 2);

            let num_children = w.children.len() as u32;
            let default_h = if num_children > 0 {
                (w.height.saturating_sub(w.padding * 2 + w.spacing * (num_children - 1))) / num_children
            } else {
                30
            };

            for child in w.children.iter_mut() {
                let h = if child.height > 0 { child.height } else { default_h };
                layout_widget_tree(child, x + pad, curr_y, child_w, h);
                curr_y += h as i32 + sp;
            }
        }
        WidgetKind::HBox => {
            let pad = w.padding as i32;
            let sp = w.spacing as i32;
            let mut curr_x = x + pad;
            let child_h = w.height.saturating_sub(w.padding * 2);

            let num_children = w.children.len() as u32;
            let default_w = if num_children > 0 {
                (w.width.saturating_sub(w.padding * 2 + w.spacing * (num_children - 1))) / num_children
            } else {
                60
            };

            for child in w.children.iter_mut() {
                let cw = if child.width > 0 { child.width } else { default_w };
                layout_widget_tree(child, curr_x, y + pad, cw, child_h);
                curr_x += cw as i32 + sp;
            }
        }
        WidgetKind::Grid => {
            let rows = if w.grid_rows > 0 { w.grid_rows } else { 1 };
            let cols = if w.grid_cols > 0 { w.grid_cols } else { 1 };
            let pad = w.padding as i32;
            let sp = w.spacing as i32;

            let cell_w = (w.width.saturating_sub(w.padding * 2 + w.spacing * (cols.saturating_sub(1)))) / cols;
            let cell_h = (w.height.saturating_sub(w.padding * 2 + w.spacing * (rows.saturating_sub(1)))) / rows;

            for (idx, child) in w.children.iter_mut().enumerate() {
                let r = (idx as u32) / cols;
                let c = (idx as u32) % cols;
                let cx = x + pad + (c as i32) * (cell_w as i32 + sp);
                let cy = y + pad + (r as i32) * (cell_h as i32 + sp);
                layout_widget_tree(child, cx, cy, cell_w, cell_h);
            }
        }
        _ => {
            if w.width == 0 {
                w.width = 100;
            }
            if w.height == 0 {
                w.height = 30;
            }
        }
    }
}

// ---------------- Отрисовка дерева виджетов ----------------

pub fn render_widget_tree(w: &Widget, buf: &mut [u32], buf_w: u32, buf_h: u32) {
    match w.kind {
        WidgetKind::Window => {
            fill_rect_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.bg_color);
            draw_rect_border_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.border_color);
            // Заголовок окна
            fill_rect_buf(buf, buf_w, buf_h, w.x, w.y, w.width, 24, 0xFF2A374E);
            let title = if !w.title.is_empty() { &w.title } else { &w.text };
            draw_text_buf(buf, buf_w, buf_h, w.x + 8, w.y + 4, title, 0xFFFFFFFF);
        }
        WidgetKind::Button => {
            fill_rect_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.bg_color);
            draw_rect_border_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.border_color);

            let text_len = (w.text.len() * 8) as u32;
            let tx = w.x + (w.width.saturating_sub(text_len) / 2) as i32;
            let ty = w.y + (w.height.saturating_sub(16) / 2) as i32;
            draw_text_buf(buf, buf_w, buf_h, tx, ty, &w.text, w.fg_color);
        }
        WidgetKind::Label => {
            if w.bg_color != 0 {
                fill_rect_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.bg_color);
            }
            let ty = w.y + (w.height.saturating_sub(16) / 2) as i32;
            draw_text_buf(buf, buf_w, buf_h, w.x + 4, ty, &w.text, w.fg_color);
        }
        WidgetKind::Input => {
            fill_rect_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.bg_color);
            draw_rect_border_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.border_color);
            draw_text_buf(buf, buf_w, buf_h, w.x + 6, w.y + 6, &w.text, w.fg_color);
        }
        WidgetKind::ProgressBar => {
            fill_rect_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.bg_color);
            draw_rect_border_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.border_color);

            let pct = w.value.clamp(0, 100) as u32;
            let fill_w = (w.width.saturating_sub(2) * pct) / 100;
            fill_rect_buf(buf, buf_w, buf_h, w.x + 1, w.y + 1, fill_w, w.height.saturating_sub(2), 0xFF89B4FA);

            let pct_str = format!("{}%", pct);
            let tx = w.x + (w.width.saturating_sub(pct_str.len() as u32 * 8) / 2) as i32;
            let ty = w.y + (w.height.saturating_sub(16) / 2) as i32;
            draw_text_buf(buf, buf_w, buf_h, tx, ty, &pct_str, 0xFFFFFFFF);
        }
        WidgetKind::CheckBox => {
            let box_size = 18u32;
            let box_y = w.y + (w.height.saturating_sub(box_size) / 2) as i32;

            fill_rect_buf(buf, buf_w, buf_h, w.x, box_y, box_size, box_size, 0xFF181825);
            draw_rect_border_buf(buf, buf_w, buf_h, w.x, box_y, box_size, box_size, w.border_color);

            if w.checked {
                draw_text_buf(buf, buf_w, buf_h, w.x + 5, box_y + 1, "V", 0xFF89B4FA);
            }
            draw_text_buf(buf, buf_w, buf_h, w.x + box_size as i32 + 8, box_y + 1, &w.text, w.fg_color);
        }
        WidgetKind::GroupBox => {
            fill_rect_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.bg_color);
            draw_rect_border_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.border_color);
            if !w.text.is_empty() {
                draw_text_buf(buf, buf_w, buf_h, w.x + 8, w.y + 4, &w.text, w.fg_color);
            }
        }
        WidgetKind::Slider => {
            let pad = 4i32;
            let track_h = 6u32;
            let track_y = w.y + (w.height.saturating_sub(track_h) / 2) as i32;
            fill_rect_buf(buf, buf_w, buf_h, w.x + pad, track_y, w.width.saturating_sub(pad as u32 * 2), track_h, 0xFF313244);
            let pct = w.value.clamp(0, 100) as u32;
            let handle_x = w.x + pad + ((w.width.saturating_sub(pad as u32 * 2 + 12)) * pct / 100) as i32;
            fill_rect_buf(buf, buf_w, buf_h, handle_x, w.y + 4, 12, w.height.saturating_sub(8), 0xFF89B4FA);
        }
        WidgetKind::Badge => {
            fill_rect_buf(buf, buf_w, buf_h, w.x, w.y, w.width, w.height, w.bg_color);
            draw_text_buf(buf, buf_w, buf_h, w.x + 6, w.y + (w.height.saturating_sub(16) / 2) as i32, &w.text, w.fg_color);
        }
        _ => {}
    }

    for child in w.children.iter() {
        render_widget_tree(child, buf, buf_w, buf_h);
    }
}

// ---------------- Парсинг разметки DUIL ----------------

pub fn parse_duil_markup(script: &str) -> Widget {
    let mut root = Widget::new(WidgetKind::Window, "DUIL Application");
    let mut stack: Vec<Widget> = Vec::new();

    for line in script.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }

        let mut parts = trimmed.split_whitespace();
        let tag = match parts.next() {
            Some(t) => t.to_lowercase(),
            None => continue,
        };

        let kind = match tag.as_str() {
            "window" => WidgetKind::Window,
            "vbox" => WidgetKind::VBox,
            "hbox" => WidgetKind::HBox,
            "grid" => WidgetKind::Grid,
            "button" => WidgetKind::Button,
            "label" => WidgetKind::Label,
            "input" => WidgetKind::Input,
            "progressbar" | "progress" => WidgetKind::ProgressBar,
            "checkbox" | "check" => WidgetKind::CheckBox,
            "groupbox" | "group" => WidgetKind::GroupBox,
            "slider" => WidgetKind::Slider,
            "badge" => WidgetKind::Badge,
            "spacer" => WidgetKind::Spacer,
            _ => continue,
        };

        let mut widget = Widget::new(kind, "");

        for token in parts {
            if let Some((k, v)) = token.split_once('=') {
                widget.set_attr(k, v);
            } else if widget.text.is_empty() {
                widget.text = token.to_string();
            }
        }

        if kind == WidgetKind::Window {
            root = widget;
        } else if let Some(parent) = stack.last_mut() {
            parent.children.push(widget);
        } else {
            root.children.push(widget);
        }

        if matches!(kind, WidgetKind::VBox | WidgetKind::HBox | WidgetKind::Grid | WidgetKind::GroupBox) {
            if let Some(added) = root.children.last().cloned() {
                stack.push(added);
            }
        }
    }

    root
}

// ---------------- Обработка кликов и интерактивность ----------------

pub fn hit_test_and_click(w: &mut Widget, cx: i32, cy: i32) -> Option<String> {
    if cx >= w.x && cx <= (w.x + w.width as i32) && cy >= w.y && cy <= (w.y + w.height as i32) {
        if w.kind == WidgetKind::CheckBox {
            w.checked = !w.checked;
        }
        if let Some(ref action) = w.onclick {
            return Some(action.clone());
        }
        for child in w.children.iter_mut() {
            if let Some(act) = hit_test_and_click(child, cx, cy) {
                return Some(act);
            }
        }
    }
    None
}

// ---------------- Приложение Калькулятор DUIL ----------------

pub struct DuilApp {
    pub root: Widget,
}

impl DuilApp {
    pub fn new_calculator() -> Self {
        let duil_code = "\
window title=Calculator width=320 height=420 bg=0x1E1E2E
  vbox padding=10 spacing=8
    label Result: 0 id=100 text=0 bg=0x11111B fg=0x00FFCC w=300 h=40
    grid grid=4x4 w=300 h=300 spacing=5
      button 7 onclick=digit_7
      button 8 onclick=digit_8
      button 9 onclick=digit_9
      button + onclick=op_add bg=0xFF9500
      button 4 onclick=digit_4
      button 5 onclick=digit_5
      button 6 onclick=digit_6
      button - onclick=op_sub bg=0xFF9500
      button 1 onclick=digit_1
      button 2 onclick=digit_2
      button 3 onclick=digit_3
      button * onclick=op_mul bg=0xFF9500
      button C onclick=clear bg=0xFF3B30
      button 0 onclick=digit_0
      button = onclick=eval bg=0x34C759
      button / onclick=op_div bg=0xFF9500
";

        let mut root = parse_duil_markup(duil_code);
        layout_widget_tree(&mut root, 0, 0, 320, 420);

        crate::renderer::ensure_framebuffer();

        Self { root }
    }
}

pub fn run_calculator_demo() {
    let _app = DuilApp::new_calculator();
    crate::println!("  [duil] Запущено графическое приложение 'Calculator'");
}

pub fn run_duil_app(path: &str) {
    if let Ok(data) = crate::ext2::read_file(path) {
        if let Ok(text) = core::str::from_utf8(&data) {
            let mut root = parse_duil_markup(text);
            layout_widget_tree(&mut root, 0, 0, 400, 300);

            crate::renderer::ensure_framebuffer();
            crate::println!("  [duil] Запущено приложение '{}'", path);
        }
    } else {
        crate::println!("  [duil] Файл '{}' не найден", path);
    }
}

pub fn cmd_duil(arg: &str) {
    let mut parts = arg.trim().split_whitespace();
    let sub = parts.next().unwrap_or("");

    match sub {
        "run" => {
            if let Some(path) = parts.next() {
                run_duil_app(path);
            } else {
                crate::println!("  Использование: duil run <path.duil>");
            }
        }
        "calc" | "calculator" => {
            run_calculator_demo();
        }
        _ => {
            crate::println!("Использование: duil [run <file.duil>|calc]");
        }
    }
}
