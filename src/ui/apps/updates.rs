//! Отрисовка окна Updates (центр обновлений ОС).
//!
//! Кнопки заданы функциями в координатах области содержимого; те же
//! прямоугольники использует обработчик кликов в desktop.rs.

use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::Window;
use crate::update::center::{Center, Stage};
use alloc::format;

const HEADER_H: i32 = 56;
const BODY_Y: i32 = 64;

/// Главная кнопка стадии (справа внизу).
pub fn primary_btn_rect(win_w: u32, win_h: u32) -> Rect {
    Rect::new(win_w as i32 - 208, win_h as i32 - 38, 196, 28)
}

/// Вторая кнопка (левее главной).
pub fn secondary_btn_rect(win_w: u32, win_h: u32) -> Rect {
    Rect::new(win_w as i32 - 336, win_h as i32 - 38, 120, 28)
}

fn draw_button(r: &mut Renderer, theme: &UiTheme, x: i32, y: i32, rect: Rect, label: &str, primary: bool) {
    let bg = if primary { theme.accent } else { theme.titlebar_inactive };
    let fg = if primary { Color::WHITE } else { theme.text_primary };
    crate::ui::widgets::icon_button(
        r,
        x + rect.x,
        y + rect.y,
        rect.w,
        rect.h,
        6,
        bg,
        fg,
        crate::ui::widgets::icon_for_label(label),
        label,
    );
}

/// Текст с переносом по ширине окна; возвращает y следующей строки.
fn draw_wrapped(r: &mut Renderer, x: i32, mut y: i32, width: u32, text: &str, color: Color, max_lines: usize) -> i32 {
    let per_line = ((width.saturating_sub(32)) / 8).max(16) as usize;
    let mut rest = text;
    for _ in 0..max_lines {
        if rest.is_empty() {
            break;
        }
        let mut cut = rest.len().min(per_line);
        while cut > 0 && !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut < rest.len() {
            if let Some(space) = rest[..cut].rfind(' ') {
                if space > 0 {
                    cut = space;
                }
            }
        }
        r.draw_text(x + 16, y, rest[..cut].trim_end(), color, None);
        rest = rest[cut..].trim_start();
        y += 18;
    }
    y
}

fn headline(c: &Center) -> &'static str {
    if c.blocked().is_some() {
        return "Updates are unavailable";
    }
    match c.stage() {
        Stage::Idle => "Check for a new DeiX build",
        Stage::Checking => "Checking the update server...",
        Stage::UpToDate => "You are up to date",
        Stage::Available => "An update is available",
        Stage::Downloading => "Downloading the update",
        Stage::Ready => "The update is ready to install",
        Stage::Scheduled => "An update is waiting for a restart",
        Stage::Failed => "The update did not complete",
    }
}

pub fn draw_updates(r: &mut Renderer, theme: &UiTheme, w: &Window, c: &Center, content_y: i32) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);
    let x = w.x;
    let y = content_y;
    let ww = w.width;
    let wh = w.height;
    let warn = Color::rgb(230, 160, 60);
    let bad = Color::rgb(220, 60, 60);

    r.draw_icon(x + 16, y + 14, IconType::Reload, theme.accent);
    r.draw_text(x + 44, y + 10, "Updates", theme.accent, None);
    r.draw_text(x + 44, y + 28, &format!("{}  |  {}", crate::OS_RELEASE, c.installed_line()), theme.text_secondary, None);
    r.draw_hline(x + 12, y + HEADER_H, ww.saturating_sub(24), theme.titlebar_active);
    r.draw_text(x + 16, y + BODY_Y, headline(c), theme.text_primary, None);

    let mut ly = y + BODY_Y + 28;
    if let Some(reason) = c.blocked() {
        draw_wrapped(r, x, ly, ww, reason, warn, 5);
    } else {
        match c.stage() {
            Stage::Idle | Stage::UpToDate => {
                if let Some(info) = c.info() {
                    ly = draw_wrapped(r, x, ly, ww, info, theme.text_secondary, 3);
                }
                let hint = if c.stage() == Stage::Idle {
                    "The server sends a signed manifest; this system verifies the signature."
                } else {
                    "Press the button to ask the server again."
                };
                draw_wrapped(r, x, ly + 8, ww, hint, theme.text_secondary, 3);
            }
            Stage::Checking => {
                draw_wrapped(r, x, ly, ww, "Contacting the server and verifying the manifest signature.", theme.text_secondary, 3);
            }
            Stage::Available => {
                for line in c.release_lines() {
                    r.draw_text(x + 16, ly, &line, theme.text_primary, None);
                    ly += 20;
                }
                draw_wrapped(
                    r,
                    x,
                    ly + 8,
                    ww,
                    "Downloading only fetches the package. Nothing is installed until you confirm.",
                    theme.text_secondary,
                    3,
                );
            }
            Stage::Downloading => {
                let percent = c.percent();
                let bar = Rect::new(16, BODY_Y + 30, ww.saturating_sub(32), 18);
                r.draw_rect(x + bar.x, y + bar.y, bar.w, bar.h, theme.titlebar_active);
                let fill_w = (bar.w.saturating_sub(4)) as u64 * percent as u64 / 100;
                r.fill_rect(x + bar.x + 2, y + bar.y + 2, fill_w as u32, bar.h - 4, theme.accent);
                r.draw_text(x + 16, y + BODY_Y + 56, &format!("{}%  {}", percent, c.progress_line()), theme.text_secondary, None);
                draw_wrapped(r, x, y + BODY_Y + 82, ww, "Do not power off the machine.", theme.text_secondary, 2);
            }
            Stage::Ready => {
                for line in c.release_lines() {
                    r.draw_text(x + 16, ly, &line, theme.text_primary, None);
                    ly += 20;
                }
                if c.confirming() {
                    ly = draw_wrapped(
                        r,
                        x,
                        ly + 8,
                        ww,
                        "The system restarts, asks for your password as usual, and replaces /system and the kernel. Your data in /userdata is kept.",
                        warn,
                        4,
                    );
                    draw_wrapped(
                        r,
                        x,
                        ly + 4,
                        ww,
                        "A power loss while the kernel is written can leave the disk unbootable.",
                        bad,
                        3,
                    );
                } else {
                    draw_wrapped(
                        r,
                        x,
                        ly + 8,
                        ww,
                        "Installing restarts the system; the update is applied after you sign in.",
                        theme.text_secondary,
                        3,
                    );
                }
            }
            Stage::Scheduled => {
                draw_wrapped(
                    r,
                    x,
                    ly,
                    ww,
                    "The update is applied after the next restart and sign-in. Cancel removes the request.",
                    theme.text_secondary,
                    4,
                );
            }
            Stage::Failed => {
                draw_wrapped(r, x, ly, ww, c.error().unwrap_or("Unknown error."), bad, 5);
            }
        }
    }

    if let Some(label) = c.secondary_label() {
        draw_button(r, theme, x, y, secondary_btn_rect(ww, wh), label, false);
    }
    if let Some(label) = c.primary_label() {
        draw_button(r, theme, x, y, primary_btn_rect(ww, wh), label, true);
    }
}
