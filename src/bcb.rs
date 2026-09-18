// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// bcb — BOOTLOADER CONTROL BLOCK (аналог BCB из Android bootloader).
//
// Назначение: одноразовый «флажок» режима загрузки, который ставится
// специальной командой ПЕРЕД перезапуском и определяет, что загрузится
// при СЛЕДУЮЩЕЙ загрузке:
//   * NORMAL    (0) — обычная ОС;
//   * RECOVERY  (1) — графическая оболочка recovery (TWRP/OrangeFox-стиль),
//                     которая идёт ВЫШЕ (раньше) обычной ОС в запуске;
//   * FASTBOOTD (2) — графический прошивальщик, который идёт ЕЩЁ ВЫШЕ
//                     recovery (проверяется первым).
//
// Флажок ОДНОРАЗОВЫЙ: boot_flow() читает его, СРАЗУ сбрасывает в NORMAL и
// загружает соответствующую стадию — следующая загрузка всегда обычная.
//
// BCB хранится в отдельном секторе диска (LBA 3000 — свободная зона между
// ядром и ext2-томом, вне MBR и вне разделов). Формат:
//   magic "DEIXBCB1" (8) | boot_mode (u32 LE) | reserved (заполнено 0xFF).
// no_std-совместимо: только константы и ata-вызовы.


/// Магическая сигнатура BCB-сектора.
pub const BCB_MAGIC: [u8; 8] = *b"DEIXBCB1";

/// LBA сектора BCB (свободная зона: ядро ~1..1150, ext2 с 4096).
pub const BCB_LBA: u32 = 3000;

/// Режимы загрузки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMode {
    /// Обычная загрузка ОС.
    Normal,
    /// Графическая оболочка recovery (выше ОС в запуске).
    Recovery,
    /// Графический прошивальщик (выше recovery).
    Fastbootd,
    /// DSM — Download System Manager (аналог EDL, ВЫШЕ всех: emergency).
    Dsm,
}

impl BootMode {
    pub fn from_u32(v: u32) -> BootMode {
        match v {
            1 => BootMode::Recovery,
            2 => BootMode::Fastbootd,
            3 => BootMode::Dsm,
            _ => BootMode::Normal,
        }
    }
    pub fn as_u32(&self) -> u32 {
        match self {
            BootMode::Normal => 0,
            BootMode::Recovery => 1,
            BootMode::Fastbootd => 2,
            BootMode::Dsm => 3,
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            BootMode::Normal => "normal",
            BootMode::Recovery => "recovery",
            BootMode::Fastbootd => "fastbootd",
            BootMode::Dsm => "dsm",
        }
    }
}

/// Читает BCB-сектор и возвращает режим загрузки (Normal, если BCB не
/// размечен/некорректен).
pub fn read_boot_mode() -> BootMode {
    let mut sector = [0u8; 512];
    if crate::ata::read_sectors(BCB_LBA, 1, &mut sector).is_err() {
        return BootMode::Normal;
    }
    if &sector[..8] != &BCB_MAGIC {
        return BootMode::Normal;
    }
    let mode = u32::from_le_bytes(sector[8..12].try_into().unwrap_or([0; 4]));
    BootMode::from_u32(mode)
}

/// Записывает режим загрузки в BCB (ставит флажок перед перезапуском).
/// СОХРАНЯЕТ A/B-слот (offset 12..16): иначе сброс флажка затирал бы
/// активный слот нулями/0xFF и цепочка всегда грузилась бы с "a".
pub fn write_boot_mode(mode: BootMode) {
    let mut sector = [0xFFu8; 512];
    sector[..8].copy_from_slice(&BCB_MAGIC);
    sector[8..12].copy_from_slice(&mode.as_u32().to_le_bytes());
    // Сохраняем A/B-слот (12..16) И флаг ota_pending (16..20), если BCB
    // уже размечен: иначе сброс флажка затирал ota_pending нулями/0xFF и
    // OTA-уведомление срабатывало ложно при каждом буте.
    let mut old = [0u8; 512];
    if crate::ata::read_sectors(BCB_LBA, 1, &mut old).is_ok() && &old[..8] == &BCB_MAGIC {
        sector[12..20].copy_from_slice(&old[12..20]);
    } else {
        sector[12..16].copy_from_slice(&0u32.to_le_bytes()); // слот a по умолчанию
        sector[16..20].copy_from_slice(&0u32.to_le_bytes()); // ota_pending = 0
    }
    let _ = crate::ata::write_sectors(BCB_LBA, 1, &sector);
    crate::println!("  [bcb] флажок установлен: {} (слот {})", mode.name(), slot_name());
}

/// Сбрасывает флажок в Normal (одноразовость).
pub fn clear_boot_mode() {
    write_boot_mode(BootMode::Normal);
}

/// ==================== A/B СЛОТЫ ====================
/// Текущий активный слот (0 = a, 1 = b) хранится в BCB-секторе по
/// смещению 12..16 (после boot_mode). OTA прошивает НЕактивный слот и
/// переключает сюда; откат — записью обратно.

const SLOT_OFFSET: usize = 12;

/// Текущий активный слот: 0 = a, 1 = b (по умолчанию a).
pub fn read_slot() -> u32 {
    let mut sector = [0u8; 512];
    if crate::ata::read_sectors(BCB_LBA, 1, &mut sector).is_err() {
        return 0;
    }
    if &sector[..8] != &BCB_MAGIC {
        return 0;
    }
    u32::from_le_bytes([
        sector[SLOT_OFFSET],
        sector[SLOT_OFFSET + 1],
        sector[SLOT_OFFSET + 2],
        sector[SLOT_OFFSET + 3],
    ])
}

/// Устанавливает активный слот (0 = a, 1 = b).
pub fn write_slot(slot: u32) {
    let mut sector = [0u8; 512];
    if crate::ata::read_sectors(BCB_LBA, 1, &mut sector).is_ok() {
        if &sector[..8] != &BCB_MAGIC {
            sector[..8].copy_from_slice(&BCB_MAGIC);
            sector[8..12].copy_from_slice(&0u32.to_le_bytes());
        }
    } else {
        sector[..8].copy_from_slice(&BCB_MAGIC);
        sector[8..12].copy_from_slice(&0u32.to_le_bytes());
    }
    sector[SLOT_OFFSET..SLOT_OFFSET + 4].copy_from_slice(&(slot & 1).to_le_bytes());
    let _ = crate::ata::write_sectors(BCB_LBA, 1, &sector);
    crate::println!("  [bcb] активный слот: {}", if slot & 1 == 0 { "a" } else { "b" });
}

/// Имя слота ("a"/"b") для вывода.
pub fn slot_name() -> &'static str {
    if read_slot() == 0 { "a" } else { "b" }
}

/// ПРОВЕРКА ПРИ КАЖДОЙ ЗАГРУЗКЕ (вызывается в самом начале kernel_main,
/// ДО инициализации остальной ОС — recovery/fastbootd «выше» ОС в запуске).
///
/// Логика приоритетов (как в Android bootloader):
///   1) fastbootd  — проверяется ПЕРВЫМ (самый высокий приоритет);
///   2) recovery   — второй;
///   3) normal     — обычная ОС.
/// Флажок читается и СРАЗУ сбрасывается в Normal — одноразовый.
/// Возвращает true, если загрузка была перенаправлена в recovery/fastbootd
/// (обычная ОС в этом случае не запускается в этой же сессии).
pub fn boot_flow() -> bool {
    let mode = read_boot_mode();
    // Одноразовость: сбрасываем флажок сразу после чтения.
    clear_boot_mode();

    match mode {
        BootMode::Dsm => {
            crate::serial_println!("[bcb] DSM: загрузка Download System Manager (выше fastbootd)");
            crate::println!("  [bcb] DSM: флажок установлен — запускаем emergency-прошивальщик.");
            crate::dsm::dsm_gui();
            true
        }
        BootMode::Fastbootd => {
            crate::serial_println!("[bcb] FASTBOOTD: загрузка прошивальщика (выше recovery)");
            crate::println!("  [bcb] FASTBOOTD: флажок установлен — запускаем прошивальщик.");
            crate::fastbootd_ui::fastbootd_gui();
            true
        }
        BootMode::Recovery => {
            crate::serial_println!("[bcb] RECOVERY: загрузка рекавери (выше ОС)");
            crate::println!("  [bcb] RECOVERY: флажок установлен — запускаем рекавери.");
            crate::recovery_ui::recovery_gui();
            true
        }
        BootMode::Normal => {
            // Обычная загрузка ОС.
            false
        }
    }
}
