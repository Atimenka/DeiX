//! Постоянное хранение журналов в `/userdata/log/`.
//!
//! Запись выполняется отложенно из основного цикла оболочки, а не из
//! обработчика прерывания: опрос ATA в ISR заблокировал бы систему на
//! миллисекунды и нарушил бы требование «из прерывания — только в буфер».
//!
//! Каждый файл хранит машиночитаемые строки `key=value`, чтобы журнал можно
//! было разобрать внешним инструментом, и дополнительно человекочитаемый
//! заголовок сессии.

use alloc::string::String;
use alloc::vec::Vec;

use super::code::Subsystem;
use super::log::format_compact;
use super::record::ErrorRecord;
use super::ring;
use super::severity::Severity;
use super::NONE;
use crate::vfs;

/// Каталог журналов на пользовательском разделе.
pub const LOG_DIR: &str = "/userdata/log";
/// Каталог отчётов об отказе.
pub const PANIC_DIR: &str = "/userdata/log/panic";

/// Общий журнал всех событий.
pub const FILE_SYSTEM: &str = "/userdata/log/system.log";
/// Только ошибки и выше.
pub const FILE_ERROR: &str = "/userdata/log/error.log";
/// События фазы загрузки.
pub const FILE_BOOT: &str = "/userdata/log/boot.log";
/// Отказ ядра.
pub const FILE_PANIC: &str = "/userdata/log/panic.log";
/// События Dinit и служб.
pub const FILE_DINIT: &str = "/userdata/log/dinit.log";
/// События модулей ядра.
pub const FILE_KMOD: &str = "/userdata/log/kmod.log";
/// Файл последнего отчёта об отказе.
pub const FILE_LAST_PANIC: &str = "/userdata/log/panic/last.panic";
/// Счётчик загрузок — позволяет отличить события прошлой сессии.
pub const FILE_BOOT_COUNT: &str = "/userdata/log/boot.count";

/// Предельный размер одного файла журнала в байтах.
///
/// При превышении файл усекается с конца: свежая часть журнала важнее
/// архива, а неограниченный рост быстро съел бы весь раздел.
const MAX_LOG_BYTES: usize = 64 * 1024;

/// Метка времени последней записи, уже сохранённой на диск.
///
/// Хранит `timestamp_ms`, а не позицию в кольце, потому что кольцо
/// перезаписывается и позиция теряет смысл.
static FLUSHED_UPTO: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Значение `ring::total_events()` на момент прошлого `flush` — быстрый
/// признак «новых событий нет».
static LAST_SEEN_TOTAL: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Создает структуру каталогов журналов.
///
/// Вызывается после монтирования `/userdata`. Ошибки регистрируются, но не
/// прерывают загрузку: система обязана работать и без постоянного журнала.
pub fn init() {
    for dir in [LOG_DIR, PANIC_DIR] {
        if vfs::exists(dir) {
            continue;
        }
        if let Err(e) = vfs::mkdir(dir) {
            super::warn(
                NONE,
                &alloc::format!("не удалось создать {}: {}", dir, e.message()),
            );
        }
    }
}

/// Читает счётчик загрузок и возвращает номер текущей сессии.
pub fn next_boot_session() -> u32 {
    let previous = vfs::read_file(FILE_BOOT_COUNT)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    let session = previous.wrapping_add(1);
    // Запись может не удаться до монтирования тома — тогда номер просто
    // не сохранится, и следующая загрузка получит тот же номер.
    let _ = vfs::write_file(FILE_BOOT_COUNT, alloc::format!("{}", session).as_bytes());
    session
}

/// Определяет, в какие файлы попадает запись.
///
/// Одна запись может попасть в несколько журналов: ошибка модуля ядра
/// относится и к `kmod.log`, и к `error.log`.
fn destinations(record: &ErrorRecord) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();

    if record.severity >= Severity::Debug {
        out.push(FILE_SYSTEM);
    }
    if record.severity.is_error() {
        out.push(FILE_ERROR);
    }
    if record.severity >= Severity::Panic {
        out.push(FILE_PANIC);
    }
    if super::log::booting() || record.timestamp_ms < 10_000 {
        out.push(FILE_BOOT);
    }
    match record.code.subsystem() {
        Subsystem::Dinit if record.code != NONE => out.push(FILE_DINIT),
        Subsystem::Kmod if record.code != NONE => out.push(FILE_KMOD),
        _ => {}
    }

    out
}

/// Сохраняет новые записи из кольцевого буфера на диск.
///
/// Возвращает число записанных строк. Вызывается из основного цикла
/// оболочки; частые вызовы дёшевы, потому что при отсутствии новых событий
/// функция выходит сразу.
pub fn flush() -> usize {
    // Быстрый выход без копирования буфера: если счётчик событий не
    // изменился с прошлого вызова, новых записей нет.
    let total = ring::total_events();
    let seen = LAST_SEEN_TOTAL.swap(total, core::sync::atomic::Ordering::Relaxed);
    if seen == total {
        return 0;
    }

    let upto = FLUSHED_UPTO.load(core::sync::atomic::Ordering::Relaxed);
    let records = ring::all();

    // Отбираем записи, ещё не сохранённые на диск. Метка времени монотонна,
    // поэтому сравнение по ней корректно даже после перезаписи кольца.
    let fresh: Vec<&ErrorRecord> = records.iter().filter(|r| r.timestamp_ms > upto).collect();
    if fresh.is_empty() {
        return 0;
    }

    let mut max_ts = upto;

    // Строки группируются по файлам: append переписывает файл целиком,
    // поэтому пишем каждый журнал один раз за вызов, а не на каждую
    // запись. Первый сброс после загрузки содержит десятки записей —
    // без группировки он превращался в десятки полных перезаписей
    // каждого файла на медленном PIO-диске.
    let mut batches: Vec<(&'static str, String, usize)> = Vec::new();
    for record in fresh {
        max_ts = max_ts.max(record.timestamp_ms);
        let line = format_line(record);
        for file in destinations(record) {
            match batches.iter_mut().find(|b| b.0 == file) {
                Some(b) => {
                    b.1.push_str(&line);
                    b.2 += 1;
                }
                None => batches.push((file, line.clone(), 1)),
            }
        }
    }

    let mut written = 0usize;
    for (file, data, count) in batches {
        if append(file, data.as_bytes()).is_ok() {
            written += count;
        }
    }

    FLUSHED_UPTO.store(max_ts, core::sync::atomic::Ordering::Relaxed);
    written
}

/// Форматирует одну строку журнала: человекочитаемое время + `key=value`.
fn format_line(record: &ErrorRecord) -> String {
    let mut buf = [0u8; 160];
    let n = format_compact(record, &mut buf);
    let kv = core::str::from_utf8(&buf[..n]).unwrap_or("");
    alloc::format!(
        "{} {} {}\n",
        super::log::format_time(record.timestamp_ms),
        record.code.as_string(),
        kv
    )
}

/// Дописывает данные в конец файла, ограничивая его размер.
fn append(path: &str, data: &[u8]) -> Result<(), crate::vfs::VfsError> {
    let mut content = vfs::read_file(path).unwrap_or_default();
    content.extend_from_slice(data);

    // Усекаем с начала, сохраняя целые строки.
    if content.len() > MAX_LOG_BYTES {
        let cut = content.len() - MAX_LOG_BYTES;
        let line_start = content[cut..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| cut + p + 1)
            .unwrap_or(cut);
        let mut header = String::from("# (начало журнала усечено)\n");
        header.push_str(core::str::from_utf8(&content[line_start..]).unwrap_or(""));
        content = header.into_bytes();
    }

    vfs::write_file(path, &content)
}

/// Читает файл журнала как текст.
fn read(path: &str) -> Option<String> {
    vfs::read_file(path)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Последние `n` строк файла журнала.
pub fn tail(path: &str, n: usize) -> Vec<String> {
    let Some(text) = read(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(n)
        .rev()
        .map(String::from)
        .collect()
}

/// Полный перечень файлов журналов с их описанием — для `log list` и
/// графического Центра ошибок.
pub const KNOWN_FILES: &[(&str, &str)] = &[
    (FILE_SYSTEM, "Все события системы"),
    (FILE_ERROR, "Ошибки и выше"),
    (FILE_BOOT, "События фазы загрузки"),
    (FILE_PANIC, "Отказы ядра"),
    (FILE_DINIT, "Службы и Dinit"),
    (FILE_KMOD, "Модули ядра"),
];
