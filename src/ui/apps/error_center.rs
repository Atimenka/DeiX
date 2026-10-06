//! Error Center — графический просмотр журнала диагностики.
//!
//! Данные берутся из кольцевого буфера `crate::diag` в момент отрисовки:
//! это те же записи, что показывает CLI-команда `error list`, включая
//! события, произошедшие до запуска GUI.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::diag;
use crate::renderer::{Color, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::Window;

/// Высота строки списка.
pub const ROW_H: i32 = 18;
/// Смещение первой строки списка от начала содержимого окна.
pub const LIST_START: i32 = 96;
/// Сколько строк помещается в списке.
pub const LIST_ROWS: usize = 8;

/// Записи, которые показывает список: ошибки и предупреждения,
/// самые свежие первыми.
pub fn visible_records() -> Vec<diag::ErrorRecord> {
    let mut records = diag::ring::filter(64, |r| r.severity >= diag::Severity::Warning);
    records.reverse();
    records
}

/// Отрисовка окна Error Center.
pub fn draw_error_center(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    selected: Option<usize>,
    scroll: usize,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    r.draw_text(w.x + 12, content_y + 8, "DEIX ERROR CENTER", theme.accent, None);

    // Счётчики по уровням — реальная сводка буфера.
    let s = diag::summary();
    let sev = &s.by_severity;
    let counts = format!(
        "Critical {}   Errors {}   Warnings {}   Info {}",
        sev[diag::Severity::Critical.level() as usize]
            + sev[diag::Severity::Panic.level() as usize]
            + sev[diag::Severity::Fatal.level() as usize],
        sev[diag::Severity::Error.level() as usize],
        sev[diag::Severity::Warning.level() as usize],
        sev[diag::Severity::Info.level() as usize] + sev[diag::Severity::Notice.level() as usize],
    );
    r.draw_text(w.x + 12, content_y + 28, &counts, theme.text_primary, None);

    let session = format!(
        "Сессия #{}   событий: {}   в буфере: {}{}",
        s.boot_session,
        s.total_events,
        s.buffered,
        if diag::panic::has_previous_failure() {
            "   ЕСТЬ ОТЧЁТ О ПРЕДЫДУЩЕМ ОТКАЗЕ"
        } else {
            ""
        }
    );
    r.draw_text(w.x + 12, content_y + 44, &session, theme.text_secondary, None);

    // Шапка списка.
    let list_y = content_y + LIST_START - 22;
    r.fill_rect(w.x + 8, list_y, w.width - 16, 20, theme.titlebar_active);
    r.draw_text(w.x + 16, list_y + 2, "SEV    CODE          MESSAGE", theme.text_primary, None);

    let records = visible_records();
    if records.is_empty() {
        r.draw_text(
            w.x + 16,
            content_y + LIST_START + 4,
            "Ошибок и предупреждений не зарегистрировано.",
            theme.text_secondary,
            None,
        );
        return;
    }

    let mut ty = content_y + LIST_START;
    for (i, rec) in records.iter().enumerate().skip(scroll).take(LIST_ROWS) {
        let is_sel = selected == Some(i);
        let bg = if is_sel { theme.titlebar_active } else { theme.window_bg };
        r.fill_rect(w.x + 8, ty, w.width - 16, ROW_H as u32, bg);

        let sev_color = match rec.severity {
            diag::Severity::Warning => Color::rgb(234, 179, 8),
            diag::Severity::Error => Color::RED,
            _ => Color::rgb(244, 63, 94),
        };
        r.draw_text(w.x + 16, ty + 2, rec.severity.tag(), sev_color, None);
        r.draw_text(w.x + 72, ty + 2, &rec.code.as_string(), theme.text_primary, None);

        let mut msg = String::from(rec.message());
        if rec.occurrences > 1 {
            msg.push_str(&format!(" (x{})", rec.occurrences));
        }
        let max_chars = ((w.width as i32 - 190) / 8).max(8) as usize;
        if msg.chars().count() > max_chars {
            msg = msg.chars().take(max_chars - 1).collect();
            msg.push('…');
        }
        let fg = if is_sel { theme.accent } else { theme.text_primary };
        r.draw_text(w.x + 176, ty + 2, &msg, fg, None);
        ty += ROW_H + 2;
    }

    // Карточка выбранной записи (§16): код, время, количество, действие
    // и расширенный идентификатор, который пользователь может переписать
    // в отчёт разработчику.
    let card_y = content_y + LIST_START + (LIST_ROWS as i32) * (ROW_H + 2) + 6;
    if let Some(rec) = selected.and_then(|i| records.get(i)) {
        r.fill_rect(w.x + 8, card_y, w.width - 16, 2, theme.accent);
        let line1 = format!(
            "{}  {}  {}",
            rec.code.as_string(),
            diag::code::summary_of(rec.code),
            diag::format_time(rec.timestamp_ms)
        );
        r.draw_text(w.x + 12, card_y + 8, &line1, theme.text_primary, None);

        let line2 = format!(
            "pid {}   повторов {}   действие: {}",
            rec.pid,
            rec.occurrences,
            rec.action.name()
        );
        r.draw_text(w.x + 12, card_y + 26, &line2, theme.text_secondary, None);

        let line3 = format!("ID: {}", diag::extended_id(rec));
        r.draw_text(w.x + 12, card_y + 44, &line3, theme.accent, None);

        if let Some(cause) = diag::ring::by_id(rec.related_id) {
            let line4 = format!(
                "Первопричина: {} {}",
                cause.code.as_string(),
                cause.message()
            );
            r.draw_text(w.x + 12, card_y + 62, &line4, theme.text_secondary, None);
        }
    } else {
        r.draw_text(
            w.x + 12,
            card_y + 8,
            "Выберите запись, чтобы увидеть карточку ошибки.",
            theme.text_secondary,
            None,
        );
    }
}
