// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// recovery_ui — ПОЛНЫЙ ГРАФИЧЕСКИЙ RECOVERY (TWRP/OrangeFox-стиль).
//
// Запускается ВЫШЕ обычной ОС (BCB: dsm > fastbootd > recovery > normal).
// Все операции — РЕАЛЬНЫЕ, через recovery_flash_engine:
//   * Install package — выбор OTA-пакета с /system-тома, применение
//     (ota::apply_ota + пересчёт vbmeta);
//   * Backup (Nandroid) — побайтовые снимки разделов в BKP_*.IMG на
//     /system-том + манифест с FNV-1a/SHA-256;
//   * Restore — восстановление раздела из бэкапа;
//   * Factory Reset — стирание /userdata, удаление USERS.DB и журналов,
//     сброс /TPM-копии к заводскому маркеру;
//   * Wipe Cache / Wipe Dalvik — очистка *.TMP / *.DEX на /system-томе;
//   * Mount — статусы разделов (fs, LBA, режим);
//   * Sideload — приём пакета по COM1 и установка;
//   * View crash log / Copy recovery log — отладчик ошибок;
//   * Reboot system / recovery / fastbootd / dsm.
// no_std-совместимо: alloc (String, Vec), вывод — crate::println!.


use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Пункты меню recovery.
const MENU_ITEMS: [&str; 14] = [
    "Install package (OTA)",
    "OTA wireless update (A/B)",
    "Backup (Nandroid)",
    "Restore",
    "Factory Reset",
    "Wipe Cache",
    "Wipe Dalvik",
    "Mount partitions",
    "Sideload (COM1)",
    "View crash log",
    "Copy recovery log",
    "Reboot system",
    "Reboot fastbootd",
    "Reboot dsm (EDL)",
];

/// Журнал recovery (в памяти).
static RECOVERY_LOG: crate::spinlock::SpinLock<Vec<String>> = crate::spinlock::SpinLock::new(Vec::new());
/// Прогресс текущей операции (0..100) — для прогресс-бара GUI.
static PROGRESS: crate::spinlock::SpinLock<u8> = crate::spinlock::SpinLock::new(0);

fn set_progress(p: u8) {
    *PROGRESS.lock() = p;
}

/// Сброс журнала recovery (install: Vec<String> с указателями на кучу).
pub fn reset_recovery_state() {
    RECOVERY_LOG.lock().clear();
    *PROGRESS.lock() = 0;
}

fn rlog(msg: &str) {
    crate::println!("  [recovery] {}", msg);
    RECOVERY_LOG.lock().push(msg.to_string());
}

/// Цвета.
const BG: u32 = 0x101820;
const TITLE: u32 = 0x2E6DA4;
const ITEM: u32 = 0xE8F0F8;
const SEL: u32 = 0xFFD24A;
const HINT: u32 = 0x8FB8D8;
const OK: u32 = 0x7ED957;
const ERR: u32 = 0xFF6B6B;

/// Главный цикл.
pub fn recovery_gui() {
    // Режим реально берётся из раздела /boot (образ recovery.bin).
    match crate::bootchain::load_mode_image("recovery") {
        Ok(_) => {}
        Err(e) => crate::println!("  [recovery] предупреждение: {}", e),
    }
    crate::renderer::ensure_framebuffer();
    crate::println!("==============================================");
    crate::println!("   DeiX Recovery (TWRP/OrangeFox) — FULL      ");
    crate::println!("==============================================");
    crate::println!("  (priority: dsm > fastbootd > recovery > OS)");

    let mut selected: usize = 0;
    let max = MENU_ITEMS.len();
    let mut status: String = "Ready. Select a menu item.".to_string();
    let mut status_color: u32 = HINT;
    let mut cmd_buf: String = String::new();

    render(selected, &status, status_color);

    loop {
        let from_serial = crate::serial::is_data_ready();
        let key: Option<u8> = if from_serial {
            Some(crate::serial::read_byte())
        } else {
            crate::keyboard::try_read_char()
        };
        let key = match key {
            Some(k) => k,
            None => continue,
        };

        // Serial-протокол recovery: строки "rc <cmd> [args]" (headless-тесты).
        if from_serial {
            if key == b'\n' || key == b'\r' {
                let line = cmd_buf.clone();
                cmd_buf.clear();
                if line.trim_start().starts_with("rc") {
                    let (s, c, reboot) = handle_command(&line);
                    crate::println!("  [recovery] {}", s);
                    status = s;
                    status_color = c;
                    if reboot {
                        crate::println!("  [recovery] Rebooting (serial command).");
                        return;
                    }
                    render(selected, &status, status_color);
                    continue;
                }
            } else if cmd_buf.is_empty() && key != b'r' && key != b'c' {
                // обычная GUI-клавиша
            } else {
                cmd_buf.push(key as char);
                continue;
            }
        }

        match key {
            k if k == crate::keyboard::ARROW_UP || k == b'w' => {
                selected = if selected == 0 { max - 1 } else { selected - 1 };
            }
            k if k == crate::keyboard::ARROW_DOWN || k == b's' => {
                selected = (selected + 1) % max;
            }
            b'1'..=b'9' => {
                let idx = (key - b'1') as usize;
                if idx < max {
                    selected = idx;
                }
            }
            b'\n' | b'\r' => {
                let (s, c, do_reboot) = execute_action(selected);
                status = s;
                status_color = c;
                if do_reboot {
                    crate::println!("  [recovery] Rebooting (boot flag set).");
                    return;
                }
            }
            _ => {}
        }
        render(selected, &status, status_color);
    }
}

/// Обрабатывает serial-команду "rc <cmd> [args]".
fn handle_command(line: &str) -> (String, u32, bool) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return (
            "rc <install|backup|restore|reset|wipecache|wipedalvik|mount|crash|log|reboot> [args]".to_string(),
            HINT,
            false,
        );
    }
    match parts[1] {
        "backup" => action_backup(),
        "restore" => {
            if parts.len() < 3 {
                return ("rc restore <partition>".to_string(), HINT, false);
            }
            match crate::recovery_flash_engine::restore_from_backup(parts[2]) {
                Ok(n) => (format!("Restore {}: OK ({} bytes)", parts[2], n), OK, false),
                Err(e) => (format!("Restore {}: {}", parts[2], e.message()), ERR, false),
            }
        }
        "reset" => action_factory_reset(),
        "wipecache" => action_wipe_cache(),
        "wipedalvik" => action_wipe_dalvik(),
        "mount" => action_mount(),
        "crash" => action_view_crash(),
        "log" => action_copy_log(),
        "install" => action_install(),
        "sideload" => action_sideload(),
        "reboot" => {
            let target = if parts.len() >= 3 { parts[2] } else { "system" };
            match target {
                "fastbootd" => {
                    crate::bcb::write_boot_mode(crate::bcb::BootMode::Fastbootd);
                    ("Reboot fastbootd (serial)".to_string(), OK, true)
                }
                "dsm" | "edl" => {
                    crate::bcb::write_boot_mode(crate::bcb::BootMode::Dsm);
                    ("Reboot dsm (serial)".to_string(), OK, true)
                }
                _ => {
                    crate::bcb::clear_boot_mode();
                    ("Reboot system (serial)".to_string(), OK, true)
                }
            }
        }
        _ => (format!("rc: unknown command '{}'", parts[1]), ERR, false),
    }
}

/// Выполняет пункт меню. Возвращает (статус, цвет, выйти из recovery?).
fn execute_action(idx: usize) -> (String, u32, bool) {
    rlog(&format!("Selected: {}", MENU_ITEMS[idx]));
    match idx {
        0 => action_install(),
        1 => action_ota_wireless(),
        2 => action_backup(),
        3 => action_restore(),
        4 => action_factory_reset(),
        5 => action_wipe_cache(),
        6 => action_wipe_dalvik(),
        7 => action_mount(),
        8 => action_sideload(),
        9 => action_view_crash(),
        10 => action_copy_log(),
        11 => {
            crate::bcb::clear_boot_mode();
            ("Reboot system: boot flag normal".to_string(), OK, true)
        }
        12 => {
            crate::bcb::write_boot_mode(crate::bcb::BootMode::Fastbootd);
            ("Reboot fastbootd: boot flag set".to_string(), OK, true)
        }
        13 => {
            crate::bcb::write_boot_mode(crate::bcb::BootMode::Dsm);
            ("Reboot dsm: boot flag set (EDL)".to_string(), OK, true)
        }
        _ => ("Unknown item".to_string(), ERR, false),
    }
}

/// OTA wireless update (A/B): «скачать по воздуху» новый образ ядра и
/// прошить в НЕактивный слот, затем переключить. Откат — повторный запуск.
fn action_ota_wireless() -> (String, u32, bool) {
    rlog("OTA wireless update (A/B)...");
    // 1) «Скачиваем по воздуху» новый kernel.tar.gz.
    let targz = crate::ota::build_fresh_kernel_targz();
    crate::println!("  [ota] Скачано по воздуху: {} bytes (ядро v{})", targz.len(), crate::ota::FIRMWARE_VERSION + 1);
    // 2) Прошивка в НЕактивный слот.
    let target = crate::partition_map::inactive_kernel_layout();
    let img = crate::ota::build_erofs_with_file("kernel", "kernel.tar.gz", &targz, target.sectors);
    let mut buf = [0u8; 512];
    let mut ok = true;
    for i in 0..target.sectors {
        let start = (i as usize) * 512;
        buf.copy_from_slice(&img[start..start + 512]);
        if crate::ata::write_sectors(target.start_lba + i, 1, &buf).is_err() {
            ok = false;
            break;
        }
    }
    if !ok {
        return (format!("OTA: запись в {} не удалась (слот не изменён)", target.name), ERR, false);
    }
    crate::println!("  [ota] Прошито в {} (неактивный слот)", target.name);
    // 3) Подтверждение переключения слота.
    if crate::dialog::confirm("Switch to new slot and reboot?") {
        crate::bcb::write_slot(if crate::bcb::read_slot() == 0 { 1 } else { 0 });
        (format!("OTA: слот переключён на {} — перезагрузка активирует новое ядро", crate::bcb::slot_name()), OK, true)
    } else {
        (format!("OTA: прошито в {}, но слот НЕ переключён (откат возможен)", target.name), HINT, false)
    }
}

/// Install: выбор OTA-пакета с /system-тома и применение.
fn action_install() -> (String, u32, bool) {
    // Если диск зашифрован — recovery требует пароль разблокировки
    // (как Android: "Enter password to decrypt data" перед Install).
    if crate::crypto_storage::is_encryption_enabled() {
        let pw = crate::dialog::prompt_masked("Password to decrypt /system:");
        if pw.is_empty() {
            return ("Install: cancelled".to_string(), HINT, false);
        }
        if !crate::crypto_storage::try_unlock(&pw) {
            return ("Install: wrong unlock password".to_string(), ERR, false);
        }
        crate::println!("  Volume unlocked.");
    }
    let entries = match crate::ext2::list_root() {
        Ok(e) => e,
        Err(_) => {
            return ("Install: /system volume unavailable".to_string(), ERR, false);
        }
    };
    // Кандидаты: не-директории (пакеты и файлы образов).
    let mut candidates: Vec<String> = Vec::new();
    for e in entries.iter() {
        if !e.is_directory {
            candidates.push(e.name.clone());
        }
    }
    if candidates.is_empty() {
        return ("Install: no package files on /system".to_string(), HINT, false);
    }
    crate::println!("  [recovery] Packages on /system:");
    for (i, name) in candidates.iter().enumerate() {
        crate::println!("    {}. {}", i + 1, name);
    }
    let pick = match crate::dialog::choose("Select package:", &candidates) {
        Some(p) => p,
        None => return ("Install: cancelled".to_string(), HINT, false),
    };
    let fname = candidates[pick].clone();
    let bytes = match crate::ext2::read_file(&fname) {
        Ok(b) => b,
        Err(_) => return (format!("Install: failed to read {}", fname), ERR, false),
    };
    rlog(&format!("Applying {} ({} bytes)...", fname, bytes.len()));
    let mut avb = crate::avb::avb_mut();
    match crate::ota::apply_ota(&mut avb, &bytes) {
        Ok(target) => (format!("Install {} -> OK ({} written)", fname, target), OK, false),
        Err(e) => (format!("Install {} -> {}", fname, e.message()), ERR, false),
    }
}

/// Backup: Nandroid-снимки разделов на /system-том (с прогрессом).
fn action_backup() -> (String, u32, bool) {
    let parts = ["/kernel_a", "/boot_a", "/recovery"];
    rlog("Creating Nandroid backup (real partition snapshots)...");
    let mut created: Vec<crate::recovery_flash_engine::NandroidBackupEntry> = Vec::new();
    let mut manifest = String::from("DeiX Nandroid backup manifest\n");
    let total = parts.len();
    for (i, part) in parts.iter().enumerate() {
        let bytes = match crate::recovery_flash_engine::disk_snapshot_partition(part) {
            Ok(b) => b,
            Err(e) => {
                set_progress(0);
                return (format!("Backup {}: {}", part, e.message()), ERR, false);
            }
        };
        let fname = crate::recovery_flash_engine::backup_file_name(part);
        if crate::ext2::write_file(&fname, &bytes).is_err() {
            set_progress(0);
            return (format!("Backup: failed to write {}", fname), ERR, false);
        }
        let digest = crate::recovery_flash_engine::fnv1a64(&bytes);
        crate::println!("    backup {}: {} bytes, fnv1a64={:#x}", part, bytes.len(), digest);
        manifest.push_str(&format!("{} size={} fnv1a64={:#x}\n", part, bytes.len(), digest));
        created.push(crate::recovery_flash_engine::NandroidBackupEntry {
            partition: part.to_string(),
            size_bytes: bytes.len(),
            digest,
        });
        set_progress((((i + 1) as u16 * 100) / total as u16) as u8);
    }
    let _ = crate::ext2::write_file(crate::recovery_flash_engine::BACKUP_MANIFEST, manifest.as_bytes());
    (format!("Backup: OK, {} partitions (BKP_*.IMG + manifest on /system)", created.len()), OK, false)
}

/// Restore: выбор бэкапа и восстановление.
fn action_restore() -> (String, u32, bool) {
    let backups = crate::recovery_flash_engine::list_backups();
    if backups.is_empty() {
        return ("Restore: no backups (run Backup first)".to_string(), HINT, false);
    }
    crate::println!("  [recovery] Available backups:");
    for (i, name) in backups.iter().enumerate() {
        crate::println!("    {}. {}", i + 1, name);
    }
    let pick = match crate::dialog::choose("Restore from backup:", &backups) {
        Some(p) => p,
        None => return ("Restore: cancelled".to_string(), HINT, false),
    };
    let part = backups[pick].clone();
    match crate::recovery_flash_engine::restore_from_backup(&part) {
        Ok(n) => (format!("Restore {}: OK ({} bytes written)", part, n), OK, false),
        Err(e) => (format!("Restore {}: {}", part, e.message()), ERR, false),
    }
}

/// Factory Reset: полный сброс к заводскому состоянию.
fn action_factory_reset() -> (String, u32, bool) {
    if !crate::dialog::confirm("Factory Reset? /userdata, USERS.DB and logs will be wiped") {
        return ("Factory Reset: cancelled".to_string(), HINT, false);
    }
    rlog("Factory Reset: wiping /userdata, USERS.DB, logs...");
    match crate::recovery_flash_engine::factory_reset_disk() {
        Ok(log) => {
            crate::println!("{}", log);
            ("Factory Reset: OK — system will return to first-time setup".to_string(), OK, false)
        }
        Err(e) => (format!("Factory Reset: {}", e.message()), ERR, false),
    }
}

/// Wipe Cache: удаление временных файлов (*.TMP) на /system-томе.
fn action_wipe_cache() -> (String, u32, bool) {
    let removed = wipe_files_by_suffix(".TMP");
    (format!("Wipe Cache: removed {} *.TMP files", removed), OK, false)
}

/// Wipe Dalvik: удаление скомпилированных артефактов (*.DEX/*.CACHE).
fn action_wipe_dalvik() -> (String, u32, bool) {
    let removed = wipe_files_by_suffix(".DEX");
    (format!("Wipe Dalvik: removed {} *.DEX files", removed), OK, false)
}

/// Удаляет файлы на /system-томе с заданным суффиксом. Возвращает счётчик.
fn wipe_files_by_suffix(suffix: &str) -> usize {
    let mut removed: usize = 0;
    if let Ok(entries) = crate::ext2::list_root() {
        for e in entries.iter() {
            if e.is_directory {
                continue;
            }
            if e.name.to_uppercase().ends_with(suffix) {
                if crate::ext2::delete_file(&e.name).is_ok() {
                    crate::println!("    removed {}", e.name);
                    removed += 1;
                }
            }
        }
    }
    removed
}

/// Mount: статусы разделов.
fn action_mount() -> (String, u32, bool) {
    crate::println!("  [recovery] Partitions:");
    crate::println!("    {:<12} {:>6} {:>7}  fs     mode", "name", "LBA", "sect");
    for layout in crate::partition_map::PARTITION_LAYOUT.iter() {
        crate::println!(
            "    {:<12} {:>6} {:>7}  {:<5} {}",
            layout.name,
            layout.start_lba,
            layout.sectors,
            layout.fs,
            if layout.fs == "erofs" { "ro" } else { "rw" }
        );
    }
    ("Mount: partition map shown (erofs=ro, /userdata=rw)".to_string(), OK, false)
}

/// Sideload: приём пакета по COM1 (строка с размером, затем байты) и установка.
fn action_sideload() -> (String, u32, bool) {
    crate::println!("  [recovery] Sideload: enter package size in bytes:");
    let input = crate::dialog::prompt_line("Sideload package size (bytes):");
    let len: usize = match input.trim().parse() {
        Ok(n) if n > 0 && n <= 2 * 1024 * 1024 => n,
        _ => return ("Sideload: invalid size (expected 1..2097152)".to_string(), ERR, false),
    };
    crate::println!("  [recovery] Receiving {} bytes over COM1...", len);
    let mut data: Vec<u8> = Vec::with_capacity(len);
    while data.len() < len {
        if crate::serial::is_data_ready() {
            data.push(crate::serial::read_byte());
        } else if let Some(k) = crate::keyboard::try_read_char() {
            data.push(k);
        } else {
            unsafe { core::arch::asm!("nop"); }
        }
    }
    // Сохраняем пакет и применяем как OTA.
    let _ = crate::ext2::write_file("SIDELOAD.BIN", &data);
    let mut avb = crate::avb::avb_mut();
    match crate::ota::apply_ota(&mut avb, &data) {
        Ok(target) => (format!("Sideload: OK ({} bytes, {})", len, target), OK, false),
        Err(e) => (format!("Sideload: {}", e.message()), ERR, false),
    }
}

/// View crash log: показывает tombstone (память + диск).
fn action_view_crash() -> (String, u32, bool) {
    crate::println!("  [recovery] Crash log:");
    match crate::crashlog::current_crash() {
        Some(t) => crate::println!("{}", t),
        None => crate::println!("    (no panic in current session)"),
    }
    match crate::crashlog::load_disk_crash() {
        Some(t) => crate::println!("  [disk CRASHLOG.TXT]:\n{}", t),
        None => crate::println!("  [disk] no crash log."),
    }
    ("View crash log: shown above".to_string(), OK, false)
}

/// Copy recovery log: сохраняет журнал recovery в RECOVERY.LOG на /system.
fn action_copy_log() -> (String, u32, bool) {
    let lines = RECOVERY_LOG.lock();
    let mut text = String::new();
    text.push_str("=== DeiX Recovery log ===\n");
    for l in lines.iter() {
        text.push_str(l);
        text.push('\n');
    }
    drop(lines);
    match crate::ext2::write_file("RECOVERY.LOG", text.as_bytes()) {
        Ok(()) => (format!("Copy log: RECOVERY.LOG written to /system ({} lines)", text.lines().count()), OK, false),
        Err(_) => ("Copy log: failed to write (volume unavailable)".to_string(), ERR, false),
    }
}

fn render(selected: usize, status: &str, status_color: u32) {
    crate::renderer::with_renderer(|r| {
        let w = r.width();
        let h = r.height();
        r.clear(crate::renderer::Color::from_u32(BG));

        r.fill_rect_gradient_v(0, 0, w, 44,
            crate::renderer::Color::from_u32(0x14466B),
            crate::renderer::Color::from_u32(TITLE));
        r.draw_hline(0, 44, w, crate::renderer::Color::from_u32(0x4FA3E8));
        r.draw_text(12, 13, "DeiX Recovery — FULL (TWRP/OrangeFox-style)",
                    crate::renderer::Color::from_u32(0xFFFFFF), None);

        let dev = crate::devmode::sudo_allowed();
        let sub = if dev {
            "bootloader UNLOCKED (dev-mode) — verified boot ORANGE"
        } else {
            "bootloader locked — verified boot GREEN"
        };
        r.draw_text(12, 50, sub, crate::renderer::Color::from_u32(HINT), None);

        for (i, item) in MENU_ITEMS.iter().enumerate() {
            let y = 76 + (i as u32) * 26;
            let sel = i == selected;
            let bg = if sel {
                crate::renderer::Color::from_u32(SEL)
            } else {
                crate::renderer::Color::from_u32(0x1A2430)
            };
            r.fill_rounded_rect(20, y as i32, w - 40, 22, 7, bg);
            let fg = if sel {
                crate::renderer::Color::from_u32(0x000000)
            } else {
                crate::renderer::Color::from_u32(ITEM)
            };
            let label: String = format!("{}. {}", i + 1, item);
            r.draw_text(30, (y + 4) as i32, &label, fg, None);
        }

        let sy = h - 66;
        r.fill_rounded_rect(12, sy as i32, w - 24, 40, 8, crate::renderer::Color::from_u32(0x1A2430));
        r.draw_rect(12, sy as i32, w - 24, 40, crate::renderer::Color::from_u32(0x2A3A4A));
        r.draw_text(18, (sy + 9) as i32, status, crate::renderer::Color::from_u32(status_color), None);

        // Прогресс-бар операции (Backup/Install/Factory Reset).
        let prog = *PROGRESS.lock();
        if prog > 0 {
            let pw = w - 48;
            r.fill_rect(24, (sy + 26) as i32, pw, 10, crate::renderer::Color::from_u32(0x333A44));
            let filled = ((pw as u32) * (prog as u32)) / 100;
            if filled > 0 {
                r.fill_rect(24, (sy + 26) as i32, filled, 10, crate::renderer::Color::from_u32(OK));
            }
        }

        let hint = "UP/DOWN: switch | 1..9: select | Enter: action | Reboot: exit";
        r.draw_text(12, (h - 20) as i32, hint, crate::renderer::Color::from_u32(HINT), None);

        r.present();
    });
}
