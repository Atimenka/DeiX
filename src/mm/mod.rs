#![allow(dead_code)]
//! Менеджер физической и виртуальной памяти DeiX.
//!
//! ## Архитектура
//!
//! ### Физический аллокатор (страницы 4 КиБ)
//! - Buddy allocator: 4 КиБ → 2 МиБ страницы
//! - Отслеживает свободные/занятые физические фреймы через битовую карту
//! - Регионы: 0x0–0x100000 зарезервированы (загрузчик/ядро),
//!            0x100000–0x10000000 (256 МиБ) доступны для выделения
//!
//! ### Виртуальная память (4-уровневые таблицы страниц x86_64)
//! - Identity-map первые 4 ГиБ (уже сделано загрузчиком)
//! - Выделение новых виртуальных регионов для пользовательских процессов
//! - Map/unmap физических страниц в виртуальное адресное пространство
//!
//! ### Куча ядра
//! - Растёт от 0x200000 (2 МиБ) вверх до ~0x1200000 (18 МиБ максимум)
//! - Использует linked-list аллокатор (allocator.rs)

pub mod phys;
pub mod virt;

use crate::spinlock::SpinLock;

/// Размер страницы памяти (4 КиБ — стандарт x86_64).
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12;

/// Всего физической памяти, доступной ядру (256 МиБ).
pub const TOTAL_MEMORY: usize = 256 * 1024 * 1024;

/// Адрес начала доступной физической памяти (сразу после ядра/загрузчика).
pub const FREE_MEMORY_START: usize = 0x100000; // 1 МиБ

/// Адрес начала ядра в виртуальной памяти (совпадает с физическим — identity map).
pub const KERNEL_BASE: usize = 0x10000;

/// Адрес начала кучи ядра.
pub const KERNEL_HEAP_START: usize = 0x200000; // 2 МиБ
pub const KERNEL_HEAP_MAX: usize = 18 * 1024 * 1024; // 18 МиБ

/// Информация о регионе физической памяти.
#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    pub start: usize,
    pub size: usize,
    pub usable: bool,
}

/// Глобальный счётчик выделенной памяти для диагностики.
static ALLOCATED_PAGES: SpinLock<usize> = SpinLock::new(0);
static FREED_PAGES: SpinLock<usize> = SpinLock::new(0);

pub fn stats() -> (usize, usize) {
    let alloc = *ALLOCATED_PAGES.lock();
    let freed = *FREED_PAGES.lock();
    (alloc, freed)
}

pub fn print_stats() {
    let (alloc, freed) = stats();
    crate::println!(
        "  Memory: {} pages allocated, {} freed, {} in use ({} KiB)",
        alloc,
        freed,
        alloc.saturating_sub(freed),
        alloc.saturating_sub(freed) * PAGE_SIZE / 1024
    );
}
