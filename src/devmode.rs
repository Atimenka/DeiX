// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// devmode — РЕЖИМ РАЗРАБОТЧИКА (dev-режим) DeiX OS.
//
// Что даёт dev-режим:
//   * sudo/su/root становятся ДОСТУПНЫ (в обычном режиме запрещены политикой).
//   * Загрузчик считается разблокированным: на разделы можно записывать
//     произвольные образы командой install.
//   * Verified Boot переходит в ORANGE state (предупреждение + задержка 5 с).
//
// Что ТЕРЯЕТСЯ при включении dev-режима:
//   * Гарантии целостности системы перестают действовать: содержимое
//     /system больше не считается доверенным.
//
// Возврат к заводскому состоянию:
//   * выключение dev-режима и полная переустановка системы командой
//     install (иначе Red state из-за изменённых файлов).
// no_std-совместимо: alloc (String), вывод — crate::println!.


use alloc::string::String;

/// Глобальное состояние dev-режима (в .data — сброс через early_init при
/// необходимости; см. install).
pub static DEV_MODE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Флаг: sudo разрешён (true только в dev-режиме).
pub fn sudo_allowed() -> bool {
    DEV_MODE.load(core::sync::atomic::Ordering::Relaxed)
}

/// Включает dev-режим: дает sudo.
/// Требует серверной активации (devauth, см. docs/DEVAUTH.md) и
/// подтверждения (пользователь должен ввести 'yes').
pub fn enable_dev_mode() {
    match crate::devauth::ensure_unlocked() {
        Ok(source) => {
            crate::println!("  [dev] Активация подтверждена: {}.", source);
        }
        Err(reason) => {
            crate::println!("  [dev] Активация НЕ пройдена: {}", reason);
            crate::println!("  [dev] Порядок: 'dev request' -> одобрение владельцем");
            crate::println!("  [dev] проекта (docs/DEVAUTH.md) -> 'dev on'.");
            return;
        }
    }

    crate::println!("  [dev] ВКЛЮЧЕНИЕ РЕЖИМА РАЗРАБОТЧИКА");
    crate::println!("  [dev] sudo/su/root станут доступны.");
    crate::print!("  Подтвердите вводом 'yes': ");

    let confirm = read_input_line();
    if confirm.trim() != "yes" {
        crate::println!("  [dev] Отменено.");
        return;
    }

    DEV_MODE.store(true, core::sync::atomic::Ordering::Relaxed);
    crate::println!("  [dev] РЕЖИМ РАЗРАБОТЧИКА ВКЛЮЧЁН.");
}

/// Выключает dev-режим.
pub fn disable_dev_mode() {
    DEV_MODE.store(false, core::sync::atomic::Ordering::Relaxed);
    crate::println!("  [dev] Dev-режим выключен.");
}

/// Состояние dev-режима (команда `dev status`).
pub fn dev_status() {
    crate::println!("=== Dev-режим ===");
    crate::println!("  dev_mode: {}", sudo_allowed());
    crate::println!("  sudo доступен: {}", sudo_allowed());
    for line in crate::devauth::status_lines() {
        crate::println!("{}", line);
    }
    crate::println!("  команды: dev request | dev on | dev off | dev revoke");
}

/// Чтение строки с клавиатуры или COM1.
fn read_input_line() -> String {
    let mut line = String::new();
    loop {
        let c: Option<u8> = if crate::serial::is_data_ready() {
            Some(crate::serial::read_byte())
        } else {
            crate::keyboard::try_read_char()
        };
        match c {
            Some(b'\n') | Some(b'\r') => break,
            Some(0x08) | Some(0x7F) => {
                line.pop();
            }
            Some(other) => line.push(other as char),
            None => {}
        }
    }
    line
}
