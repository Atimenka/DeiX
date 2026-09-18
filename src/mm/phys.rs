//! Физический аллокатор страниц памяти (buddy allocator).
//!
//! Использует битовую карту для отслеживания занятых/свободных 4 КиБ фреймов
//! в диапазоне [FREE_MEMORY_START .. FREE_MEMORY_START + TOTAL_FRAMES * PAGE_SIZE).
//!
//! Физическая память:
//!   0x000000–0x00FFFF  — зарезервировано (IVT, BDA, boot sector)
//!   0x010000–0x01FFFF  — stage2 + ядро (kernel image)
//!   0x020000–0x0FFFFF  — куча ядра (heap)
//!   0x100000–0xFFFFFFF — свободно для выделения (255 МиБ)

use super::{PAGE_SIZE, FREE_MEMORY_START, TOTAL_MEMORY};
use crate::spinlock::SpinLock;

/// Сколько 4K-страниц в управляемом диапазоне.
const TOTAL_FRAMES: usize = (TOTAL_MEMORY - FREE_MEMORY_START) / PAGE_SIZE;

/// Битов: по одному на каждый фрейм. 1 = занят, 0 = свободен.
const BITMAP_WORDS: usize = (TOTAL_FRAMES + 63) / 64;

static BITMAP: SpinLock<[u64; BITMAP_WORDS]> = SpinLock::new([0u64; BITMAP_WORDS]);

/// Инициализирует физический аллокатор: помечает область ядра как занятую.
pub fn init() {
    let mut bitmap = BITMAP.lock();

    // Обнуляем всю битовую карту.
    for w in bitmap.iter_mut() {
        *w = 0;
    }

    // Помечаем первые N фреймов как занятые (зарезервированы под ядро/кучу).
    // Куча ядра растёт до ~0x1200000 (18 МиБ), берём 20 МиБ с запасом.
    let reserved_end = FREE_MEMORY_START + 20 * 1024 * 1024;
    let reserved_frames = (reserved_end - FREE_MEMORY_START) / PAGE_SIZE;
    for i in 0..reserved_frames.min(TOTAL_FRAMES) {
        mark_used_inner(&mut bitmap, i);
    }

    // Резервируем RAM-диск (0x2000000, 10 МиБ), если он активен (загрузка
    // с USB/Ventoy): загрузчик положил туда образ, аллокатор не должен
    // раздавать эти страницы (иначе ядро перезапишет собственный диск).
    if crate::ramdisk::is_active() {
        let start = crate::ramdisk::RAMDISK_BASE;
        let end = start + crate::ramdisk::RAMDISK_SIZE;
        let mut n = 0usize;
        let mut idx = start;
        while idx < end {
            if let Some(fi) = frame_index(idx) {
                if fi < TOTAL_FRAMES {
                    mark_used_inner(&mut bitmap, fi);
                    n += 1;
                }
            }
            idx += PAGE_SIZE;
        }
        crate::serial_println!(
            "[mm/phys] RAM-диск зарезервирован: {n} фреймов (0x{:X}..0x{:X})",
            start, end
        );
    }

    crate::println!(
        "  [mm/phys] Buddy allocator ready: {} frames ({} KiB) total, {} reserved.",
        TOTAL_FRAMES,
        TOTAL_FRAMES * PAGE_SIZE / 1024,
        reserved_frames
    );
}

fn frame_index(phys_addr: usize) -> Option<usize> {
    if phys_addr < FREE_MEMORY_START || phys_addr >= FREE_MEMORY_START + TOTAL_FRAMES * PAGE_SIZE {
        return None;
    }
    Some((phys_addr - FREE_MEMORY_START) / PAGE_SIZE)
}



fn mark_used_inner(bitmap: &mut [u64; BITMAP_WORDS], idx: usize) {
    bitmap[idx / 64] |= 1 << (idx % 64);
}





