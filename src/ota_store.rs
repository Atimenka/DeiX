// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// ota_store — ХРАНИЛИЩЕ СКАЧАННЫХ OTA-ПАКЕТОВ в разделе /OTA (LBA 17664).
// Простой файловый формат поверх сырых секторов:
//   сектор 0: маркер "DEIXOTA1" + u32 count + записи (name[16] + off + size);
//   далее   : данные файлов.
// Раздел /OTA — ext2 в разметке (для host-инструментов), ядро работает с ним
// напрямую как с сырым хранилищем (как с /TPM).
// no_std-совместимо: alloc (Vec, String), вывод — crate::println!.


use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::ata;

/// LBA начала раздела /OTA (см. partition_map::PARTITION_LAYOUT).
/// ВАЖНО: раньше здесь стояло 16128+4352 — это устаревшие значения от
/// старой разметки; запись OTA-пакета затирала EROFS-разделы /vendor_boot,
/// /boot_a, /boot_b, /super, /dsm и /recovery (ломала цепочку загрузки).
/// Правильное согласование (правило «карта разделов в трёх местах»):
///   partition_map.rs = 17664+2816, tools/make_deix_fs.py PRIMARY = то же.
pub const OTA_PART_LBA: u32 = 17664;
pub const OTA_PART_SECTORS: u32 = 2816;
pub const OTA_MARKER: [u8; 8] = *b"DEIXOTA1";

/// Записывает файл в /OTA (перезаписывает раздел целиком).
pub fn store_file(name: &str, data: &[u8]) -> Result<(), String> {
    let mut sector = [0u8; 512];
    sector[..8].copy_from_slice(&OTA_MARKER);
    sector[8..12].copy_from_slice(&1u32.to_le_bytes());
    let nb = name.as_bytes();
    let n = nb.len().min(15);
    sector[12..12 + n].copy_from_slice(&nb[..n]);
    // данные начинаются со 2-го сектора.
    let data_off: u32 = 512; // байт от начала раздела
    sector[28..32].copy_from_slice(&data_off.to_le_bytes());
    sector[32..36].copy_from_slice(&(data.len() as u32).to_le_bytes());
    ata::write_sectors(OTA_PART_LBA, 1, &sector)
        .map_err(|_| "OTA: не удалось записать маркер".to_string())?;

    // Пишем данные по секторам.
    let mut off = 0usize;
    let mut lba = OTA_PART_LBA + 1;
    while off < data.len() {
        let mut buf = [0u8; 512];
        let take = (data.len() - off).min(512);
        buf[..take].copy_from_slice(&data[off..off + take]);
        if lba >= OTA_PART_LBA + OTA_PART_SECTORS {
            return Err("OTA: раздел переполнен".into());
        }
        ata::write_sectors(lba, 1, &buf).map_err(|_| "OTA: запись данных".to_string())?;
        lba += 1;
        off += take;
    }
    Ok(())
}

/// Читает файл из /OTA по имени. Возвращает байты или Err.
pub fn load_file(name: &str) -> Result<Vec<u8>, String> {
    let mut sector = [0u8; 512];
    if ata::read_sectors(OTA_PART_LBA, 1, &mut sector).is_err() {
        return Err("OTA: нет раздела".into());
    }
    if &sector[..8] != &OTA_MARKER {
        return Err("OTA: раздел пуст".into());
    }
    let count = u32::from_le_bytes([sector[8], sector[9], sector[10], sector[11]]) as usize;
    for i in 0..count {
        let e = 12 + i * 20;
        let mut fname = String::new();
        for &b in &sector[e..e + 16] {
            if b == 0 { break; }
            fname.push(b as char);
        }
        let off = u32::from_le_bytes([sector[e + 16], sector[e + 17], sector[e + 18], sector[e + 19]]);
        let size = u32::from_le_bytes([sector[e + 20], sector[e + 21], sector[e + 22], sector[e + 23]]);
        if fname == name {
            let mut out: Vec<u8> = Vec::with_capacity(size as usize);
            let mut lba = OTA_PART_LBA + off / 512;
            let mut rem = size as usize;
            while rem > 0 {
                let mut buf = [0u8; 512];
                if ata::read_sectors(lba, 1, &mut buf).is_err() {
                    return Err("OTA: чтение данных".into());
                }
                let take = rem.min(512);
                out.extend_from_slice(&buf[..take]);
                lba += 1;
                rem -= take;
            }
            return Ok(out);
        }
    }
    Err(format!("OTA: файл '{}' не найден", name))
}

/// Есть ли файл в /OTA.
pub fn has_file(name: &str) -> bool {
    load_file(name).is_ok()
}


