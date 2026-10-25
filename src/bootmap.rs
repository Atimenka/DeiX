//! Карта загрузки DEIXKIMG v2 — сектор LBA 2047 (формат: docs/BOOTMAP.md).
//!
//! Загрузчик (MBR → ramboot → stage2) не знает файловых систем. Файлы
//! `/system/boot/stage2.bin`, `/system/boot/ramboot.bin` и
//! `/system/kernel/kernel.bin` он находит по этой карте: для каждого файла в
//! ней записан список участков диска (LBA, секторов). На EROFS каждый файл —
//! один участок; после `sysfs convert` (ext2) файл может состоять из
//! нескольких, и карту переписывает конверсия.
//!
//! Модуль только разбирает, проверяет и собирает карту и читает файл по её
//! участкам. Запись карты на диск — дело установщика, `sysfs` и обновлений.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::ata;
use crate::partition_map::{KERNEL_IMAGE_INFO_LBA, KERNEL_IMAGE_INFO_MAGIC};

pub const VERSION: u32 = 2;

const SECTOR: usize = 512;

const OFF_KERNEL_LBA: usize = 8;
const OFF_KERNEL_SIZE: usize = 12;
const OFF_SHA256: usize = 16;
const OFF_VERSION: usize = 48;
const OFF_STAGE2_SECTORS: usize = 52;
const OFF_RAMBOOT_SECTORS: usize = 56;
const OFF_STAGE2: usize = 64;
const OFF_RAMBOOT: usize = 128;
const OFF_KERNEL: usize = 192;

/// Сколько участков карта вмещает на файл (сектор 512 байт).
pub const STAGE2_SLOTS: usize = 8;
pub const RAMBOOT_SLOTS: usize = 8;
pub const KERNEL_SLOTS: usize = 40;

/// Пределы, которые держат MBR и ramboot (память загрузчика).
const MAX_STAGE2_SECTORS: u32 = 64;
const MAX_RAMBOOT_SECTORS: u32 = 8;
const MAX_KERNEL_SECTORS: u32 = 4096;
/// Один участок: MBR и ramboot читают длину как 16-битное слово.
const MAX_EXTENT_SECTORS: u32 = 0xFFFF;

/// Непрерывный участок диска.
#[derive(Clone, Copy)]
pub struct Extent {
    pub lba: u32,
    pub sectors: u32,
}

pub struct BootMap {
    /// Размер kernel.bin в байтах (с нулевым отступом до 4096).
    pub kernel_size: u32,
    pub kernel_sha256: [u8; 32],
    pub stage2_sectors: u32,
    pub ramboot_sectors: u32,
    pub stage2: Vec<Extent>,
    pub ramboot: Vec<Extent>,
    pub kernel: Vec<Extent>,
}

fn u32_at(sec: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([sec[off], sec[off + 1], sec[off + 2], sec[off + 3]])
}

fn read_table(sec: &[u8], off: usize, slots: usize) -> Vec<Extent> {
    let mut table = Vec::new();
    for i in 0..slots {
        let sectors = u32_at(sec, off + i * 8 + 4);
        if sectors == 0 {
            break;
        }
        table.push(Extent { lba: u32_at(sec, off + i * 8), sectors });
    }
    table
}

fn write_table(sec: &mut [u8], off: usize, slots: usize, table: &[Extent]) -> Result<(), String> {
    if table.len() > slots {
        return Err(format!("файл разбит на {} участков, карта вмещает {}", table.len(), slots));
    }
    for (i, e) in table.iter().enumerate() {
        let at = off + i * 8;
        sec[at..at + 4].copy_from_slice(&e.lba.to_le_bytes());
        sec[at + 4..at + 8].copy_from_slice(&e.sectors.to_le_bytes());
    }
    Ok(())
}

impl BootMap {
    /// Проверяет, что участки лежат в `/system`, покрывают файлы и не выходят
    /// за пределы, на которые рассчитан загрузчик.
    pub fn validate(&self) -> Result<(), String> {
        let system = crate::partition_map::lookup_layout("/system")
            .ok_or_else(|| String::from("раздел /system не найден в карте разделов"))?;
        let first = system.start_lba;
        let end = system.start_lba + system.sectors;
        let kernel_sectors = self.kernel_size.div_ceil(SECTOR as u32);
        let files = [
            ("stage2", &self.stage2, self.stage2_sectors, MAX_STAGE2_SECTORS),
            ("ramboot", &self.ramboot, self.ramboot_sectors, MAX_RAMBOOT_SECTORS),
            ("kernel", &self.kernel, kernel_sectors, MAX_KERNEL_SECTORS),
        ];
        for (name, table, need, max) in files {
            if need == 0 || need > max {
                return Err(format!("{}: недопустимый размер ({} секторов)", name, need));
            }
            let mut total = 0u32;
            for e in table.iter() {
                let inside = e.lba >= first
                    && e.lba.checked_add(e.sectors).map_or(false, |last| last <= end)
                    && e.sectors <= MAX_EXTENT_SECTORS;
                if !inside {
                    return Err(format!("{}: участок LBA {}+{} вне /system", name, e.lba, e.sectors));
                }
                total += e.sectors;
            }
            if total < need {
                return Err(format!("{}: участки покрывают {} секторов из {}", name, total, need));
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<[u8; SECTOR], String> {
        self.validate()?;
        let mut sec = [0u8; SECTOR];
        sec[0..8].copy_from_slice(&KERNEL_IMAGE_INFO_MAGIC);
        sec[OFF_KERNEL_LBA..OFF_KERNEL_LBA + 4].copy_from_slice(&self.kernel[0].lba.to_le_bytes());
        sec[OFF_KERNEL_SIZE..OFF_KERNEL_SIZE + 4].copy_from_slice(&self.kernel_size.to_le_bytes());
        sec[OFF_SHA256..OFF_SHA256 + 32].copy_from_slice(&self.kernel_sha256);
        sec[OFF_VERSION..OFF_VERSION + 4].copy_from_slice(&VERSION.to_le_bytes());
        sec[OFF_STAGE2_SECTORS..OFF_STAGE2_SECTORS + 4].copy_from_slice(&self.stage2_sectors.to_le_bytes());
        sec[OFF_RAMBOOT_SECTORS..OFF_RAMBOOT_SECTORS + 4].copy_from_slice(&self.ramboot_sectors.to_le_bytes());
        write_table(&mut sec, OFF_STAGE2, STAGE2_SLOTS, &self.stage2)?;
        write_table(&mut sec, OFF_RAMBOOT, RAMBOOT_SLOTS, &self.ramboot)?;
        write_table(&mut sec, OFF_KERNEL, KERNEL_SLOTS, &self.kernel)?;
        Ok(sec)
    }
}

/// Разбирает сектор карты. `Ok(None)` — в секторе нет подписи DEIXKIMG.
pub fn parse(sec: &[u8; SECTOR]) -> Result<Option<BootMap>, String> {
    if sec[0..8] != KERNEL_IMAGE_INFO_MAGIC {
        return Ok(None);
    }
    let version = u32_at(sec, OFF_VERSION);
    if version != VERSION {
        return Err(format!("карта загрузки версии {}, ожидалась {}", version, VERSION));
    }
    let mut kernel_sha256 = [0u8; 32];
    kernel_sha256.copy_from_slice(&sec[OFF_SHA256..OFF_SHA256 + 32]);
    let map = BootMap {
        kernel_size: u32_at(sec, OFF_KERNEL_SIZE),
        kernel_sha256,
        stage2_sectors: u32_at(sec, OFF_STAGE2_SECTORS),
        ramboot_sectors: u32_at(sec, OFF_RAMBOOT_SECTORS),
        stage2: read_table(sec, OFF_STAGE2, STAGE2_SLOTS),
        ramboot: read_table(sec, OFF_RAMBOOT, RAMBOOT_SLOTS),
        kernel: read_table(sec, OFF_KERNEL, KERNEL_SLOTS),
    };
    map.validate()?;
    Ok(Some(map))
}

/// Читает карту с загрузочного диска.
pub fn read_boot_map() -> Result<Option<BootMap>, String> {
    let mut sec = [0u8; SECTOR];
    ata::read_sectors(KERNEL_IMAGE_INFO_LBA, 1, &mut sec)
        .map_err(|_| String::from("сектор карты загрузки не прочитан"))?;
    parse(&sec)
}

/// Читает `size` байт файла по списку участков. `read(lba, секторов, буфер)` —
/// чтение секторов с нужного диска.
pub fn read_extents<F>(table: &[Extent], size: usize, mut read: F) -> Result<Vec<u8>, String>
where
    F: FnMut(u32, u8, &mut [u8]) -> Result<(), ()>,
{
    let want = size.div_ceil(SECTOR);
    let mut buf = vec![0u8; want * SECTOR];
    let mut done = 0usize;
    for e in table.iter() {
        let mut off = 0u32;
        while off < e.sectors && done < want {
            let n = (e.sectors - off).min((want - done) as u32).min(128);
            let start = done * SECTOR;
            let end = start + n as usize * SECTOR;
            read(e.lba + off, n as u8, &mut buf[start..end])
                .map_err(|_| format!("сектор LBA {} не прочитан", e.lba + off))?;
            off += n;
            done += n as usize;
        }
    }
    if done < want {
        return Err(String::from("участки карты короче файла"));
    }
    buf.truncate(size);
    Ok(buf)
}
