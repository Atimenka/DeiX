#![allow(dead_code)]
//! DeiX Browser — текстовый веб-браузер.
//!
//! ## Возможности
//!
//! - HTTP/1.0 GET запросы
//! - Парсинг HTML (теги: h1-h6, p, a, br, ul, li, b, i, title, pre, table, img-alt)
//! - Навигация по ссылкам (номера)
//! - Адресная строка (ввод URL)
//! - История (back/forward) — 16 записей
//! - Индикатор загрузки
//!
//! ## Управление
//!
//! - Введи URL в адресной строке → Enter → загрузка
//! - Введи номер ссылки → Enter → переход по ссылке
//! - `b` — назад, `f` — вперёд, `r` — обновить
//! - `q` — выход из браузера

use crate::net::http;
use crate::{print, println};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::{vec, vec::Vec};

const MAX_HISTORY: usize = 16;

// ==================== DOM ====================

#[derive(Debug, Clone)]
enum HtmlNode {
    Text(String),
    Element {
        tag: String,
        attrs: Vec<(String, String)>,
        children: Vec<HtmlNode>,
    },
}

#[derive(Debug, Clone)]
struct Link {
    text: String,
    href: String,
}

struct ParsedPage {
    title: String,
    links: Vec<Link>,
    text_lines: Vec<String>,  // отрендеренный текст (с учётом ширины)
    dom: Vec<HtmlNode>,
}

// ==================== HTML Parser ====================

fn parse_html(html: &str) -> Vec<HtmlNode> {
    let chars: Vec<char> = html.chars().collect();
    let mut pos = 0usize;
    parse_nodes(&chars, &mut pos, "")
}

fn parse_nodes(chars: &[char], pos: &mut usize, _until: &str) -> Vec<HtmlNode> {
    let mut nodes = Vec::new();
    while *pos < chars.len() {
        if peek_str(chars, pos, "</") && !_until.is_empty() {
            // Проверим, не закрывающий ли это тег ожидаемого типа
            let close_tag = format!("</{}", _until);
            if peek_str(chars, pos, &close_tag) { break; }
        }
        if peek_str(chars, pos, "<!--") {
            skip_until(chars, pos, "-->");
            continue;
        }
        if peek_char(chars, pos) == '<' && !peek_str(chars, pos, "</") {
            if let Some(node) = parse_element(chars, pos) {
                nodes.push(node);
                continue;
            }
        }
        // Текст
        let text = read_until(chars, pos, '<');
        if !text.trim().is_empty() {
            // Декодируем HTML entities
            let decoded = decode_entities(&text);
            nodes.push(HtmlNode::Text(decoded));
        } else if !text.is_empty() {
            nodes.push(HtmlNode::Text(text));
        }
    }
    nodes
}

fn parse_element(chars: &[char], pos: &mut usize) -> Option<HtmlNode> {
    *pos += 1; // skip '<'
    let tag = read_ident(chars, pos).to_lowercase();
    if tag.is_empty() { return None; }

    let mut attrs = Vec::new();
    // Парсим атрибуты
    loop {
        skip_ws(chars, pos);
        if *pos >= chars.len() { break; }
        if peek_char(chars, pos) == '>' { *pos += 1; break; }
        if peek_char(chars, pos) == '/' && *pos+1 < chars.len() && chars[*pos+1] == '>' {
            *pos += 2; // self-closing
            return Some(HtmlNode::Element { tag, attrs, children: Vec::new() });
        }
        let key = read_ident(chars, pos).to_lowercase();
        if key.is_empty() { break; }
        skip_ws(chars, pos);
        let val = if peek_char(chars, pos) == '=' {
            *pos += 1;
            skip_ws(chars, pos);
            if peek_char(chars, pos) == '"' || peek_char(chars, pos) == '\'' {
                let q = chars[*pos]; *pos += 1;
                let v = read_until_char(chars, pos, q);
                if *pos < chars.len() { *pos += 1; } // closing quote
                v
            } else {
                read_ident(chars, pos)
            }
        } else { String::new() };
        attrs.push((key, val));
    }

    // void elements — не имеют закрывающего тега
    let void_tags = ["br", "hr", "img", "input", "meta", "link"];
    if void_tags.contains(&tag.as_str()) {
        return Some(HtmlNode::Element { tag, attrs, children: Vec::new() });
    }

    // Парсим дочерние элементы
    let children = parse_nodes(chars, pos, &tag);

    // Пропускаем закрывающий тег
    skip_until(chars, pos, ">");

    Some(HtmlNode::Element { tag, attrs, children })
}

// ==================== Рендерер HTML → текст ====================

fn render_page(dom: &[HtmlNode], width: usize) -> ParsedPage {
    let mut title = String::new();
    let mut links = Vec::new();
    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut bold = false;
    let mut italic = false;

    render_nodes(dom, &mut title, &mut links, &mut lines, &mut current_line,
                 &mut bold, &mut italic, width);

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    ParsedPage { title, links, text_lines: lines, dom: dom.to_vec() }
}

fn render_nodes(
    nodes: &[HtmlNode], title: &mut String, links: &mut Vec<Link>,
    lines: &mut Vec<String>, current: &mut String,
    bold: &mut bool, italic: &mut bool, width: usize,
) {
    for node in nodes {
        match node {
            HtmlNode::Text(text) => {
                add_text(current, text, bold, italic, lines, width);
            }
            HtmlNode::Element { tag, attrs, children } => {
                match tag.as_str() {
                    "title" => {
                        *title = collect_text(children);
                    }
                    "h1" => { flush_line(current, lines);
                        let h = format!("=== {} ===", collect_text(children).to_uppercase());
                        lines.push(h); lines.push(String::new());
                    }
                    "h2" => { flush_line(current, lines);
                        let h = format!("-- {} --", collect_text(children));
                        lines.push(h); lines.push(String::new());
                    }
                    "h3" | "h4" | "h5" | "h6" => {
                        flush_line(current, lines);
                        let h = format!("** {} **", collect_text(children));
                        lines.push(h); lines.push(String::new());
                    }
                    "p" => { flush_line(current, lines); lines.push(String::new());
                        render_nodes(children, title, links, lines, current, bold, italic, width);
                        flush_line(current, lines); lines.push(String::new());
                    }
                    "br" => { flush_line(current, lines); }
                    "hr" => { flush_line(current, lines);
                        lines.push("-".repeat(width)); lines.push(String::new());
                    }
                    "b" | "strong" => { *bold = true;
                        render_nodes(children, title, links, lines, current, bold, italic, width);
                        *bold = false;
                    }
                    "i" | "em" => { *italic = true;
                        render_nodes(children, title, links, lines, current, bold, italic, width);
                        *italic = false;
                    }
                    "a" => {
                        let href = attrs.iter().find(|(k,_)| k=="href")
                            .map(|(_,v)| v.clone()).unwrap_or_default();
                        let link_text = collect_text(children);
                        let idx = links.len() + 1;
                        links.push(Link { text: link_text.clone(), href });
                        let txt = if link_text.is_empty() { format!("[{}]", idx) }
                            else { format!("{}{{{}}}", link_text, idx) };
                        add_text(current, &txt, bold, italic, lines, width);
                    }
                    "ul" => {
                        flush_line(current, lines);
                        for child in children {
                            if let HtmlNode::Element { tag, .. } = child {
                                if tag == "li" {
                                    let txt = format!("  • {}", collect_text(&[child.clone()]));
                                    lines.push(txt);
                                }
                            }
                        }
                        lines.push(String::new());
                    }
                    "ol" => {
                        flush_line(current, lines);
                        let mut n = 1;
                        for child in children {
                            if let HtmlNode::Element { tag, .. } = child {
                                if tag == "li" {
                                    let txt = format!("  {}. {}", n, collect_text(&[child.clone()]));
                                    lines.push(txt); n += 1;
                                }
                            }
                        }
                        lines.push(String::new());
                    }
                    "pre" | "code" => {
                        flush_line(current, lines);
                        let code = collect_text(children);
                        for l in code.lines() { lines.push(format!("    {}", l)); }
                        lines.push(String::new());
                    }
                    "img" => {
                        let alt = attrs.iter().find(|(k,_)| k=="alt")
                            .map(|(_,v)| v.clone()).unwrap_or_default();
                        let src = attrs.iter().find(|(k,_)| k=="src")
                            .map(|(_,v)| v.clone()).unwrap_or_default();
                        lines.push(format!("[IMG: {} ({})]", alt, src));
                    }
                    "table" => { lines.push("[TABLE]".into()); }
                    "script" | "style" | "head" | "meta" | "link" => {
                        // Пропускаем
                    }
                    "div" | "span" | "body" | "html" | "header" | "footer" | "nav" |
                    "section" | "article" | "main" | "form" => {
                        render_nodes(children, title, links, lines, current, bold, italic, width);
                    }
                    _ => {
                        render_nodes(children, title, links, lines, current, bold, italic, width);
                    }
                }
            }
        }
    }
}

fn collect_text(nodes: &[HtmlNode]) -> String {
    let mut s = String::new();
    for n in nodes {
        match n {
            HtmlNode::Text(t) => s.push_str(t),
            HtmlNode::Element { children, .. } => s.push_str(&collect_text(children)),
        }
    }
    s
}

fn add_text(current: &mut String, text: &str, bold: &bool, italic: &bool,
            _lines: &mut Vec<String>, _width: usize) {
    let mut decorated = text.to_string();
    if *bold { decorated = format!("\x1B[1m{}\x1B[0m", decorated); }
    if *italic { decorated = format!("\x1B[3m{}\x1B[0m", decorated); }
    current.push_str(&decorated);
    // НЕ делаем перенос слов — просто копим.
}

fn flush_line(current: &mut String, lines: &mut Vec<String>) {
    let line = current.trim().to_string();
    if !line.is_empty() {
        // Убираем ANSI-коды для строк длиннее ширины
        lines.push(line);
    }
    current.clear();
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
     .replace("&lt;", "<")
     .replace("&gt;", ">")
     .replace("&quot;", "\"")
     .replace("&apos;", "'")
     .replace("&nbsp;", " ")
     .replace("&#39;", "'")
}

// ==================== Helpers ====================

fn peek_char(chars: &[char], pos: &usize) -> char { chars.get(*pos).copied().unwrap_or('\0') }
fn peek_str(chars: &[char], pos: &usize, s: &str) -> bool {
    let sc: Vec<char> = s.chars().collect();
    if *pos + sc.len() > chars.len() { return false; }
    chars[*pos..*pos+sc.len()] == sc[..]
}
fn skip_ws(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && chars[*pos].is_whitespace() { *pos += 1; }
}
fn read_ident(chars: &[char], pos: &mut usize) -> String {
    let mut s = String::new();
    while *pos < chars.len() && (chars[*pos].is_alphanumeric() || chars[*pos]=='_' || chars[*pos]=='-') {
        s.push(chars[*pos]); *pos += 1;
    }
    s
}
fn read_until_char(chars: &[char], pos: &mut usize, delim: char) -> String {
    let mut s = String::new();
    while *pos < chars.len() && chars[*pos] != delim { s.push(chars[*pos]); *pos += 1; }
    s
}
fn read_until(chars: &[char], pos: &mut usize, delim: char) -> String {
    let mut s = String::new();
    while *pos < chars.len() && chars[*pos] != delim { s.push(chars[*pos]); *pos += 1; }
    s
}
fn skip_until(chars: &[char], pos: &mut usize, s: &str) {
    while *pos < chars.len() && !peek_str(chars, pos, s) { *pos += 1; }
    if peek_str(chars, pos, s) { *pos += s.len(); }
}

// ==================== Браузер ====================

struct HistoryEntry {
    url: String,
    page: ParsedPage,
}

pub struct Browser {
    history: Vec<HistoryEntry>,
    current_idx: usize,
    width: usize,
    height: usize,
}

impl Browser {
    pub fn new() -> Self {
        Browser { history: Vec::new(), current_idx: 0, width: 78, height: 22 }
    }

    fn current_page(&self) -> Option<&ParsedPage> {
        if self.history.is_empty() { None } else { Some(&self.history[self.current_idx].page) }
    }

    fn current_url(&self) -> &str {
        if self.history.is_empty() { "" } else { &self.history[self.current_idx].url }
    }

    fn navigate(&mut self, url: &str) -> bool {
        crate::print!("  Fetching... ");
        match http::get(url) {
            Ok(resp) => {
                if resp.status == 301 || resp.status == 302 {
                    // Редирект
                    if let Some(loc) = resp.headers.iter().find(|(k,_)| k.eq_ignore_ascii_case("location")) {
                        crate::println!("redirect → {}", loc.1);
                        return self.navigate(&loc.1);
                    }
                }
                if resp.status >= 400 {
                    crate::println!("HTTP {} (error)", resp.status);
                    let body = core::str::from_utf8(&resp.body).unwrap_or("");
                    let mut lines = Vec::new();
                    for l in body.lines().take(20) { lines.push(l.to_string()); }
                    let page = ParsedPage {
                        title: format!("Error {}", resp.status),
                        links: Vec::new(), text_lines: lines,
                        dom: vec![HtmlNode::Text(body.into())],
                    };
                    self.push_history(url, page);
                    return true;
                }

                let body_str = core::str::from_utf8(&resp.body).unwrap_or("");
                crate::println!("{} bytes (HTTP {})", resp.body.len(), resp.status);
                let dom = parse_html(body_str);
                let page = render_page(&dom, self.width);

                self.push_history(url, page);
                true
            }
            Err(e) => {
                crate::println!("ERROR: {}", e);
                let lines = vec![format!("  Failed: {}", e)];
                let page = ParsedPage { title: "Error".into(), links: Vec::new(), text_lines: lines, dom: Vec::new() };
                self.push_history(url, page);
                true // мы всё равно показываем страницу (даже с ошибкой)
            }
        }
    }

    fn push_history(&mut self, url: &str, page: ParsedPage) {
        // Удаляем всё после current_idx.
        while self.history.len() > self.current_idx + 1 {
            self.history.pop();
        }
        self.history.push(HistoryEntry { url: url.to_string(), page });
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
        self.current_idx = self.history.len() - 1;
    }

    fn back(&mut self) -> bool {
        if self.current_idx > 0 { self.current_idx -= 1; true } else { false }
    }

    fn forward(&mut self) -> bool {
        if self.current_idx + 1 < self.history.len() { self.current_idx += 1; true } else { false }
    }

    /// Рендерит страницу на VGA-экран.
    fn render(&self) {
        if let Some(page) = self.current_page() {
            // Заголовок
            println!("╔{}╗", "═".repeat(self.width));
            let title = if page.title.is_empty() { "DeiX Browser" } else { &page.title };
            println!("║{:^width$}║", title, width = self.width);
            println!("╠{}╣", "═".repeat(self.width));

            // Контент
            let max_lines = self.height - 4; // место для адресной строки
            for (i, line) in page.text_lines.iter().enumerate() {
                if i >= max_lines { break; }
                // Простая обрезка (без ANSI)
                let clean = strip_ansi(line);
                let display = if clean.len() > self.width {
                    format!("{}…", &clean[..self.width.saturating_sub(1)])
                } else {
                    format!("{:<width$}", clean, width = self.width)
                };
                println!("║{}║", display);
            }
            // Заполняем пустые строки.
            let displayed = page.text_lines.len().min(max_lines);
            for _ in displayed..max_lines {
                println!("║{}║", " ".repeat(self.width));
            }

            println!("╠{}╣", "═".repeat(self.width));

            // Ссылки
            if !page.links.is_empty() {
                let link_line = if page.links.len() <= 8 {
                    page.links.iter().enumerate()
                        .map(|(i,l)| format!("{}.{}", i+1, &l.text.chars().take(8).collect::<String>()))
                        .collect::<Vec<_>>().join(" ")
                } else {
                    format!("{} links (type number to follow)", page.links.len())
                };
                println!("║{:<width$}║", link_line, width = self.width);
                println!("╠{}╣", "═".repeat(self.width));
            }

            // Адресная строка
            println!("║ URL: {:<width_minus5$}║", self.current_url(), width_minus5 = self.width - 6);
            println!("╚{}╝", "═".repeat(self.width));
            println!(" [b]ack [f]wd [r]eload [q]uit | type URL or link #");
        }
    }
}

fn strip_ansi(s: &str) -> String {
    let mut r = String::new();
    let mut skip = false;
    for c in s.chars() {
        if c == '\x1B' { skip = true; continue; }
        if skip {
            if c == 'm' { skip = false; }
            continue;
        }
        r.push(c);
    }
    r
}

// ==================== Главный цикл браузера ====================

pub fn run(url: &str) {
    if !crate::rtl8139::is_ready() {
        println!("Browser needs a network card.");
        println!("Run QEMU with: -net nic,model=rtl8139 -net user");
        return;
    }

    let mut browser = Browser::new();
    browser.width = 78;
    browser.height = 20;

    crate::vgaglobal::with_writer(|w| w.clear_screen());

    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                            🪐 DeiX Browser v0.2                              ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");
    println!();

    let start_url = if url.is_empty() { "http://info.cern.ch/" } else { url };

    if !browser.navigate(start_url) {
        println!("Failed to load start page.");
    }
    browser.render();

    // Главный цикл
    loop {
        print!("browser> ");
        let line = crate::keyboard::read_line();
        let line = line.trim().to_string();

        if line.is_empty() { continue; }

        match line.as_str() {
            "q" | "quit" | "exit" => {
                println!("Exiting browser.");
                break;
            }
            "b" | "back" => {
                if browser.back() { browser.render(); }
                else { println!("(already at first page)"); }
            }
            "f" | "forward" | "fwd" => {
                if browser.forward() { browser.render(); }
                else { println!("(already at last page)"); }
            }
            "r" | "reload" | "refresh" => {
                let url = browser.current_url().to_string();
                browser.navigate(&url);
                browser.render();
            }
            "h" | "help" | "?" => {
                println!("  [b]ack  [f]orward  [r]eload  [q]uit");
                println!("  Enter URL or link number to navigate");
                if let Some(p) = browser.current_page() {
                    println!("  Links: {}", p.links.len());
                    for (i, l) in p.links.iter().enumerate() {
                        println!("    {}. {} → {}", i+1, l.text, l.href);
                    }
                }
            }
            _ => {
                // Проверяем: число (ссылка) или URL
                if let Ok(num) = line.parse::<usize>() {
                    if let Some(page) = browser.current_page() {
                        if num >= 1 && num <= page.links.len() {
                            let href = &page.links[num-1].href;
                            let base = browser.current_url();
                            let full_url = resolve_relative(base, href);
                            browser.navigate(&full_url);
                            browser.render();
                        } else {
                            println!("Link #{} not found.", num);
                        }
                    }
                } else if line.contains("://") || line.contains('.') {
                    let url = if line.contains("://") { line.to_string() }
                        else { format!("http://{}", line) };
                    browser.navigate(&url);
                    browser.render();
                } else {
                    // Поиск в Google
                    let query = line.replace(' ', "+");
                    let url = format!("http://www.google.com/search?q={}", query);
                    browser.navigate(&url);
                    browser.render();
                }
            }
        }
    }

    crate::vgaglobal::with_writer(|w| w.clear_screen());
}

fn resolve_relative(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if href.starts_with("//") {
        format!("http:{}", href)
    } else if href.starts_with('/') {
        if let Some((host, _, _)) = http::parse_url(base) {
            format!("http://{}{}", host, href)
        } else { href.to_string() }
    } else {
        // Относительный путь
        let base_without_last = if let Some(slash) = base.rfind('/') {
            if slash > 7 { &base[..slash] } else { base }
        } else { base };
        format!("{}/{}", base_without_last, href)
    }
}

// ==================== CLI ====================

pub fn cmd_browser(arg: &str) {
    run(arg);
}

// ── Shared HTML parsing for GUI browser ──

pub fn parse_html_static(html: &str) -> Vec<HtmlNode> { parse_html(html) }

pub fn render_html_static(dom: &[HtmlNode], width: usize) -> (String, Vec<(String, String)>, Vec<String>) {
    let page = render_page(dom, width);
    let links: Vec<(String, String)> = page.links.iter().map(|l| (l.text.clone(), l.href.clone())).collect();
    (page.title, links, page.text_lines)
}
