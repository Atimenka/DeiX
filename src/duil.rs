//! DUIL — Declarative UI Language & UI Framework DeiX OS.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetKind {
    Window,
    VBox,
    HBox,
    Button,
    Label,
    Input,
    List,
    WebView,
    Canvas,
    Spacer,
    TextBox,
    ProgressBar,
    GroupBox,
    CheckBox,
    Grid,
}

#[derive(Debug, Clone)]
pub struct Widget {
    pub id: u32,
    pub kind: WidgetKind,
    pub text: String,
    pub value: i32,
    pub children: Vec<Widget>,
    pub onclick: Option<String>,
}

impl Widget {
    pub fn new(kind: WidgetKind, text: &str) -> Self {
        Self {
            id: 0,
            kind,
            text: text.to_string(),
            value: 0,
            children: Vec::new(),
            onclick: None,
        }
    }
}

pub struct EventLoop {
    pub surface_id: u32,
    pub focus_id: Option<u32>,
    pub running: bool,
}

impl EventLoop {
    pub fn new(surface_id: u32) -> Self {
        Self {
            surface_id,
            focus_id: None,
            running: true,
        }
    }

    pub fn run_step(&mut self) {
        let mouse = crate::mouse::snapshot();
        if mouse.left_button {
            // Обработка клика
        }
        crate::sched::yield_now();
    }
}

pub fn parse_duil_markup(script: &str) -> Widget {
    let mut root = Widget::new(WidgetKind::Window, "DUIL App");
    let mut vbox = Widget::new(WidgetKind::VBox, "");

    for line in script.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("button") {
            vbox.children.push(Widget::new(WidgetKind::Button, "Кнопка"));
        } else if trimmed.starts_with("label") {
            vbox.children.push(Widget::new(WidgetKind::Label, "Текст"));
        } else if trimmed.starts_with("input") {
            vbox.children.push(Widget::new(WidgetKind::Input, "Ввод"));
        } else if trimmed.starts_with("progressbar") {
            vbox.children.push(Widget::new(WidgetKind::ProgressBar, "50%"));
        } else if trimmed.starts_with("checkbox") {
            vbox.children.push(Widget::new(WidgetKind::CheckBox, "Выбор"));
        }
    }

    root.children.push(vbox);
    root
}

pub fn run_duil_app(path: &str) {
    if let Ok(data) = crate::ext2::read_file(path) {
        if let Ok(text) = core::str::from_utf8(&data) {
            let _widget_tree = parse_duil_markup(text);
            let mut comp = crate::compositor::COMPOSITOR.lock();
            let surface_id = comp.create_surface(400, 300);
            crate::println!("  [duil] Запущено приложение '{}' на поверхности {}", path, surface_id);
        }
    } else {
        crate::println!("  [duil] Файл '{}' не найден", path);
    }
}

pub fn run_calculator_demo() {
    let duil_code = "\
window Calculator
  vbox
    label Result: 0
    grid 4x4
      button 7
      button 8
      button 9
      button +
      button 4
      button 5
      button 6
      button -
      button 1
      button 2
      button 3
      button *
      button C
      button 0
      button =
      button /
";
    let _widget_tree = parse_duil_markup(duil_code);
    let mut comp = crate::compositor::COMPOSITOR.lock();
    let surface_id = comp.create_surface(320, 400);
    crate::println!("  [duil] Запущен калькулятор (DUIL App) на поверхности {}", surface_id);
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
