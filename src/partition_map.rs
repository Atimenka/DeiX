// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// partition_map — глобальная карта разделов (7x erofs/ro + userdata ext4/rw)
// и её верификация при старте ядра.
// no_std-совместимо (ядро DeiX OS): только core/alloc (BTreeMap, String, Vec),
// вывод — через crate::println!/crate::print! (стиль dxinit.rs).


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

/// Скрытый раздел TPM (/TPM). НЕ входит в PARTITION_MAP — он не монтируется
/// в Ring 3 и не перечисляется пользователю. Пароли и ключи живут в самом
/// TPM (NV-хранилище, см. tpm.rs); раздел недоступен на чтение/запись из
/// пользовательского пространства, стирание невозможно (writable=false).
pub const TPM_PARTITION_NAME: &str = "/TPM";

/// + /userdata ext4/rw; /system — логический раздел внутри /super).
pub const PARTITION_MAP: [PartitionPolicy; 8] = [
PartitionPolicy {
    name: "/kernel",
    fs: "erofs",
    default_mode: "ro",
    ring: "Ring 0",
    description: "сэндвич ядра: kernel.tar.gz -> kernel.img (EROFS 0xE0F5E1E2)",
},
PartitionPolicy {
    name: "/init_boot",
    fs: "erofs",
    default_mode: "ro",
    ring: "Ring 0",
    description: "микроядро, структуры PID 1, скрипт init.deix",
},
PartitionPolicy {
    name: "/boot",
    fs: "erofs",
    default_mode: "ro",
    ring: "Ring 0",
    description: "таблицы параметров ядра и корневой ramdisk",
},
PartitionPolicy {
    name: "/vendor_boot",
    fs: "erofs",
    default_mode: "ro",
    ring: "Ring 0",
    description: "HAL и прошивка вендора",
},
PartitionPolicy {
    name: "/super",
    fs: "erofs",
    default_mode: "ro",
    ring: "Ring 0",
    description: "контейнер динамических разделов system/vendor/product",
},
PartitionPolicy {
    name: "/recovery",
    fs: "erofs",
    default_mode: "ro",
    ring: "Ring 0",
    description: "изолированная среда восстановления TWRP/OrangeFox",
},
PartitionPolicy {
    name: "/userdata",
    fs: "ext4",
    default_mode: "rw",
    ring: "Ring 3",
    description: "единственный пользовательский раздел данных",
},
PartitionPolicy {
    name: "/system",
    fs: "erofs",
    default_mode: "ro",
    ring: "Ring 0",
    description: "логический системный раздел внутри /super",
},
];

/// ВАЛИДАЦИЯ КАРТЫ РАЗДЕЛОВ: каждый системный раздел обязан быть erofs + ro,
/// /userdata — ext4 + rw. Возвращает Ok(()) при непротиворечивой политике,
/// Err(список нарушений) — при ошибке конфигурации. Развёрнутый match
/// исключает обход правил (стиль Vault).
pub fn validate_partition_map() -> Result<(), Vec<String>> {
let mut violations: Vec<String> = Vec::new();

for policy in PARTITION_MAP.iter() {
    let ok: bool = match policy.name {
        "/userdata" => policy.fs == "ext4" && policy.default_mode == "rw",
        _ => policy.fs == "erofs" && policy.default_mode == "ro",
    };
    match ok {
        true => {}
        false => {
            violations.push(format!(
                "раздел {} нарушает политику (fs={}, mode={})",
                policy.name, policy.fs, policy.default_mode
            ));
        }
    }
}

match violations.is_empty() {
    true => Ok(()),
    false => Err(violations),
}
}

/// СТАРТОВАЯ ПРОВЕРКА КАРТЫ РАЗДЕЛОВ с выводом в консоль ядра.
/// Вызывается из kernel_main() на стадии init_boot (Ring 0).
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
            crate::println!(
                "  [partition_map] Политика разделов подтверждена: 7x erofs/ro + userdata ext4/rw"
            );
            crate::println!(
                "  [partition_map] Скрытый раздел {}: недоступен Ring 3, пароли — только в TPM (NV)",
                TPM_PARTITION_NAME
            );
        }
        Err(violations) => {
            for violation in violations.iter() {
                crate::println!("    [ERROR] {}", violation);
            }
        }
    }
}

/// ==================== ФИЗИЧЕСКАЯ РАСКЛАДКА ДИСКА (LBA) ====================
/// Согласована с tools/make_deix_fs.py: образ 8 МиБ = 16384 сектора по 512
/// байт. Используется прошивальщиками (fastbootd, DSM/EDL) и recovery для
/// РЕАЛЬНЫХ операций чтения/записи/стирания разделов.
#[derive(Debug, Clone, Copy)]
pub struct PartitionLayout {
    pub name: &'static str,
    pub start_lba: u32,
    pub sectors: u32,
    pub fs: &'static str,
    /// Разрешена ли запись из прошивальщика (fastbootd/DSM/EDL).
    pub flashable: bool,
}

/// A/B СЛОТЫ: /kernel и /boot имеют два слота (a/b). Активный слот хранится
/// в BCB (bcb::read_slot/write_slot). OTA прошивает НЕактивный слот и
/// переключает — откат через bcb/rollback.
pub const PARTITION_LAYOUT: [PartitionLayout; 13] = [
    PartitionLayout { name: "/system",     start_lba: 4096,  sectors: 8192, fs: "ext2",  flashable: true },
    PartitionLayout { name: "/userdata",   start_lba: 12800, sectors: 512,  fs: "ext2",  flashable: true },
    PartitionLayout { name: "/kernel_a",   start_lba: 13313, sectors: 1279,  fs: "erofs", flashable: true },
    PartitionLayout { name: "/kernel_b",   start_lba: 14593, sectors: 1279,  fs: "erofs", flashable: true },
    PartitionLayout { name: "/init_boot",  start_lba: 15873, sectors: 255,  fs: "erofs", flashable: true },
    PartitionLayout { name: "/vendor_boot",start_lba: 16129, sectors: 255,  fs: "erofs", flashable: true },
    PartitionLayout { name: "/boot_a",     start_lba: 16385, sectors: 255,  fs: "erofs", flashable: true },
    PartitionLayout { name: "/boot_b",     start_lba: 16641, sectors: 255,  fs: "erofs", flashable: true },
    PartitionLayout { name: "/super",      start_lba: 16897, sectors: 1023, fs: "erofs", flashable: true },
    PartitionLayout { name: "/dsm",        start_lba: 17921, sectors: 255,  fs: "erofs", flashable: true },
    PartitionLayout { name: "/recovery",   start_lba: 18177, sectors: 255,  fs: "erofs", flashable: true },
    PartitionLayout { name: "/OTA",        start_lba: 18432, sectors: 2048, fs: "ext2",  flashable: true },
    PartitionLayout { name: "/TPM",        start_lba: 12288, sectors: 512,  fs: "tpm",   flashable: false },
];

/// Имя активного слота ядра (по BCB): "/kernel_a" или "/kernel_b".
pub fn active_kernel_layout() -> &'static PartitionLayout {
    match crate::bcb::read_slot() {
        1 => lookup_layout("/kernel_b").unwrap(),
        _ => lookup_layout("/kernel_a").unwrap(),
    }
}

/// Раздел ядра для конкретного слота: 0 = A, иначе B.
pub fn kernel_layout_for_slot(slot: u8) -> &'static PartitionLayout {
    match slot {
        0 => lookup_layout("/kernel_a").unwrap(),
        _ => lookup_layout("/kernel_b").unwrap(),
    }
}

/// Имя активного слота boot (по BCB): "/boot_a" или "/boot_b".
pub fn active_boot_layout() -> &'static PartitionLayout {
    match crate::bcb::read_slot() {
        1 => lookup_layout("/boot_b").unwrap(),
        _ => lookup_layout("/boot_a").unwrap(),
    }
}

/// НЕактивный слот ядра (куда OTA пишет новое ядро).
pub fn inactive_kernel_layout() -> &'static PartitionLayout {
    match crate::bcb::read_slot() {
        1 => lookup_layout("/kernel_a").unwrap(),
        _ => lookup_layout("/kernel_b").unwrap(),
    }
}

/// Загрузчик (stage2) лежит сразу после MBR.
pub const BOOTLOADER_LBA: u32 = 1;
pub const BOOTLOADER_SECTORS: u32 = 2048;

/// Поиск физической раскладки по имени раздела.
pub fn lookup_layout(name: &str) -> Option<&'static PartitionLayout> {
    let mut found: Option<&'static PartitionLayout> = None;
    for layout in PARTITION_LAYOUT.iter() {
        if layout.name == name {
            found = Some(layout);
            break;
        }
    }
    found
}

