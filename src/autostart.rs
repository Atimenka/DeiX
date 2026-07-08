#![allow(dead_code)]
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

        crate::print!("  [autostart] > ");
        crate::println!("{}", line);

        // Выполняем команду через CLI.
        cli::execute(line);

        line_count += 1;
    }

    crate::println!("  [autostart] Done ({} commands executed).", line_count);
}

/// Создаёт стандартный AUTOSTART.CFG с комментариями.
pub fn create_default() {
    let content = concat!(
        "# DeiX Autostart Configuration\n",
        "# Lines starting with # are comments.\n",
        "# Commands are executed in order before the login screen.\n",
        "#\n",
        "# Example:\n",
        "#   echo Starting network...\n",
        "#   pkg install netping\n",
        "#   echo Boot complete.\n",
        "\n",
        "echo DeiX autostart: OK\n",
        "echo Type 'help' for available commands.\n",
    );

    if !ext2::is_formatted() {
        let _ = ext2::format();
    }
    let _ = ext2::write_file(AUTOSTART_FILE, content.as_bytes());
}
