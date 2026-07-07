//! Источник случайных байт для соли пароля (см. auth.rs) — используется
//! ТОЛЬКО для генерации соли (не для генерации самих криптографических
//! ключей общего назначения), поэтому требования к качеству энтропии
//! здесь менее строгие, чем, например, для деривации сессионных ключей
//! шифрования: соль не обязана быть непредсказуемой для атакующего,
//! который уже украл файл с хэшами — она нужна лишь для того, чтобы
//! одинаковые пароли у разных пользователей давали разные хэши (защита
//! от rainbow-таблиц и от сравнения хэшей между собой).
//!
//! Тем не менее стараемся получить максимально доступное на этой машине
//! качество энтропии:
//!   1) Если CPU поддерживает инструкцию RDRAND (проверяем через CPUID,
//!      leaf 1, ECX бит 30 — официально задокументированный Intel/AMD
//!      способ обнаружения) — используем её. Это настоящий аппаратный
//!      генератор случайных чисел на кристалле процессора.
//!   2) Если RDRAND недоступен (старые CPU, некоторые гипервизоры без
//!      его проброса) — ЧЕСТНЫЙ fallback: комбинация счётчика тиков PIT
//!      (timer::ticks(), см. timer.rs), значения TSC (инструкция rdtsc,
//!      считает такты процессора — на практике "дребезжит" даже между
//!      двумя соседними вызовами из-за конвейера/кэшей) и адреса
//!      локальной стековой переменной (ASLR здесь у нас нет, но сам
//!      адрес стека отличается между перезагрузками из-за разного
//!      количества данных, обработанных до этого момента). Это НЕ
//!      криптостойкий RNG — честно объявляем это в документации, но для
//!      соли (см. выше) этого достаточно.

use core::arch::asm;

/// true, если CPUID сообщает о поддержке RDRAND (leaf 1, ECX бит 30).
fn has_rdrand() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "mov eax, 1",
            "push rbx",
            "cpuid",
            "pop rbx",
            out("ecx") ecx,
            out("eax") _,
            out("edx") _,
            options(nostack)
        );
    }
    (ecx & (1 << 30)) != 0
}

/// Пытается получить одно 64-битное случайное значение через RDRAND.
/// Согласно спецификации Intel, RDRAND может кратковременно "не быть
/// готов" (флаг CF=0) — повторяем несколько раз перед тем, как сдаться,
/// как это рекомендует официальный Intel Digital Random Number
/// Generator Software Implementation Guide.
fn try_rdrand64() -> Option<u64> {
    for _ in 0..10 {
        let value: u64;
        let success: u8;
        unsafe {
            asm!(
                "rdrand {val}",
                "setc {ok}",
                val = out(reg) value,
                ok = out(reg_byte) success,
                options(nostack)
            );
        }
        if success != 0 {
            return Some(value);
        }
    }
    None
}

fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// Fallback-источник энтропии, когда RDRAND недоступен — см. пояснение
/// в комментарии к модулю про то, почему это приемлемо именно для соли.
fn fallback_u64(extra_mix: u64) -> u64 {
    let stack_marker: u64 = 0;
    let stack_addr = &stack_marker as *const u64 as u64;
    let ticks = crate::timer::ticks();
    let tsc = rdtsc();

    // Простое перемешивание нескольких слабо связанных источников —
    // не криптографическое, но не позволяет одному предсказуемому
    // источнику (например, tick-counter, одинаковый на первых
    // миллисекундах после каждой загрузки) целиком определить результат.
    tsc.wrapping_mul(0x9E3779B97F4A7C15)
        ^ ticks.rotate_left(17)
        ^ stack_addr.rotate_right(13)
        ^ extra_mix.wrapping_mul(0xBF58476D1CE4E5B9)
}

/// Заполняет буфер случайными байтами — публичный API этого модуля.
/// Используется auth.rs для генерации соли пароля.
pub fn fill_random(buf: &mut [u8]) {
    let use_rdrand = has_rdrand();
    let mut counter: u64 = 0;

    let mut i = 0;
    while i < buf.len() {
        let word = if use_rdrand {
            try_rdrand64().unwrap_or_else(|| {
                counter = counter.wrapping_add(1);
                fallback_u64(counter)
            })
        } else {
            counter = counter.wrapping_add(1);
            fallback_u64(counter)
        };

        let bytes = word.to_le_bytes();
        let take = (buf.len() - i).min(8);
        buf[i..i + take].copy_from_slice(&bytes[..take]);
        i += take;
    }
}
