//! Доступ к инструкции CPUID.
//!
//! Нужен системе диагностики: в отчёт об отказе попадает модель процессора,
//! чтобы сбой можно было связать с конкретным железом (ошибки микрокода и
//! errata зависят от модели и степпинга).

use core::arch::asm;

/// Читает лист CPUID.
///
/// `rbx` зарезервирован LLVM, поэтому регистр сохраняется на стек вручную.
fn leaf(eax: u32) -> [u32; 4] {
    let (ebx, ecx, edx): (u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {ebx:e}, ebx",
            "pop rbx",
            ebx = out(reg) ebx,
            inout("eax") eax => _,
            out("ecx") ecx,
            out("edx") edx,
            options(nostack)
        );
    }
    [eax, ebx, ecx, edx]
}

/// Строка производителя процессора (лист 0: EBX, EDX, ECX).
pub fn vendor_string() -> alloc::string::String {
    let [_, ebx, ecx, edx] = leaf(0);
    let mut raw = [0u8; 12];
    raw[0..4].copy_from_slice(&ebx.to_le_bytes());
    raw[4..8].copy_from_slice(&edx.to_le_bytes());
    raw[8..12].copy_from_slice(&ecx.to_le_bytes());
    alloc::string::String::from_utf8_lossy(&raw).into_owned()
}

/// Максимальный номер расширенного листа (0x8000_0000+).
fn max_ext_leaf() -> u32 {
    leaf(0x8000_0000)[0]
}

/// Модель процессора (листы 0x80000002..0x80000004).
///
/// Возвращает `None`, если расширенные листы недоступны — на некоторых
/// эмуляторах их нет, и это не должно ломать отчёт об отказе.
pub fn brand_string() -> Option<alloc::string::String> {
    if max_ext_leaf() < 0x8000_0004 {
        return None;
    }
    let mut raw = [0u8; 48];
    for (i, base) in (0x8000_0002..=0x8000_0004u32).enumerate() {
        let regs = leaf(base);
        for (j, reg) in regs.iter().enumerate() {
            let off = i * 16 + j * 4;
            raw[off..off + 4].copy_from_slice(&reg.to_le_bytes());
        }
    }
    let text = alloc::string::String::from_utf8_lossy(&raw);
    let trimmed = text.trim_matches('\0').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(alloc::string::String::from(trimmed))
    }
}

/// Идентификатор семейства, модели и степпинга (лист 1).
///
/// Возвращает `(family, model, stepping)`.
fn signature() -> (u8, u8, u8) {
    let eax = leaf(1)[0];
    let stepping = (eax & 0xF) as u8;
    let base_model = ((eax >> 4) & 0xF) as u8;
    let base_family = ((eax >> 8) & 0xF) as u8;
    let ext_model = ((eax >> 16) & 0xF) as u8;
    let ext_family = ((eax >> 20) & 0xFF) as u8;

    // Для семейств 6 и 15 модель и семейство расширяются старшими полями.
    let family = if base_family == 0xF {
        base_family.saturating_add(ext_family)
    } else {
        base_family
    };
    let model = if base_family == 0x6 || base_family == 0xF {
        (ext_model << 4) | base_model
    } else {
        base_model
    };

    (family, model, stepping)
}

/// Краткая строка о процессоре для отчётов.
pub fn describe() -> alloc::string::String {
    let (family, model, stepping) = signature();
    match brand_string() {
        Some(brand) => alloc::format!(
            "{} (family {}, model {}, stepping {})",
            brand, family, model, stepping
        ),
        None => alloc::format!(
            "{} (family {}, model {}, stepping {})",
            vendor_string(),
            family,
            model,
            stepping
        ),
    }
}

