//! Кольцевой буфер событий диагностики.
//!
//! Буфер предвыделен целиком в `.bss` и не обращается к куче, поэтому запись
//! в него допустима из контекста прерывания, до инициализации выделителя и
//! из обработчика паники.
//!
//! Синхронизация: `IrqSpinLock` с отключением прерываний. Это обязательно —
//! событие может прийти из IRQ поверх уже выполняющейся записи, и без
//! маскирования прерываний индекс записи был бы потерян.

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use super::record::ErrorRecord;
use crate::spinlock::IrqSpinLock;

/// Ёмкость буфера. При ~120 байт на запись это порядка 60 КиБ `.bss`.
pub const CAPACITY: usize = 512;

/// Окно агрегации: повтор того же сбоя внутри этого интервала не создаёт
/// новую запись, а увеличивает счётчик существующей.
const AGGREGATE_WINDOW_MS: u64 = 2_000;

/// Сколько последних записей просматривается при поиске серии для агрегации.
const AGGREGATE_SCAN: usize = 16;

/// Скользящий буфер событий.
struct Ring {
    slots: [ErrorRecord; CAPACITY],
    /// Индекс следующей записи.
    head: usize,
    /// Сколько записей реально заполнено (до первого переполнения).
    filled: usize,
}

impl Ring {
    const fn new() -> Self {
        Self {
            slots: [const { ErrorRecord::empty() }; CAPACITY],
            head: 0,
            filled: 0,
        }
    }
}

static RING: IrqSpinLock<Ring> = IrqSpinLock::new(Ring::new());

/// Монотонно растущий идентификатор события. Не сбрасывается при переполнении
/// буфера, поэтому `related_id` остаётся осмысленным и для вытесненных записей.
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// Всего событий зарегистрировано за время работы (включая вытесненные).
static TOTAL_EVENTS: AtomicUsize = AtomicUsize::new(0);

/// Всего записей отброшено переполнением буфера.
static DROPPED_EVENTS: AtomicUsize = AtomicUsize::new(0);

/// Момент старта текущей загрузки в миллисекундах — для нумерации сессий.
static BOOT_SESSION: AtomicU32 = AtomicU32::new(1);

/// Идентификатор текущей загрузки.
pub fn boot_session() -> u32 {
    BOOT_SESSION.load(Ordering::Relaxed)
}

/// Устанавливает идентификатор загрузки (вызывается один раз на старте).
pub fn set_boot_session(id: u32) {
    BOOT_SESSION.store(id.max(1), Ordering::Relaxed);
}

/// Всего событий зарегистрировано с момента старта.
pub fn total_events() -> usize {
    TOTAL_EVENTS.load(Ordering::Relaxed)
}

/// Сколько событий не поместилось в буфер.
pub fn dropped_events() -> usize {
    DROPPED_EVENTS.load(Ordering::Relaxed)
}

/// Сколько записей сейчас в буфере.
pub fn len() -> usize {
    RING.lock().filled
}

/// Помещает запись в буфер.
///
/// Если идентичный сбой уже зарегистрирован внутри окна агрегации, счётчик
/// существующей записи увеличивается, а новая запись не создаётся.
/// Возвращает идентификатор записи, в которую попало событие.
pub fn push(record: &mut ErrorRecord, now_ms: u64) -> u32 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    TOTAL_EVENTS.fetch_add(1, Ordering::Relaxed);

    record.id = id;
    record.timestamp_ms = now_ms;
    record.occurrences = record.occurrences.max(1);
    if record.first_seen_ms == 0 {
        record.first_seen_ms = now_ms;
    }
    record.last_seen_ms = now_ms;

    let identity = record.identity();

    let mut ring = RING.lock();

    // Ищем серию среди последних записей.
    let scan = ring.filled.min(AGGREGATE_SCAN);
    for step in 0..scan {
        // Идём от самой свежей записи назад.
        let idx = (ring.head + CAPACITY - 1 - step) % CAPACITY;
        let existing = &mut ring.slots[idx];
        if existing.identity() != identity || existing.severity != record.severity {
            continue;
        }
        if now_ms.saturating_sub(existing.last_seen_ms) > AGGREGATE_WINDOW_MS {
            continue;
        }
        existing.occurrences = existing.occurrences.saturating_add(record.occurrences);
        existing.last_seen_ms = now_ms;
        if record.related_id != 0 {
            existing.related_id = record.related_id;
        }
        return id;
    }

    // Серии нет — кладём новую запись.
    let idx = ring.head;
    if ring.filled == CAPACITY {
        DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed);
    } else {
        ring.filled += 1;
    }
    ring.slots[idx] = *record;
    ring.head = (idx + 1) % CAPACITY;
    id
}

/// Копирует последние `limit` записей в порядке от старых к новым.
///
/// Выделяет память, поэтому вызывается только из контекста, где куча
/// доступна и прерывания не критичны (CLI, графический Центр ошибок).
pub fn snapshot(limit: usize) -> alloc::vec::Vec<ErrorRecord> {
    let ring = RING.lock();
    let count = ring.filled.min(limit);
    let mut out = alloc::vec::Vec::with_capacity(count);
    let start = (ring.head + CAPACITY - count) % CAPACITY;
    for i in 0..count {
        out.push(ring.slots[(start + i) % CAPACITY]);
    }
    out
}

/// Все записи буфера от старых к новым.
pub fn all() -> alloc::vec::Vec<ErrorRecord> {
    snapshot(CAPACITY)
}

/// Записи, отобранные предикатом, от старых к новым.
pub fn filter<F: Fn(&ErrorRecord) -> bool>(limit: usize, pred: F) -> alloc::vec::Vec<ErrorRecord> {
    snapshot(CAPACITY)
        .into_iter()
        .filter(|r| pred(r))
        .rev()
        .take(limit)
        .collect::<alloc::vec::Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// Запись по идентификатору, если она ещё в буфере.
pub fn by_id(id: u32) -> Option<ErrorRecord> {
    if id == 0 {
        return None;
    }
    let ring = RING.lock();
    for step in 0..ring.filled {
        let idx = (ring.head + CAPACITY - 1 - step) % CAPACITY;
        if ring.slots[idx].id == id {
            return Some(ring.slots[idx]);
        }
    }
    None
}

/// Последняя ошибка указанной подсистемы не старше `within_ms` — для
/// автоматического связывания вторичных сбоев с первопричиной
/// (отказ диска → ошибка чтения EXT2 → ошибка VFS).
pub fn find_recent_error(sub: crate::diag::Subsystem, within_ms: u64, now_ms: u64) -> Option<u32> {
    let ring = RING.lock();
    for step in 0..ring.filled {
        let idx = (ring.head + CAPACITY - 1 - step) % CAPACITY;
        let r = &ring.slots[idx];
        if now_ms.saturating_sub(r.last_seen_ms) > within_ms {
            break;
        }
        if r.severity.is_error() && r.code != crate::diag::NONE && r.code.subsystem() == sub {
            return Some(r.id);
        }
    }
    None
}

/// Обходит последние `limit` записей от старых к новым без выделения памяти.
///
/// Используется отчётом об отказе: он собирается в обработчике паники, где
/// куча может быть как раз тем, что повреждено.
pub fn for_each<F: FnMut(&ErrorRecord)>(limit: usize, mut f: F) {
    let ring = RING.lock();
    let count = ring.filled.min(limit);
    let start = (ring.head + CAPACITY - count) % CAPACITY;
    for i in 0..count {
        f(&ring.slots[(start + i) % CAPACITY]);
    }
}

/// Очищает буфер (например, после того как журнал сохранён на диск).
pub fn clear() {
    let mut ring = RING.lock();
    ring.head = 0;
    ring.filled = 0;
}

