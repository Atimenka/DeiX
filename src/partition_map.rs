//! partition_map — карта разделов DeiX OS (/system EROFS/ro + /userdata ext2/rw)
//! и её верификация при старте ядра.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub struct PartitionPolicy {
    pub name: &'static str,
    pub fs: &'static str,
    pub default_mode: &'static str,
    pub ring: &'static str,
    pub description: &'static str,
}

pub const PARTITION_MAP: [PartitionPolicy; 2] = [
    PartitionPolicy {
        name: "/system",
        fs: "erofs",
        default_mode: "ro",
        ring: "Ring 0",
        description: "системный раздел EROFS: ядро, модули, библиотеки",
    },
    PartitionPolicy {
        name: "/userdata",
        fs: "ext2",
        default_mode: "rw",
        ring: "Ring 3",
        description: "пользовательские данные, аккаунты, конфигурация",
    },
];

pub fn validate_partition_map() -> Result<(), Vec<String>> {
    let mut violations: Vec<String> = Vec::new();

    for policy in PARTITION_MAP.iter() {
        let ok: bool = match policy.name {
            "/userdata" => policy.fs == "ext2" && policy.default_mode == "rw",
            _ => policy.fs == "erofs" && policy.default_mode == "ro",
        };
        if !ok {
            violations.push(format!(
                "раздел {} нарушает политику (fs={}, mode={})",
                policy.name, policy.fs, policy.default_mode
            ));
        }
    }

    // Геометрия: служебные области и разделы не должны пересекаться.
    let boot_end = BOOTLOADER_LBA + BOOTLOADER_SECTORS; // первый LBA после загрузчика
    if boot_end > KERNEL_IMAGE_INFO_LBA {
        violations.push(format!(
            "область загрузчика (до LBA {}) пересекает дескриптор ядра (LBA {})",
            boot_end - 1, KERNEL_IMAGE_INFO_LBA
        ));
    }
    let panic_start = crate::diag::panic::PANIC_LBA;
    let panic_end = panic_start + crate::diag::panic::PANIC_SECTORS;
    if KERNEL_IMAGE_INFO_LBA >= panic_start {
        violations.push(format!(
            "дескриптор ядра (LBA {}) попадает в область аварийного дампа ({}..{})",
            KERNEL_IMAGE_INFO_LBA, panic_start, panic_end - 1
        ));
    }
    if let Some(system) = lookup_layout("/system") {
        if panic_end > system.start_lba {
            violations.push(format!(
                "область аварийного дампа ({}..{}) пересекает /system (LBA {})",
                panic_start, panic_end - 1, system.start_lba
            ));
        }
        if let Some(userdata) = lookup_layout("/userdata") {
            if system.start_lba + system.sectors > userdata.start_lba {
                violations.push(format!(
                    "/system ({}+{}) пересекает /userdata (LBA {})",
                    system.start_lba, system.sectors, userdata.start_lba
                ));
            }
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

pub fn validate_partition_map_report() {
    crate::println!("  [partition_map] Карта разделов DeiX OS:");
    for policy in PARTITION_MAP.iter() {
        crate::println!(
            "    {:<12} {:>5} {:<2}  {:<7}  {}",
            policy.name,
            policy.fs,
            policy.default_mode,
            policy.ring,
            policy.description
        );
    }
    match validate_partition_map() {
        Ok(()) => {
            crate::println!("  [partition_map] Политика разделов подтверждена: /system erofs/ro + /userdata ext2/rw");
        }
        Err(violations) => {
            for violation in violations.iter() {
                crate::println!("    [ERROR] {}", violation);
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PartitionLayout {
    pub name: &'static str,
    pub start_lba: u32,
    pub sectors: u32,
    pub fs: &'static str,
    pub flashable: bool,
}

pub const PARTITION_LAYOUT: [PartitionLayout; 2] = [
    PartitionLayout { name: "/system",     start_lba: 4096,  sectors: 8704, fs: "erofs", flashable: true },
    PartitionLayout { name: "/userdata",   start_lba: 12800, sectors: 5632, fs: "ext2",  flashable: true },
];

/// Сырые служебные области до первого раздела (/system @ LBA 4096):
///   LBA 0            — MBR (boot_sector.bin)
///   LBA 1..=2046     — stage2 + ramboot + сырой kernel.bin
///   LBA 2047         — дескриптор образа ядра (DEIXKIMG: LBA/размер/SHA-256)
///   LBA 2048..=2111  — область аварийного дампа (DEIXPNIC, см. diag/panic.rs)
pub const BOOTLOADER_LBA: u32 = 1;
pub const BOOTLOADER_SECTORS: u32 = 2046;

/// Сектор с дескриптором образа ядра, записывается build.sh.
pub const KERNEL_IMAGE_INFO_LBA: u32 = 2047;
/// Магическая подпись дескриптора образа ядра.
pub const KERNEL_IMAGE_INFO_MAGIC: [u8; 8] = *b"DEIXKIMG";

pub fn lookup_layout(name: &str) -> Option<&'static PartitionLayout> {
    for layout in PARTITION_LAYOUT.iter() {
        if layout.name == name {
            return Some(layout);
        }
    }
    None
}
