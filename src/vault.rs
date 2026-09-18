// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// vault — KERNEL SECURITY VAULT: 7 системных разделов erofs/ro, rw только в
// Fastbootd/EDL/Recovery (иначе panic!), политика /userdata (ext4/rw).
// no_std-совместимо (ядро DeiX OS): только core/alloc (BTreeMap, String, Vec),
// вывод — через crate::println!/crate::print! (стиль dxinit.rs).


use crate::init_parser::{BootStage, MountMode};
use alloc::string::String;
use alloc::format;

/// Список критических системных разделов ядра (KERNEL SECURITY VAULT).
/// Эти семь точек монтируются исключительно через erofs и по умолчанию
/// блокируются в ReadOnly. /userdata в этот список не входит — он
/// обрабатывается отдельной веткой политики.
pub const SYSTEM_PARTITIONS: [&str; 7] = [
"/kernel",
"/init_boot",
"/boot",
"/vendor_boot",
"/super",
"/system",
"/recovery",
];


/// Отказ политики, НЕ являющийся терминальным: нарушение правил /userdata.
/// Парсер превращает его в предупреждение (ParseWarning) и продолжает разбор.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultRejection {
/// rw-монтирование /userdata вне разрешённых стадий.
UserdataPolicy(String),
/// Файловая система /userdata отличается от ext4.
NonExt4Userdata(String),
/// Попытка доступа к скрытому разделу /TPM из пользовательского кода.
TpmPartitionDenied(String),
}

impl VaultRejection {
/// Детальное описание причины отказа (для журнала ядра).
pub fn detail(&self) -> String {
    match self {
        VaultRejection::UserdataPolicy(msg) => msg.clone(),
        VaultRejection::NonExt4Userdata(msg) => msg.clone(),
        VaultRejection::TpmPartitionDenied(msg) => msg.clone(),
    }
}

/// Короткое имя категории (для лога).
pub fn kind_name(&self) -> &'static str {
    match self {
        VaultRejection::UserdataPolicy(_) => "userdata-policy-violation",
        VaultRejection::NonExt4Userdata(_) => "non-ext4-userdata",
VaultRejection::TpmPartitionDenied(_) => "tpm-partition-denied",
    }
}
}

/// ОЦЕНКА ПРАВИЛА VAULT ДЛЯ КОМАНДЫ МОНТИРОВАНИЯ.
///
/// Возвращает:
/// * `Ok(())` — команда допущена политикой к исполнению.
/// * `Err(VaultRejection)` — политика /userdata нарушена (предупреждение).
/// * `panic!` — терминальное нарушение системного раздела (правила 1-2).
///
/// Проверка построена на развёрнутых match-выражениях (отдельные ветки для
/// каждого критического пути, драйвера и режима) — исключается обход логики
/// через манипуляции с условиями (требование ТЗ).
pub fn evaluate(dst: &str, fs_type: &str, mode: MountMode, stage: BootStage) -> Result<(), VaultRejection> {
// Шаг 1: идентификация точки монтирования развёрнутым match.
let is_system: bool = match dst {
    "/kernel" => true,
    "/init_boot" => true,
    "/boot" => true,
    "/vendor_boot" => true,
    "/super" => true,
    "/system" => true,
    "/recovery" => true,
    _ => false,
};

match dst {
    // ---- СКРЫТЫЙ РАЗДЕЛ /TPM --------------------------------------------
    // Доступ запрещён ВСЕГДА: раздел принадлежит TPM, пользовательский
    // код не может его монтировать, читать или стирать. Единственный
    // легальный доступ — сам TPM-модуль (Ring 0) и EDL.
    "/TPM" => Err(VaultRejection::TpmPartitionDenied(format!(
        "доступ к скрытому разделу /TPM запрещён (Ring 3); пароли — только в TPM NV"
    ))),
    // ---- ПОЛЬЗОВАТЕЛЬСКИЙ РАЗДЕЛ /userdata ------------------------------
    "/userdata" => {
        match fs_type {
            "ext4" => {
                match mode {
                    MountMode::ReadOnly => Ok(()),
                    MountMode::ReadWrite => {
                        match stage.allows_userdata_rw() {
                            true => Ok(()),
                            false => Err(VaultRejection::UserdataPolicy(format!(
                                "rw-монтирование /userdata на стадии '{}' вне разрешённых (Boot/Recovery)",
                                stage.as_token()
                            ))),
                        }
                    }
                }
            }
            // Иной драйвер для userdata — нарушение политики.
            _ => Err(VaultRejection::NonExt4Userdata(format!(
                "файловая система '{}' для /userdata отличается от ext4",
                fs_type
            ))),
        }
    }
    // ---- СИСТЕМНЫЕ РАЗДЕЛЫ ----------------------------------------------
    other if is_system => {
        let _: &str = other;
        match fs_type {
            "erofs" => {
                match mode {
                    MountMode::ReadOnly => Ok(()),
                    MountMode::ReadWrite => {
                        match stage.is_flash_authorized() {
                            true => Ok(()),
                            false => {
                                // ТЕРМИНАЛЬНАЯ БЛОКИРОВКА: rw системного
                                // раздела вне прошивочного контекста.
                                panic!(
                                    "SECURITY_VIOLATION: Hard-locked system partition reached with RW flags. Boot halted."
                                );
                            }
                        }
                    }
                }
            }
            // Системный раздел через не-erofs драйвер — терминальное
            // нарушение: образ раздела не может быть EROFS-неизменяемым.
            _ => {
                panic!(
                    "SECURITY_VIOLATION: Hard-locked system partition reached with RW flags. Boot halted."
                );
            }
        }
    }
    // ---- ИНЫЕ ТОЧКИ (пользовательские/виртуальные ФС) -------------------
    _ => Ok(()),
}
}
