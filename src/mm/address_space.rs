//! Private x86-64 page-table construction for future scheduled user processes.
//!
//! Kernel mappings are copied as supervisor-only. User pages live in an empty
//! PML4 slot, are separately allocated, and are never shared with other spaces.
//! Constructing a table does not switch CR3 or start a process; its owner must
//! ensure that it is no longer active on any CPU before it is dropped.
//!
//! Current boot uses an identity map for the kernel heap below 4 GiB. The
//! kernel build rejects allocations outside that map instead of treating an
//! arbitrary kernel virtual address as a physical address.

use alloc::alloc::{alloc_zeroed, dealloc};
use alloc::vec::Vec;
use core::alloc::Layout;
use core::ptr::NonNull;

const PAGE: usize = 4096;
const ENTRIES: usize = 512;
const PRESENT: u64 = 1;
const WRITE: u64 = 2;
const USER: u64 = 4;
const ADDRESS: u64 = 0x000f_ffff_ffff_f000;
/// PML4 slot 1, separate from the kernel identity map in slot 0.
pub const USER_BASE: u64 = 0x0000_0080_0000_0000;
pub const USER_END: u64 = 0x0000_0100_0000_0000;
const MAX_USER_PAGES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error { OutOfMemory, KernelMappingConflict, InvalidAddress, AlreadyMapped, NotMapped, InvalidPhysicalAddress }

struct Page { ptr: NonNull<u64> }
impl Page {
    fn new() -> Result<Self, Error> {
        let layout = Layout::from_size_align(PAGE, PAGE).unwrap();
        let ptr = NonNull::new(unsafe { alloc_zeroed(layout) } as *mut u64).ok_or(Error::OutOfMemory)?;
        let page = Self { ptr };
        // Kernel allocation addresses must be identity mapped physical RAM.
        #[cfg(not(test))]
        if page.address() >= 0x1_0000_0000 { return Err(Error::InvalidPhysicalAddress); }
        if page.address() & !ADDRESS != 0 { return Err(Error::InvalidPhysicalAddress); }
        Ok(page)
    }
    fn address(&self) -> u64 { self.ptr.as_ptr() as u64 }
    fn entries(&self) -> &[u64; ENTRIES] { unsafe { &*(self.ptr.as_ptr() as *const [u64; ENTRIES]) } }
    fn entries_mut(&mut self) -> &mut [u64; ENTRIES] { unsafe { &mut *(self.ptr.as_ptr() as *mut [u64; ENTRIES]) } }
    fn bytes_mut(&mut self) -> &mut [u8; PAGE] { unsafe { &mut *(self.ptr.as_ptr() as *mut [u8; PAGE]) } }
}
impl Drop for Page {
    fn drop(&mut self) { unsafe { dealloc(self.ptr.as_ptr() as *mut u8, Layout::from_size_align(PAGE, PAGE).unwrap()) } }
}

pub struct AddressSpace {
    // All tables and data pages are owned; the allocations do not move when
    // Vec reallocates, and table addresses remain stable.
    pages: Vec<Page>,
    user_pages: usize,
}
impl AddressSpace {
    /// Copies kernel PML4 entries but strips user permission at the root.
    /// Slot 1 must be unused by the kernel and is reserved for user pages.
    pub fn new(kernel_root: &[u64; ENTRIES]) -> Result<Self, Error> {
        if kernel_root[1] & PRESENT != 0 { return Err(Error::KernelMappingConflict); }
        let mut root = Page::new()?;
        for (dst, src) in root.entries_mut().iter_mut().zip(kernel_root.iter()) { *dst = *src & !USER; }
        Ok(Self { pages: alloc::vec![root], user_pages: 0 })
    }
    /// Physical address of the root; caller must manage CR3 and TLB lifetime.
    pub fn root_physical(&self) -> u64 { self.pages[0].address() }
    fn indices(va: u64) -> Result<[usize; 4], Error> {
        if !(USER_BASE..USER_END).contains(&va) || va & (PAGE as u64 - 1) != 0 { return Err(Error::InvalidAddress); }
        Ok([((va >> 39) & 511) as usize, ((va >> 30) & 511) as usize,
            ((va >> 21) & 511) as usize, ((va >> 12) & 511) as usize])
    }
    fn owned_index(&self, entry: u64) -> Result<usize, Error> {
        let addr = entry & ADDRESS;
        self.pages.iter().position(|p| p.address() == addr).ok_or(Error::InvalidPhysicalAddress)
    }
    fn leaf_table(&self, va: u64) -> Result<(usize, usize), Error> {
        let idx = Self::indices(va)?;
        let mut table = 0;
        for i in &idx[..3] {
            let entry = self.pages[table].entries()[*i];
            if entry & PRESENT == 0 { return Err(Error::NotMapped); }
            table = self.owned_index(entry)?;
        }
        Ok((table, idx[3]))
    }
    /// Allocates a zero-filled, private user page. Executability is not yet
    /// controlled here (NX setup is not enabled by the existing boot code).
    pub fn map_zeroed(&mut self, va: u64, writable: bool) -> Result<(), Error> {
        let idx = Self::indices(va)?;
        if self.user_pages >= MAX_USER_PAGES { return Err(Error::OutOfMemory); }
        let mut table = 0;
        for i in &idx[..3] {
            let entry = self.pages[table].entries()[*i];
            if entry & PRESENT != 0 {
                table = self.owned_index(entry)?;
            } else {
                let page = Page::new()?;
                let address = page.address();
                self.pages.push(page);
                self.pages[table].entries_mut()[*i] = address | PRESENT | WRITE | USER;
                table = self.pages.len() - 1;
            }
        }
        if self.pages[table].entries()[idx[3]] & PRESENT != 0 { return Err(Error::AlreadyMapped); }
        let page = Page::new()?;
        let address = page.address();
        self.pages.push(page);
        self.pages[table].entries_mut()[idx[3]] = address | PRESENT | USER | if writable { WRITE } else { 0 };
        self.user_pages += 1;
        Ok(())
    }
    /// Check an address against this space's owned PTEs, without dereferencing it.
    pub fn user_accessible(&self, address: u64, write: bool) -> bool {
        let page = address & !(PAGE as u64 - 1);
        let Ok((table, index)) = self.leaf_table(page) else { return false; };
        let entry = self.pages[table].entries()[index];
        entry & (PRESENT | USER) == (PRESENT | USER)
            && (!write || entry & WRITE != 0)
            && self.owned_index(entry).is_ok()
    }
    /// Kernel-side initialization of a mapped user page, including read-only
    /// code pages. This privileged loader access bypasses the user PTE flags.
    pub fn page_for_loading(&mut self, va: u64) -> Result<&mut [u8; PAGE], Error> {
        let (table, index) = self.leaf_table(va)?;
        let entry = self.pages[table].entries()[index];
        if entry & PRESENT == 0 { return Err(Error::NotMapped); }
        let page = self.owned_index(entry)?;
        Ok(self.pages[page].bytes_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_tables_and_pages_do_not_alias() {
        let mut kernel = [0u64; ENTRIES]; kernel[0] = 0x2000 | PRESENT | WRITE | USER;
        let mut a = AddressSpace::new(&kernel).unwrap();
        let mut b = AddressSpace::new(&kernel).unwrap();
        assert_eq!(a.pages[0].entries()[0] & USER, 0);
        assert_ne!(a.root_physical(), b.root_physical());
        a.map_zeroed(USER_BASE, true).unwrap(); b.map_zeroed(USER_BASE, true).unwrap();
        a.page_for_loading(USER_BASE).unwrap()[8] = 42;
        assert_eq!(b.page_for_loading(USER_BASE).unwrap()[8], 0);
        assert_eq!(a.page_for_loading(USER_BASE).unwrap()[8], 42);
    }
    #[test]
    fn rejects_conflicts_and_non_user_addresses() {
        let mut root = [0; ENTRIES]; root[1] = PRESENT;
        assert!(matches!(AddressSpace::new(&root), Err(Error::KernelMappingConflict)));
        let mut a = AddressSpace::new(&[0; ENTRIES]).unwrap();
        assert_eq!(a.map_zeroed(0x1000, true), Err(Error::InvalidAddress));
        assert_eq!(a.map_zeroed(USER_END, true), Err(Error::InvalidAddress));
        assert_eq!(a.map_zeroed(USER_BASE + 1, true), Err(Error::InvalidAddress));
        a.map_zeroed(USER_BASE, true).unwrap();
        assert_eq!(a.map_zeroed(USER_BASE, true), Err(Error::AlreadyMapped));
        assert_eq!(a.page_for_loading(USER_BASE + PAGE as u64), Err(Error::NotMapped));
    }
    #[test]
    fn read_only_flag_is_enforced() {
        let mut a = AddressSpace::new(&[0; ENTRIES]).unwrap();
        a.map_zeroed(USER_BASE, false).unwrap();
        let (table, i) = a.leaf_table(USER_BASE).unwrap();
        assert_eq!(a.pages[table].entries()[i] & WRITE, 0);
        assert_ne!(a.pages[table].entries()[i] & USER, 0);
    }
}
