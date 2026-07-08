#![allow(dead_code)]
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

fn frame_address(index: usize) -> usize {
    FREE_MEMORY_START + index * PAGE_SIZE
}

fn is_used(bitmap: &[u64; BITMAP_WORDS], idx: usize) -> bool {
    let word = idx / 64;
    let bit = idx % 64;
    (bitmap[word] >> bit) & 1 == 1
}

fn mark_used_inner(bitmap: &mut [u64; BITMAP_WORDS], idx: usize) {
    bitmap[idx / 64] |= 1 << (idx % 64);
}

fn mark_free_inner(bitmap: &mut [u64; BITMAP_WORDS], idx: usize) {
    bitmap[idx / 64] &= !(1 << (idx % 64));
}

/// Выделяет одну физическую страницу (4 КиБ). Возвращает физический адрес.
pub fn alloc_page() -> Option<usize> {
    let mut bitmap = BITMAP.lock();
    for i in 0..TOTAL_FRAMES {
        if !is_used(&bitmap, i) {
            mark_used_inner(&mut bitmap, i);
            { let _ = super::ALLOCATED_PAGES.lock().wrapping_add(1); }
            return Some(frame_address(i));
        }
    }
    None
}

/// Выделяет `count` последовательных физических страниц.
pub fn alloc_pages(count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let mut bitmap = BITMAP.lock();
    let mut run = 0usize;
    let mut start = 0usize;

    for i in 0..TOTAL_FRAMES {
        if !is_used(&bitmap, i) {
            if run == 0 {
                start = i;
            }
            run += 1;
            if run >= count {
                for j in start..start + count {
                    mark_used_inner(&mut bitmap, j);
                }
                *super::ALLOCATED_PAGES.lock() += count;
                return Some(frame_address(start));
            }
        } else {
            run = 0;
        }
    }
    None
}

/// Освобождает физическую страницу.
pub fn free_page(phys_addr: usize) {
    if let Some(idx) = frame_index(phys_addr) {
        let mut bitmap = BITMAP.lock();
        if is_used(&bitmap, idx) {
            mark_free_inner(&mut bitmap, idx);
            *super::FREED_PAGES.lock() += 1;
        }
    }
}

/// Освобождает `count` последовательных страниц начиная с phys_addr.
pub fn free_pages(phys_addr: usize, count: usize) {
    for i in 0..count {
        free_page(phys_addr + i * PAGE_SIZE);
    }
}
