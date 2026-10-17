//! Сводный диагностический отчёт (`bugreport`) и алиасы `dmesg`/`crashlog`.
//!
//! Источник данных — единая система диагностики `crate::diag`: кольцевой
//! буфер событий, реестр кодов DX-* и отчёты об отказе. Собственного
//! хранилища у этого модуля нет.

use alloc::format;
use alloc::string::String;

/// Путь файла отчёта на пользовательском разделе.
pub const BUGREPORT_FILE: &str = "/userdata/BUGREPORT.TXT";

/// Собирает полный диагностический отчёт в одну строку.
pub fn collect_report() -> String {
    let mut out = String::new();
    out.push_str("========================================================\n");
    out.push_str("  DeiX OS — FULL DEBUG REPORT (bugreport)\n");
    out.push_str("========================================================\n");

    // --- SYSTEM ---
    out.push_str("\n[SYSTEM]\n");
    out.push_str(&format!("  OS: DeiX v{} ({})\n", crate::KERNEL_VERSION, crate::BUILD_ID));
    out.push_str(&format!("  cpu: {}\n", crate::cpuid::describe()));
    out.push_str(&format!("  uptime: {} ms\n", crate::timer::uptime_ms()));
    out.push_str(&format!("  rtc: {}\n", crate::rtc::now().as_string()));
    let dev = crate::devmode::sudo_allowed();
    out.push_str(&format!("  dev-mode: {}\n", if dev { "ON (bootloader unlocked, ORANGE)" } else { "OFF (bootloader locked, GREEN)" }));
    let enc = crate::crypto_storage::is_encryption_enabled();
    out.push_str(&format!("  disk encryption: {}\n", if enc { "XTS-AES-256 enabled (DEIXCRYP marker)" } else { "disabled (factory)" }));

    // --- DIAGNOSTICS ---
    let s = crate::diag::summary();
    out.push_str("\n[DIAGNOSTICS]\n");
    out.push_str(&format!("  boot session: #{}\n", s.boot_session));
    out.push_str(&format!(
        "  events: {} total, {} buffered, {} dropped\n",
        s.total_events, s.buffered, s.dropped
    ));
    for sev in crate::diag::Severity::ALL {
        let n = s.by_severity[sev.level() as usize];
        if n > 0 {
            out.push_str(&format!("  {:<8} {}\n", sev.tag(), n));
        }
    }

    // --- MOUNTS ---
    out.push_str("\n[MOUNTS]\n");
    for line in crate::vfs::describe_mounts() {
        out.push_str(&format!("  {}\n", line));
    }

    // --- ACCOUNTS ---
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

    // --- EVENT LOG ---
    out.push_str("\n[EVENT LOG] (last 60 records)\n");
    for record in crate::diag::ring::snapshot(60) {
        out.push_str("  ");
        out.push_str(&crate::diag::format_human(&record));
        out.push('\n');
    }

    // --- CRASH REPORT ---
    out.push_str("\n[CRASH REPORT]\n");
    match crate::diag::panic::load_raw_report() {
        Some(text) => {
            out.push_str("  сохранённый отчёт об отказе (первые 20 строк):\n");
            for l in text.lines().take(20) {
                out.push_str("    ");
                out.push_str(l);
                out.push('\n');
            }
            out.push_str("  полный текст: panic last\n");
        }
        None => out.push_str("  отчётов об отказе нет\n"),
    }

    out.push_str("\n========================================================\n");
    out.push_str("  End of report. Attach BUGREPORT.TXT when reporting a bug.\n");
    out
}

/// Сохраняет отчёт на пользовательский раздел.
pub fn save_report_to_disk(report: &str) -> Result<(), ()> {
    crate::vfs::write_file(BUGREPORT_FILE, report.as_bytes()).map_err(|_| ())
}

/// Полный цикл: собрать, напечатать (экран + serial), сохранить на диск.
pub fn cmd_bugreport() {
    let report = collect_report();
    crate::println!("{}", report);
    crate::serial_println!("{}", report);
    match save_report_to_disk(&report) {
        Ok(()) => crate::println!("  [debugger] Отчёт сохранён: {}.", BUGREPORT_FILE),
        Err(_) => crate::println!("  [debugger] Не удалось сохранить {} (том недоступен).", BUGREPORT_FILE),
    }
}

/// `dmesg` — журнал ядра (алиас `log` для привычности).
pub fn cmd_dmesg() {
    let records = crate::diag::ring::all();
    crate::println!("[dmesg] записей в буфере: {}", records.len());
    for record in &records {
        crate::println!("{}", crate::diag::format_human(record));
    }
}

/// `crashlog` — алиас `panic last`.
pub fn cmd_crashlog() {
    crate::diag::cli::cmd_panic("last");
}

/// `crashlog clear` — алиас `panic clear`.
pub fn cmd_crashlog_clear() {
    crate::diag::cli::cmd_panic("clear");
}
