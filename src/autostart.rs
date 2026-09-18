//! Система автозапуска DeiX.
//!
//! При старте ядро ищет на ext2-диске файл `AUTOSTART.CFG` и исполняет
//! перечисленные в нём команды построчно. Простой формат — как в DOS
//! AUTOEXEC.BAT или Unix rc.local.
//!
//! ## Формат AUTOSTART.CFG
//!
//! ```text
//! # Комментарий (строки с #)
//! echo Starting DeiX services...
//! lang ru
//! pkg install netping
//! run NETPING.MEX
//! echo Boot complete.
//! ```
//!
//! ## Порядок выполнения
//!
//! 1. Загрузка модулей ядра (.kmod)
//! 2. Исполнение AUTOSTART.CFG (этот модуль)
//! 3. Экран входа пользователя
//! 4. CLI

use crate::ext2;
use crate::cli;


const AUTOSTART_FILE: &str = "AUTOSTART.CFG";
const MAX_LINES: usize = 64;
const MAX_LINE_LEN: usize = 256;

/// Исполняет скрипт автозапуска (если существует).
/// Вызывается ДО экрана входа пользователя.

/// Содержимое AUTOSTART.CFG «из коробки».
///
/// По умолчанию файл пустой (только комментарии): любая блокирующая
/// команда здесь вешает загрузку, а проверить это пользователю нечем —
/// система просто перестаёт отвечать.
const DEFAULT_CFG: &str = "\
# AUTOSTART.CFG — команды, выполняемые ПОСЛЕ входа пользователя.
# Выполняется от имени вошедшего: до аутентификации файл не читается.
#
# ВНИМАНИЕ: НЕ добавляйте сюда 'gpu mode'. Эта команда запускает
# блокирующий цикл рабочего стола (ui::run_desktop_session), который
# ждёт Esc и не возвращает управление. В автозапуске это выглядит как
# полное зависание системы: CLI не стартует, ввод не принимается.
# Графический режим включайте вручную командой 'gpu mode' из CLI.
";

/// Создаёт AUTOSTART.CFG со значениями по умолчанию, если его ещё нет.
///
/// Вызывается после первого успешного входа. Существующий файл не
/// трогаем — пользователь мог его отредактировать.
pub fn ensure_default() {
    if !ext2::is_formatted() {
        return;
    }
    if ext2::read_file(AUTOSTART_FILE).is_ok() {
        return;
    }
    match ext2::write_file(AUTOSTART_FILE, DEFAULT_CFG.as_bytes()) {
        Ok(()) => crate::println!("  [autostart] создан AUTOSTART.CFG (по умолчанию: gpu mode)"),
        Err(_) => crate::println!("  [autostart] не удалось создать AUTOSTART.CFG"),
    }
}

pub fn run() {
    if !ext2::is_formatted() {
        crate::println!("  [autostart] Disk not formatted — skipping.");
        return;
    }

    let data = match ext2::read_file(AUTOSTART_FILE) {
        Ok(d) => d,
        Err(_) => {
            crate::println!("  [autostart] No AUTOSTART.CFG found (normal — create one to auto-run commands at boot).");
            return;
        }
    };

    let text = match core::str::from_utf8(&data) {
        Ok(t) => t,
        Err(_) => {
            crate::println!("  [autostart] AUTOSTART.CFG: not valid UTF-8, skipping.");
            return;
        }
    };

    crate::println!("  [autostart] Executing AUTOSTART.CFG...");

    let mut line_count = 0usize;

    for line in text.lines() {
        if line_count >= MAX_LINES {
            crate::println!("  [autostart] Too many lines (max {}), stopping.", MAX_LINES);
            break;
        }

        let line = line.trim();

        // Пустые строки и комментарии.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.len() > MAX_LINE_LEN {
            crate::println!("  [autostart] Line too long ({} chars), skipping.", line.len());
            continue;
        }

        // ЗАЩИТА ОТ ЗАВИСАНИЯ: некоторые команды не возвращают
        // управление — они уходят в собственный цикл и ждут Esc или
        // ввода пользователя. В автозапуске это выглядит как полное
        // зависание системы сразу после входа: CLI не стартует.
        // Проверять руками нечем, поэтому отсекаем их здесь.
        let cmd = line.split_whitespace().next().unwrap_or("");
        const BLOCKING: [&str; 6] = ["gpu", "dsm", "recovery", "fastbootd", "lock", "logo"];
        if BLOCKING.contains(&cmd) {
            crate::println!(
                "  [autostart] пропуск '{}': команда блокирующая (запустите вручную)",
                line
            );
            line_count += 1;
            continue;
        }

        crate::print!("  [autostart] > ");
        crate::println!("{}", line);

        // Выполняем команду через CLI.
        cli::execute(line);

        line_count += 1;
    }

    crate::println!("  [autostart] Done ({} commands executed).", line_count);
}

