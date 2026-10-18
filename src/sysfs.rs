// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// sysfs — утилита обслуживания раздела /system (команда `sysfs`).
//
//   sysfs status   — тип ФС на /system и состояние тома;
//   sysfs convert  — живая конверсия /system из EROFS (только чтение)
//                    в ext2 (чтение/запись) — ТОЛЬКО в dev-режиме.
//
// Как работает конверсия:
//   1. Содержимое EROFS перечисляется и ЦЕЛИКОМ читается в память через
//      vfs: образ /system лениво читается с диска, поэтому до
//      форматирования все файлы должны быть уже в куче. Если хоть один
//      файл не прочитался, диск не трогается.
//   2. Раздел форматируется в ext2 (ext2::format_system, без
//      шифрования — загрузчик читает раздел открытым).
//   3. Файлы из памяти пишутся на новый том напрямую через драйвер ext2.
//   4. vfs::remount_system() сбрасывает кеш и определение типа ФС —
//      дальше /system читается драйвером ext2; запись разрешена только
//      в dev-режиме (vfs::system_writable).
//
// Конверсия необратима штатными средствами: вернуть EROFS можно только
// перепрошивкой раздела (install / flash образа).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::println;

pub fn cmd_sysfs(arg: &str) {
    match arg.trim() {
        "" | "status" => status(),
        "convert" => convert(),
        _ => {
            println!("  usage: sysfs [status|convert]");
            println!("    status  - show /system filesystem type and volume state");
            println!("    convert - convert /system from EROFS to ext2 (dev mode only)");
        }
    }
}

fn status() {
    println!("=== /system filesystem ===");
    if crate::ext2::is_formatted_vol(crate::ext2::VolId::System) {
        println!("  type: ext2 (converted from EROFS)");
        match crate::ext2::volume_stats(crate::ext2::VolId::System) {
            Ok((total, free, groups)) => {
                println!(
                    "  volume: {} KiB total, {} KiB free, {} group(s)",
                    total, free, groups
                );
            }
            Err(_) => println!("  volume: superblock unreadable"),
        }
        println!(
            "  writes: {}",
            if crate::devmode::sudo_allowed() {
                "allowed (dev mode)"
            } else {
                "denied (enable dev mode: 'dev on')"
            }
        );
    } else {
        println!("  type: EROFS (read-only)");
        println!("  convert to ext2: 'sysfs convert' (requires dev mode)");
    }
}

/// Рекурсивно собирает пути всех обычных файлов /system (через vfs).
fn collect_paths(dir: &str, out: &mut Vec<String>) -> Result<(), String> {
    let entries = crate::vfs::readdir(dir).map_err(|e| e.message())?;
    for e in entries {
        let path = if dir.ends_with('/') {
            format!("{}{}", dir, e.name)
        } else {
            format!("{}/{}", dir, e.name)
        };
        if e.is_dir {
            collect_paths(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn convert() {
    if !crate::devmode::sudo_allowed() {
        println!("  [sysfs] convert requires dev mode. Run 'dev on' first.");
        return;
    }
    if crate::ext2::is_formatted_vol(crate::ext2::VolId::System) {
        println!("  [sysfs] /system is already ext2 - nothing to convert.");
        return;
    }

    println!("  [sysfs] WARNING: /system will be reformatted to ext2 IN PLACE.");
    println!("  [sysfs] Verified-boot style integrity of /system will be VOID.");

    // 1. Список файлов и их содержимое: после форматирования читать
    //    EROFS будет неоткуда, поэтому всё нужно держать в памяти.
    println!("  [sysfs] 1/4 reading EROFS files into memory...");
    let mut paths: Vec<String> = Vec::new();
    if let Err(e) = collect_paths("/system", &mut paths) {
        println!("  [sysfs] ERROR: cannot read /system: {}", e);
        return;
    }
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(paths.len());
    for path in &paths {
        match crate::vfs::read_file(path) {
            Ok(data) => files.push((path.clone(), data)),
            Err(e) => {
                println!("  [sysfs] ERROR: cannot read {}: {}", path, e.message());
                println!("  [sysfs] /system was not modified.");
                return;
            }
        }
    }
    println!("  [sysfs]     {} files read", files.len());

    // 2. Форматирование раздела в ext2.
    println!("  [sysfs] 2/4 formatting /system as ext2...");
    if crate::ext2::format_system().is_err() {
        println!("  [sysfs] ERROR: ext2 format failed. /system partition state is");
        println!("  [sysfs] undefined on disk; the files are still in memory only");
        println!("  [sysfs] until reboot. Re-run 'sysfs convert' or reflash /system.");
        return;
    }

    // 3. Перенос файлов: пишем из памяти напрямую драйвером ext2.
    println!("  [sysfs] 3/4 copying files to the ext2 volume...");
    let mut copied = 0usize;
    let mut bytes: u64 = 0;
    let mut failed = 0usize;
    for (path, data) in &files {
        // Путь внутри тома: без префикса /system.
        let key = path.strip_prefix("/system").unwrap_or(path).trim_start_matches('/');
        if let Err(e) =
            crate::ext2::write_file_path_vol(crate::ext2::VolId::System, key, data)
        {
            println!("  [sysfs]     FAIL {}: {:?}", path, e);
            failed += 1;
            continue;
        }
        bytes += data.len() as u64;
        copied += 1;
    }
    println!("  [sysfs]     {} files, {} KiB copied, {} failed", copied, bytes / 1024, failed);

    // 4. Перемонтирование: vfs начинает читать /system как ext2,
    //    блочный кеш EROFS сбрасывается.
    println!("  [sysfs] 4/4 remounting /system...");
    crate::vfs::remount_system();

    match crate::vfs::readdir("/system") {
        Ok(entries) => {
            println!("  [sysfs] /system is now ext2: {} top-level entries.", entries.len());
            println!("  [sysfs] Writes are allowed while dev mode is on.");
            if failed > 0 {
                println!("  [sysfs] WARNING: {} files were NOT copied (see above).", failed);
            }
        }
        Err(e) => {
            println!("  [sysfs] ERROR: remounted /system is unreadable: {}", e.message());
        }
    }
}
