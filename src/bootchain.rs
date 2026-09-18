// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
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

/// Читает весь EROFS-раздел с диска.
pub fn read_partition_image(layout: &PartitionLayout) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity((layout.sectors as usize) * 512);
    let mut cur = 0u32;
    let mut left = layout.sectors;
    while left > 0 {
        let batch = left.min(256);
        let mut buf = vec![0u8; (batch * 512) as usize];
        ata::read_sectors(layout.start_lba + cur, batch as u8, &mut buf)
            .map_err(|_| format!("read {} err", layout.name))?;
        out.extend_from_slice(&buf);
        left -= batch;
        cur += batch;
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
    out.push_str("  [bootchain] Полная цепочка загрузки:\n");
    let mut total = 0usize;

    // 1) /dsm -> dsm.bin (emergency Download System Manager — сразу после
    //    первого загрузчика, доступен всегда, даже при сломанной ОС)
    match load_link("/dsm", &["dsm.bin"]) {
        Ok(link) => {
            out.push_str(&format!(
                "    /dsm -> {} ({} байт: emergency DSM готов)\n",
                link.files.join(", "), link.loaded
            ));
            total += link.loaded;
        }
        Err(e) => return Err(format!("dsm: {}", e)),
    }

    // 2) /init_boot -> bootloader.bin (загрузчик второго уровня)
    match load_link("/init_boot", &["bootloader.bin"]) {
        Ok(link) => {
            out.push_str(&format!(
                "    /init_boot -> {} ({} байт: {})\n",
                link.files.join(", "), link.loaded, "bootloader v2 loaded"
            ));
            total += link.loaded;
        }
        Err(e) => return Err(format!("init_boot: {}", e)),
    }

    // 3) /vendor_boot -> vendor.bin (прошивка вендора/HAL)
    match load_link("/vendor_boot", &["vendor.bin"]) {
        Ok(link) => {
            out.push_str(&format!(
                "    /vendor_boot -> {} ({} байт: {})\n",
                link.files.join(", "), link.loaded, "vendor HAL loaded"
            ));
            total += link.loaded;
        }
        Err(e) => return Err(format!("vendor_boot: {}", e)),
    }

    // 4) /boot_<slot> -> fastbootd.bin + recovery.bin (образы режимов, A/B слот)
    let boot_name = crate::partition_map::active_boot_layout().name;
    match load_link(boot_name, &["fastbootd.bin", "recovery.bin"]) {
        Ok(link) => {
            out.push_str(&format!(
                "    {} -> {} ({} байт: режимы fastbootd/recovery готовы, слот {})\n",
                boot_name, link.files.join(", "), link.loaded, crate::bcb::slot_name()
            ));
            total += link.loaded;
        }
        Err(e) => return Err(format!("boot: {}", e)),
    }

    // 5) /kernel_<slot> -> kernel.tar.gz (распаковка kernel.bin + библиотек)
    match load_kernel() {
        Ok(info) => {
            out.push_str(&format!("    /kernel -> kernel.tar.gz ({})\n", info));
            total += 1;
        }
        Err(e) => return Err(format!("kernel: {}", e)),
    }

    out.push_str(&format!("  [bootchain] Все разделы задействованы ({} звеньев, {} байт).\n", 5, total));
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

/// Загружает kernel.tar.gz из /kernel и распаковывает (gzip + tar).
/// Возвращает строку-сводку (kernel.bin + библиотеки).
pub fn load_kernel() -> Result<String, String> {
    let layout = crate::partition_map::active_kernel_layout();
    let image = read_partition_image(layout)?;
    let tar_gz = erofs_extract(&image, "kernel.tar.gz")?;

    // Распаковка gzip (deflate).
    let tar = crate::inflate::gunzip(&tar_gz, 4 * 1024 * 1024)
        .map_err(|e| format!("gunzip: {:?}", e))?;

    // Разбор tar (ustar) — используем kernel_loader::TarArchive.
    let archive = crate::kernel_loader::TarArchive::parse(tar)
        .map_err(|e| format!("tar: {}", e.message()))?;

    // Извлекаем kernel.bin и библиотеки.
    let kernel_bin = archive.extract("kernel.bin").map_err(|e| e.message())?;
    let mut summary = format!(
        "kernel.bin {} байт, EROFS-магия {}",
        kernel_bin.len(),
        if kernel_bin.len() >= 1028 {
            let m = u32::from_le_bytes([kernel_bin[1024], kernel_bin[1025], kernel_bin[1026], kernel_bin[1027]]);
            format!("{:#010x}", m)
        } else {
            "—".to_string()
        }
    );

    // Библиотеки.
    for lib in ["libdeix_core.so", "libdeix_net.so", "libdeix_gfx.so"] {
        if let Ok(data) = archive.extract(lib) {
            summary.push_str(&format!(", {} ({} байт)", lib, data.len()));
        }
    }
    Ok(summary)
}

/// Загружает образ режима из его раздела:
///   fastbootd/recovery -> /boot, dsm -> /dsm.
/// Вызывается перед запуском соответствующей оболочки — режим реально
/// берётся из раздела.
pub fn load_mode_image(mode: &str) -> Result<Vec<u8>, String> {
    let (partition, fname) = match mode {
        "fastbootd" | "recovery" => (crate::partition_map::active_boot_layout().name, match mode {
            "fastbootd" => "fastbootd.bin",
            _ => "recovery.bin",
        }),
        "dsm" => ("/dsm", "dsm.bin"),
        _ => return Err(format!("неизвестный режим {}", mode)),
    };
    let layout = lookup_layout(partition).ok_or_else(|| format!("нет раздела {}", partition))?;
    let image = read_partition_image(layout)?;
    let data = erofs_extract(&image, fname)?;
    crate::println!(
        "  [bootchain] Режим {} загружен из {} ({} байт: {})",
        mode, partition, data.len(), describe(&data)
    );
    Ok(data)
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
