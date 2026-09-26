// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// bugreport — ПОЛНЫЙ ОТЛАДЧИК ОШИБОК.
// Собирает единый диагностический отчёт: версия ОС, аптайм, память,
// состояние dev-режима/Verified Boot/шифрования, карта разделов,
// список пользователей, последние строки журнала ядра (dmesg) и
// crash-лог (tombstone). Отчёт выводится на экран, уходит в serial и
// сохраняется на /userdata-том (BUGREPORT.TXT).
// no_std-совместимо: alloc (String, Vec), вывод — crate::println!.

use alloc::format;
use alloc::string::String;

/// Имя файла отчёта на /userdata-томе (ext2 P2).
pub const BUGREPORT_FILE: &str = "BUGREPORT.TXT";

/// Собирает полный диагностический отчёт в одну строку.
pub fn collect_report() -> String {
    let mut out = String::new();
    out.push_str("========================================================\n");
    out.push_str("  DeiX OS — FULL DEBUG REPORT (bugreport)\n");
    out.push_str("========================================================\n");

    // --- SYSTEM ---
    out.push_str("\n[SYSTEM]\n");
    out.push_str(&format!("  OS: DeiX v0.2.1-beta (x86_64, no_std, Safe Rust)\n"));
    out.push_str(&format!("  uptime: {} ms\n", crate::timer::uptime_ms()));
    let dev = crate::devmode::sudo_allowed();
    out.push_str(&format!("  dev-mode: {}\n", if dev { "ON (bootloader unlocked, ORANGE)" } else { "OFF (bootloader locked, GREEN)" }));
    let enc = crate::crypto_storage::is_encryption_enabled();
    out.push_str(&format!("  disk encryption: {}\n", if enc { "XTS-AES-256 enabled (DEIXCRYP marker)" } else { "disabled (factory)" }));

    // --- USERS ---
    out.push_str("\n[ACCOUNTS]\n");
    match crate::auth::list_usernames() {
        Ok(names) => {
            if names.is_empty() {
                out.push_str("  users: NONE (first setup)\n");
            } else {
                out.push_str(&format!("  users: {}\n", names.join(", ")));
            }
        }
        Err(_) => out.push_str("  users: N/A\n"),
    }

    // --- PARTITIONS ---
    out.push_str("\n[PARTITIONS]\n");
    for layout in crate::partition_map::PARTITION_LAYOUT.iter() {
        out.push_str(&format!(
            "  {:<12} LBA {:>6}..{:>6} ({:>5} sect)  fs={:<5} flash={}\n",
            layout.name,
            layout.start_lba,
            layout.start_lba + layout.sectors - 1,
            layout.sectors,
            layout.fs,
            if layout.flashable { "yes" } else { "NO (protected)" },
        ));
    }
    out.push_str(&format!(
        "  {:<12} LBA {:>6}..{:>6} (stage2, {} sect)\n",
        "/bootloader",
        crate::partition_map::BOOTLOADER_LBA,
        crate::partition_map::BOOTLOADER_LBA + crate::partition_map::BOOTLOADER_SECTORS - 1,
        crate::partition_map::BOOTLOADER_SECTORS,
    ));

    // --- LOGS (последние строки журнала ядра) ---
    out.push_str("\n[DMESG] (last 60 lines)\n");
    let lines = crate::syslog::last_lines(60);
    for line in lines.iter() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }

    // --- CRASH LOG ---
    out.push_str("\n[CRASH]\n");
    let cur = crate::crashlog::current_crash();
    match cur {
        Some(text) => {
            out.push_str("  current session crash:\n");
            for l in text.lines() {
                out.push_str("    ");
                out.push_str(l);
                out.push('\n');
            }
        }
        None => out.push_str("  current session: no panic\n"),
    }
    match crate::crashlog::load_disk_crash() {
        Some(text) => {
            out.push_str("  disk crashlog (CRASHLOG.TXT):\n");
            for l in text.lines().take(20) {
                out.push_str("    ");
                out.push_str(l);
                out.push('\n');
            }
        }
        None => out.push_str("  disk crashlog: none\n"),
    }

    out.push_str("\n========================================================\n");
    out.push_str("  End of report. Attach BUGREPORT.TXT when reporting a bug.\n");
    out
}

/// Сохраняет отчёт на /system-том (BUGREPORT.TXT) — если ext2 доступен.
pub fn save_report_to_disk(report: &str) -> Result<(), ()> {
    crate::ext2::write_file(BUGREPORT_FILE, report.as_bytes()).map_err(|_| ())
}

/// Полный цикл: собрать, напечатать (экран + serial), сохранить на диск.
pub fn cmd_bugreport() {
    let report = collect_report();
    crate::println!("{}", report);
    match save_report_to_disk(&report) {
        Ok(()) => crate::println!("  [debugger] Отчёт сохранён: {} на /system-томе.", BUGREPORT_FILE),
        Err(_) => crate::println!("  [debugger] Не удалось сохранить {} (том недоступен).", BUGREPORT_FILE),
    }
}

/// Показывает кольцевой журнал ядра (dmesg).
pub fn cmd_dmesg() {
    let lines = crate::syslog::snapshot();
    crate::println!("[dmesg] строк в журнале: {}", lines.len());
    for line in lines.iter() {
        crate::println!("{}", line);
    }
}

/// Показывает crash-лог (текущая сессия + диск).
pub fn cmd_crashlog() {
    match crate::crashlog::current_crash() {
        Some(text) => crate::println!("[crashlog] (память):\n{}", text),
        None => crate::println!("[crashlog] паники в текущей сессии не было."),
    }
    match crate::crashlog::load_disk_crash() {
        Some(text) => {
            crate::println!("[crashlog] (диск CRASHLOG.TXT):");
            crate::println!("{}", text);
            crate::println!("[crashlog] для удаления: 'crashlog clear'");
        }
        None => crate::println!("[crashlog] на диске crash-лога нет."),
    }
}

/// Удаляет crash-лог с диска.
pub fn cmd_crashlog_clear() {
    crate::crashlog::clear_disk_crash();
    crate::crashlog::clear_current_crash();
    crate::println!("[crashlog] очищен.");
}
