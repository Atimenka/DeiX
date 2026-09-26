// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// devmode — РЕЖИМ РАЗРАБОТЧИКА (dev-режим) DeiX OS.
//
// Что даёт dev-режим:
//   * sudo/su/root становятся ДОСТУПНЫ (в обычном режиме запрещены политикой).
//   * Bootloader РАЗБЛОКИРОВАН (как fastboot oem unlock) — можно прошивать
//     что угодно через fastbootd/EDL/рекавери, пока ОС не превратится в кирпич.
//   * Verified Boot переходит в ORANGE state (предупреждение + задержка 5 с).
//
// Что ТЕРЯЕТСЯ при включении dev-режима:
//   * OTA-гарантия и лицензия безопасности ПЕРЕСТАЮТ действовать:
//     обновления больше не приходят автоматически — пользователь должен
//     обновляться вручную через рекавери/прошивальщик.
//
// Возврат к заводскому состоянию:
//   * lock bootloader (снова locked) + переустановка vbmeta —
//     но ОТКАТ OTA-гарантии выполняется только после полной перепрошивки
//     через EDL/рекавери (иначе Red state из-за изменённых файлов).
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
/// Требует подтверждения (пользователь должен ввести 'yes').
pub fn enable_dev_mode() {
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
