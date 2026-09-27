// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// bootchain — ЗАГРУЗКА ЯДРА И СИСТЕМНЫХ КОМПОНЕНТОВ
//
// Порядок загрузки:
//   1) MBR (boot_sector) + stage2
//   2) /system (EROFS) -> /system/kernel/kernel.bin
//   3) Dinit (PID 1)
// no_std-совместимо: alloc (Vec, String), вывод — crate::println!.


use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::ata;
use crate::partition_map::{lookup_layout, PartitionLayout};

/// Читает весь EROFS-раздел с диска напрямую в результирующий вектор без лишних аллокаций.
pub fn read_partition_image(layout: &PartitionLayout) -> Result<Vec<u8>, String> {
    let total_bytes = (layout.sectors as usize) * 512;
    let mut out: Vec<u8> = vec![0u8; total_bytes];
    let mut cur = 0u32;
    let mut left = layout.sectors;
    while left > 0 {
        let batch = left.min(128) as u8;
        let start_byte = (cur as usize) * 512;
        let end_byte = start_byte + (batch as usize) * 512;
        ata::read_sectors(layout.start_lba + cur, batch, &mut out[start_byte..end_byte])
            .map_err(|_| format!("read {} err", layout.name))?;
        left -= batch as u32;
        cur += batch as u32;
    }
    Ok(out)
}

/// Извлекает файл из EROFS-раздела по имени.
pub fn erofs_extract(image: &[u8], name: &str) -> Result<Vec<u8>, String> {
    crate::erofs::read_file(image, name)
        .map_err(|e| alloc::format!("файл '{}': {}", name, e.message()))
}

/// Проходит цепочку загрузки (normal): init_boot -> vendor_boot -> boot -> kernel.
/// Возвращает Ok(сводка) или Err(причина). Вызывается из kernel_main ПОСЛЕ
/// инициализации ATA/heap, ДО экрана входа. Для fastbootd/recovery режимов
/// вызывается отдельно (см. load_mode_image).
pub fn run_boot_chain() -> Result<String, String> {
    let mut out = String::new();
    out.push_str("  [bootchain] Цепочка загрузки системного раздела /system (EROFS):\n");

    match load_kernel() {
        Ok(info) => {
            out.push_str(&format!("    /system -> {}\n", info));
        }
        Err(e) => return Err(format!("system: {}", e)),
    }

    out.push_str("  [bootchain] Системный раздел проверен и загружен.\n");
    Ok(out)
}

/// Загружает kernel.bin из /system/kernel/kernel.bin (EROFS).
pub fn load_kernel() -> Result<String, String> {
    let layout = lookup_layout("/system")
        .ok_or_else(|| String::from("раздел /system не найден"))?;
    let image = read_partition_image(layout)?;

    let kernel_bin = erofs_extract(&image, "kernel/kernel.bin")
        .or_else(|_| erofs_extract(&image, "kernel.bin"))?;

    let summary = format!(
        "/system/kernel/kernel.bin ({} байт)",
        kernel_bin.len()
    );

    Ok(summary)
}
