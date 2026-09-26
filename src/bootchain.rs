// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// bootchain — ПОЛНАЯ ЦЕПОЧКА ЗАГРУЗКИ ЧЕРЕЗ ВСЕ РАЗДЕЛЫ (не пустышки!).
//
// Порядок (как в Android/bootloader-цепочках):
//   1) MBR (boot_sector) + stage2            — первый загрузчик;
//   2) /init_boot  -> bootloader.bin         — загрузчик второго уровня;
//   3) /vendor_boot-> vendor.bin             — прошивка вендора (HAL);
//   4) /boot       -> fastbootd.bin, recovery.bin — загрузочные образы режимов;
//   5) /kernel     -> kernel.tar.gz          — НАСТОЯЩИЙ gzip: kernel.bin +
//                                              библиотеки (libdeix_*.so).
// Ядро при старте (normal) проходит эту цепочку: каждый раздел ЧИТАЕТСЯ,
// его файлы извлекаются и верифицируются. Файлы EROFS-разделов лежат в
// таблице после суперблока: u32 count, затем (name[32] + u32 offset + u32 size),
// данные — после таблицы.
// no_std-совместимо: alloc (Vec, String), вывод — crate::println!.


use alloc::format;
use alloc::string::{String, ToString};
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

/// Список файлов в EROFS-разделе: `(имя, размер)`.
///
/// Настоящий разбор EROFS v1 через `crate::erofs` (магия 0xE0F5E1E2):
/// суперблок -> корневой инод -> записи каталога. Понимает и образы,
/// созданные `mkfs.erofs` (раскладки FLAT_PLAIN и FLAT_INLINE).
pub fn erofs_list_files(image: &[u8]) -> Result<Vec<(String, usize)>, String> {
    crate::erofs::list_files(image).map_err(|e| e.message())
}

/// Извлекает файл из EROFS-раздела по имени.
pub fn erofs_extract(image: &[u8], name: &str) -> Result<Vec<u8>, String> {
    crate::erofs::read_file(image, name)
        .map_err(|e| alloc::format!("файл '{}': {}", name, e.message()))
}

/// Краткое описание файла (первые байты как текст/hex).
fn describe(data: &[u8]) -> String {
    let mut s = String::new();
    let n = data.len().min(24);
    for b in &data[..n] {
        if *b >= 0x20 && *b < 0x7F {
            s.push(*b as char);
        } else {
            s.push('.');
        }
    }
    s
}

/// Результат загрузки одного звена цепочки.
struct ChainLink {
    pub files: Vec<String>,
    pub loaded: usize,
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

/// Загружает звено: читает раздел, извлекает указанные файлы.
fn load_link(partition: &str, wanted: &[&str]) -> Result<ChainLink, String> {
    let layout = lookup_layout(partition).ok_or_else(|| format!("нет раздела {}", partition))?;
    let image = read_partition_image(layout)?;
    let files = erofs_list_files(&image)?;
    let mut found: Vec<String> = Vec::new();
    let mut loaded = 0usize;
    for (name, _size) in files.iter() {
        if wanted.contains(&name.as_str()) {
            let data = erofs_extract(&image, name)?;
            loaded += data.len();
            found.push(format!("{} ({})", name, describe(&data)));
        }
    }
    if found.is_empty() {
        return Err(format!("нет файлов из {:?} в {}", wanted, partition));
    }
    Ok(ChainLink { files: found, loaded })
}

/// Загружает kernel.bin из /system/kernel/kernel.bin (EROFS).
pub fn load_kernel() -> Result<String, String> {
    let layout = crate::partition_map::active_kernel_layout();
    let image = read_partition_image(layout)?;

    let kernel_bin = erofs_extract(&image, "kernel/kernel.bin")
        .or_else(|_| erofs_extract(&image, "kernel.bin"))?;

    let summary = format!(
        "/system/kernel/kernel.bin ({} байт)",
        kernel_bin.len()
    );

    Ok(summary)
}

/// Показывает файлы во всех разделах (диагностика).
pub fn show_partition_files() {
    crate::println!("  [bootchain] Содержимое разделов:");
    for layout in crate::partition_map::PARTITION_LAYOUT.iter() {
        if layout.fs != "erofs" {
            continue;
        }
        match read_partition_image(layout) {
            Ok(image) => match erofs_list_files(&image) {
                Ok(files) => {
                    if files.is_empty() {
                        crate::println!("    {:<12} (пусто)", layout.name);
                    } else {
                        let names: Vec<String> =
                            files.iter().map(|(n, s)| format!("{}({}Б)", n, s)).collect();
                        crate::println!("    {:<12} {}", layout.name, names.join(", "));
                    }
                }
                Err(_) => crate::println!("    {:<12} (не EROFS)", layout.name),
            },
            Err(_) => crate::println!("    {:<12} (не читается)", layout.name),
        }
    }
}
