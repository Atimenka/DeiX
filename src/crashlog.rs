// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// crashlog — ЖУРНАЛ ОШИБОК ЯДРА (аналог Android tombstone / last_kmsg).
//
// Паник-хендлер вызывает record_crash(): дамп пишется СЫРЫМИ секторами
// ATA (LBA 2048, свободная зона между загрузчиком и ext2-томом) с маркером
// DEIXCRSH — БЕЗ файловой системы и БЕЗ шифрования. Это критично: паника
// не должна трогать ext2/зашифрованный том (запись в нестабильном
// состоянии может повредить данные). При следующей загрузке ОС и recovery
// показывают «Произошла ошибка» и позволяют посмотреть дамп (команда
// `crashlog`, пункт recovery «View crash log»).
// no_std-совместимо: alloc (String), потокобезопасно через SpinLock.


use alloc::string::{String, ToString};
use crate::spinlock::SpinLock;

/// Сырые сектора crash-лога (свободная зона: stage2 < 1150, ext2 с 4096).
pub const CRASH_LBA: u32 = 2048;
pub const CRASH_SECTORS: u32 = 4; // 2 КиБ под дамп
/// Маркер дампа в первом секторе.
pub const CRASH_MARKER: [u8; 8] = *b"DEIXCRSH";

/// Дамп последней паники ТЕКУЩЕЙ сессии (в памяти).
static LAST_CRASH: SpinLock<Option<String>> = SpinLock::new(None);

/// Записывает факт паники: в память + сырыми секторами на диск.
pub fn record_crash(msg: &str) {
    let full: String = format_crash_header(msg);
    *LAST_CRASH.lock() = Some(full.clone());
    // Сырая запись на диск (не зависит от ext2/шифрования/ENGINE).
    let mut data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    data.extend_from_slice(&CRASH_MARKER);
    data.extend_from_slice(full.as_bytes());
    data.resize((CRASH_SECTORS * 512) as usize, 0);
    // Пишем по одному сектору (паника — нестабильное состояние).
    for i in 0..CRASH_SECTORS {
        let start = (i * 512) as usize;
        let mut sector = [0u8; 512];
        sector.copy_from_slice(&data[start..start + 512]);
        let _ = crate::ata::write_sectors(CRASH_LBA + i, 1, &sector);
    }
}

/// Формирует заголовок дампа (уровень, сообщение).
fn format_crash_header(msg: &str) -> String {
    let mut out = String::new();
    out.push_str("=== DeiX Crash Log (tombstone) ===\n");
    out.push_str("level: kernel panic\n");
    out.push_str("message: ");
    out.push_str(msg);
    out.push('\n');
    out.push_str("kernel: DeiX v0.2-beta (x86_64, no_std)\n");
    out.push_str("cause hint: последние строки журнала — 'dmesg' / 'bugreport'\n");
    out
}


/// Дамп текущей сессии (в памяти), если есть.
pub fn current_crash() -> Option<String> {
    LAST_CRASH.lock().clone()
}

/// Читает crash-лог с диска (сырые сектора LBA 2048), если маркер есть.
pub fn load_disk_crash() -> Option<String> {
    let mut sector = [0u8; 512];
    if crate::ata::read_sectors(CRASH_LBA, 1, &mut sector).is_err() {
        return None;
    }
    if &sector[..8] != &CRASH_MARKER {
        return None;
    }
    let mut data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    for i in 0..CRASH_SECTORS {
        let mut buf = [0u8; 512];
        if crate::ata::read_sectors(CRASH_LBA + i, 1, &mut buf).is_err() {
            break;
        }
        data.extend_from_slice(&buf);
    }
    // Отрезаем маркер и хвостовые нули.
    let body = &data[8..];
    let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
    Some(String::from_utf8_lossy(&body[..end]).to_string())
}

/// Есть ли crash-лог на диске (для экрана «Произошла ошибка» при буте).
pub fn has_disk_crash() -> bool {
    load_disk_crash().is_some()
}

/// Удаляет crash-лог с диска (после «ОК, понял» / Factory Reset).
pub fn clear_disk_crash() {
    let zeros = [0u8; 512];
    for i in 0..CRASH_SECTORS {
        let _ = crate::ata::write_sectors(CRASH_LBA + i, 1, &zeros);
    }
}

/// Очищает дамп текущей сессии.
pub fn clear_current_crash() {
    *LAST_CRASH.lock() = None;
}
