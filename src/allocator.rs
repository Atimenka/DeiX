//! Простой аллокатор кучи (heap) для нашего мини-ядра.

use crate::sync::without_interrupts;
use core::alloc::{GlobalAlloc, Layout};
use core::mem;
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

const HEAP_SIZE: usize = 16 * 1024 * 1024;
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

#[repr(align(16))]
struct HeapStorage([u8; HEAP_SIZE]);

static mut HEAP_STORAGE: HeapStorage = HeapStorage([0; HEAP_SIZE]);

struct FreeBlock {
    size: usize,
    next: Option<&'static mut FreeBlock>,
}

impl FreeBlock {
    const fn new(size: usize) -> Self {
        FreeBlock { size, next: None }
    }

    fn start_addr(&self) -> usize {
        self as *const _ as usize
    }

    fn end_addr(&self) -> usize {
        self.start_addr() + self.size
    }
}

struct LinkedListAllocator {
    head: FreeBlock,
    initialized: bool,
}

impl LinkedListAllocator {
    const fn new() -> Self {
        LinkedListAllocator {
            head: FreeBlock::new(0),
            initialized: false,
        }
    }

    unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.add_free_region(heap_start, heap_size);
        self.initialized = true;
    }

    unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
        assert!(align_up(addr, mem::align_of::<FreeBlock>()) == addr);
        assert!(size >= mem::size_of::<FreeBlock>());

        let mut node = FreeBlock::new(size);
        node.next = self.head.next.take();
        let node_ptr = addr as *mut FreeBlock;
        node_ptr.write(node);
        self.head.next = Some(&mut *node_ptr);
    }

    fn find_region(&mut self, size: usize, align: usize) -> Option<(&'static mut FreeBlock, usize)> {
        let mut current = &mut self.head;
        while let Some(ref mut region) = current.next {
            if let Ok(alloc_start) = Self::alloc_from_region(region, size, align) {
                let next = region.next.take();
                let ret = current.next.take().unwrap();
                current.next = next;
                return Some((ret, alloc_start));
            } else {
                current = current.next.as_mut().unwrap();
            }
        }
        None
    }

    fn alloc_from_region(region: &FreeBlock, size: usize, align: usize) -> Result<usize, ()> {
        let alloc_start = align_up(region.start_addr(), align);
        let alloc_end = alloc_start.checked_add(size).ok_or(())?;

        if alloc_end > region.end_addr() {
            return Err(());
        }

        let excess_size = region.end_addr() - alloc_end;
        if excess_size > 0 && excess_size < mem::size_of::<FreeBlock>() {
            return Err(());
        }

        Ok(alloc_start)
    }

    fn size_align(layout: Layout) -> (usize, usize) {
        let layout = layout
            .align_to(mem::align_of::<FreeBlock>())
            .expect("align_to failed")
            .pad_to_align();
        let size = layout.size().max(mem::size_of::<FreeBlock>());
        (size, layout.align())
    }
}

fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

struct LockedAllocator {
    inner: crate::spinlock::SpinLock<LinkedListAllocator>,
}

impl LockedAllocator {
    const fn new() -> Self {
        LockedAllocator {
            inner: crate::spinlock::SpinLock::new(LinkedListAllocator::new()),
        }
    }

    pub fn init(&self) {
        without_interrupts(|| {
            let mut alloc = self.inner.lock();
            if !alloc.initialized {
                let heap_start = ptr::addr_of_mut!(HEAP_STORAGE) as usize;
                unsafe { alloc.init(heap_start, HEAP_SIZE) };
            }
        });
    }
}

unsafe impl GlobalAlloc for LockedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        without_interrupts(|| {
            let (size, align) = LinkedListAllocator::size_align(layout);
            let mut allocator = self.inner.lock();

            if let Some((region, alloc_start)) = allocator.find_region(size, align) {
                let alloc_end = alloc_start.checked_add(size).expect("overflow");
                let excess_size = region.end_addr() - alloc_end;
                if excess_size > 0 {
                    allocator.add_free_region(alloc_end, excess_size);
                }
                ALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
                alloc_start as *mut u8
            } else {
                ptr::null_mut()
            }
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        without_interrupts(|| {
            let (size, _) = LinkedListAllocator::size_align(layout);
            let mut allocator = self.inner.lock();
            allocator.add_free_region(ptr as usize, size);
            ALLOCATED_BYTES.fetch_sub(size.min(ALLOCATED_BYTES.load(Ordering::Relaxed)), Ordering::Relaxed);
        })
    }
}

#[global_allocator]
static ALLOCATOR: LockedAllocator = LockedAllocator::new();

pub fn init() {
    ALLOCATOR.init();
}

pub fn total_heap_bytes() -> usize {
    HEAP_SIZE
}

pub fn allocated_heap_bytes() -> usize {
    ALLOCATED_BYTES.load(Ordering::Relaxed)
}

#[alloc_error_handler]
fn alloc_error_handler(layout: Layout) -> ! {
    panic!("allocation error: {:?}", layout);
}
