//! Простое переключение языка интерфейса CLI: English (по умолчанию) или
//! Russian. Здесь только состояние + enum сообщений; сами тексты лежат в
//! cli.rs рядом с тем местом, где используются — так проще поддерживать.

use core::sync::atomic::{AtomicU8, Ordering};

const LANG_EN: u8 = 0;
const LANG_RU: u8 = 1;

static CURRENT_LANG: AtomicU8 = AtomicU8::new(LANG_EN);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

pub fn current() -> Lang {
    if CURRENT_LANG.load(Ordering::Relaxed) == LANG_RU {
        Lang::Ru
    } else {
        Lang::En
    }
}

pub fn set(lang: Lang) {
    let value = match lang {
        Lang::En => LANG_EN,
        Lang::Ru => LANG_RU,
    };
    CURRENT_LANG.store(value, Ordering::Relaxed);
}

/// Удобный макрос: `t!(en: "Hello", ru: "Privet")` возвращает нужную
/// строку в зависимости от текущего языка. Годится только для случаев
/// без форматирования (простые строковые литералы), иначе временное
/// значение format_args! не переживёт границу match — для форматированных
/// сообщений используй println_t!.
#[macro_export]
macro_rules! t {
    (en: $en:expr, ru: $ru:expr) => {
        match $crate::lang::current() {
            $crate::lang::Lang::En => $en,
            $crate::lang::Lang::Ru => $ru,
        }
    };
}

/// Печатает одно из двух сообщений (en/ru) в зависимости от текущего
/// языка, поддерживая форматирование с аргументами — в отличие от `t!`,
/// разворачивается в println! отдельно в каждой ветке match, поэтому
/// не ловит ошибку времени жизни временного значения format_args!.
#[macro_export]
macro_rules! println_t {
    (en: $en:expr, ru: $ru:expr) => {
        match $crate::lang::current() {
            $crate::lang::Lang::En => $crate::println!($en),
            $crate::lang::Lang::Ru => $crate::println!($ru),
        }
    };
    (en: $en:expr, ru: $ru:expr; $($arg:expr),+ $(,)?) => {
        match $crate::lang::current() {
            $crate::lang::Lang::En => $crate::println!($en, $($arg),+),
            $crate::lang::Lang::Ru => $crate::println!($ru, $($arg),+),
        }
    };
}
