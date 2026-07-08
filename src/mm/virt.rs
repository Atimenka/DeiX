#![allow(dead_code)]
//! Менеджер виртуальной памяти (4-уровневые таблицы страниц x86_64).
//!
//! Загрузчик (long_mode_init.asm) уже настроил identity-map первых 4 ГиБ
//! с флагами Present + Writable. Здесь мы добавляем:
//!   - Выделение виртуальных адресных регионов
//!   - Map/unmap физических страниц в виртуальное пространство
//!   - Поддержку пользовательских страниц (флаг User/Supervisor)
//!
//! x86_64 paging structure:
//!   PML4 (512 записей) → PDPT → PD → PT → 4K Page
//!   Каждый уровень: 512 записей по 8 байт = 4 КиБ (ровно 1 страница)

use super::PAGE_SIZE;
use crate::spinlock::SpinLock;

/// Флаги записей таблицы страниц.
pub mod flags {
    pub const PRESENT: u64 = 1 << 0;
    pub const WRITABLE: u64 = 1 << 1;
    pub const USER: u64 = 1 << 2; // Доступно из ring 3
    pub const WRITE_THROUGH: u64 = 1 << 3;
    pub const CACHE_DISABLE: u64 = 1 << 4;
    pub const HUGE_PAGE: u64 = 1 << 7; // Для PDPT (1 ГиБ) или PD (2 МиБ)
    pub const NO_EXECUTE: u64 = 1 << 63;
}

/// Запись таблицы страниц — 8 байт.
#[repr(C)]
#[derive(Clone, Copy)]
struct PageEntry(u64);

impl PageEntry {
    const fn empty() -> Self {
        PageEntry(0)
    }

    fn present(&self) -> bool {
        self.0 & flags::PRESENT != 0
    }

    fn address(&self) -> usize {
        (self.0 & 0x000F_FFFF_FFFF_F000) as usize
    }

    fn set(&mut self, phys_addr: usize, flags: u64) {
        self.0 = (phys_addr as u64 & 0x000F_FFFF_FFFF_F000) | flags;
    }

    fn clear(&mut self) {
        self.0 = 0;
    }
}

/// Таблица страниц (512 записей = 4 КиБ).
#[repr(C, align(4096))]
struct PageTable([PageEntry; 512]);

impl PageTable {
    fn new_zeroed() -> &'static mut Self {
        let page = super::phys::alloc_page()
            .expect("Out of physical memory for page table");
        let ptr = page as *mut PageTable;
        unsafe {
            core::ptr::write_bytes(ptr, 0, 1);
            &mut *ptr
        }
    }
}

/// Выделяет новую таблицу PML4 и делает её активной (загружает в CR3).
/// Используется для создания адресного пространства пользовательского процесса.
pub fn new_address_space() -> usize {
    let pml4_phys = super::phys::alloc_page()
        .expect("Out of memory for PML4");

    // Копируем kernel-space записи (последние 256 записей PML4)
    // из текущего адресного пространства — ядро должно быть видно
    // во всех процессах.
    unsafe {
        let current_pml4 = current_pml4_phys();
        let src = current_pml4 as *const u8;
        let dst = pml4_phys as *mut u8;

        // Копируем только верхнюю половину (kernel space):
        // записи 256..511 = байты 2048..4095
        core::ptr::copy_nonoverlapping(
            src.add(2048),
            dst.add(2048),
            2048,
        );
    }

    pml4_phys
}

/// Возвращает физический адрес активной PML4 (из CR3).
pub unsafe fn current_pml4_phys() -> usize {
    let cr3: usize;
    core::arch::asm!("mov {}, cr3", out(reg) cr3);
    cr3 & !0xFFF // сброс служебных битов
}

/// Загружает PML4 в CR3 (переключение адресного пространства).
pub unsafe fn switch_address_space(pml4_phys: usize) {
    core::arch::asm!("mov cr3, {}", in(reg) pml4_phys);
}

/// Отображает физическую страницу в виртуальном адресном пространстве.
///
/// `pml4_phys` — физический адрес PML4 (или 0 для текущего).
/// `virt_addr` — виртуальный адрес для отображения.
/// `phys_addr` — физический адрес страницы.
/// `flags` — флаги (PRESENT | WRITABLE | USER | NO_EXECUTE).
pub fn map_page(pml4_phys: usize, virt_addr: usize, phys_addr: usize, extra_flags: u64) {
    let pml4 = if pml4_phys == 0 {
        unsafe { current_pml4_phys() }
    } else {
        pml4_phys
    };

    let pml4_idx = (virt_addr >> 39) & 0x1FF;
    let pdpt_idx = (virt_addr >> 30) & 0x1FF;
    let pd_idx = (virt_addr >> 21) & 0x1FF;
    let pt_idx = (virt_addr >> 12) & 0x1FF;

    unsafe {
        let pml4_ptr = pml4 as *mut PageTable;
        let pdpt = ensure_table(&mut (*pml4_ptr).0[pml4_idx], extra_flags);
        let pd = ensure_table(&mut (*pdpt).0[pdpt_idx], extra_flags);
        let pt = ensure_table(&mut (*pd).0[pd_idx], extra_flags);

        let flags = flags::PRESENT | flags::WRITABLE | extra_flags;
        (*pt).0[pt_idx].set(phys_addr, flags);
    }
}

/// Снимает отображение виртуальной страницы.
pub fn unmap_page(pml4_phys: usize, virt_addr: usize) {
    let pml4 = if pml4_phys == 0 {
        unsafe { current_pml4_phys() }
    } else {
        pml4_phys
    };

    let pml4_idx = (virt_addr >> 39) & 0x1FF;
    let pdpt_idx = (virt_addr >> 30) & 0x1FF;
    let pd_idx = (virt_addr >> 21) & 0x1FF;
    let pt_idx = (virt_addr >> 12) & 0x1FF;

    unsafe {
        let pml4_ptr = pml4 as *mut PageTable;
        if !(*pml4_ptr).0[pml4_idx].present() {
            return;
        }
        let pdpt = (*pml4_ptr).0[pml4_idx].address() as *mut PageTable;
        if !(*pdpt).0[pdpt_idx].present() {
            return;
        }
        let pd = (*pdpt).0[pdpt_idx].address() as *mut PageTable;
        if !(*pd).0[pd_idx].present() {
            return;
        }
        let pt = (*pd).0[pd_idx].address() as *mut PageTable;

        // Освобождаем физическую страницу.
        let phys = (*pt).0[pt_idx].address();
        super::phys::free_page(phys);

        (*pt).0[pt_idx].clear();

        // Инвалидируем TLB для этого адреса.
        core::arch::asm!("invlpg [{}]", in(reg) virt_addr);
    }
}

/// Возвращает физический адрес, соответствующий виртуальному (обход таблиц).
pub fn translate(virt_addr: usize, pml4_phys: usize) -> Option<usize> {
    let pml4 = if pml4_phys == 0 {
        unsafe { current_pml4_phys() }
    } else {
        pml4_phys
    };

    let pml4_idx = (virt_addr >> 39) & 0x1FF;
    let pdpt_idx = (virt_addr >> 30) & 0x1FF;
    let pd_idx = (virt_addr >> 21) & 0x1FF;
    let pt_idx = (virt_addr >> 12) & 0x1FF;
    let offset = virt_addr & 0xFFF;

    unsafe {
        let pml4_ptr = pml4 as *mut PageTable;
        if !(*pml4_ptr).0[pml4_idx].present() { return None; }
        let pdpt = (*pml4_ptr).0[pml4_idx].address() as *mut PageTable;
        if !(*pdpt).0[pdpt_idx].present() { return None; }
        let pd = (*pdpt).0[pdpt_idx].address() as *mut PageTable;
        if !(*pd).0[pd_idx].present() { return None; }
        let pt = (*pd).0[pd_idx].address() as *mut PageTable;
        if !(*pt).0[pt_idx].present() { return None; }
        Some((*pt).0[pt_idx].address() + offset)
    }
}

/// Выделяет `count` страниц виртуальной памяти (без привязки к физической).
/// Возвращает виртуальный адрес.
///
/// Простой возрастающий аллокатор для адресного пространства процесса.
static NEXT_USER_VADDR: SpinLock<usize> = SpinLock::new(0x4000_0000); // 1 ГиБ

pub fn alloc_virtual_pages(count: usize) -> usize {
    let mut next = NEXT_USER_VADDR.lock();
    let addr = *next;
    *next += count * PAGE_SIZE;
    // Выравниваем на границу 2 МиБ для возможности huge pages.
    *next = (*next + 0x1F_FFFF) & !0x1F_FFFF;
    addr
}

/// Выделяет и отображает `count` страниц в указанном адресном пространстве.
/// Возвращает виртуальный адрес.
pub fn alloc_and_map(pml4_phys: usize, count: usize, extra_flags: u64) -> Option<usize> {
    let virt = alloc_virtual_pages(count);

    for i in 0..count {
        let phys = super::phys::alloc_page()?;
        map_page(pml4_phys, virt + i * PAGE_SIZE, phys, extra_flags);
    }

    Some(virt)
}

/// Гарантирует существование таблицы на следующем уровне.
/// Если записи нет — выделяет новую страницу и создаёт запись.
unsafe fn ensure_table(entry: &mut PageEntry, extra_flags: u64) -> *mut PageTable {
    if entry.present() {
        entry.address() as *mut PageTable
    } else {
        let phys = super::phys::alloc_page()
            .expect("Out of memory for page table");
        let ptr = phys as *mut PageTable;
        core::ptr::write_bytes(ptr, 0, 1);
        entry.set(phys, flags::PRESENT | flags::WRITABLE | extra_flags);
        ptr
    }
}

/// Инвалидирует весь TLB (после массового изменения таблиц страниц).
pub fn flush_tlb() {
    unsafe {
        let cr3: usize;
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
        core::arch::asm!("mov cr3, {}", in(reg) cr3);
    }
}
