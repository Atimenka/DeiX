//! Загрузчик ELF64 для x86-64 — разбор и размещение программ Linux.
//!
//! Поддерживается то, что реально можно запустить в DeiX сегодня:
//! статические PIE (`ET_DYN` без интерпретатора). Для них выполняются
//! релокации `R_X86_64_RELATIVE` — этого достаточно, потому что у
//! статического PIE нет внешних символов.
//!
//! Остальные типы честно отклоняются с объяснением причины, а не
//! падают в неопределённое поведение.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::{USER_IMAGE_BASE, USER_IMAGE_MAX, USER_STACK_BOTTOM, USER_STACK_TOP};

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EM_X86_64: u16 = 62;

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;

const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

// Теги динамической секции.
const DT_NULL: u64 = 0;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_RELAENT: u64 = 9;

const R_X86_64_RELATIVE: u32 = 8;

fn rd_u16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(o)?, *b.get(o + 1)?]))
}
fn rd_u32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(o)?,
        *b.get(o + 1)?,
        *b.get(o + 2)?,
        *b.get(o + 3)?,
    ]))
}
fn rd_u64(b: &[u8], o: usize) -> Option<u64> {
    let mut v = [0u8; 8];
    for (i, s) in v.iter_mut().enumerate() {
        *s = *b.get(o + i)?;
    }
    Some(u64::from_le_bytes(v))
}

/// Программный заголовок.
struct Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_filesz: u64,
    p_memsz: u64,
}

/// Успешно загруженная программа.
pub struct Loaded {
    /// Абсолютный адрес точки входа (уже с учётом базы).
    pub entry: u64,
    /// Вершина стека для программы.
    pub stack_top: u64,
    /// Сколько сегментов PT_LOAD размещено.
    pub segments: usize,
}

fn parse_phdrs(img: &[u8]) -> Result<Vec<Phdr>, String> {
    let phoff = rd_u64(img, 32).ok_or("обрезанный заголовок")?;
    let phentsize = rd_u16(img, 54).ok_or("обрезанный заголовок")? as usize;
    let phnum = rd_u16(img, 56).ok_or("обрезанный заголовок")? as usize;

    if phentsize < 56 {
        return Err(format!("странный размер phdr: {}", phentsize));
    }

    let mut out = Vec::with_capacity(phnum);
    for i in 0..phnum {
        let o = phoff as usize + i * phentsize;
        out.push(Phdr {
            p_type: rd_u32(img, o).ok_or("обрезанная таблица программ")?,
            p_flags: rd_u32(img, o + 4).ok_or("обрезанная таблица программ")?,
            p_offset: rd_u64(img, o + 8).ok_or("обрезанная таблица программ")?,
            p_vaddr: rd_u64(img, o + 16).ok_or("обрезанная таблица программ")?,
            p_filesz: rd_u64(img, o + 32).ok_or("обрезанная таблица программ")?,
            p_memsz: rd_u64(img, o + 40).ok_or("обрезанная таблица программ")?,
        });
    }
    Ok(out)
}

/// Базовые проверки формата, общие для загрузки и диагностики.
fn check_header(img: &[u8]) -> Result<(u16, u64), String> {
    if img.len() < 64 {
        return Err("файл меньше заголовка ELF".to_string());
    }
    if img[0..4] != ELF_MAGIC {
        return Err(format!(
            "не ELF (первые байты {:02x} {:02x} {:02x} {:02x})",
            img[0], img[1], img[2], img[3]
        ));
    }
    if img[4] != ELFCLASS64 {
        return Err("не 64-битный ELF (нужен ELFCLASS64)".to_string());
    }
    if img[5] != ELFDATA2LSB {
        return Err("не little-endian".to_string());
    }
    let e_type = rd_u16(img, 16).ok_or("обрезанный заголовок")?;
    let e_machine = rd_u16(img, 18).ok_or("обрезанный заголовок")?;
    if e_machine != EM_X86_64 {
        return Err(format!("архитектура {} != x86-64", e_machine));
    }
    let entry = rd_u64(img, 24).ok_or("обрезанный заголовок")?;
    Ok((e_type, entry))
}

/// Диагностика без запуска.
pub fn inspect(img: &[u8]) -> Result<Vec<String>, String> {
    let (e_type, entry) = check_header(img)?;
    let phdrs = parse_phdrs(img)?;

    let mut out = Vec::new();
    out.push(format!(
        "тип: {}",
        match e_type {
            ET_EXEC => "EXEC (абсолютный адрес)",
            ET_DYN => "DYN (PIE / разделяемый)",
            _ => "другой",
        }
    ));
    out.push(format!("точка входа: {:#x}", entry));

    let interp = phdrs.iter().find(|p| p.p_type == PT_INTERP);
    match interp {
        Some(p) => {
            let s = img
                .get(p.p_offset as usize..(p.p_offset + p.p_filesz) as usize)
                .unwrap_or(&[]);
            let name: String = s.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
            out.push(format!("интерпретатор: {} (динамический)", name));
        }
        None => out.push("интерпретатор: нет (статический)".to_string()),
    }

    for p in phdrs.iter().filter(|p| p.p_type == PT_LOAD) {
        out.push(format!(
            "  LOAD vaddr={:#x} filesz={} memsz={} flags={}{}{}",
            p.p_vaddr,
            p.p_filesz,
            p.p_memsz,
            if p.p_flags & PF_R != 0 { "R" } else { "-" },
            if p.p_flags & PF_W != 0 { "W" } else { "-" },
            if p.p_flags & PF_X != 0 { "X" } else { "-" },
        ));
    }
    Ok(out)
}

/// Применяет релокации RELA (нужны для PIE).
///
/// У статического PIE все релокации — `R_X86_64_RELATIVE`: в память по
/// адресу `base + r_offset` кладётся `base + r_addend`.
fn apply_relocations(img: &[u8], phdrs: &[Phdr], base: u64) -> Result<usize, String> {
    let dynseg = match phdrs.iter().find(|p| p.p_type == PT_DYNAMIC) {
        Some(d) => d,
        None => return Ok(0),
    };

    let mut rela: Option<u64> = None;
    let mut relasz: u64 = 0;
    let mut relaent: u64 = 24;

    let mut o = dynseg.p_offset as usize;
    let end = o + dynseg.p_filesz as usize;
    while o + 16 <= end {
        let tag = rd_u64(img, o).ok_or("обрезанная .dynamic")?;
        let val = rd_u64(img, o + 8).ok_or("обрезанная .dynamic")?;
        match tag {
            DT_NULL => break,
            DT_RELA => rela = Some(val),
            DT_RELASZ => relasz = val,
            DT_RELAENT => relaent = val,
            _ => {}
        }
        o += 16;
    }

    let rela = match rela {
        Some(r) if relasz > 0 && relaent >= 24 => r,
        _ => return Ok(0),
    };

    // rela — виртуальный адрес; переводим в смещение в файле по PT_LOAD.
    let file_off = |vaddr: u64| -> Option<u64> {
        for p in phdrs.iter().filter(|p| p.p_type == PT_LOAD) {
            if vaddr >= p.p_vaddr && vaddr < p.p_vaddr + p.p_filesz {
                return Some(p.p_offset + (vaddr - p.p_vaddr));
            }
        }
        None
    };
    let mut ro = file_off(rela).ok_or("RELA вне сегментов PT_LOAD")? as usize;
    let count = (relasz / relaent) as usize;
    let mut applied = 0usize;

    for _ in 0..count {
        let r_offset = rd_u64(img, ro).ok_or("обрезанная RELA")?;
        let r_info = rd_u64(img, ro + 8).ok_or("обрезанная RELA")?;
        let r_addend = rd_u64(img, ro + 16).ok_or("обрезанная RELA")?;
        ro += relaent as usize;

        let r_type = (r_info & 0xFFFF_FFFF) as u32;
        if r_type != R_X86_64_RELATIVE {
            // Внешние символы означают, что нужен динамический линковщик.
            return Err(format!(
                "релокация типа {} не поддерживается (нужен ld.so)",
                r_type
            ));
        }
        let target = base + r_offset;
        if target < USER_IMAGE_BASE || target + 8 > USER_IMAGE_BASE + USER_IMAGE_MAX {
            return Err("релокация указывает вне образа".to_string());
        }
        unsafe { core::ptr::write_unaligned(target as *mut u64, base + r_addend) };
        applied += 1;
    }
    Ok(applied)
}

/// Загружает программу в память и готовит стек.
pub fn load(img: &[u8]) -> Result<Loaded, String> {
    let (e_type, entry) = check_header(img)?;
    let phdrs = parse_phdrs(img)?;

    if phdrs.iter().any(|p| p.p_type == PT_INTERP) {
        return Err(
            "динамический бинарник: нужен ld-linux-x86-64.so.2 и glibc, \
             пока поддерживаются только статические"
                .to_string(),
        );
    }

    let loads: Vec<&Phdr> = phdrs.iter().filter(|p| p.p_type == PT_LOAD).collect();
    if loads.is_empty() {
        return Err("нет сегментов PT_LOAD".to_string());
    }

    let min_vaddr = loads.iter().map(|p| p.p_vaddr).min().unwrap();
    let max_vaddr = loads.iter().map(|p| p.p_vaddr + p.p_memsz).max().unwrap();
    let span = max_vaddr - min_vaddr;

    if span > USER_IMAGE_MAX {
        return Err(format!(
            "образ {} КиБ больше лимита {} КиБ",
            span / 1024,
            USER_IMAGE_MAX / 1024
        ));
    }

    // База размещения: PIE двигаем на USER_IMAGE_BASE, EXEC обязан
    // лечь по своему адресу — а он у Linux-бинарников 0x400000, где
    // находится куча ядра.
    let base = match e_type {
        ET_DYN => USER_IMAGE_BASE.wrapping_sub(min_vaddr),
        ET_EXEC => {
            return Err(format!(
                "статический не-PIE: слинкован по {:#x}, там куча ядра. \
                 Пересоберите с -static-pie",
                min_vaddr
            ))
        }
        _ => return Err("неподдерживаемый тип ELF".to_string()),
    };

    // Очищаем область и раскладываем сегменты.
    unsafe {
        core::ptr::write_bytes(USER_IMAGE_BASE as *mut u8, 0, USER_IMAGE_MAX as usize);
    }

    for p in loads.iter() {
        let dst = base + p.p_vaddr;
        if dst < USER_IMAGE_BASE || dst + p.p_memsz > USER_IMAGE_BASE + USER_IMAGE_MAX {
            return Err("сегмент выходит за пределы области программы".to_string());
        }
        let src = img
            .get(p.p_offset as usize..(p.p_offset + p.p_filesz) as usize)
            .ok_or("сегмент выходит за границы файла")?;
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), dst as *mut u8, src.len());
            // .bss — разница memsz и filesz — уже обнулена выше.
        }
    }

    let relocs = apply_relocations(img, &phdrs, base)?;
    if relocs > 0 {
        crate::println!("  [linux] применено релокаций: {}", relocs);
    }

    // Стек: минимальный System V ABI — argc=0, argv[0]=NULL, envp=NULL,
    // auxv=AT_NULL. Многие программы читают это сразу на входе.
    let stack_top = USER_STACK_TOP & !0xF;
    unsafe {
        core::ptr::write_bytes(
            USER_STACK_BOTTOM as *mut u8,
            0,
            (stack_top - USER_STACK_BOTTOM) as usize,
        );
        let sp = (stack_top - 64) as *mut u64;
        core::ptr::write(sp, 0); // argc = 0
        core::ptr::write(sp.add(1), 0); // argv[0] = NULL
        core::ptr::write(sp.add(2), 0); // envp[0] = NULL
        core::ptr::write(sp.add(3), 0); // auxv: AT_NULL
        core::ptr::write(sp.add(4), 0);
    }


    Ok(Loaded {
        entry: base + entry,
        stack_top: stack_top - 64,
        segments: loads.len(),
    })
}
