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
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use super::{USER_IMAGE_BASE, USER_IMAGE_MAX, USER_STACK_SIZE};

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

    let start = usize::try_from(phoff).map_err(|_| "неверное смещение phdr")?;
    let table_size = phnum.checked_mul(phentsize).ok_or("переполнение таблицы phdr")?;
    let end = start.checked_add(table_size).ok_or("переполнение таблицы phdr")?;
    if end > img.len() { return Err("обрезанная таблица программ".to_string()); }
    let mut out = Vec::with_capacity(phnum);
    for i in 0..phnum {
        let o = start + i * phentsize;
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
            let s = file_range(img, p.p_offset, p.p_filesz).unwrap_or(&[]);
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
fn file_range(img: &[u8], offset: u64, length: u64) -> Option<&[u8]> {
    let start = usize::try_from(offset).ok()?;
    let end = start.checked_add(usize::try_from(length).ok()?)?;
    img.get(start..end)
}

/// Validate every relocation before any memory is written. The resulting
/// offsets refer to the original ELF virtual address space, not kernel memory.
fn relocation_plan(img: &[u8], phdrs: &[Phdr]) -> Result<Vec<(u64, i64)>, String> {
    let mut plan = Vec::new();
    let mut dynamic = phdrs.iter().filter(|p| p.p_type == PT_DYNAMIC);
    let Some(dynseg) = dynamic.next() else { return Ok(plan); };
    if dynamic.next().is_some() { return Err("несколько сегментов .dynamic".to_string()); }
    let data = file_range(img, dynseg.p_offset, dynseg.p_filesz).ok_or("обрезанная .dynamic")?;
    if data.len() % 16 != 0 { return Err("неверная длина .dynamic".to_string()); }
    let mut rela = None;
    let mut relasz = 0u64;
    let mut relaent = 24u64;
    let mut terminated = false;
    for entry in data.chunks_exact(16) {
        let tag = rd_u64(entry, 0).ok_or("обрезанная .dynamic")?;
        let value = rd_u64(entry, 8).ok_or("обрезанная .dynamic")?;
        match tag {
            DT_NULL => { terminated = true; break; }
            DT_RELA => rela = Some(value),
            DT_RELASZ => relasz = value,
            DT_RELAENT => relaent = value,
            _ => {}
        }
    }
    if !terminated { return Err(".dynamic без DT_NULL".to_string()); }
    if relasz == 0 { return Ok(plan); }
    let rela = rela.ok_or("DT_RELA отсутствует")?;
    if relaent != 24 || relasz % relaent != 0 { return Err("неверная таблица RELA".to_string()); }
    let rela_end = rela.checked_add(relasz).ok_or("переполнение RELA")?;
    let owner = phdrs.iter().filter(|p| p.p_type == PT_LOAD).find(|p| {
        p.p_vaddr <= rela && p.p_vaddr.checked_add(p.p_filesz).map_or(false, |end| rela_end <= end)
    }).ok_or("RELA вне сегментов PT_LOAD")?;
    let off = owner.p_offset.checked_add(rela - owner.p_vaddr).ok_or("переполнение RELA")?;
    let data = file_range(img, off, relasz).ok_or("обрезанная RELA")?;
    for item in data.chunks_exact(24) {
        let offset = rd_u64(item, 0).ok_or("обрезанная RELA")?;
        let info = rd_u64(item, 8).ok_or("обрезанная RELA")?;
        let addend = rd_u64(item, 16).ok_or("обрезанная RELA")? as i64;
        if info != R_X86_64_RELATIVE as u64 { return Err("неподдерживаемая релокация".to_string()); }
        let end = offset.checked_add(8).ok_or("переполнение цели RELA")?;
        if !phdrs.iter().filter(|p| p.p_type == PT_LOAD && p.p_flags & PF_W != 0).any(|p| {
            p.p_vaddr <= offset && p.p_vaddr.checked_add(p.p_memsz).map_or(false, |v| end <= v)
        }) { return Err("цель RELA вне записываемого сегмента".to_string()); }
        plan.push((offset, addend));
    }
    Ok(plan)
}

/// Загружает программу в глобальную область Ring 3 и готовит стек.
pub fn load(img: &[u8]) -> Result<Loaded, String> {
    let loaded = load_into(img, USER_IMAGE_BASE, USER_IMAGE_MAX)?;
    Ok(Loaded {
        entry: loaded.entry,
        stack_top: loaded.stack_top,
        segments: loaded.segments,
    })
}

/// Размещает PIE-образ по произвольной базе `base` и возвращает точку
/// входа. Буфер должен быть доступен ядру для записи; эта функция сама
/// не создаёт отдельное адресное пространство или изоляцию.
///
/// Работает только с PIE (`ET_DYN`): у таких образов все абсолютные
/// ссылки исправляются релокациями `R_X86_64_RELATIVE`, поэтому базу
/// можно выбрать. Не-PIE (`ET_EXEC`) слинкован на конкретный адрес и
/// перемещению не подлежит.
pub fn load_into(img: &[u8], base: u64, capacity: u64) -> Result<Loaded, String> {
    let (e_type, entry) = check_header(img)?;
    let phdrs = parse_phdrs(img)?;

    if phdrs.iter().any(|p| p.p_type == PT_INTERP) {
        return Err(
            "динамический бинарник: нужен ld-linux-x86-64.so.2 и glibc, \
             пока поддерживаются только статические"
                .to_string(),
        );
    }

    if e_type != ET_DYN {
        return Err("поддерживаются только статические PIE (ET_DYN)".to_string());
    }
    let loads: Vec<&Phdr> = phdrs.iter().filter(|p| p.p_type == PT_LOAD).collect();
    if loads.is_empty() { return Err("нет сегментов PT_LOAD".to_string()); }
    if capacity < USER_STACK_SIZE || capacity > isize::MAX as u64 {
        return Err("неверный размер области процесса".to_string());
    }
    let area_end = base.checked_add(capacity).ok_or("переполнение области процесса")?;
    let stack_bottom = area_end - USER_STACK_SIZE;
    let min_vaddr = loads.iter().map(|p| p.p_vaddr).min().unwrap();
    let bias = base.checked_sub(min_vaddr).ok_or("неверная база образа")?;
    let mut entry_ok = false;
    for (index, p) in loads.iter().enumerate() {
        if p.p_filesz > p.p_memsz { return Err("filesz больше memsz".to_string()); }
        file_range(img, p.p_offset, p.p_filesz).ok_or("сегмент вне файла")?;
        let end = p.p_vaddr.checked_add(p.p_memsz).ok_or("переполнение сегмента")?;
        let dst = bias.checked_add(p.p_vaddr).ok_or("переполнение адреса сегмента")?;
        let dst_end = bias.checked_add(end).ok_or("переполнение адреса сегмента")?;
        if dst < base || dst_end > stack_bottom { return Err("сегмент пересекает стек или выходит за область".to_string()); }
        if p.p_flags & PF_X != 0 && entry >= p.p_vaddr && entry < end { entry_ok = true; }
        for other in loads.iter().take(index) {
            let other_end = other.p_vaddr.checked_add(other.p_memsz).ok_or("переполнение сегмента")?;
            if p.p_memsz != 0 && other.p_memsz != 0 && p.p_vaddr < other_end && other.p_vaddr < end {
                return Err("перекрывающиеся сегменты".to_string());
            }
        }
    }
    if !entry_ok { return Err("точка входа вне исполняемого сегмента".to_string()); }
    let entry_address = bias.checked_add(entry).ok_or("переполнение точки входа")?;
    let relocations = relocation_plan(img, &phdrs)?;
    let mut writes = Vec::with_capacity(relocations.len());
    for (offset, addend) in relocations {
        let target = bias.checked_add(offset).ok_or("переполнение цели RELA")?;
        let value = if addend >= 0 { bias.checked_add(addend as u64) }
            else { bias.checked_sub(addend.unsigned_abs()) }
            .ok_or("переполнение значения RELA")?;
        writes.push((target, value));
    }

    // All file, layout, entry and relocation checks precede writes.
    unsafe { core::ptr::write_bytes(base as *mut u8, 0, capacity as usize); }
    for p in &loads {
        let src = file_range(img, p.p_offset, p.p_filesz).unwrap();
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), (bias + p.p_vaddr) as *mut u8, src.len()); }
    }
    for (target, value) in writes {
        unsafe { core::ptr::write_unaligned(target as *mut u64, value); }
    }
    let stack_top = area_end & !0xF;
    unsafe {
        let sp = (stack_top - 64) as *mut u64;
        core::ptr::write(sp, 0);
        core::ptr::write(sp.add(1), 0);
        core::ptr::write(sp.add(2), 0);
        core::ptr::write(sp.add(3), 0);
        core::ptr::write(sp.add(4), 0);
    }

    Ok(Loaded {
        entry: entry_address,
        stack_top: stack_top - 64,
        segments: loads.len(),
    })
}

/// Load a static PIE into an already-created private address space.
/// This populates owned pages via their kernel mappings; it does not activate
/// the address space, create a process, or provide a syscall ABI.
pub fn load_private(img: &[u8], space: &mut crate::mm::address_space::AddressSpace) -> Result<Loaded, String> {
    use crate::mm::address_space::USER_BASE;
    use alloc::collections::BTreeMap;
    const PAGE: u64 = 4096;
    const LIMIT: u64 = 4 * 1024 * 1024;
    const STACK: u64 = 64 * 1024;
    let (kind, entry) = check_header(img)?;
    if kind != ET_DYN { return Err("нужен ELF static PIE".to_string()); }
    let headers = parse_phdrs(img)?;
    if headers.iter().any(|p| p.p_type == PT_INTERP) { return Err("динамический интерпретатор не поддерживается".to_string()); }
    let loads: Vec<&Phdr> = headers.iter().filter(|p| p.p_type == PT_LOAD).collect();
    if loads.is_empty() { return Err("нет сегментов PT_LOAD".to_string()); }
    let mut pages: BTreeMap<u64, bool> = BTreeMap::new();
    let mut executable_entry = false;
    for (idx, p) in loads.iter().enumerate() {
        if p.p_filesz > p.p_memsz { return Err("filesz больше memsz".to_string()); }
        file_range(img, p.p_offset, p.p_filesz).ok_or("сегмент вне файла")?;
        let end = p.p_vaddr.checked_add(p.p_memsz).ok_or("переполнение сегмента")?;
        if end > LIMIT - STACK { return Err("сегмент пересекает стек или выходит за область".to_string()); }
        if p.p_flags & PF_X != 0 && p.p_vaddr <= entry && entry < end { executable_entry = true; }
        for other in loads.iter().take(idx) {
            let other_end = other.p_vaddr.checked_add(other.p_memsz).ok_or("переполнение сегмента")?;
            if p.p_memsz != 0 && other.p_memsz != 0 && p.p_vaddr < other_end && other.p_vaddr < end {
                return Err("перекрывающиеся сегменты".to_string());
            }
        }
        if p.p_memsz != 0 {
            let start = p.p_vaddr & !(PAGE - 1);
            let page_end = end.checked_add(PAGE - 1).ok_or("переполнение сегмента")? & !(PAGE - 1);
            let mut page = start;
            while page < page_end {
                let writable = p.p_flags & PF_W != 0;
                pages.entry(page).and_modify(|old| *old |= writable).or_insert(writable);
                page += PAGE;
            }
        }
    }
    if !executable_entry { return Err("точка входа вне исполняемого сегмента".to_string()); }
    let relocations = relocation_plan(img, &headers)?;
    let mut writes = Vec::with_capacity(relocations.len());
    for (offset, addend) in relocations {
        let value = if addend >= 0 { USER_BASE.checked_add(addend as u64) }
            else { USER_BASE.checked_sub(addend.unsigned_abs()) }
            .ok_or("переполнение значения RELA")?;
        writes.push((offset, value));
    }
    for (offset, writable) in pages {
        space.map_zeroed(USER_BASE + offset, writable).map_err(|_| "не удалось выделить страницу")?;
    }
    let mut stack = LIMIT - STACK;
    while stack < LIMIT {
        space.map_zeroed(USER_BASE + stack, true).map_err(|_| "не удалось выделить стек")?;
        stack += PAGE;
    }
    for p in &loads {
        let bytes = file_range(img, p.p_offset, p.p_filesz).ok_or("сегмент вне файла")?;
        let mut copied = 0usize;
        while copied < bytes.len() {
            let addr = USER_BASE + p.p_vaddr + copied as u64;
            let page = addr & !(PAGE - 1);
            let offset = (addr & (PAGE - 1)) as usize;
            let size = (PAGE as usize - offset).min(bytes.len() - copied);
            space.page_for_loading(page).map_err(|_| "страница сегмента отсутствует")?[offset..offset+size]
                .copy_from_slice(&bytes[copied..copied+size]);
            copied += size;
        }
    }
    for (offset, value) in writes {
        for (i, byte) in value.to_le_bytes().iter().enumerate() {
            let addr = USER_BASE + offset + i as u64;
            let page = addr & !(PAGE - 1);
            space.page_for_loading(page).map_err(|_| "страница RELA отсутствует")?[(addr & (PAGE - 1)) as usize] = *byte;
        }
    }
    // The zeroed stack provides argc, argv terminator and an empty environment.
    Ok(Loaded { entry: USER_BASE + entry, stack_top: USER_BASE + LIMIT - 64, segments: loads.len() })
}
