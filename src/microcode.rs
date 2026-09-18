//! ЗАГРУЗКА МИКРОКОДА Intel (для i7-2640M / Sandy Bridge, sig 0x206A7).
//!
//! Микрокод применяется через MSR (только CPL=0, до включения прерываний):
//!   1) IA32_UCODE_WRITE (0x79)   — пишем микрокод кусками по 2048 байт;
//!   2) запись 0 в 0x79           — завершение;
//!   3) IA32_UCODE_TRIGGER (0x8B) — триггер загрузки;
//!   4) чтение 0x8B               — EDX = новый revision (проверка).
//! Формат микрокода: заголовок 48 байт + данные (total_size), контрольная
//! сумма всех 32-битных слов = 0 (проверяем перед загрузкой).
//! На других CPU (sig != 0x206A7) загрузка пропускается.
//! no_std: только core::arch::asm.


use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

/// Разрешено ли ПРИМЕНЯТЬ микрокод (запись в MSR 0x79).
///
/// По умолчанию выключено: на реальном железе это вызывало перезагрузку,
/// а в QEMU дефект не воспроизводится. Чтение текущей версии безопасно
/// и работает всегда.
static APPLY_ENABLED: AtomicBool = AtomicBool::new(false);

const MSR_UCODE_WRITE: u32 = 0x79;
const MSR_UCODE_TRIGGER: u32 = 0x8B;
const MSR_BIOS_SIGN_ID: u32 = 0x8B;

/// Вшитый микрокод Sandy Bridge (06-2a-07, rev 0x2F).
static MICROCODE: &[u8] = include_bytes!("data/mc-06-2a-07.bin");

/// Сигнатура CPU, для которой предназначен микрокод (0x206A7).
const MC_SIGNATURE: u32 = 0x0002_06A7;

/// Текущий revision микрокода (MSR 0x8B после сброса 0).
pub fn current_revision() -> u32 {
    unsafe {
        let mut eax: u32 = 0;
        let mut edx: u32 = 0;
        asm!(
            "xor eax, eax",
            "xor edx, edx",
            "wrmsr",
            "xor eax, eax",
            "rdmsr",
            in("ecx") MSR_BIOS_SIGN_ID,
            out("eax") eax,
            out("edx") edx,
            options(nomem, nostack, preserves_flags),
        );
        edx
    }
}

/// Контрольная сумма микрокода (сумма всех 32-битных слов = 0).
fn checksum_ok(data: &[u8]) -> bool {
    if data.len() < 48 || data.len() % 4 != 0 {
        return false;
    }
    let mut sum: u32 = 0;
    let mut i = 0;
    while i < data.len() {
        sum = sum.wrapping_add(u32::from_le_bytes([
            data[i],
            data[i + 1],
            data[i + 2],
            data[i + 3],
        ]));
        i += 4;
    }
    sum == 0
}

/// Сигнатура из заголовка микрокода (offset 12).
fn mc_signature(data: &[u8]) -> u32 {
    u32::from_le_bytes([data[12], data[13], data[14], data[15]])
}

/// Применяет микрокод (если сигнатура CPU совпадает и микрокод новее).
/// Вызывается рано в kernel_main (до interrupts::init, CPL=0, IF=0).
pub fn init() -> u32 {
    let before = current_revision();

    if !checksum_ok(MICROCODE) {
        crate::serial_println!("[microcode] ПРОПУЩЕН: повреждённый файл микрокода");
        return before;
    }
    if mc_signature(MICROCODE) != MC_SIGNATURE {
        crate::serial_println!("[microcode] ПРОПУЩЕН: сигнатура CPU не 0x{MC_SIGNATURE:X}");
        return before;
    }
    // Не понижаем revision: применяем только если микрокод новее текущего.
    let mc_rev = u32::from_le_bytes([MICROCODE[4], MICROCODE[5], MICROCODE[6], MICROCODE[7]]);
    if before != 0 && mc_rev <= before {
        crate::serial_println!("[microcode] не требуется (rev {before:#x} >= {mc_rev:#x})");
        return before;
    }

    // ЗАПИСЬ МИКРОКОДА ОТКЛЮЧЕНА ПО УМОЛЧАНИЮ.
    //
    // На реальном железе (проверено пользователем на ноутбуке Sandy
    // Bridge) применение микрокода приводило к МГНОВЕННОЙ ПЕРЕЗАГРУЗКЕ:
    // загрузка доходила до строки про кириллический шрифт и машина
    // уходила в ребут. В QEMU этого не видно — там WRMSR 0x79
    // эмулируется как no-op, поэтому дефект и не проявлялся.
    //
    // Причин может быть несколько, и надёжно различить их без
    // отладчика нельзя: вшитый образ не совпадает со степпингом
    // конкретного CPU, BIOS уже применил более новую версию и
    // повторная загрузка запрещена, либо нарушен порядок операций
    // (Intel требует выравнивания 16 байт и отключённых прерываний
    // с прогревом кеша).
    //
    // Микрокод ядру НЕ НУЖЕН: он исправляет ошибки процессора, но без
    // него система полностью работоспособна. Включить можно командой
    // 'microcode apply' — осознанно, понимая риск ребута.
    if !APPLY_ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
        crate::serial_println!(
            "[microcode] применение отключено (rev {before:#x}); 'microcode apply' — включить"
        );
        return before;
    }

    // Запись кусками по 2048 байт (256 записей по 8 байт через WRMSR 0x79).
    let mut off = 0usize;
    while off + 2048 <= MICROCODE.len() {
        for i in (0..2048).step_by(8) {
            let lo = u32::from_le_bytes([
                MICROCODE[off + i],
                MICROCODE[off + i + 1],
                MICROCODE[off + i + 2],
                MICROCODE[off + i + 3],
            ]);
            let hi = u32::from_le_bytes([
                MICROCODE[off + i + 4],
                MICROCODE[off + i + 5],
                MICROCODE[off + i + 6],
                MICROCODE[off + i + 7],
            ]);
            unsafe {
                asm!(
                    "wrmsr",
                    in("ecx") MSR_UCODE_WRITE,
                    in("eax") lo,
                    in("edx") hi,
                    options(nomem, nostack, preserves_flags),
                );
            }
        }
        off += 2048;
    }
    // Завершение: записать 0 в 0x79.
    unsafe {
        asm!(
            "xor eax, eax",
            "xor edx, edx",
            "wrmsr",
            in("ecx") MSR_UCODE_WRITE,
            options(nomem, nostack, preserves_flags),
        );
    }
    // Триггер загрузки.
    unsafe {
        asm!(
            "xor eax, eax",
            "xor edx, edx",
            "wrmsr",
            in("ecx") MSR_UCODE_TRIGGER,
            options(nomem, nostack, preserves_flags),
        );
    }

    let after = current_revision();
    crate::println!(
        "  [microcode] CPU 0x{:X}: микрокод {} -> {} (Sandy Bridge 06-2a-07)",
        MC_SIGNATURE,
        before,
        after
    );
    crate::serial_println!("[microcode] rev {before:#x} -> {after:#x}");
    after
}


/// CLI: `microcode [status|apply]`.
pub fn cmd_microcode(arg: &str) {
    match arg.trim() {
        "apply" => {
            crate::println!("  ВНИМАНИЕ: применение микрокода на реальном железе");
            crate::println!("  приводило к перезагрузке. Включаю на этот сеанс.");
            APPLY_ENABLED.store(true, Ordering::Relaxed);
            let rev = init();
            crate::println!("  [microcode] текущая версия: {:#x}", rev);
        }
        _ => {
            crate::println!("  [microcode] версия CPU: {:#x}", current_revision());
            crate::println!("  [microcode] применение: отключено по умолчанию");
            crate::println!("  Микрокод исправляет ошибки процессора, но ядру не нужен:");
            crate::println!("  без него система полностью работоспособна.");
            crate::println!("  'microcode apply' — применить (риск перезагрузки).");
        }
    }
}
