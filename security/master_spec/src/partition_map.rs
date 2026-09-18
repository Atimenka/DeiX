// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// Модуль ГЛОБАЛЬНОЙ КАРТЫ РАЗДЕЛОВ DeiX OS. Определяет политику каждого
// раздела: драйвер ФС, режим доступа по умолчанию, кольцо защиты и роль.
// Политика жёсткая: все системные разделы (/kernel, /init_boot, /boot,
// /vendor_boot, /super, /system, /recovery) — erofs/ro в Ring 0; единственный
// пользовательский раздел /userdata — ext4/rw в Ring 3. Функция
// validate_partition_map() проверяет непротиворечивость карты при старте.
// no_std-совместимо: только константы и &'static str.

/// Политика раздела: имя, ФС, режим по умолчанию, кольцо доступа, роль.
pub struct PartitionPolicy {
    pub name: &'static str,
    pub fs: &'static str,
    pub default_mode: &'static str,
    pub ring: &'static str,
    pub description: &'static str,
}

/// Глобальная карта разделов DeiX OS (строго по ТЗ: 7 системных erofs/ro
/// + /userdata ext4/rw; /system — логический раздел внутри /super).
pub const PARTITION_MAP: [PartitionPolicy; 8] = [
    PartitionPolicy {
        name: "/kernel",
        fs: "erofs",
        default_mode: "ro",
        ring: "Ring 0",
        description: "сэндвич ядра: kernel.tar.gz -> kernel.img (магия 0xE0F5E0F5)",
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
