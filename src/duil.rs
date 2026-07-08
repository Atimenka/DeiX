//! DeiX UI Language (DUIL) — декларативный язык разметки интерфейса.
//!
//! ## Синтаксис
//!
//! DUIL — это XML-подобный язык для описания окон, кнопок, текста:
//!
//! ```duil
//! <window title="My App" x=100 y=50 w=400 h=300>
//!   <vbox spacing=4 padding=8>
//!     <label text="Welcome to DeiX!" font="bold" color=#FFFFFF />
//!     <hbox>
//!       <button text="OK" onclick="echo clicked" />
//!       <button text="Cancel" onclick="close" />
//!     </hbox>
//!     <terminal rows=10 />
//!   </vbox>
//! </window>
//! ```
//!
//! ## Элементы
//! - window, vbox, hbox, label, button, textbox, terminal, canvas
//! - Свойства: x, y, w, h, text, color, bg, font, onclick
//!
//! ## Интеграция
//!
//! DUIL-файлы рендерятся в ui/mod.rs через программный рендерер.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ==================== AST ====================

#[derive(Debug, Clone)]
pub struct UiElement {
    pub tag: String,
    pub attrs: BTreeMap<String, String>,
    pub children: Vec<UiElement>,
    pub text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UiDocument {
    pub root: Option<UiElement>,
}

// ==================== Парсер ====================

pub fn parse(source: &str) -> Result<UiDocument, String> {
    let chars: Vec<char> = source.chars().collect();
    let mut pos = 0usize;
    let mut doc = UiDocument { root: None };

    skip_ws(&chars, &mut pos);

    if peek(&chars, &pos) == Some('<') && !peek_str(&chars, &pos, "<!--") {
        doc.root = Some(parse_element(&chars, &mut pos)?);
    }

    Ok(doc)
}

fn parse_element(chars: &[char], pos: &mut usize) -> Result<UiElement, String> {
    expect_char(chars, pos, '<')?;

    // Read tag name
    let tag = read_ident(chars, pos);
    if tag.is_empty() { return Err("Empty tag".into()); }

    let mut attrs = BTreeMap::new();
    let mut text = None;

    // Parse attributes
    loop {
        skip_ws(chars, pos);
        match peek(chars, pos) {
            Some('>') => { *pos += 1; break; }
            Some('/') => {
                *pos += 1;
                if peek(chars, pos) == Some('>') { *pos += 1; }
                // Self-closing tag
                return Ok(UiElement { tag, attrs, children: Vec::new(), text: None });
            }
            Some(c) if c.is_alphanumeric() || c == '_' => {
                let key = read_ident(chars, pos);
                skip_ws(chars, pos);
                if peek(chars, pos) == Some('=') {
                    *pos += 1;
                    skip_ws(chars, pos);
                    let value = if peek(chars, pos) == Some('"') || peek(chars, pos) == Some('\'') {
                        let q = chars[*pos]; *pos += 1;
                        let val = read_until(chars, pos, q);
                        *pos += 1; // closing quote
                        val
                    } else {
                        read_ident(chars, pos)
                    };
                    attrs.insert(key, value);
                } else {
                    attrs.insert(key, "true".into());
                }
            }
            _ => break,
        }
    }

    let mut children = Vec::new();

    // Parse children and text
    loop {
        skip_ws(chars, pos);
        if peek_str(chars, pos, &format!("</{}", tag)) {
            // Closing tag
            *pos += 2 + tag.len();
            skip_ws(chars, pos);
            if peek(chars, pos) == Some('>') { *pos += 1; }
            break;
        }
        if peek(chars, pos) == Some('<') && peek_str(chars, pos, "<!--") {
            // Comment
            *pos += 4;
            while !peek_str(chars, pos, "-->") && *pos < chars.len() { *pos += 1; }
            if peek_str(chars, pos, "-->") { *pos += 3; }
            continue;
        }
        if peek(chars, pos) == Some('<') {
            children.push(parse_element(chars, pos)?);
        } else {
            // Text content
            let t = read_until_any(chars, pos, &['<']);
            if !t.trim().is_empty() {
                text = Some(t.trim().into());
            }
        }
        if *pos >= chars.len() { break; }
    }

    Ok(UiElement { tag, attrs, children, text })
}

// ==================== Рендерер ====================

/// Рендерит DUIL-документ в окна на рабочем столе.
pub fn render(doc: &UiDocument) {
    if let Some(root) = &doc.root {
        render_element(root, 0);
    }
}

fn render_element(el: &UiElement, depth: usize) {
    let indent = "  ".repeat(depth);
    match el.tag.as_str() {
        "window" => {
            let title = el.attrs.get("title").cloned().unwrap_or_default();
            let x: u32 = el.attrs.get("x").and_then(|v| v.parse().ok()).unwrap_or(100);
            let y: u32 = el.attrs.get("y").and_then(|v| v.parse().ok()).unwrap_or(50);
            let w: u32 = el.attrs.get("w").and_then(|v| v.parse().ok()).unwrap_or(400);
            let h: u32 = el.attrs.get("h").and_then(|v| v.parse().ok()).unwrap_or(300);
            crate::println!("{}[window] '{}' {}x{}+{}+{}", indent, title, w, h, x, y);
            for child in &el.children { render_element(child, depth + 1); }
        }
        "button" => {
            let text = el.attrs.get("text").cloned().unwrap_or_default();
            let onclick = el.attrs.get("onclick").cloned().unwrap_or_default();
            crate::println!("{}[button] '{}' -> '{}'", indent, text, onclick);
        }
        "label" => {
            let text = el.attrs.get("text").cloned().or(el.text.clone()).unwrap_or_default();
            crate::println!("{}[label] '{}'", indent, text);
        }
        "terminal" => {
            let rows = el.attrs.get("rows").and_then(|v| v.parse::<u32>().ok()).unwrap_or(10);
            crate::println!("{}[terminal] {} rows", indent, rows);
        }
        _ => {
            let text = el.text.as_deref().unwrap_or("");
            crate::println!("{}[{}] {}", indent, el.tag, text);
            for child in &el.children { render_element(child, depth + 1); }
        }
    }
}

// ==================== Helpers ====================

fn peek(chars: &[char], pos: &usize) -> Option<char> { chars.get(*pos).copied() }
fn peek_str(chars: &[char], pos: &usize, s: &str) -> bool {
    let s_chars: Vec<char> = s.chars().collect();
    chars[*pos..].starts_with(&s_chars)
}

fn skip_ws(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && chars[*pos].is_whitespace() { *pos += 1; }
}

fn expect_char(chars: &[char], pos: &mut usize, c: char) -> Result<(), String> {
    if peek(chars, pos) != Some(c) { return Err(format!("Expected '{}'", c)); }
    *pos += 1;
    Ok(())
}

fn read_ident(chars: &[char], pos: &mut usize) -> String {
    let mut s = String::new();
    while *pos < chars.len() && (chars[*pos].is_alphanumeric() || chars[*pos] == '_' || chars[*pos] == '-') {
        s.push(chars[*pos]); *pos += 1;
    }
    s
}

fn read_until(chars: &[char], pos: &mut usize, delim: char) -> String {
    let mut s = String::new();
    while *pos < chars.len() && chars[*pos] != delim { s.push(chars[*pos]); *pos += 1; }
    s
}

fn read_until_any(chars: &[char], pos: &mut usize, delims: &[char]) -> String {
    let mut s = String::new();
    while *pos < chars.len() && !delims.contains(&chars[*pos]) { s.push(chars[*pos]); *pos += 1; }
    s
}

// ==================== CLI ====================

pub fn cmd_duil(arg: &str) {
    if arg.is_empty() { crate::println!("duil <file.duil>  — render UI layout"); return; }
    match crate::ext2::read_file(arg) {
        Ok(data) => match core::str::from_utf8(&data) {
            Ok(src) => match parse(src) {
                Ok(doc) => render(&doc),
                Err(e) => crate::println!("DUIL parse error: {}", e),
            },
            Err(_) => crate::println!("Not valid UTF-8"),
        },
        Err(_) => crate::println!("File not found: {}", arg),
    }
}
