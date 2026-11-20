//! Профиль загрузки: сколько миллисекунд, памяти кучи и секторов диска
//! уходит на каждый шаг.
//!
//! Каждая метка фиксирует три счётчика на момент вызова: аптайм PIT,
//! занятую кучу и накопленный дисковый ввод-вывод. Ничего не оценивается
//! и не округляется «для красоты» — это те же счётчики, что показывают
//! `uptime` и `dmesg`. Метки идут в serial как `[boot] ...` и копятся в
//! таблице, которую печатает команда `bootprof`.
//!
//! Разрешение времени — тик PIT (10 мс). Шаг, в котором ядро ждёт
//! человека (экран входа), помечен в названии: его длительность отражает
//! скорость пользователя, а не системы.

use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;

const MAX_MARKS: usize = 96;

#[derive(Clone, Copy)]
struct Mark {
    label: &'static str,
    t_ms: u64,
    heap: usize,
    sectors_read: u64,
    sectors_written: u64,
}

struct Table {
    marks: [Option<Mark>; MAX_MARKS],
    len: usize,
    dropped: usize,
}

static TABLE: SpinLock<Table> = SpinLock::new(Table { marks: [None; MAX_MARKS], len: 0, dropped: 0 });

/// Фиксирует шаг загрузки и печатает строку в serial.
pub fn mark(label: &'static str) {
    let t_ms = crate::timer::uptime_ms();
    let heap = crate::allocator::allocated_heap_bytes();
    let (sectors_read, sectors_written) = crate::ata::io_counters();

    let delta = without_interrupts(|| {
        let mut table = TABLE.lock();
        let prev = if table.len == 0 { 0 } else { table.marks[table.len - 1].map_or(0, |m| m.t_ms) };
        if table.len < MAX_MARKS {
            let at = table.len;
            table.marks[at] = Some(Mark { label, t_ms, heap, sectors_read, sectors_written });
            table.len += 1;
        } else {
            table.dropped += 1;
        }
        t_ms.saturating_sub(prev)
    });

    crate::serial_println!(
        "[boot] t={:>6} ms +{:>6} ms heap={:>6} KiB rd={:>6} wr={:>5} {}",
        t_ms,
        delta,
        heap / 1024,
        sectors_read,
        sectors_written,
        label
    );
}

/// Печатает таблицу меток на экран (команда `bootprof`).
pub fn report() {
    let snapshot = without_interrupts(|| {
        let table = TABLE.lock();
        (table.marks, table.len, table.dropped)
    });
    let (marks, len, dropped) = snapshot;
    crate::println!("  {:>7} {:>7} {:>8} {:>7}  step", "t, ms", "+ms", "heap KiB", "rd sec");
    let mut prev = 0u64;
    for mark in marks.iter().take(len).flatten() {
        crate::println!(
            "  {:>7} {:>7} {:>8} {:>7}  {}",
            mark.t_ms,
            mark.t_ms.saturating_sub(prev),
            mark.heap / 1024,
            mark.sectors_read,
            mark.label
        );
        prev = mark.t_ms;
    }
    if len == 0 {
        crate::println!("  no marks recorded");
    }
    if dropped > 0 {
        crate::println!("  ({} marks did not fit into the table)", dropped);
    }
    if let Some(last) = marks.iter().take(len).flatten().last() {
        crate::println!(
            "  disk: {} sectors read, {} written; heap in use: {} KiB of {} KiB",
            last.sectors_read,
            last.sectors_written,
            crate::allocator::allocated_heap_bytes() / 1024,
            crate::allocator::total_heap_bytes() / 1024
        );
    }
}
