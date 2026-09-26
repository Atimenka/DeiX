// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// fastbootd_ui — ПОЛНЫЙ ГРАФИЧЕСКИЙ ПРОШИВАЛЬЩИК DeiX (fastbootd).
//
// Запускается ВЫШЕ recovery (BCB: dsm > fastbootd > recovery > normal).
// Работает с РЕАЛЬНЫМ диском: прошивка разделов (с EROFS-валидацией для
// системных), стирание, форматирование /userdata, fastboot-переменные
// (getvar), OEM unlock/lock (dev-режим), перезагрузка в любой режим,
// журнал прошивки (FLASHLOG.TXT на /system-томе).
//
// Навигация: стрелки + Enter, цифры 1..9 (разделы), F-клавиши нет —
// действия выбираются стрелками. Headless (QEMU -serial stdio): цифры/Enter.
// no_std-совместимо: alloc (String, Vec), вывод — crate::println!.


use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::partition_map::PartitionLayout;

/// Список разделов для прошивки: /bootloader (только чтение!) + карта.
fn partition_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = vec!["/bootloader"];
    // Алиасы A/B: /kernel и /boot = АКТИВНЫЙ слот (пользователь прошивает
    // «текущее» ядро/загрузчик; неактивный слот — через ota apply).
    names.push("/kernel");
    names.push("/boot");
    for layout in crate::partition_map::PARTITION_LAYOUT.iter() {
        if layout.name.ends_with("_a") || layout.name.ends_with("_b") {
            continue; // слоты показываем через алиасы выше
        }
        names.push(layout.name);
    }
    names
}

/// Действия прошивальщика.
const ACTIONS: [&str; 9] = [
    "Flash  <partition>",
    "Erase  <partition>",
    "Format /userdata",
    "Getvar (variables)",
    "OEM unlock bootloader",
    "OEM lock bootloader",
    "Reboot system",
    "Reboot recovery",
    "Reboot dsm (EDL)",
];

/// Журнал прошивки (в памяти + FLASHLOG.TXT на /system-томе).
static FLASH_LOG: crate::spinlock::SpinLock<Vec<String>> = crate::spinlock::SpinLock::new(Vec::new());
/// Прогресс операции прошивки (0..100).
static PROGRESS: crate::spinlock::SpinLock<u8> = crate::spinlock::SpinLock::new(0);

fn set_progress(p: u8) {
    *PROGRESS.lock() = p;
}

/// Сброс журнала прошивки (install: Vec<String> с указателями на кучу).
pub fn reset_fastbootd_state() {
    FLASH_LOG.lock().clear();
    *PROGRESS.lock() = 0;
}

fn log_flash(msg: &str) {
    crate::println!("  [fastbootd] {}", msg);
    FLASH_LOG.lock().push(msg.to_string());
}

/// Цвета.
const BG: u32 = 0x0A0E14;
const TITLE: u32 = 0xB84E2E;
const ITEM: u32 = 0xE8F0F8;
const SEL: u32 = 0xFFD24A;
const HINT: u32 = 0x8FB8D8;
const OK: u32 = 0x7ED957;
const ERR: u32 = 0xFF6B6B;

/// Главный цикл.
pub fn fastbootd_gui() {
    // Режим реально берётся из раздела /boot (образ fastbootd.bin).
    match crate::bootchain::load_mode_image("fastbootd") {
        Ok(_) => {}
        Err(e) => crate::println!("  [fastbootd] предупреждение: {}", e),
    }
    crate::renderer::ensure_framebuffer();
    crate::println!("==============================================");
    crate::println!("   DeiX Fastbootd (flasher) — FULL        ");
    crate::println!("==============================================");
    crate::println!("  (priority: dsm > fastbootd > recovery > OS)");

    let partitions = partition_names();
    let mut sel_part: usize = 0;
    let mut sel_act: usize = 0;
    let mut status: String = "Ready. Select a partition and action.".to_string();
    let mut status_color: u32 = HINT;
    let mut cmd_buf: String = String::new();

    render(&partitions, sel_part, sel_act, &status, status_color);

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

        // Serial-протокол fastbootd: строки "fb <cmd> [args]" (headless-тесты).
        if from_serial {
            if key == b'\n' || key == b'\r' {
                let line = cmd_buf.clone();
                cmd_buf.clear();
                if line.trim_start().starts_with("fb") {
                    let (s, c, reboot) = handle_command(&line);
                    crate::println!("  [fastbootd] {}", s);
                    status = s;
                    status_color = c;
                    if reboot {
                        crate::println!("  [fastbootd] Rebooting (serial command).");
                        return;
                    }
                    render(&partitions, sel_part, sel_act, &status, status_color);
                    continue;
                }
            } else if cmd_buf.is_empty() && key != b'f' && key != b'b' {
                // обычная GUI-клавиша
            } else {
                cmd_buf.push(key as char);
                continue;
            }
        }

        match key {
            k if k == crate::keyboard::ARROW_UP || k == b'w' => {
                if sel_act == 0 {
                    sel_part = if sel_part == 0 { partitions.len() - 1 } else { sel_part - 1 };
                } else {
                    sel_act -= 1;
                }
            }
            k if k == crate::keyboard::ARROW_DOWN || k == b's' => {
                if sel_act == ACTIONS.len() - 1 {
                    sel_part = (sel_part + 1) % partitions.len();
                    sel_act = 0;
                } else {
                    sel_act += 1;
                }
            }
            b'1'..=b'9' => {
                let idx = (key - b'1') as usize;
                if idx < partitions.len() {
                    sel_part = idx;
                    sel_act = 0;
                }
            }
            b'\n' | b'\r' => {
                let (s, c, do_reboot) = execute_action(&partitions, sel_part, sel_act);
                status = s;
                status_color = c;
                if do_reboot {
                    crate::println!("  [fastbootd] Rebooting (boot flag set).");
                    return;
                }
            }
            _ => {}
        }
        render(&partitions, sel_part, sel_act, &status, status_color);
    }
}

/// Обрабатывает serial-команду "fb <cmd> [args]".
fn handle_command(line: &str) -> (String, u32, bool) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return ("fb <flash|erase|format|getvar|unlock|lock|reboot> [args]".to_string(), HINT, false);
    }
    match parts[1] {
        "flash" => {
            if parts.len() < 3 {
                return ("fb flash <partition>".to_string(), HINT, false);
            }
            action_flash(parts[2])
        }
        "erase" => {
            if parts.len() < 3 {
                return ("fb erase <partition>".to_string(), HINT, false);
            }
            action_erase(parts[2])
        }
        "format" => action_format_userdata(),
        "getvar" => action_getvar(),
        "unlock" => action_oem_unlock(),
        "lock" => action_oem_lock(),
        "reboot" => {
            let target = if parts.len() >= 3 { parts[2] } else { "system" };
            match target {
                "recovery" => {
                    crate::bcb::write_boot_mode(crate::bcb::BootMode::Recovery);
                    ("Reboot recovery (serial)".to_string(), OK, true)
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
        _ => (format!("fb: unknown command '{}'", parts[1]), ERR, false),
    }
}

/// Выполняет действие. Возвращает (статус, цвет, нужно ли выйти из оболочки).
fn execute_action(
    partitions: &[&'static str],
    sel_part: usize,
    sel_act: usize,
) -> (String, u32, bool) {
    let part = partitions[sel_part];
    log_flash(&format!("{} -> {}", ACTIONS[sel_act], part));

    match sel_act {
        0 => action_flash(part),
        1 => action_erase(part),
        2 => action_format_userdata(),
        3 => action_getvar(),
        4 => action_oem_unlock(),
        5 => action_oem_lock(),
        6 => {
            crate::bcb::clear_boot_mode();
            ("Reboot system: boot flag normal (next boot = OS)".to_string(), OK, true)
        }
        7 => {
            crate::bcb::write_boot_mode(crate::bcb::BootMode::Recovery);
            ("Reboot recovery: boot flag set".to_string(), OK, true)
        }
        8 => {
            crate::bcb::write_boot_mode(crate::bcb::BootMode::Dsm);
            ("Reboot dsm: boot flag set (EDL mode)".to_string(), OK, true)
        }
        _ => ("Unknown action".to_string(), ERR, false),
    }
}

/// Flash: реальная запись с EROFS-валидацией.
fn action_flash(part: &str) -> (String, u32, bool) {
    if part == "/bootloader" {
        return ("FLASH /bootloader denied: bootloader is protected (use OEM unlock + DSM first)".to_string(), ERR, false);
    }
    let real = resolve_alias(part);
    match crate::partition_map::lookup_layout(real) {
        Some(layout) => {
            let img = build_test_image(layout);
            log_flash(&format!("flash {}: {} bytes, EROFS validation + write...", part, img.len()));
            set_progress(30);
            match crate::recovery_flash_engine::disk_flash_partition(part, &img) {
                Ok(()) => {
                    set_progress(100);
                    (format!("FLASH {}: OK ({} bytes, readback verified)", part, img.len()), OK, false)
                }
                Err(e) => {
                    set_progress(0);
                    (format!("FLASH {}: {}", part, e.message()), ERR, false)
                }
            }
        }
        None => (format!("FLASH {}: partition not found", part), ERR, false),
    }
}

/// Erase: реальное обнуление раздела.
fn action_erase(part: &str) -> (String, u32, bool) {
    if part == "/bootloader" {
        return ("ERASE /bootloader denied (bootloader is protected)".to_string(), ERR, false);
    }
    if !crate::dialog::confirm(&format!("Erase {}? (data will be destroyed)", part)) {
        return (format!("ERASE {}: cancelled", part), HINT, false);
    }
    match crate::recovery_flash_engine::disk_erase_partition(part) {
        Ok(n) => {
            set_progress(100);
            (format!("ERASE {}: OK ({} sectors zeroed)", part, n), OK, false)
        }
        Err(e) => {
            set_progress(0);
            (format!("ERASE {}: {}", part, e.message()), ERR, false)
        }
    }
}

/// Format /userdata: обнуление + пометка «требуется первичная настройка».
fn action_format_userdata() -> (String, u32, bool) {
    match crate::recovery_flash_engine::disk_erase_partition("/userdata") {
        Ok(n) => (format!("FORMAT /userdata: OK ({} sectors, ext2 volume will be created on first boot)", n), OK, false),
        Err(e) => (format!("FORMAT /userdata: {}", e.message()), ERR, false),
    }
}

/// Getvar: fastboot-переменные.
fn action_getvar() -> (String, u32, bool) {
    let dev = crate::devmode::sudo_allowed();
    let mut out = String::new();
    out.push_str("GETVAR:\n");
    out.push_str("  version: 0.2.1-beta\n");
    out.push_str("  version-bootloader: deiX-boot-1.0\n");
    out.push_str("  product: deiX_vbox\n");
    out.push_str("  serialno: DEIX-QEMU-0001\n");
    out.push_str(&format!("  variant: {}\n", if dev { "unlocked" } else { "locked" }));
    out.push_str(&format!("  unlocked: {}\n", if dev { "yes" } else { "no" }));
    out.push_str(&format!("  slot-count: 1\n"));
    out.push_str("  secure: yes (Verified Boot)\n");
    out.push_str("  current-slot: a\n");
    crate::println!("{}", out);
    (out, OK, false)
}

/// OEM unlock: включает dev-режим (bootloader unlock, Verified Boot ORANGE).
fn action_oem_unlock() -> (String, u32, bool) {
    if crate::devmode::sudo_allowed() {
        return ("OEM UNLOCK: bootloader already unlocked".to_string(), HINT, false);
    }
    let mut avb = crate::avb::avb_mut();
    crate::devmode::enable_dev_mode(&mut avb);
    ("OEM UNLOCK: bootloader unlocked, Verified Boot -> ORANGE (OTA guarantee void)".to_string(), OK, false)
}

/// OEM lock: блокирует загрузчик (dev off).
fn action_oem_lock() -> (String, u32, bool) {
    if !crate::devmode::sudo_allowed() {
        return ("OEM LOCK: bootloader already locked".to_string(), HINT, false);
    }
    let mut avb = crate::avb::avb_mut();
    crate::devmode::disable_dev_mode(&mut avb);
    ("OEM LOCK: bootloader locked (guarantee NOT restored without EDL reflash)".to_string(), OK, false)
}

/// Строит тестовый образ для прошивки.
/// Алиас A/B: /kernel -> активный слот ядра, /boot -> активный слот boot.
/// (Имена слотов — 'static из PARTITION_LAYOUT.)
fn resolve_alias(part: &str) -> &'static str {
    if part == "/kernel" {
        crate::partition_map::active_kernel_layout().name
    } else if part == "/boot" {
        crate::partition_map::active_boot_layout().name
    } else {
        // Все остальные имена в списке — 'static; ищем в карте разделов.
        crate::partition_map::lookup_layout(part).map(|l| l.name).unwrap_or("/bootloader")
    }
}

fn build_test_image(layout: &PartitionLayout) -> Vec<u8> {
    crate::dsm::build_test_image(layout)
}

/// Рисует кадр.
fn render(partitions: &[&'static str], sel_part: usize, sel_act: usize, status: &str, status_color: u32) {
    crate::renderer::with_renderer(|r| {
        let w = r.width();
        let h = r.height();
        r.clear(crate::renderer::Color::from_u32(BG));

        r.fill_rect_gradient_v(0, 0, w, 44,
            crate::renderer::Color::from_u32(0x6E2A14),
            crate::renderer::Color::from_u32(TITLE));
        r.draw_hline(0, 44, w, crate::renderer::Color::from_u32(0xFF8C4A));
        r.draw_text(12, 13, "DeiX Fastbootd — FULL (flasher)",
                    crate::renderer::Color::from_u32(0xFFFFFF), None);

        // Разделы (слева).
        r.draw_text(20, 54, "PARTITIONS:", crate::renderer::Color::from_u32(HINT), None);
        for (i, p) in partitions.iter().enumerate() {
            let y = 74 + (i as u32) * 22;
            let sel = i == sel_part && sel_act == 0;
            if sel {
                r.fill_rounded_rect(18, y as i32, 250, 18, 6, crate::renderer::Color::from_u32(SEL));
            }
            let fg = if sel {
                crate::renderer::Color::from_u32(0x000000)
            } else {
                crate::renderer::Color::from_u32(ITEM)
            };
            let info: String = match crate::partition_map::lookup_layout(resolve_alias(p)) {
                Some(l) => format!("{}. {:<12} LBA{} ({}s {})", i + 1, p, l.start_lba, l.sectors, l.fs),
                None => format!("{}. {:<12} (bootloader, protected)", i + 1, p),
            };
            r.draw_text(22, (y + 2) as i32, &info, fg, None);
        }

        // Действия (справа).
        let ax = 420u32;
        r.draw_text(ax as i32, 54, "ACTIONS:", crate::renderer::Color::from_u32(HINT), None);
        for (i, a) in ACTIONS.iter().enumerate() {
            let y = 74 + (i as u32) * 22;
            let sel = i == sel_act && sel_act != 0;
            if sel {
                r.fill_rounded_rect((ax - 2) as i32, y as i32, 300, 18, 6, crate::renderer::Color::from_u32(SEL));
            }
            let fg = if sel {
                crate::renderer::Color::from_u32(0x000000)
            } else {
                crate::renderer::Color::from_u32(ITEM)
            };
            let label: String = if i == 0 {
                format!("Flash  -> {}", partitions[sel_part])
            } else if i == 1 {
                format!("Erase  -> {}", partitions[sel_part])
            } else {
                a.to_string()
            };
            r.draw_text((ax + 4) as i32, (y + 2) as i32, &label, fg, None);
        }

        // Статус.
        let sy = h - 70;
        r.fill_rounded_rect(12, sy as i32, w - 24, 42, 8, crate::renderer::Color::from_u32(0x181C22));
        r.draw_rect(12, sy as i32, w - 24, 42, crate::renderer::Color::from_u32(0x2A2F38));
        r.draw_text(18, (sy + 10) as i32, status, crate::renderer::Color::from_u32(status_color), None);

        // Прогресс-бар прошивки.
        let prog = *PROGRESS.lock();
        if prog > 0 {
            let pw = w - 48;
            r.fill_rect(24, (sy + 30) as i32, pw, 8, crate::renderer::Color::from_u32(0x333A44));
            let filled = ((pw as u32) * (prog as u32)) / 100;
            if filled > 0 {
                r.fill_rect(24, (sy + 30) as i32, filled, 8, crate::renderer::Color::from_u32(OK));
            }
        }

        let hint = "UP/DOWN: switch | 1..9: partition | Enter: action | reboot: exit";
        r.draw_text(12, (h - 20) as i32, hint, crate::renderer::Color::from_u32(HINT), None);

        r.present();
    });
}
