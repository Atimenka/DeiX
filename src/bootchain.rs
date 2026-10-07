//! bootchain — проверка системного образа после старта ядра.
//!
//! Само ядро к этому моменту уже исполняется: MBR (boot_sector) загрузил
//! stage2, stage2 — ramboot и сырой kernel.bin из служебной области диска
//! (LBA 1..=2046). Этот модуль НЕ загрузчик: он проверяет, что системный
//! раздел /system (EROFS) цел и что /system/kernel/kernel.bin совпадает
//! с реально загруженным сырым ядром (по размеру и SHA-256 из дескриптора
//! DEIXKIMG, записанного build.sh в сектор LBA 2047).
//!
//! Расхождение образов — DX-KRN-0013 (KERNEL_IMAGE_MISMATCH).

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::ata;
use crate::partition_map::{
    lookup_layout, PartitionLayout, KERNEL_IMAGE_INFO_LBA, KERNEL_IMAGE_INFO_MAGIC,
};

/// Читает весь раздел с диска напрямую в результирующий вектор без лишних аллокаций.
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

/// Дескриптор образа ядра из сектора LBA 2047 (пишется build.sh).
struct KernelImageInfo {
    /// LBA первого сектора сырого kernel.bin.
    lba: u32,
    /// Точный размер kernel.bin в байтах.
    size: u32,
    /// SHA-256 файла kernel.bin, вычисленный при сборке.
    sha256: [u8; 32],
}

/// Читает и разбирает дескриптор DEIXKIMG. `Ok(None)` — дескриптора нет
/// (носитель собран старой версией build.sh).
fn read_kernel_image_info() -> Result<Option<KernelImageInfo>, String> {
    let mut sector = [0u8; 512];
    ata::read_sectors(KERNEL_IMAGE_INFO_LBA, 1, &mut sector)
        .map_err(|_| String::from("сектор дескриптора ядра не прочитан"))?;

    if sector[0..8] != KERNEL_IMAGE_INFO_MAGIC {
        return Ok(None);
    }

    let lba = u32::from_le_bytes([sector[8], sector[9], sector[10], sector[11]]);
    let size = u32::from_le_bytes([sector[12], sector[13], sector[14], sector[15]]);
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(&sector[16..48]);

    // Границы: сырое ядро лежит в служебной области до дескриптора.
    let sectors = size.div_ceil(512);
    if size == 0 || lba == 0 || lba + sectors > KERNEL_IMAGE_INFO_LBA {
        return Err(format!(
            "дескриптор ядра некорректен (LBA {}, {} байт)",
            lba, size
        ));
    }

    Ok(Some(KernelImageInfo { lba, size, sha256 }))
}

/// Читает сырой kernel.bin из служебной области по данным дескриптора.
fn read_raw_kernel(info: &KernelImageInfo) -> Result<Vec<u8>, String> {
    let sectors = info.size.div_ceil(512);
    let mut buf: Vec<u8> = vec![0u8; (sectors as usize) * 512];
    let mut cur = 0u32;
    let mut left = sectors;
    while left > 0 {
        let batch = left.min(128) as u8;
        let start = (cur as usize) * 512;
        let end = start + (batch as usize) * 512;
        ata::read_sectors(info.lba + cur, batch, &mut buf[start..end])
            .map_err(|_| String::from("сырое ядро не прочитано"))?;
        left -= batch as u32;
        cur += batch as u32;
    }
    buf.truncate(info.size as usize);
    Ok(buf)
}

fn hex8(digest: &[u8; 32]) -> String {
    let mut s = String::new();
    for b in digest.iter().take(4) {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Проверяет системный раздел и совпадение образов ядра.
/// Вызывается из kernel_main ПОСЛЕ инициализации ATA/heap, ДО экрана входа.
pub fn run_boot_chain() -> Result<String, String> {
    let mut out = String::new();
    out.push_str("  [bootchain] Проверка системного образа (/system, EROFS):\n");

    // 1. /system/kernel/kernel.bin существует и читается из EROFS.
    let erofs_kernel = crate::vfs::read_file("/system/kernel/kernel.bin")
        .map_err(|e| format!("system: /system/kernel/kernel.bin: {}", e.message()))?;
    out.push_str(&format!(
        "    /system/kernel/kernel.bin: {} байт\n",
        erofs_kernel.len()
    ));

    // 2. Сверка с сырым ядром, которое реально загрузил stage2.
    match read_kernel_image_info() {
        Ok(Some(info)) => {
            let raw = read_raw_kernel(&info)?;
            let raw_hash = crate::crypto::sha256::sha256(&raw);
            let erofs_hash = crate::crypto::sha256::sha256(&erofs_kernel);

            if raw_hash != info.sha256 {
                let msg = format!(
                    "KERNEL_IMAGE_MISMATCH: сырое ядро (LBA {}) не совпадает с дескриптором сборки",
                    info.lba
                );
                crate::diag::critical_with(
                    crate::diag::ErrorCode::new(crate::diag::Subsystem::Kernel, 13),
                    crate::diag::Action::DegradeSubsystem,
                    &msg,
                );
                return Err(msg);
            }

            if erofs_kernel.len() != info.size as usize || erofs_hash != info.sha256 {
                let msg = format!(
                    "KERNEL_IMAGE_MISMATCH: /system/kernel/kernel.bin ({} байт, sha256 {}…) не совпадает с загруженным ядром ({} байт, sha256 {}…)",
                    erofs_kernel.len(),
                    hex8(&erofs_hash),
                    info.size,
                    hex8(&raw_hash)
                );
                crate::diag::critical_with(
                    crate::diag::ErrorCode::new(crate::diag::Subsystem::Kernel, 13),
                    crate::diag::Action::DegradeSubsystem,
                    &msg,
                );
                return Err(msg);
            }

            out.push_str(&format!(
                "    образ ядра подтверждён: {} байт, sha256 {}… (raw LBA {} == /system)\n",
                info.size,
                hex8(&info.sha256),
                info.lba
            ));
        }
        Ok(None) => {
            out.push_str(
                "    дескриптор DEIXKIMG отсутствует — сверка raw/EROFS пропущена (старый образ)\n",
            );
        }
        Err(e) => {
            crate::diag::error(
                crate::diag::ErrorCode::new(crate::diag::Subsystem::Kernel, 13),
                &e,
            );
            out.push_str(&format!("    сверка образов ядра не выполнена: {}\n", e));
        }
    }

    out.push_str("  [bootchain] Системный образ проверен.\n");
    Ok(out)
}
