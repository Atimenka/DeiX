// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// recovery_flash_engine — TWRP/OrangeFox (nandroid FNV-1a),
// Fastbootd (атомарная прошивка с откатом), EDL (raw flash, unbrick).
// no_std-совместимо (ядро DeiX OS): только core/alloc (BTreeMap, String, Vec),
// вывод — через crate::println!/crate::print! (стиль dxinit.rs).


use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use alloc::vec;

/// Строго типизированное перечисление ошибок восстановления/прошивки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlashError {
    PartitionUnknown { partition: String },
    ImageInvalid { partition: String, detail: String },
    TargetLocked { partition: String },
    AtomicCommitFailed { partition: String },
    RawDeviceUnavailable { device: String },
    OffsetOutOfRange { device: String, offset: u64 },
    ReadbackMismatch { device: String, offset: u64 },
    NoBackupEntry { partition: String },
    /// Ошибка ввода-вывода при реальном доступе к разделу.
    DiskIo { detail: String },
}

impl FlashError {
    pub fn message(&self) -> String {
        match self {
            FlashError::PartitionUnknown { partition } => {
                format!("неизвестный раздел '{}'", partition)
            }
            FlashError::ImageInvalid { partition, detail } => {
                format!("образ для '{}' некорректен: {}", partition, detail)
            }
            FlashError::TargetLocked { partition } => {
                format!("раздел '{}' заблокирован от записи", partition)
            }
            FlashError::AtomicCommitFailed { partition } => {
                format!("атомарный коммит для '{}' не удался", partition)
            }
            FlashError::RawDeviceUnavailable { device } => {
                format!("сырое устройство '{}' недоступно", device)
            }
            FlashError::OffsetOutOfRange { device, offset } => {
                format!("смещение {} вне границ устройства '{}'", offset, device)
            }
            FlashError::ReadbackMismatch { device, offset } => {
                format!("readback-контроль для '{}' @ {} не совпал", device, offset)
            }
            FlashError::NoBackupEntry { partition } => {
                format!("нет резервной копии раздела '{}'", partition)
            }
            FlashError::DiskIo { detail } => format!("ошибка диска: {}", detail),
        }
    }
}

/// Запись nandroid-бэкапа: раздел, размер, 64-битный FNV-1a дайджест.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NandroidBackupEntry {
    pub partition: String,
    pub size_bytes: usize,
    pub digest: u64,
}

/// Вычисление FNV-1a 64 (реальный хэш, ноль внешних крейтов).
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325u64;
    for byte in data.iter() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3u64);
    }
    hash
}









// ==================== РЕАЛЬНЫЙ ДОСТУП К РАЗДЕЛАМ (физическая карта) ====================
// Операции прошивальщиков/recovery выполняются НАД НАСТОЯЩИМ ДИСКОМ через
// ATA по карте разделов (partition_map::PARTITION_LAYOUT), а не над
// «моделью в памяти» — это полноценная прошивка, стирание и резервное
// копирование.

/// Читает `count` секторов раздела со смещением `off_sect` (реальный диск).
pub fn disk_read_partition(name: &str, off_sect: u32, count: u32) -> Result<Vec<u8>, FlashError> {
    let (start, total) = resolve_layout(name)
        .ok_or_else(|| FlashError::PartitionUnknown { partition: name.to_string() })?;
    if off_sect + count > total {
        return Err(FlashError::OffsetOutOfRange {
            device: name.to_string(),
            offset: (off_sect + count) as u64,
        });
    }
    let mut out: Vec<u8> = Vec::new();
    let mut left = count;
    let mut cur = off_sect;
    while left > 0 {
        let batch = left.min(256);
        let mut buf = vec![0u8; (batch * 512) as usize];
        crate::ata::read_sectors(start + cur, batch as u8, &mut buf)
            .map_err(|_| FlashError::DiskIo { detail: format!("read {} @{}", name, cur) })?;
        out.extend_from_slice(&buf);
        left -= batch;
        cur += batch;
    }
    Ok(out)
}

/// Записывает данные в раздел со смещением (data кратен 512).
pub fn disk_write_partition(name: &str, off_sect: u32, data: &[u8]) -> Result<(), FlashError> {
    let (start, total) = resolve_layout(name)
        .ok_or_else(|| FlashError::PartitionUnknown { partition: name.to_string() })?;
    if data.len() % 512 != 0 {
        return Err(FlashError::ImageInvalid {
            partition: name.to_string(),
            detail: "данные должны быть кратны 512 байт".to_string(),
        });
    }
    let sectors = (data.len() / 512) as u32;
    if off_sect + sectors > total {
        return Err(FlashError::OffsetOutOfRange {
            device: name.to_string(),
            offset: (off_sect + sectors) as u64,
        });
    }
    let mut off_bytes = 0usize;
    let mut cur = off_sect;
    let mut left = sectors;
    while left > 0 {
        let batch = left.min(256) as usize;
        crate::ata::write_sectors(start + cur, batch as u8, &data[off_bytes..off_bytes + batch * 512])
            .map_err(|_| FlashError::DiskIo { detail: format!("write {} @{}", name, cur) })?;
        off_bytes += batch * 512;
        left -= batch as u32;
        cur += batch as u32;
    }
    Ok(())
}

/// Обнуляет весь раздел (ERASE). Возвращает число обнулённых секторов.
pub fn disk_erase_partition(name: &str) -> Result<u32, FlashError> {
    let (start, total) = resolve_layout(name)
        .ok_or_else(|| FlashError::PartitionUnknown { partition: name.to_string() })?;
    let zeros = vec![0u8; 256 * 512];
    let mut left = total;
    let mut cur = 0u32;
    while left > 0 {
        let batch = left.min(256);
        crate::ata::write_sectors(start + cur, batch as u8, &zeros[..(batch * 512) as usize])
            .map_err(|_| FlashError::DiskIo { detail: format!("erase {} @{}", name, cur) })?;
        left -= batch;
        cur += batch;
    }
    Ok(total)
}

/// Полный снимок раздела (для Nandroid-бэкапа).
pub fn disk_snapshot_partition(name: &str) -> Result<Vec<u8>, FlashError> {
    let (_, total) = resolve_layout(name)
        .ok_or_else(|| FlashError::PartitionUnknown { partition: name.to_string() })?;
    disk_read_partition(name, 0, total)
}

/// Реальная прошивка раздела: валидация (EROFS-магия для системных
/// erofs-разделов), затем побайтовая запись с контролем чтения (readback).
pub fn disk_flash_partition(name: &str, image: &[u8]) -> Result<(), FlashError> {
    let layout = match crate::partition_map::lookup_layout(name) {
        Some(l) => l,
        None => {
            return Err(FlashError::PartitionUnknown {
                partition: name.to_string(),
            })
        }
    };
    if !layout.flashable {
        return Err(FlashError::TargetLocked {
            partition: name.to_string(),
        });
    }
    let max_bytes = (layout.sectors as usize) * 512;
    if image.len() > max_bytes {
        return Err(FlashError::ImageInvalid {
            partition: name.to_string(),
            detail: format!("образ {} байт больше раздела {} байт", image.len(), max_bytes),
        });
    }
    // Валидация EROFS для системных разделов.
    if layout.fs == "erofs" {
        // Валидация НАСТОЯЩЕГО EROFS (спецификация v1, магия 0xE0F5E1E2):
        // разбираем суперблок целиком, а не сверяем 4 байта самодельной
        // сигнатуры. Битый образ в системный раздел не попадёт.
        if let Err(e) = crate::erofs::parse_superblock(image) {
            return Err(FlashError::ImageInvalid {
                partition: name.to_string(),
                detail: e.message(),
            });
        }
    }
    // Дополняем до полного размера раздела нулями и пишем.
    let mut padded = image.to_vec();
    padded.resize(max_bytes, 0);
    disk_write_partition(name, 0, &padded)?;
    // Readback-контроль: читаем первые 4 КиБ и сверяем с началом образа.
    let check = disk_read_partition(name, 0, 8)?;
    let check_len = image.len().min(check.len());
    if check[..check_len] != image[..check_len] {
        return Err(FlashError::ReadbackMismatch {
            device: name.to_string(),
            offset: 0,
        });
    }
    Ok(())
}

/// Разрешение имени раздела в (start_lba, sectors) по физической карте.
fn resolve_layout(name: &str) -> Option<(u32, u32)> {
    match crate::partition_map::lookup_layout(name) {
        Some(l) => Some((l.start_lba, l.sectors)),
        None => None,
    }
}

// ==================== NANDROID BACKUP / RESTORE НА ДИСК ====================
// Резервные копии хранятся в файлах на /system-томе (ext2 P1):
//   BKP_<part>.IMG      — побайтовый снимок раздела;
//   BKP_MANIFEST.TXT    — манифест: имя, размер, FNV-1a-дайджест, SHA-256.
// (Том 4 МиБ — хватит на 1-2 раздела; в реальной системе был бы /backup.)

/// Имя файла бэкапа для раздела.
pub fn backup_file_name(partition: &str) -> String {
    let safe = partition.trim_start_matches('/');
    alloc::format!("BKP_{}.IMG", safe)
}

/// Имя манифеста бэкапов.
pub const BACKUP_MANIFEST: &str = "BKP_MANIFEST.TXT";


/// Список доступных бэкапов (по манифесту на /system-томе).
pub fn list_backups() -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    if let Ok(manifest) = crate::ext2::read_file(BACKUP_MANIFEST) {
        let text = String::from_utf8_lossy(&manifest);
        for line in text.lines().skip(1) {
            let name = line.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                result.push(name.to_string());
            }
        }
    }
    result
}

/// Восстанавливает раздел из бэкапа (BKP_<part>.IMG).
pub fn restore_from_backup(partition: &str) -> Result<usize, FlashError> {
    let fname = backup_file_name(partition);
    let bytes = crate::ext2::read_file(&fname)
        .map_err(|_| FlashError::NoBackupEntry { partition: partition.to_string() })?;
    disk_flash_partition(partition, &bytes)?;
    Ok(bytes.len())
}

// ==================== FACTORY RESET ====================
// Сброс к заводскому состоянию: стирание /userdata, удаление USERS.DB и
// журналов из /system-тома, сброс /TPM-копии базы к заводскому маркеру.
// После этого при следующей загрузке ОС начнётся первичная настройка.

/// Заводской маркер /TPM (как делает tools/make_deix_fs.py: "DEIXTPM" + v1 + count0).
const FACTORY_TPM_MARKER: [u8; 16] = *b"DEIXTPM\x00\x01\x00\x00\x00\x00\x00\x00\x00";

pub fn factory_reset_disk() -> Result<String, FlashError> {
    let mut log = String::new();

    // 1) /userdata — обнуляем.
    let n = disk_erase_partition("/userdata")?;
    log.push_str(&alloc::format!("  /userdata: {} секторов обнулено\n", n));

    // 2) /system-том: удаляем USERS.DB и отчёт bugreport.
    for f in ["USERS.DB", crate::bugreport::BUGREPORT_FILE] {
        let _ = crate::ext2::delete_file(f);
    }
    // 2b) Сырой crash-лог (LBA 2048) стираем.
    crate::crashlog::clear_disk_crash();
    log.push_str("  /system: USERS.DB, BUGREPORT.TXT удалены; crash-лог стёрт\n");

    // 3) /TPM-копия базы — сброс к заводскому маркеру (записи базы больше нет).
    let mut sector = [0u8; 512];
    sector[..16].copy_from_slice(&FACTORY_TPM_MARKER);
    let _ = crate::ata::write_sectors(crate::tpm::TPM_PARTITION_LBA, 1, &sector);
    log.push_str(&format!(
        "  /TPM (LBA {}): резервная копия базы сброшена к заводскому маркеру\n",
        crate::tpm::TPM_PARTITION_LBA
    ));

    Ok(log)
}
