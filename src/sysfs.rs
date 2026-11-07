// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// sysfs — утилита обслуживания раздела /system (команда `sysfs`).
//
//   sysfs status   — тип ФС на /system и состояние тома;
//   sysfs convert  — живая конверсия /system из EROFS (только чтение)
//                    в ext2 (чтение/запись) — ТОЛЬКО в dev-режиме.
//
// Как работает конверсия:
//      дальше /system читается драйвером ext2; запись разрешена только
//      в dev-режиме (vfs::system_writable).
//
// Конверсия необратима штатными средствами: вернуть EROFS можно только
// перепрошивкой раздела (install / flash образа).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

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

    let mut paths: Vec<String> = Vec::new();
    if let Err(e) = collect_paths("/system", &mut paths) {
        println!("  [sysfs] ERROR: cannot read /system: {}", e);
        return;
    }
        println!("  [sysfs] until reboot. Re-run 'sysfs convert' or reflash /system.");
        return;
    }

        {
            println!("  [sysfs]     FAIL {}: {:?}", path, e);
            failed += 1;
            continue;
        }
        bytes += data.len() as u64;
        copied += 1;
    }
    println!("  [sysfs]     {} files, {} KiB copied, {} failed", copied, bytes / 1024, failed);

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
