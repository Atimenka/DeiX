// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// Модуль KERNEL SECURITY VAULT — хардкодный барьер безопасности разделов.
// Правила:
//   1. Системные разделы (/kernel, /init_boot, /boot, /vendor_boot, /super,
//      /system, /recovery) монтируются ИСКЛЮЧИТЕЛЬНО драйвером erofs.
//      Иной драйвер — терминальное нарушение (panic! с эталонным текстом).
//   2. rw-монтирование системного раздела разрешено ТОЛЬКО в штатных
//      прошивочных контекстах: Fastbootd, Edl, Recovery. Вне них — panic!.
//   3. /userdata: драйвер ext4; rw — только на стадиях Boot, Recovery,
//      Fastbootd, Edl. Нарушение политики /userdata — предупреждение
//      (VaultRejection), НЕ паника: единственный легитимный panic! — по
//      правилам 1-2 (системные разделы).
// Модуль изолирован от парсера: парсер вызывает vault::evaluate() и сам
// решает, как зарегистрировать отказ (предупреждение) или принять команду.
// no_std-совместимо: только константы, enum, простые строки.
use crate::init_parser::{BootStage, MountMode};

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

/// Эталонный текст паники Vault (строго по ТЗ, байт-в-байт).
pub const SECURITY_VIOLATION_PANIC_TEXT: &str =
    "SECURITY_VIOLATION: Hard-locked system partition reached with RW flags. Boot halted.";

/// Отказ политики, НЕ являющийся терминальным: нарушение правил /userdata.
/// Парсер превращает его в предупреждение (ParseWarning) и продолжает разбор.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultRejection {
    /// rw-монтирование /userdata вне разрешённых стадий.
    UserdataPolicy(String),
    /// Файловая система /userdata отличается от ext4.
    NonExt4Userdata(String),
}

impl VaultRejection {
    /// Детальное описание причины отказа (для журнала ядра).
    pub fn detail(&self) -> String {
        match self {
            VaultRejection::UserdataPolicy(msg) => msg.clone(),
            VaultRejection::NonExt4Userdata(msg) => msg.clone(),
        }
    }

    /// Короткое имя категории (для лога).
    pub fn kind_name(&self) -> &'static str {
        match self {
            VaultRejection::UserdataPolicy(_) => "userdata-policy-violation",
            VaultRejection::NonExt4Userdata(_) => "non-ext4-userdata",
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
