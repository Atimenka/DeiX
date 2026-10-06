//! Команды оболочки: `log`, `error`, `panic`, `diagnostics`.

use alloc::vec::Vec;

use super::code::{ErrorCode, Subsystem, ALL_SUBSYSTEMS};
use super::record::ErrorRecord;
use super::severity::Severity;
use super::{log, panic, persist, ring, NONE};
use crate::println;

/// Сколько записей показывают списки по умолчанию.
const DEFAULT_LIMIT: usize = 30;

/// `log [list|errors|warnings|critical|kernel|boot|dinit|kmod|<файл>]`.
pub fn cmd_log(arg: &str) {
    let arg = arg.trim();
    match arg {
        "" => print_records(ring::snapshot(DEFAULT_LIMIT)),
        "list" => {
            println!("  Файлы журналов в {}:", persist::LOG_DIR);
            for &(path, desc) in persist::KNOWN_FILES {
                let size = crate::vfs::stat(path).map(|s| s.size).unwrap_or(0);
                println!("    {:<28} {:>7} Б  {}", path, size, desc);
            }
            println!("  В буфере: {} записей, всего за сессию: {}",
                ring::len(), ring::total_events());
        }
        "errors" => print_records(filter_severity(Severity::Error)),
        "warnings" => print_records(filter_severity(Severity::Warning)),
        "critical" => print_records(filter_severity(Severity::Critical)),
        "kernel" => print_records(filter_subsystem(Subsystem::Kernel)),
        "boot" => print_file(persist::FILE_BOOT),
        "dinit" => print_file(persist::FILE_DINIT),
        "kmod" => print_file(persist::FILE_KMOD),
        "flush" => {
            let n = persist::flush();
            println!("  Сохранено на диск: {} строк.", n);
        }
        "clear" => {
            ring::clear();
            println!("  Кольцевой буфер очищен (файлы на диске не тронуты).");
        }
        _ => {
            println!("  log                 — последние события");
            println!("  log list            — файлы журналов и их размер");
            println!("  log errors          — только ошибки и выше");
            println!("  log warnings        — предупреждения и выше");
            println!("  log critical        — только критические");
            println!("  log kernel          — события ядра");
            println!("  log boot|dinit|kmod — журнал с диска");
            println!("  log flush           — записать буфер на диск");
            println!("  log clear           — очистить буфер в памяти");
        }
    }
}

/// `error [list|show DX-XXX-NNNN]`.
pub fn cmd_error(arg: &str) {
    let arg = arg.trim();
    if arg.is_empty() || arg == "list" {
        let errors = ring::filter(DEFAULT_LIMIT, |r| r.severity.is_error());
        if errors.is_empty() {
            println!("  Ошибок за текущую сессию не зарегистрировано.");
            return;
        }
        for r in &errors {
            println!(
                "  {:<9} {}  {}",
                r.severity.tag(),
                r.code.as_string(),
                r.message()
            );
        }
        println!("  (подробности: error show <код>)");
        return;
    }

    if let Some(rest) = arg.strip_prefix("show") {
        let Some(code) = ErrorCode::parse(rest.trim()) else {
            println!("  Неразборчивый код: {} (ожидается DX-XXX-NNNN)", rest.trim());
            return;
        };
        show_code(code);
        return;
    }

    println!("  error list          — список ошибок сессии");
    println!("  error show <код>    — карточка ошибки, например error show DX-NET-0007");
}

/// Карточка ошибки: описание из реестра плюс зарегистрированные события.
fn show_code(code: ErrorCode) {
    println!("  {}", code.as_string());
    println!("  Подсистема:  {}", code.subsystem().name());
    match code.info() {
        Some(info) => {
            println!("  Сбой:        {}", info.summary);
            println!("  Причина:     {}", info.detail);
            println!("  Действие:    {}", info.action);
        }
        None => println!("  Код не зарегистрирован в реестре."),
    }

    let hits = ring::filter(10, |r| r.code == code);
    if hits.is_empty() {
        println!("  За текущую сессию не встречался.");
        return;
    }
    println!("  События ({} последних):", hits.len());
    for r in &hits {
        println!(
            "    {} {} pid={} ×{}",
            log::format_time(r.timestamp_ms),
            r.message(),
            r.pid,
            r.occurrences
        );
        if let Some(cause) = ring::by_id(r.related_id) {
            println!(
                "      первопричина: {} {}",
                cause.code.as_string(),
                cause.message()
            );
        }
        println!("      идентификатор: {}", super::extended_id(r));
    }
}

/// `panic [last|clear]`.
pub fn cmd_panic(arg: &str) {
    match arg.trim() {
        "" | "last" => match panic::load_raw_report() {
            Some(report) => {
                for line in report.lines() {
                    println!("  {}", line);
                }
            }
            None => println!("  Сохранённых отчётов об отказе нет."),
        },
        "clear" => {
            panic::clear_previous_failure();
            println!("  Отчёт об отказе удалён.");
        }
        _ => {
            println!("  panic last   — показать последний отчёт об отказе");
            println!("  panic clear  — удалить отчёт");
        }
    }
}

/// `diagnostics [summary|mode on|mode off]`.
pub fn cmd_diagnostics(arg: &str) {
    let arg = arg.trim();
    match arg {
        "" | "summary" => print_summary(),
        "mode on" => {
            log::set_diagnostic_mode(true);
            println!("  Диагностический режим ВКЛЮЧЁН: регистрируются TRACE и DEBUG.");
        }
        "mode off" => {
            log::set_diagnostic_mode(false);
            println!("  Диагностический режим выключен: порог журналирования INFO.");
        }
        _ => {
            println!("  diagnostics summary   — сводка по событиям");
            println!("  diagnostics mode on   — включить TRACE/DEBUG");
            println!("  diagnostics mode off  — вернуть обычный режим");
        }
    }
}

/// Сводка по буферу событий.
fn print_summary() {
    let s = super::summary();
    println!("  DeiX Diagnostic Center — сводка");
    println!("  Сессия загрузки:   #{}", s.boot_session);
    println!(
        "  Режим:             {} (порог {})",
        if s.diagnostic_mode { "диагностический" } else { "обычный" },
        s.threshold.tag()
    );
    println!(
        "  Событий:           {} всего, {} в буфере, {} вытеснено",
        s.total_events, s.buffered, s.dropped
    );

    println!("  Кодов в реестре: {}", super::CODE_COUNT);
    println!("  По уровням:");
    for sev in Severity::ALL {
        let n = s.by_severity[sev.level() as usize];
        if n > 0 {
            println!("    {:<8} {:<16} {}", sev.tag(), sev.name(), n);
        }
    }

    let mut any = false;
    for sub in ALL_SUBSYSTEMS {
        let n = s.by_subsystem[*sub as usize];
        if n > 0 {
            if !any {
                println!("  По подсистемам:");
                any = true;
            }
            println!("    {:<6} {:<16} {}", sub.tag(), sub.name(), n);
        }
    }

    if panic::has_previous_failure() {
        println!("  ВНИМАНИЕ: сохранён отчёт о предыдущем отказе — panic last");
    }
}

/// Печатает записи в человекочитаемом виде.
fn print_records(records: Vec<ErrorRecord>) {
    if records.is_empty() {
        println!("  Журнал пуст.");
        return;
    }
    for r in &records {
        println!("  {}", log::format_human(r));
    }
    println!("  ({} записей; полный журнал: log list)", records.len());
}

/// Записи с уровнем не ниже указанного.
fn filter_severity(min: Severity) -> Vec<ErrorRecord> {
    ring::filter(DEFAULT_LIMIT, |r| r.severity >= min)
}

/// Записи конкретной подсистемы.
fn filter_subsystem(sub: Subsystem) -> Vec<ErrorRecord> {
    ring::filter(DEFAULT_LIMIT, |r| r.code != NONE && r.code.subsystem() == sub)
}

/// Печатает хвост файла журнала с диска.
fn print_file(path: &str) {
    let lines = persist::tail(path, DEFAULT_LIMIT);
    if lines.is_empty() {
        println!("  {} пуст или недоступен.", path);
        return;
    }
    for line in &lines {
        println!("  {}", line);
    }
}

/// Диалог «ПРЕДЫДУЩИЙ СБОЙ СИСТЕМЫ» в текстовом виде — вызывается после
/// загрузки, если на диске найден отчёт об отказе.
pub fn announce_previous_failure() {
    let Some(prev) = panic::previous_failure() else {
        return;
    };
    println!("");
    println!("  ┌──────────────────────────────────────────────────┐");
    println!("  │             ПРЕДЫДУЩИЙ СБОЙ СИСТЕМЫ              │");
    println!("  ├──────────────────────────────────────────────────┤");
    println!("  │ DeiX обнаружил аварийное завершение прошлой      │");
    println!("  │ сессии. Отчёт об отказе сохранён.                │");
    println!("  │                                                  │");
    let code_line = alloc::format!("Код:    {}", prev.code.as_string());
    let kind_line = alloc::format!("Тип:    {}", prev.kind);
    let mod_line = alloc::format!("Модуль: {}", prev.module);
    println!("  │ {:<48} │", code_line);
    println!("  │ {:<48} │", kind_line);
    println!("  │ {:<48} │", mod_line);
    println!("  │                                                  │");
    println!("  │ Просмотр: panic last    Удалить: panic clear     │");
    println!("  └──────────────────────────────────────────────────┘");
}
