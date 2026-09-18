// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// dsm — DOWNLOAD SYSTEM MANAGER (аналог EDL Qualcomm / 9008 Emergency Download).
//
// DSM — самый низкоуровневый (emergency) режим прошивки. Запускается ВЫШЕ
// всех остальных режимов (dsm > fastbootd > recovery > normal, см. bcb.rs):
// работает даже если ОС, recovery и fastbootd не загружаются (сломанный
// загрузчик/разделы). Минимальные зависимости: ATA-диск + рендерер +
// клавиатура/serial — никаких файловых систем, никакого TPM.
//
// Возможности:
//   * INFO  — версия протокола, карта разделов (имя, LBA, размер, ФС);
//   * READ  <part> <off> <len> — чтение данных раздела (дамп hex);
//   * WRITE <part> <off> <len> — запись данных в раздел со смещением;
//   * FLASH <part> <len>       — прошивка ВСЕГО раздела (сырые данные);
//   * ERASE <part>             — обнуление раздела;
//   * VERIFY <part>            — SHA-256-контроль целостности раздела;
//   * REBOOT [normal|recovery|fastbootd|dsm] — перезагрузка с флажком.
//
// Serial-протокол (headless/QEMU): строка "dsm <cmd> [args]". Для FLASH/WRITE
// после строки ожидается <len> сырых байт. GUI: стрелки/`w`/`s` + Enter,
// цифры 1..9 — выбор раздела.
// no_std-совместимо: alloc (String, Vec), вывод — crate::println!.


use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::ata;
use crate::partition_map::{lookup_layout, PartitionLayout, BOOTLOADER_LBA, BOOTLOADER_SECTORS};

/// Версия протокола DSM.
pub const DSM_PROTOCOL_VERSION: &str = "DSM-1.0";
/// Максимум секторов за одно чтение/запись ATA (LBA28 sector count reg).
const MAX_BATCH: u32 = 256;

// ==================== НИЗКОУРОВНЕВЫЕ ОПЕРАЦИИ С ДИСКОМ ====================

/// Возвращает диапазон (start, sectors) для имени раздела, включая
/// виртуальный "/bootloader" (stage2: LBA 1..1150).
fn resolve(name: &str) -> Option<(u32, u32)> {
    if name == "/bootloader" {
        return Some((BOOTLOADER_LBA, BOOTLOADER_SECTORS));
    }
    // Алиасы A/B: "/kernel" -> АКТИВНЫЙ слот ядра, "/boot" -> АКТИВНЫЙ слот boot.
    // (Чтобы `dsm erase /kernel` реально стирал загружаемое ядро, а не
    // отвечал «неизвестный раздел» — система обязана не загрузиться.)
    let real: &str = match name {
        "/kernel" => crate::partition_map::active_kernel_layout().name,
        "/boot" => crate::partition_map::active_boot_layout().name,
        _ => name,
    };
    match lookup_layout(real) {
        Some(l) => Some((l.start_lba, l.sectors)),
        None => None,
    }
}

/// Читает `count` секторов раздела, начиная со смещения `off_sect`.
pub fn dsm_read_partition(name: &str, off_sect: u32, count: u32) -> Result<Vec<u8>, String> {
    let (start, total) = resolve(name).ok_or_else(|| format!("dsm: unknown partition '{}'", name))?;
    if off_sect + count > total {
        return Err(format!(
            "dsm: read out of bounds (off={}, count={}, total={})",
            off_sect, count, total
        ));
    }
    let mut out: Vec<u8> = Vec::new();
    let mut left = count;
    let mut cur = off_sect;
    while left > 0 {
        let batch = left.min(MAX_BATCH);
        let mut buf = vec![0u8; (batch * 512) as usize];
        ata::read_sectors(start + cur, batch as u8, &mut buf).map_err(|_| "dsm: disk read error".to_string())?;
        out.extend_from_slice(&buf);
        left -= batch;
        cur += batch;
    }
    Ok(out)
}

/// Пишет `data` в раздел, начиная со смещения `off_sect` (data кратен 512).
pub fn dsm_write_partition(name: &str, off_sect: u32, data: &[u8]) -> Result<(), String> {
    let (start, total) = resolve(name).ok_or_else(|| format!("dsm: unknown partition '{}'", name))?;
    if data.len() % 512 != 0 {
        return Err("dsm: data must be a multiple of 512 bytes".to_string());
    }
    let sectors = (data.len() / 512) as u32;
    if off_sect + sectors > total {
        return Err(format!(
            "dsm: write out of bounds (off={}, sectors={}, total={})",
            off_sect, sectors, total
        ));
    }
    let mut off_bytes = 0usize;
    let mut cur = off_sect;
    let mut left = sectors;
    while left > 0 {
        let batch = left.min(MAX_BATCH) as usize;
        ata::write_sectors(start + cur, batch as u8, &data[off_bytes..off_bytes + batch * 512])
            .map_err(|_| "dsm: disk write error".to_string())?;
        off_bytes += batch * 512;
        left -= batch as u32;
        cur += batch as u32;
    }
    Ok(())
}

/// Обнуляет весь раздел (ERASE).
pub fn dsm_erase_partition(name: &str) -> Result<u32, String> {
    let (start, total) = resolve(name).ok_or_else(|| format!("dsm: unknown partition '{}'", name))?;
    let zeros = vec![0u8; (MAX_BATCH * 512) as usize];
    let mut cur = 0u32;
    let mut left = total;
    while left > 0 {
        let batch = left.min(MAX_BATCH);
        ata::write_sectors(start + cur, batch as u8, &zeros[..(batch * 512) as usize])
            .map_err(|_| "dsm: erase error".to_string())?;
        left -= batch;
        cur += batch;
    }
    Ok(total)
}

/// SHA-256 всего раздела (VERIFY). Возвращает hex-строку.
pub fn dsm_verify_partition(name: &str) -> Result<String, String> {
    let (start, total) = resolve(name).ok_or_else(|| format!("dsm: unknown partition '{}'", name))?;
    let mut hash = crate::crypto::sha256::Sha256::new();
    let mut cur = 0u32;
    let mut left = total;
    while left > 0 {
        let batch = left.min(MAX_BATCH);
        let mut buf = vec![0u8; (batch * 512) as usize];
        ata::read_sectors(start + cur, batch as u8, &mut buf)
            .map_err(|_| "dsm: read error for VERIFY".to_string())?;
        hash.update(&buf);
        left -= batch;
        cur += batch;
    }
    let digest = hash.finalize();
    let mut hex = String::new();
    for b in digest.iter() {
        hex.push_str(&format!("{:02x}", b));
    }
    Ok(hex)
}

/// INFO: карта разделов текстом.
pub fn dsm_info_text() -> String {
    let mut out = String::new();
    out.push_str(&format!("DSM protocol: {}\n", DSM_PROTOCOL_VERSION));
    out.push_str(&format!(
        "  {:<12} LBA {:>6}..{:>6} ({:>5} sect)\n",
        "/bootloader",
        BOOTLOADER_LBA,
        BOOTLOADER_LBA + BOOTLOADER_SECTORS - 1,
        BOOTLOADER_SECTORS,
    ));
    for layout in crate::partition_map::PARTITION_LAYOUT.iter() {
        out.push_str(&format!(
            "  {:<12} LBA {:>6}..{:>6} ({:>5} sect)  fs={:<5} flash={}\n",
            layout.name,
            layout.start_lba,
            layout.start_lba + layout.sectors - 1,
            layout.sectors,
            layout.fs,
            if layout.flashable { "yes" } else { "NO" },
        ));
    }
    out
}

/// Строит образ для FLASH: НАСТОЯЩИЙ EROFS для системных разделов,
/// сырые данные для остальных.
pub fn build_test_image(layout: &PartitionLayout) -> Vec<u8> {
    let size = (layout.sectors * 512) as usize;
    if layout.fs == "erofs" {
        // Реальный EROFS-образ (спецификация v1) с меткой раздела внутри.
        let content = alloc::format!("DeiX partition {}\n", layout.name);
        let mut img = crate::erofs::build_image(&[("PARTITION.INFO", content.as_bytes())]);
        img.resize(size, 0);
        img
    } else {
        let mut img = vec![0u8; size];
        if img.len() >= 8 {
            img[..8].copy_from_slice(b"DEIXDSM1");
        }
        img
    }
}

// ==================== ГРАФИЧЕСКАЯ ОБОЛОЧКА ====================

const BG: u32 = 0x0D0D10;
const TITLE: u32 = 0xC0392B; // красный (emergency!)
const ITEM: u32 = 0xE8F0F8;
const SEL: u32 = 0xFFD24A;
const HINT: u32 = 0x8FB8D8;
const OK: u32 = 0x7ED957;
const ERR: u32 = 0xFF6B6B;

/// Действия DSM в GUI.
const ACTIONS: [&str; 7] = [
    "INFO (partition map)",
    "READ (dump first bytes)",
    "FLASH (built-in image)",
    "ERASE (zero the partition)",
    "VERIFY (SHA-256)",
    "Reboot system",
    "Reboot recovery",
];

/// Главный цикл графической оболочки DSM.
pub fn dsm_gui() {
    // DSM реально берётся из раздела /dsm (образ dsm.bin, emergency).
    match crate::bootchain::load_mode_image("dsm") {
        Ok(_) => {}
        Err(e) => crate::println!("  [dsm] предупреждение: {}", e),
    }
    crate::renderer::ensure_framebuffer();
    crate::println!("==============================================");
    crate::println!("   DeiX DSM — Download System Manager (EDL)   ");
    crate::println!("==============================================");
    crate::println!("  (priority: dsm > fastbootd > recovery > OS)");

    let partitions = partition_names();
    let mut sel_part: usize = 0;
    let mut sel_act: usize = 0;
    let mut status: String = "DSM ready. Select a partition and action.".to_string();
    let mut status_color: u32 = HINT;
    let mut cmd_buf: String = String::new();

    render(&partitions, sel_part, sel_act, &status, status_color, 0, 0);

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

        // Serial-протокол: строки "dsm <cmd> ..." — команды. Иначе — GUI-клавиши.
        if from_serial {
            if key == b'\n' || key == b'\r' {
                let line = cmd_buf.clone();
                cmd_buf.clear();
                if line.trim_start().starts_with("dsm") {
                    let answer = handle_command(&line);
                    crate::println!("{}", answer);
                    crate::syslog::log_line(&answer);
                    status_color = if answer.contains("OK") || answer.contains("sha256") { OK } else { HINT };
                    status = answer;
                    render(&partitions, sel_part, sel_act, &status, status_color, 0, 0);
                    continue;
                }
                let _ = line;
            } else if cmd_buf.is_empty() && key != b'd' && key != b's' && key != b'm' {
                // обычная GUI-клавиша с serial (цифра/w/s/Enter)
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
                let (new_status, color, do_reboot) = execute_action(&partitions, sel_part, sel_act);
                status = new_status;
                status_color = color;
                if do_reboot {
                    crate::println!("  [dsm] Rebooting (boot flag set).");
                    return;
                }
            }
            _ => {}
        }
        render(&partitions, sel_part, sel_act, &status, status_color, 0, 0);
    }
}

/// Список имён разделов DSM.
fn partition_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = vec!["/bootloader"];
    for layout in crate::partition_map::PARTITION_LAYOUT.iter() {
        names.push(layout.name);
    }
    names
}

/// Выполняет выбранное действие. Возвращает (статус, цвет, выйти из DSM?).
fn execute_action(partitions: &[&'static str], sel_part: usize, sel_act: usize) -> (String, u32, bool) {
    let part = partitions[sel_part];
    crate::println!("  [dsm] Action: {} -> {}", ACTIONS[sel_act], part);
    crate::syslog::log_line(&format!("dsm: {} -> {}", ACTIONS[sel_act], part));

    match sel_act {
        0 => {
            let text = dsm_info_text();
            crate::println!("{}", text);
            (text, OK, false)
        }
        1 => match dsm_read_partition(part, 0, 32) {
            Ok(data) => {
                let mut hex = String::new();
                for (i, b) in data.iter().take(64).enumerate() {
                    if i % 16 == 0 {
                        hex.push_str(&format!("\n  {:04x}: ", i));
                    }
                    hex.push_str(&format!("{:02x} ", b));
                }
                let msg = format!("READ {}: 32 sectors, first bytes:{}", part, hex);
                crate::println!("{}", msg);
                (msg, OK, false)
            }
            Err(e) => (format!("READ {}: {}", part, e), ERR, false),
        },
        2 => match lookup_layout(part) {
            Some(layout) => {
                let img = build_test_image(layout);
                crate::println!("  [dsm] FLASH {}: {} bytes...", part, img.len());
                match dsm_write_partition(part, 0, &img) {
                    Ok(()) => (format!("FLASH {}: OK ({} bytes)", part, img.len()), OK, false),
                    Err(e) => (format!("FLASH {}: {}", part, e), ERR, false),
                }
            }
            None => {
                let img = vec![0u8; (BOOTLOADER_SECTORS * 512) as usize];
                match dsm_write_partition(part, 0, &img) {
                    Ok(()) => (format!("FLASH {}: OK ({} bytes)", part, img.len()), OK, false),
                    Err(e) => (format!("FLASH {}: {}", part, e), ERR, false),
                }
            }
        },
        3 => match dsm_erase_partition(part) {
            Ok(n) => (format!("ERASE {}: OK ({} sectors zeroed)", part, n), OK, false),
            Err(e) => (format!("ERASE {}: {}", part, e), ERR, false),
        },
        4 => match dsm_verify_partition(part) {
            Ok(digest) => (format!("VERIFY {}: sha256={}", part, digest), OK, false),
            Err(e) => (format!("VERIFY {}: {}", part, e), ERR, false),
        },
        5 => {
            crate::bcb::clear_boot_mode();
            ("Reboot system: boot flag normal".to_string(), OK, true)
        }
        6 => {
            crate::bcb::write_boot_mode(crate::bcb::BootMode::Recovery);
            ("Reboot recovery: boot flag set".to_string(), OK, true)
        }
        _ => ("Unknown action".to_string(), ERR, false),
    }
}

/// Рисует кадр DSM.
#[allow(clippy::too_many_arguments)]
fn render(
    partitions: &[&'static str],
    sel_part: usize,
    sel_act: usize,
    status: &str,
    status_color: u32,
    _progress: u32,
    _progress_total: u32,
) {
    crate::renderer::with_renderer(|r| {
        let w = r.width();
        let h = r.height();
        r.clear(crate::renderer::Color::from_u32(BG));

        // Заголовок с градиентом (тёмный -> яркий) + нижняя линия.
        r.fill_rect_gradient_v(0, 0, w, 44,
            crate::renderer::Color::from_u32(0x7A1F16),
            crate::renderer::Color::from_u32(TITLE));
        r.draw_hline(0, 44, w, crate::renderer::Color::from_u32(0xFF6B4A));
        r.draw_text(12, 13, "DeiX DSM — Download System Manager (EDL)",
                    crate::renderer::Color::from_u32(0xFFFFFF), None);

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
            let label: String = format!("{}. {}", i + 1, p);
            r.draw_text(22, (y + 2) as i32, &label, fg, None);
        }

        let ax = 320u32;
        r.draw_text(ax as i32, 54, "ACTIONS:", crate::renderer::Color::from_u32(HINT), None);
        for (i, a) in ACTIONS.iter().enumerate() {
            let y = 74 + (i as u32) * 22;
            let sel = i == sel_act && sel_act != 0;
            if sel {
                r.fill_rounded_rect((ax - 2) as i32, y as i32, 280, 18, 6, crate::renderer::Color::from_u32(SEL));
            }
            let fg = if sel {
                crate::renderer::Color::from_u32(0x000000)
            } else {
                crate::renderer::Color::from_u32(ITEM)
            };
            r.draw_text((ax + 4) as i32, (y + 2) as i32, a, fg, None);
        }

        let sy = h - 76;
        r.fill_rounded_rect(12, sy as i32, w - 24, 40, 8, crate::renderer::Color::from_u32(0x181C22));
        r.draw_rect(12, sy as i32, w - 24, 40, crate::renderer::Color::from_u32(0x2A2F38));
        r.draw_text(18, (sy + 8) as i32, status, crate::renderer::Color::from_u32(status_color), None);

        let hint = "UP/DOWN/w/s: switch | 1..9: partition | Enter: action | serial: 'dsm <cmd>'";
        r.draw_text(12, (h - 20) as i32, hint, crate::renderer::Color::from_u32(HINT), None);

        r.present();
    });
}

// ==================== SERIAL-ПРОТОКОЛ DSM ====================

/// Обрабатывает одну строку команды "dsm <cmd> [args]". Возвращает ответ.
pub fn handle_command(line: &str) -> String {
    let trimmed = line.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.is_empty() {
        return "dsm: empty command".to_string();
    }
    let idx: usize = if parts[0] == "dsm" { 1 } else { 0 };
    if parts.len() <= idx {
        return "dsm: empty command".to_string();
    }
    let cmd = parts[idx];
    let arg = |n: usize| -> Option<&str> { parts.get(idx + n).copied() };
    match cmd {
        "info" => dsm_info_text(),
        "read" => {
            let (part, off, count) = (arg(1), arg(2), arg(3));
            if part.is_none() || off.is_none() || count.is_none() {
                return "dsm read <part> <off_sect> <count>".to_string();
            }
            let off: u32 = off.unwrap().parse().unwrap_or(0);
            let count: u32 = count.unwrap().parse().unwrap_or(1);
            match dsm_read_partition(part.unwrap(), off, count) {
                Ok(data) => {
                    let mut hex = String::new();
                    for b in data.iter().take(64) {
                        hex.push_str(&format!("{:02x}", b));
                    }
                    format!("dsm read {}: {} sectors, hex={}...", part.unwrap(), count, hex)
                }
                Err(e) => e,
            }
        }
        "erase" => match arg(1) {
            Some(part) => match dsm_erase_partition(part) {
                Ok(n) => format!("dsm erase {}: OK ({} sectors)", part, n),
                Err(e) => e,
            },
            None => "dsm erase <part>".to_string(),
        },
        "verify" => match arg(1) {
            Some(part) => match dsm_verify_partition(part) {
                Ok(d) => format!("dsm verify {}: sha256={}", part, d),
                Err(e) => e,
            },
            None => "dsm verify <part>".to_string(),
        },
        "flash" => {
            let (part, len) = (arg(1), arg(2));
            if part.is_none() || len.is_none() {
                return "dsm flash <part> <len_bytes>: expected <len> raw bytes to follow".to_string();
            }
            let len: usize = len.unwrap().parse().unwrap_or(0);
            if len == 0 || len % 512 != 0 {
                return "dsm flash: len must be a multiple of 512".to_string();
            }
            let mut data: Vec<u8> = Vec::with_capacity(len);
            while data.len() < len {
                if crate::serial::is_data_ready() {
                    data.push(crate::serial::read_byte());
                } else {
                    unsafe { core::arch::asm!("nop"); }
                }
            }
            match dsm_write_partition(part.unwrap(), 0, &data) {
                Ok(()) => format!("dsm flash {}: OK ({} bytes)", part.unwrap(), len),
                Err(e) => e,
            }
        }
        "reboot" => {
            let target = arg(1).unwrap_or("normal");
            match target {
                "recovery" => crate::bcb::write_boot_mode(crate::bcb::BootMode::Recovery),
                "fastbootd" => crate::bcb::write_boot_mode(crate::bcb::BootMode::Fastbootd),
                "dsm" => crate::bcb::write_boot_mode(crate::bcb::BootMode::Dsm),
                _ => crate::bcb::clear_boot_mode(),
            }
            format!("dsm reboot -> {}", target)
        }
        _ => format!("dsm: unknown command '{}' (info/read/erase/flash/verify/reboot)", cmd),
    }
}
