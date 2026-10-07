//! Система автозапуска DeiX.
//!
//! После входа пользователя ядро ищет `/userdata/config/autostart.cfg`
//! и исполняет перечисленные в нём команды построчно. Простой формат —
//! как в DOS AUTOEXEC.BAT или Unix rc.local. Старые сборки держали файл
//! в корне тома как AUTOSTART.CFG — он одноразово переносится.
//!
//! ## Формат autostart.cfg
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
//! 2. Исполнение autostart.cfg (этот модуль)
//! 3. Экран входа пользователя
//! 4. CLI

use crate::ext2;
use crate::cli;


/// Канонический путь сценария автозапуска в layout /userdata.
const AUTOSTART_PATH: &str = "/userdata/config/autostart.cfg";
/// Имя файла в корне тома у старых сборок.
const LEGACY_FILE: &str = "AUTOSTART.CFG";
const MAX_LINES: usize = 64;
const MAX_LINE_LEN: usize = 256;

/// Содержимое autostart.cfg «из коробки».
///
/// По умолчанию файл пустой (только комментарии): любая блокирующая
/// команда здесь вешает загрузку, а проверить это пользователю нечем —
/// система просто перестаёт отвечать.
const DEFAULT_CFG: &str = "\
# autostart.cfg — команды, выполняемые ПОСЛЕ входа пользователя.
# Выполняется от имени вошедшего: до аутентификации файл не читается.
#
# ВНИМАНИЕ: НЕ добавляйте сюда 'gpu mode'. Эта команда запускает
# блокирующий цикл рабочего стола (ui::run_desktop_session), который
# ждёт Esc и не возвращает управление. В автозапуске это выглядит как
# полное зависание системы: CLI не стартует, ввод не принимается.
# Графический режим включайте вручную командой 'gpu mode' из CLI.
";

/// Одноразовый перенос AUTOSTART.CFG из корня тома (старые сборки) в
/// /userdata/config/autostart.cfg. Существующий новый файл не перетирается.
fn migrate_legacy() {
    if crate::vfs::exists(AUTOSTART_PATH) {
        return;
    }
    let old = match ext2::read_file(LEGACY_FILE) {
        Ok(d) => d,
        Err(_) => return,
    };
    let _ = crate::vfs::mkdir("/userdata/config");
    if crate::vfs::write_file(AUTOSTART_PATH, &old).is_ok() {
        let _ = ext2::delete_file(LEGACY_FILE);
        crate::println!("  [autostart] AUTOSTART.CFG перенесён в {}", AUTOSTART_PATH);
    }
}

/// Создаёт autostart.cfg со значениями по умолчанию, если его ещё нет.
///
/// Вызывается после первого успешного входа. Существующий файл не
/// трогаем — пользователь мог его отредактировать.
pub fn ensure_default() {
    if !ext2::is_formatted() {
        return;
    }
    migrate_legacy();
    if crate::vfs::exists(AUTOSTART_PATH) {
        return;
    }
    let _ = crate::vfs::mkdir("/userdata/config");
    match crate::vfs::write_file(AUTOSTART_PATH, DEFAULT_CFG.as_bytes()) {
        Ok(()) => crate::println!("  [autostart] создан {} (шаблон)", AUTOSTART_PATH),
        Err(_) => crate::println!("  [autostart] не удалось создать {}", AUTOSTART_PATH),
    }
}

pub fn run() {
    if !ext2::is_formatted() {
        crate::println!("  [autostart] Disk not formatted — skipping.");
        return;
    }

    migrate_legacy();
    let data = match crate::vfs::read_file(AUTOSTART_PATH) {
        Ok(d) => d,
        Err(_) => {
            crate::println!("  [autostart] нет {} (это нормально — создайте его для автозапуска команд).", AUTOSTART_PATH);
            return;
        }
    };

    let text = match core::str::from_utf8(&data) {
        Ok(t) => t,
        Err(_) => {
            crate::println!("  [autostart] autostart.cfg: не UTF-8, пропускаем.");
            return;
        }
    };

    crate::println!("  [autostart] Выполняется {}...", AUTOSTART_PATH);

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
        const BLOCKING: [&str; 3] = ["gpu", "lock", "logo"];
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

