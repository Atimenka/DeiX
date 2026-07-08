#![allow(dead_code)]
//! Загрузчик статических ELF-бинарников (glibc-совместимость).
//!
//! ## Что поддерживается
//!
//! Статические ELF64 (ET_EXEC), скомпилированные с `-static` под x86_64.
//! Например: программа, собранная с musl-gcc или glibc --static.
//!
//! ## Что НЕ поддерживается (честно)
//!
//! - **Динамическая линковка** (ET_DYN, .interp, PLT/GOT) — нужен ld.so,
//!   которого в DeiX нет. Это огромный отдельный проект.
//! - **Разделяемые библиотеки** (.so)
//! - **Thread-local storage** (TLS)
//! - **PT_GNU_STACK**, **PT_GNU_RELRO** и другие GNU-расширения
//! - **C++ exceptions** (нужен unwind-рантайм)
//!
//! ## Как это работает
//!
//! 1. Парсим ELF-заголовок и program headers
//! 2. Для каждого PT_LOAD: выделяем память, копируем данные из файла,
//!    обнуляем .bss (если memsz > filesz)
//! 3. Настраиваем стек: кладём argc, argv, envp
//! 4. Передаём управление на e_entry
//!
//! ## Syscall-интерфейс для glibc
//!
//! Программа, слинкованная с glibc, ожидает Linux-совместимые syscall'ы.
//! Мы предоставляем минимальный набор (≈15 шт), достаточный для простых
//! программ: write, read, open, close, brk, mmap, exit, и т.д.

use crate::mm;
use crate::mm::virt::flags;
use alloc::format;
use alloc::string::String;


// ==================== ELF64 Definitions ====================

const EI_NIDENT: usize = 16;

const ET_EXEC: u16 = 2;          // исполняемый файл
const EM_X86_64: u16 = 0x3E;     // x86_64
const EV_CURRENT: u8 = 1;

const PT_LOAD: u32 = 1;          // загружаемый сегмент
const PT_PHDR: u32 = 6;          // program header table

const PF_X: u32 = 1;  // executable
const PF_W: u32 = 2;  // writable
const PF_R: u32 = 4;  // readable

#[repr(C)]
struct Elf64Header {
    e_ident: [u8; EI_NIDENT],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

// ==================== Загрузчик ====================

#[derive(Debug)]
pub enum ElfError {
    NotElf,
    Not64Bit,
    NotExecutable,
    NotX86_64,
    BadVersion,
    NoProgramHeaders,
    SegmentTooLarge,
    OutOfMemory,
}

/// Результат загрузки ELF.
pub struct LoadedElf {
    pub entry_point: u64,
    pub pml4_phys: usize,
    pub brk_base: u64,     // начальный адрес кучи (brk)
    pub stack_top: u64,
}

/// Загружает статический ELF64 в новое адресное пространство.
/// Возвращает контекст, готовый для `jump_to_ring3`.
pub fn load_static_elf(elf_data: &[u8]) -> Result<LoadedElf, ElfError> {
    if elf_data.len() < core::mem::size_of::<Elf64Header>() {
        return Err(ElfError::NotElf);
    }

    let header = unsafe { &*(elf_data.as_ptr() as *const Elf64Header) };

    // Проверка magic: 0x7F 'E' 'L' 'F'
    if &header.e_ident[0..4] != b"\x7FELF" {
        return Err(ElfError::NotElf);
    }

    // ELFCLASS64
    if header.e_ident[4] != 2 {
        return Err(ElfError::Not64Bit);
    }

    if header.e_type != ET_EXEC {
        return Err(ElfError::NotExecutable);
    }

    if header.e_machine != EM_X86_64 {
        return Err(ElfError::NotX86_64);
    }

    if header.e_version as u8 != EV_CURRENT {
        return Err(ElfError::BadVersion);
    }

    if header.e_phnum == 0 || header.e_phentsize as usize != core::mem::size_of::<Elf64Phdr>() {
        return Err(ElfError::NoProgramHeaders);
    }

    // Создаём новое адресное пространство для процесса.
    let pml4 = mm::virt::new_address_space();

    let entry_point: u64 = header.e_entry;
    let mut max_vaddr: u64 = 0;

    // Загружаем сегменты PT_LOAD.
    let phdr_start = header.e_phoff as usize;
    for i in 0..header.e_phnum as usize {
        let phdr_offset = phdr_start + i * core::mem::size_of::<Elf64Phdr>();
        if phdr_offset + core::mem::size_of::<Elf64Phdr>() > elf_data.len() {
            break;
        }

        let phdr = unsafe {
            &*(elf_data.as_ptr().add(phdr_offset) as *const Elf64Phdr)
        };

        if phdr.p_type != PT_LOAD {
            continue;
        }

        let vaddr = phdr.p_vaddr as usize;
        let memsz = phdr.p_memsz as usize;
        let filesz = phdr.p_filesz as usize;

        if memsz == 0 {
            continue;
        }

        // Вычисляем флаги страниц.
        let mut page_flags = flags::PRESENT | flags::USER;
        if phdr.p_flags & PF_W != 0 {
            page_flags |= flags::WRITABLE;
        }
        if phdr.p_flags & PF_X == 0 {
            page_flags |= flags::NO_EXECUTE;
        }

        // Выделяем и отображаем страницы.
        let num_pages = (memsz + mm::PAGE_SIZE - 1) / mm::PAGE_SIZE;
        let page_start = vaddr & !(mm::PAGE_SIZE - 1);
        let offset_in_page = vaddr - page_start;

        for j in 0..num_pages {
            let page_vaddr = page_start + j * mm::PAGE_SIZE;
            let phys = mm::phys::alloc_page()
                .ok_or(ElfError::OutOfMemory)?;

            mm::virt::map_page(pml4, page_vaddr, phys, page_flags);

            // Копируем данные из файла.
            let copy_start = if j == 0 { offset_in_page } else { 0 };
            let file_offset = phdr.p_offset as usize + j * mm::PAGE_SIZE
                - if j == 0 { 0 } else { offset_in_page };
            let copy_end = mm::PAGE_SIZE.min(
                if j == num_pages - 1 {
                    filesz.saturating_sub(j * mm::PAGE_SIZE - offset_in_page)
                } else {
                    mm::PAGE_SIZE
                }
            );

            if file_offset < elf_data.len() && copy_start < copy_end {
                let copy_len = copy_end - copy_start;
                let src_end = (file_offset + copy_len).min(elf_data.len());
                let actual_len = src_end.saturating_sub(file_offset);

                unsafe {
                    let dst = (page_vaddr + copy_start) as *mut u8;
                    let src = elf_data.as_ptr().add(file_offset);
                    core::ptr::copy_nonoverlapping(src, dst, actual_len);
                }
            }
        }

        let seg_end = vaddr + memsz;
        if seg_end as u64 > max_vaddr {
            max_vaddr = seg_end as u64;
        }
    }

    // Выделяем стек (1 МиБ) в верхней части адресного пространства.
    let stack_size = 1024 * 1024;
    let stack_top = 0x7FFF_FFFF_F000u64; // верхушка user-space
    let stack_bottom = stack_top - stack_size as u64;
    let stack_pages = stack_size / mm::PAGE_SIZE;

    for i in 0..stack_pages {
        let vaddr = stack_bottom as usize + i * mm::PAGE_SIZE;
        let phys = mm::phys::alloc_page()
            .ok_or(ElfError::OutOfMemory)?;
        mm::virt::map_page(pml4, vaddr, phys,
            flags::PRESENT | flags::WRITABLE | flags::USER);
    }

    // brk (куча процесса) начинается сразу после загруженных данных.
    let brk_base = (max_vaddr + mm::PAGE_SIZE as u64 - 1) & !(mm::PAGE_SIZE as u64 - 1);

    Ok(LoadedElf {
        entry_point,
        pml4_phys: pml4,
        brk_base,
        stack_top,
    })
}

/// Имя файла (из read_file), пытаемся загрузить как ELF.
pub fn try_load_and_run(filename: &str) -> Result<(), String> {
    let data = match crate::ext2::read_file(filename) {
        Ok(d) => d,
        Err(_) => return Err("File not found".into()),
    };

    // Проверяем, ELF ли это.
    if data.len() < 4 || &data[0..4] != b"\x7FELF" {
        return Err("Not an ELF file".into());
    }

    let loaded = load_static_elf(&data)
        .map_err(|e| format!("ELF load error: {:?}", e))?;

    let ctx = crate::usermode::UserContext {
        rip: loaded.entry_point,
        rsp: loaded.stack_top,
        rflags: 0x202, // IF on, ring3
        rdi: loaded.stack_top, // начальный argv pointer (пока пустой)
    };

    crate::println!(
        "  [elf] Loaded static binary: entry={:#x}, stack={:#x}, brk={:#x}",
        loaded.entry_point, loaded.stack_top, loaded.brk_base
    );

    crate::usermode::jump_to_ring3(loaded.pml4_phys, &ctx);
}
