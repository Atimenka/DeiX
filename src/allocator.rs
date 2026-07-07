//! Простой аллокатор кучи (heap) для нашего мини-ядра.
//!
//! Загрузчик (boot/stage2.asm) настраивает identity-mapping первого 4GiB
//! физической памяти через 2MiB huge pages, поэтому нам не нужно отдельно
//! возиться со страницами — достаточно зарезервировать статический массив
//! в .bss и отдать его аллокатору как область кучи.
//!
//! Алгоритм — классический связный список свободных блоков (linked list
//! allocator, first-fit): просто, понятно, для мини-ОС более чем достаточно.
//! Не самый быстрый и склонен к фрагментации при долгой работе, но у нас
//! пока нет процессов, которые бы гоняли аллокации сутками.
//!
//! ВАЖНО, физическое расположение HEAP_STORAGE: этот статический массив
//! обязан начинаться ВЫШЕ физического адреса 0x100000 (1 МиБ). Классическая
//! карта памяти PC резервирует диапазон 0xA0000-0xFFFFF (640 КиБ - 1 МиБ)
//! под legacy VGA framebuffer (0xA0000-0xBFFFF) и теневую память BIOS/
//! опциональных ROM (0xC0000-0xFFFFF) — на этих физических адресах стоит
//! MMIO-alias видеокарты, а не настоящая RAM, поэтому запись туда либо не
//! сохраняется, либо считывается обратно как "чужие" данные видеокарты,
//! даже при полном идентити-маппинге страниц (страница честно смаплена,
//! но физически это не DRAM). Раньше куча естественным образом
//! размещалась линкером сразу после короткого ассемблерного .bss (около
//! физического адреса 0x44000) и была настолько большой (на тот момент
//! 16 МиБ), что простиралась через всю эту "дыру" — если аллокация вроде
//! back buffer графического рендерера (renderer.rs) попадала на адрес
//! внутри 0xA0000-0xFFFFF, то на экране появлялся необъяснимый "шум"
//! (мусорные пиксели), не лечившийся никакими изменениями в самой логике
//! рендеринга, потому что причина была не в рендеринге, а в том, что
//! часть "кучи" физически не была настоящей оперативной памятью.
//! Исправлено в boot/linker2.ld: линкер теперь явно "перепрыгивает"
//! адрес счётчика на 0x200000 (2 МиБ) перед Rust-частью .bss, так что
//! HEAP_STORAGE гарантированно оказывается выше всей опасной зоны.
use crate::sync::without_interrupts;
use core::alloc::{GlobalAlloc, Layout};
use core::mem;
use core::ptr;

const HEAP_SIZE: usize = 16 * 1024 * 1024;

#[repr(align(16))]
#[allow(dead_code)]
struct HeapStorage([u8; HEAP_SIZE]);

static mut HEAP_STORAGE: HeapStorage = HeapStorage([0; HEAP_SIZE]);

struct FreeBlock {
    size: usize,
    next: Option<&'static mut FreeBlock>,
}

impl FreeBlock {
    fn new(size: usize) -> Self {
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
            head: FreeBlock { size: 0, next: None },
            initialized: false,
        }
    }

    unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.add_free_region(heap_start, heap_size);
        self.initialized = true;
    }

    unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
        assert_eq!(align_up(addr, mem::align_of::<FreeBlock>()), addr);
        assert!(size >= mem::size_of::<FreeBlock>());

        let mut block = FreeBlock::new(size);
        block.next = self.head.next.take();
        let block_ptr = addr as *mut FreeBlock;
        block_ptr.write(block);
        self.head.next = Some(&mut *block_ptr);
    }

    /// Ищет свободный блок, в который поместится запрошенный размер, и
    /// возвращает его вместе с ссылкой на предыдущий элемент списка (чтобы
    /// можно было "вырезать" найденный блок из списка).
    fn find_region(&mut self, size: usize, align: usize) -> Option<(&'static mut FreeBlock, usize)> {
        let mut current = &mut self.head;

        while let Some(ref mut region) = current.next {
            if let Ok(alloc_start) = Self::alloc_from_region(region, size, align) {
                let next = region.next.take();
                let ret = Some((current.next.take().unwrap(), alloc_start));
                current.next = next;
                return ret;
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
            // остаток слишком мал, чтобы хранить в нём FreeBlock — блок не подходит
            return Err(());
        }

        Ok(alloc_start)
    }

    fn size_align(layout: Layout) -> (usize, usize) {
        let layout = layout
            .align_to(mem::align_of::<FreeBlock>())
            .expect("adjusting alignment failed")
            .pad_to_align();
        let size = layout.size().max(mem::size_of::<FreeBlock>());
        (size, layout.align())
    }
}

fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

pub struct LockedAllocator {
    inner: crate::spinlock::SpinLock<LinkedListAllocator>,
}

impl LockedAllocator {
    pub const fn new() -> Self {
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
        })
    }
}

#[global_allocator]
static ALLOCATOR: LockedAllocator = LockedAllocator::new();

/// Вызывается один раз из kernel_main перед любым использованием
/// Vec/String/Box.
pub fn init() {
    ALLOCATOR.init();
}

/// Обработчик ошибки аллокации (вызывается, если alloc вернул null, а
/// компилятор ожидал успех — например, при `vec![...]` без ручной проверки).
#[alloc_error_handler]
fn alloc_error_handler(layout: Layout) -> ! {
    panic!("allocation error: {:?}", layout);
}
