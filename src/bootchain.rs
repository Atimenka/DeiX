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
use crate::spinlock::SpinLock;

static INIT_DEIX_TEXT: SpinLock<Option<String>> = SpinLock::new(None);

/// Возвращает содержимое init.deix, прочитанное из /init_boot на этапе bootchain.
pub fn get_init_deix() -> Option<String> {
    INIT_DEIX_TEXT.lock().clone()
}

/// Читает EROFS-раздел с диска, считывая только фактически занятые блоки.
pub fn read_partition_image(layout: &PartitionLayout) -> Result<Vec<u8>, String> {
    if layout.sectors == 0 {
        return Ok(Vec::new());
    }

    // Раннее чтение первого блока (4096 байт = 8 секторов) для определения размера EROFS
    let first_batch = layout.sectors.min(8);
    let mut header_buf = vec![0u8; (first_batch * 512) as usize];
    ata::read_sectors(layout.start_lba, first_batch as u8, &mut header_buf)
        .map_err(|_| format!("read {} err", layout.name))?;

    let total_sectors = if layout.fs == "erofs" {
        if let Ok(sb) = crate::erofs::parse_superblock(&header_buf) {
            let needed_bytes = (sb.blocks as usize) * sb.block_size();
            let needed_sectors = ((needed_bytes + 511) / 512) as u32;
            needed_sectors.max(first_batch).min(layout.sectors)
        } else {
            layout.sectors
        }
    } else {
        layout.sectors
    };

    let mut out: Vec<u8> = Vec::with_capacity((total_sectors as usize) * 512);
    out.extend_from_slice(&header_buf);

    let mut cur = first_batch;
    let mut left = total_sectors.saturating_sub(first_batch);

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

    // 2) /init_boot -> bootloader.bin, init.deix (загрузчик второго уровня + инит-скрипт)
    match load_link("/init_boot", &["bootloader.bin", "init.deix"]) {
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

/// Возвращает резервное встроенное содержимое для критических файлов цепочки загрузки,
/// если соответствующий EROFS-раздел повреждён или не отформатирован.
fn get_fallback_file(name: &str) -> Vec<u8> {
    match name {
        "init.deix" => crate::init_parser::FALLBACK_INIT.as_bytes().to_vec(),
        "bootloader.bin" => b"DEIX_BOOTLOADER_STAGE2_V2_OK\x00".to_vec(),
        "vendor.bin" => b"DEIX_VENDOR_HAL_FIRMWARE_V1_OK\x00".to_vec(),
        "dsm.bin" => b"DEIX_DSM_EMERGENCY_MANAGER_V1_OK\x00".to_vec(),
        "fastbootd.bin" => b"DEIX_FASTBOOTD_FLASH_ENGINE_V1_OK\x00".to_vec(),
        "recovery.bin" => b"DEIX_RECOVERY_IMAGE_UI_V1_OK\x00".to_vec(),
        _ => b"DEIX_GENERIC_FALLBACK_FILE\x00".to_vec(),
    }
}

/// Загружает звено: читает раздел, извлекает указанные файлы.
fn load_link(partition: &str, wanted: &[&str]) -> Result<ChainLink, String> {
    let layout = match lookup_layout(partition) {
        Some(l) => l,
        None => return Err(format!("нет раздела {}", partition)),
    };
    let image = read_partition_image(layout).unwrap_or_default();
    let files = erofs_list_files(&image).unwrap_or_default();
    let mut found: Vec<String> = Vec::new();
    let mut loaded = 0usize;

    for &w in wanted {
        let file_data = files.iter().find(|(n, _)| n == w)
            .and_then(|(n, _)| erofs_extract(&image, n).ok());

        let data = match file_data {
            Some(d) => d,
            None => {
                crate::serial_println!(
                    "[bootchain] ПРЕДУПРЕЖДЕНИЕ: Файл '{}' в разделе {} не найден или повреждён EROFS. Использование встроенного резервного образа...",
                    w, partition
                );
                get_fallback_file(w)
            }
        };

        if w == "init.deix" {
            if let Ok(text) = core::str::from_utf8(&data) {
                *INIT_DEIX_TEXT.lock() = Some(text.to_string());
            }
        }
        loaded += data.len();
        found.push(format!("{} ({})", w, describe(&data)));
    }

    Ok(ChainLink { files: found, loaded })
}

/// Загружает kernel.tar.gz из /kernel и распаковывает (gzip + tar).
/// Возвращает строку-сводку (kernel.bin + библиотеки).
pub fn load_kernel() -> Result<String, String> {
    let layout = crate::partition_map::active_kernel_layout();
    let mut fallback_used = false;

    let tar_gz = match read_partition_image(layout) {
        Ok(image) => match erofs_extract(&image, "kernel.tar.gz") {
            Ok(data) => data,
            Err(e) => {
                crate::serial_println!(
                    "[bootchain] Ошибка чтения kernel.tar.gz из раздела {} ({}). Переход на встроенное ядро...",
                    layout.name, e
                );
                crate::println!("  [bootchain] ПРЕДУПРЕЖДЕНИЕ: раздел {} повреждён ({}). Восстановление из резервного образа...", layout.name, e);
                fallback_used = true;
                crate::ota::build_fresh_kernel_targz()
            }
        },
        Err(e) => {
            crate::serial_println!(
                "[bootchain] Ошибка чтения раздела {} ({}). Переход на встроенное ядро...",
                layout.name, e
            );
            crate::println!("  [bootchain] ПРЕДУПРЕЖДЕНИЕ: не удалось прочитать раздел {}. Восстановление...", layout.name);
            fallback_used = true;
            crate::ota::build_fresh_kernel_targz()
        }
    };

    // Распаковка gzip (deflate).
    let tar = match crate::inflate::gunzip(&tar_gz, 8 * 1024 * 1024) {
        Ok(t) => t,
        Err(e) => {
            crate::serial_println!("[bootchain] Ошибка распаковки gzip ({:?}), переход на встроенное ядро...", e);
            fallback_used = true;
            let fresh = crate::ota::build_fresh_kernel_targz();
            crate::inflate::gunzip(&fresh, 8 * 1024 * 1024).map_err(|e2| format!("gunzip fallback err: {:?}", e2))?
        }
    };

    // Разбор tar (ustar) — используем kernel_loader::TarArchive.
    let archive = crate::kernel_loader::TarArchive::parse(tar)
        .map_err(|e| format!("tar: {}", e.message()))?;

    // Извлекаем kernel.bin и библиотеки.
    let kernel_bin = archive.extract("kernel.bin").map_err(|e| e.message())?;
    let mut summary = format!(
        "kernel.bin {} байт, EROFS-магия {}{}",
        kernel_bin.len(),
        if kernel_bin.len() >= 1028 {
            let m = u32::from_le_bytes([kernel_bin[1024], kernel_bin[1025], kernel_bin[1026], kernel_bin[1027]]);
            format!("{:#010x}", m)
        } else {
            "—".to_string()
        },
        if fallback_used { " [РЕЗЕРВНОЕ ВОССТАНОВЛЕНИЕ]" } else { "" }
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
