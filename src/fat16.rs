//! Драйвер FAT16 — настоящая рабочая файловая система поверх драйвера ATA
//! PIO (ata.rs). Реализована по официальной структуре FAT (BIOS Parameter
//! Block, таблица размещения файлов, корневой каталог с 32-байтными
//! записями) — не имитация, а совместимый формат, который сможет прочитать
//! и настоящий Linux/Windows, если примонтировать этот же образ диска.
//!
//! Ограничения ради простоты: только корневой каталог (без подкаталогов),
//! короткие имена 8.3, без длинных имён (LFN).
//!
//! Файловая система размещается в отдельной области диска, начиная с
//! фиксированного LBA (после загрузчика и ядра, с большим запасом на
//! рост) — см. FS_START_LBA. Образ диска (build.sh) дополняется нулями
//! до нужного размера, чтобы вместить весь том.

use crate::ata;
use alloc::string::String;
use alloc::vec::Vec;

pub const SECTOR_SIZE: usize = 512;

// Смещение начала тома FAT16 на диске — оставляем 2 МиБ (4096 секторов)
// после начала диска на загрузчик/ядро с большим запасом на рост.
pub const FS_START_LBA: u32 = 4096;

// Геометрия тома: 4096 секторов данных FS (после boot-сектора самого
// тома) = том занимает 8192 сектора всего (4 МиБ). Параметры подобраны
// так, чтобы FAT ровно вмещала все кластеры данных (см. подробный расчёт
// в комментариях ниже) — sectors_per_cluster = 1 для простоты (пусть и
// не оптимально по эффективности, зато код сильно проще).
const TOTAL_SECTORS: u32 = 8192;
const RESERVED_SECTORS: u32 = 1; // только boot-сектор тома
const NUM_FATS: u32 = 2;
const SECTORS_PER_FAT: u32 = 32;
const ROOT_ENTRIES: u32 = 512;
const SECTORS_PER_CLUSTER: u32 = 1;
const ROOT_DIR_SECTORS: u32 = (ROOT_ENTRIES * 32).div_ceil(SECTOR_SIZE as u32);

const FAT_AREA_LBA: u32 = FS_START_LBA + RESERVED_SECTORS;
const ROOT_DIR_LBA: u32 = FAT_AREA_LBA + NUM_FATS * SECTORS_PER_FAT;
const DATA_AREA_LBA: u32 = ROOT_DIR_LBA + ROOT_DIR_SECTORS;

const FAT_ENTRY_FREE: u16 = 0x0000;
const FAT_ENTRY_EOC: u16 = 0xFFFF; // end of chain
const FAT_ENTRY_RESERVED: u16 = 0xFFF8;

const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_VOLUME_LABEL: u8 = 0x08;

const OEM_MARKER: &[u8; 8] = b"DEIXFAT ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatError {
    DiskError,
    NotFormatted,
    FileNotFound,
    NoSpace,
    InvalidName,
    DirectoryFull,
}

pub struct FileEntry {
    pub name: String, // уже в виде "NAME.EXT" читаемом человеком
    pub size: u32,
    pub is_directory: bool,
}

fn read_sector(lba: u32) -> Result<[u8; SECTOR_SIZE], FatError> {
    let mut buf = [0u8; SECTOR_SIZE];
    ata::read_sectors(lba, 1, &mut buf).map_err(|_| FatError::DiskError)?;
    Ok(buf)
}

fn write_sector(lba: u32, data: &[u8; SECTOR_SIZE]) -> Result<(), FatError> {
    ata::write_sectors(lba, 1, data).map_err(|_| FatError::DiskError)
}

/// Проверяет, отформатирован ли уже том нашей файловой системой (ищем
/// сигнатуру 0x55AA и наш OEM-маркер в boot-секторе тома).
pub fn is_formatted() -> bool {
    match read_sector(FS_START_LBA) {
        Ok(sector) => {
            sector[510] == 0x55 && sector[511] == 0xAA && &sector[3..11] == OEM_MARKER
        }
        Err(_) => false,
    }
}

/// Форматирует том: пишет boot-сектор (с BPB), обнуляет обе FAT-таблицы
/// (с зарезервированными первыми двумя записями) и корневой каталог.
pub fn format() -> Result<(), FatError> {
    let mut boot = [0u8; SECTOR_SIZE];

    boot[0] = 0xEB;
    boot[1] = 0x3C;
    boot[2] = 0x90;
    boot[3..11].copy_from_slice(OEM_MARKER);

    boot[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes()); // bytes per sector
    boot[13] = SECTORS_PER_CLUSTER as u8;
    boot[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes());
    boot[16] = NUM_FATS as u8;
    boot[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
    boot[19..21].copy_from_slice(&(TOTAL_SECTORS as u16).to_le_bytes());
    boot[21] = 0xF8; // media descriptor: fixed disk
    boot[22..24].copy_from_slice(&(SECTORS_PER_FAT as u16).to_le_bytes());
    boot[24..26].copy_from_slice(&63u16.to_le_bytes()); // sectors per track (произвольно)
    boot[26..28].copy_from_slice(&16u16.to_le_bytes()); // heads (произвольно)
    boot[28..32].copy_from_slice(&0u32.to_le_bytes()); // hidden sectors

    boot[36] = 0x80; // physical drive number
    boot[38] = 0x29; // extended boot signature
    boot[39..43].copy_from_slice(&0x12345678u32.to_le_bytes()); // volume serial
    boot[43..54].copy_from_slice(b"DEIX VOL   "); // volume label (11 байт)
    boot[54..62].copy_from_slice(b"FAT16   ");

    boot[510] = 0x55;
    boot[511] = 0xAA;

    write_sector(FS_START_LBA, &boot)?;

    // Обнуляем обе копии FAT, кроме первых двух зарезервированных записей.
    let mut fat_sector0 = [0u8; SECTOR_SIZE];
    fat_sector0[0] = 0xF8; // копия media descriptor в младшем байте FAT[0]
    fat_sector0[1] = 0xFF;
    fat_sector0[2] = 0xFF; // FAT[1] = 0xFFFF (зарезервировано)
    fat_sector0[3] = 0xFF;

    for fat_index in 0..NUM_FATS {
        let fat_base = FAT_AREA_LBA + fat_index * SECTORS_PER_FAT;
        write_sector(fat_base, &fat_sector0)?;
        let zero = [0u8; SECTOR_SIZE];
        for s in 1..SECTORS_PER_FAT {
            write_sector(fat_base + s, &zero)?;
        }
    }

    // Обнуляем корневой каталог целиком (все записи свободны).
    let zero = [0u8; SECTOR_SIZE];
    for s in 0..ROOT_DIR_SECTORS {
        write_sector(ROOT_DIR_LBA + s, &zero)?;
    }

    Ok(())
}

fn read_fat_entry(cluster: u16) -> Result<u16, FatError> {
    let byte_offset = cluster as u32 * 2;
    let sector = FAT_AREA_LBA + byte_offset / SECTOR_SIZE as u32;
    let offset_in_sector = (byte_offset % SECTOR_SIZE as u32) as usize;
    let data = read_sector(sector)?;
    Ok(u16::from_le_bytes([data[offset_in_sector], data[offset_in_sector + 1]]))
}

fn write_fat_entry(cluster: u16, value: u16) -> Result<(), FatError> {
    let byte_offset = cluster as u32 * 2;
    let sector_offset = byte_offset / SECTOR_SIZE as u32;
    let offset_in_sector = (byte_offset % SECTOR_SIZE as u32) as usize;

    // Пишем в обе копии FAT — это обычная практика избыточности FAT16.
    for fat_index in 0..NUM_FATS {
        let sector = FAT_AREA_LBA + fat_index * SECTORS_PER_FAT + sector_offset;
        let mut data = read_sector(sector)?;
        data[offset_in_sector..offset_in_sector + 2].copy_from_slice(&value.to_le_bytes());
        write_sector(sector, &data)?;
    }
    Ok(())
}

fn find_free_cluster() -> Result<u16, FatError> {
    let total_clusters = (TOTAL_SECTORS - RESERVED_SECTORS - NUM_FATS * SECTORS_PER_FAT - ROOT_DIR_SECTORS) / SECTORS_PER_CLUSTER;
    for cluster in 2..(2 + total_clusters as u16) {
        if read_fat_entry(cluster)? == FAT_ENTRY_FREE {
            return Ok(cluster);
        }
    }
    Err(FatError::NoSpace)
}

fn cluster_to_lba(cluster: u16) -> u32 {
    DATA_AREA_LBA + (cluster as u32 - 2) * SECTORS_PER_CLUSTER
}

/// Преобразует человекочитаемое "name.ext" в формат 8.3 (11 байт, дополнено
/// пробелами, в верхнем регистре) — то, как FAT хранит короткие имена.
fn to_short_name(name: &str) -> Result<[u8; 11], FatError> {
    let mut result = [b' '; 11];
    let (base, ext) = match name.rfind('.') {
        Some(pos) => (&name[..pos], &name[pos + 1..]),
        None => (name, ""),
    };

    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return Err(FatError::InvalidName);
    }

    for (i, ch) in base.bytes().enumerate() {
        result[i] = ch.to_ascii_uppercase();
    }
    for (i, ch) in ext.bytes().enumerate() {
        result[8 + i] = ch.to_ascii_uppercase();
    }

    Ok(result)
}

fn from_short_name(raw: &[u8; 11]) -> String {
    let base = core::str::from_utf8(&raw[0..8]).unwrap_or("").trim_end();
    let ext = core::str::from_utf8(&raw[8..11]).unwrap_or("").trim_end();
    if ext.is_empty() {
        String::from(base)
    } else {
        alloc::format!("{}.{}", base, ext)
    }
}

/// Возвращает список файлов и каталогов в корневом каталоге.
pub fn list_root() -> Result<Vec<FileEntry>, FatError> {
    if !is_formatted() {
        return Err(FatError::NotFormatted);
    }

    let mut entries = Vec::new();

    for s in 0..ROOT_DIR_SECTORS {
        let sector = read_sector(ROOT_DIR_LBA + s)?;
        for chunk in sector.chunks_exact(32) {
            let first_byte = chunk[0];
            if first_byte == 0x00 {
                // 0x00 = конец каталога, дальше записей нет вообще
                return Ok(entries);
            }
            if first_byte == 0xE5 {
                continue; // удалённая запись
            }
            let attr = chunk[11];
            if attr & ATTR_VOLUME_LABEL != 0 {
                continue; // метка тома — не показываем как файл
            }

            let mut name_raw = [0u8; 11];
            name_raw.copy_from_slice(&chunk[0..11]);
            let size = u32::from_le_bytes(chunk[28..32].try_into().unwrap());

            entries.push(FileEntry {
                name: from_short_name(&name_raw),
                size,
                is_directory: attr & ATTR_DIRECTORY != 0,
            });
        }
    }

    Ok(entries)
}

/// Читает содержимое файла целиком в память.
pub fn read_file(name: &str) -> Result<Vec<u8>, FatError> {
    if !is_formatted() {
        return Err(FatError::NotFormatted);
    }
    let short_name = to_short_name(name)?;

    for s in 0..ROOT_DIR_SECTORS {
        let sector = read_sector(ROOT_DIR_LBA + s)?;
        for chunk in sector.chunks_exact(32) {
            if chunk[0] == 0x00 {
                return Err(FatError::FileNotFound);
            }
            if chunk[0] == 0xE5 {
                continue;
            }
            if chunk[0..11] == short_name {
                let mut cluster = u16::from_le_bytes([chunk[26], chunk[27]]);
                let size = u32::from_le_bytes(chunk[28..32].try_into().unwrap()) as usize;

                let mut data = Vec::with_capacity(size);
                while cluster < FAT_ENTRY_RESERVED && cluster != FAT_ENTRY_FREE {
                    let lba = cluster_to_lba(cluster);
                    let sector_data = read_sector(lba)?;
                    let remaining = size - data.len();
                    let take = remaining.min(SECTOR_SIZE);
                    data.extend_from_slice(&sector_data[..take]);

                    if data.len() >= size {
                        break;
                    }
                    cluster = read_fat_entry(cluster)?;
                }

                return Ok(data);
            }
        }
    }

    Err(FatError::FileNotFound)
}

/// Создаёт (или перезаписывает) файл с заданным содержимым в корневом каталоге.
pub fn write_file(name: &str, data: &[u8]) -> Result<(), FatError> {
    if !is_formatted() {
        return Err(FatError::NotFormatted);
    }
    let short_name = to_short_name(name)?;

    // Сначала проверяем, нет ли уже файла с таким именем — если есть,
    // освобождаем его старую цепочку кластеров перед перезаписью.
    'outer: for s in 0..ROOT_DIR_SECTORS {
        let sector = read_sector(ROOT_DIR_LBA + s)?;
        for (chunk_index, chunk) in sector.chunks_exact(32).enumerate() {
            if chunk[0] == 0x00 {
                break 'outer;
            }
            if chunk[0..11] == short_name {
                let mut cluster = u16::from_le_bytes([chunk[26], chunk[27]]);
                while cluster < FAT_ENTRY_RESERVED && cluster != FAT_ENTRY_FREE {
                    let next = read_fat_entry(cluster)?;
                    write_fat_entry(cluster, FAT_ENTRY_FREE)?;
                    cluster = next;
                }
                let _ = chunk_index;
                break 'outer;
            }
        }
    }

    // Выделяем цепочку кластеров под новые данные.
    let clusters_needed = data.len().div_ceil(SECTOR_SIZE * SECTORS_PER_CLUSTER as usize).max(1);
    let mut allocated = Vec::with_capacity(clusters_needed);
    for _ in 0..clusters_needed {
        let cluster = find_free_cluster()?;
        write_fat_entry(cluster, FAT_ENTRY_EOC)?; // временно помечаем как конец, свяжем ниже
        allocated.push(cluster);
    }
    for pair in allocated.windows(2) {
        write_fat_entry(pair[0], pair[1])?;
    }

    // Записываем сами данные по кластерам.
    for (i, &cluster) in allocated.iter().enumerate() {
        let lba = cluster_to_lba(cluster);
        let mut sector_buf = [0u8; SECTOR_SIZE];
        let start = i * SECTOR_SIZE;
        let end = (start + SECTOR_SIZE).min(data.len());
        sector_buf[..end - start].copy_from_slice(&data[start..end]);
        write_sector(lba, &sector_buf)?;
    }

    let first_cluster = allocated.first().copied().unwrap_or(0);

    // Ищем существующую (для перезаписи) или свободную запись каталога.
    for s in 0..ROOT_DIR_SECTORS {
        let mut sector = read_sector(ROOT_DIR_LBA + s)?;
        for (chunk_index, chunk) in sector.chunks_exact_mut(32).enumerate() {
            let is_match = chunk[0..11] == short_name;
            let is_free = chunk[0] == 0x00 || chunk[0] == 0xE5;
            if is_match || is_free {
                chunk[0..11].copy_from_slice(&short_name);
                chunk[11] = 0; // attr: обычный файл
                chunk[12] = 0;
                chunk[13] = 0;
                chunk[14..16].copy_from_slice(&0u16.to_le_bytes());
                chunk[16..18].copy_from_slice(&0u16.to_le_bytes());
                chunk[18..20].copy_from_slice(&0u16.to_le_bytes());
                chunk[20..22].copy_from_slice(&0u16.to_le_bytes()); // cluster high (0 для FAT16)
                chunk[22..24].copy_from_slice(&0u16.to_le_bytes());
                chunk[24..26].copy_from_slice(&0u16.to_le_bytes());
                chunk[26..28].copy_from_slice(&first_cluster.to_le_bytes());
                chunk[28..32].copy_from_slice(&(data.len() as u32).to_le_bytes());

                let _ = chunk_index;
                write_sector(ROOT_DIR_LBA + s, &sector)?;
                return Ok(());
            }
        }
        let _ = &mut sector;
    }

    Err(FatError::DirectoryFull)
}

/// Удаляет файл: освобождает всю его цепочку кластеров в FAT и помечает
/// запись каталога как удалённую (0xE5 — стандартный маркер FAT).
pub fn delete_file(name: &str) -> Result<(), FatError> {
    if !is_formatted() {
        return Err(FatError::NotFormatted);
    }
    let short_name = to_short_name(name)?;

    for s in 0..ROOT_DIR_SECTORS {
        let mut sector = read_sector(ROOT_DIR_LBA + s)?;
        for chunk in sector.chunks_exact_mut(32) {
            if chunk[0] == 0x00 {
                return Err(FatError::FileNotFound);
            }
            if chunk[0] == 0xE5 {
                continue;
            }
            if chunk[0..11] == short_name {
                let mut cluster = u16::from_le_bytes([chunk[26], chunk[27]]);
                while cluster < FAT_ENTRY_RESERVED && cluster != FAT_ENTRY_FREE {
                    let next = read_fat_entry(cluster)?;
                    write_fat_entry(cluster, FAT_ENTRY_FREE)?;
                    cluster = next;
                }
                chunk[0] = 0xE5;
                write_sector(ROOT_DIR_LBA + s, &sector)?;
                return Ok(());
            }
        }
    }

    Err(FatError::FileNotFound)
}
