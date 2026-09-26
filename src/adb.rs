// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// adb — ИНТЕРФЕЙС ОТЛАДКИ В СТИЛЕ ADB (Android Debug Bridge) для DeiX OS.
//
// Предоставляет команды для управления устройством с хост-машины через
// последовательный порт COM1 (в реальном ядре — через USB/сеть):
//   adb devices                — список подключённых устройств
//   adb shell <command>        — выполнить команду в CLI DeiX
//   adb push <name> <data>     — записать файл на устройство (в /system-том)
//   adb pull <name>            — прочитать файл с устройства
//   adb reboot                 — перезагрузка
//   adb reboot recovery        — перезагрузка в рекавери (TWRP/OrangeFox)
//   adb reboot fastbootd       — перезагрузка в прошивальщик (fastbootd)
//   adb install <pkg>          — установить пакет (через pacman)
//   adb ota <payload>          — отправить OTA-пакет
//   adb dev <on|off>           — включить/выключить dev-режим
//
// Ввод команд — с COM1 (headless) или клавиатуры (GUI); вывод — в консоль.
// no_std-совместимо: alloc (String, Vec), вывод — crate::println!.


use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Префикс команд ADB-обёртки.
const ADB_PROMPT: &str = "adb> ";

/// Обработчик команды adb. `avb` нужен для dev-режима/OTA.
pub fn cmd_adb(arg: &str, avb: &mut crate::avb::VerifiedBoot) {
    crate::println!("=== DeiX ADB (debug bridge) ===");
    if arg.is_empty() {
        crate::println!("adb <devices|shell|push|pull|reboot|install|dev>");
        return;
    }

    let parts: Vec<&str> = arg.splitn(3, ' ').collect();
    match parts.first() {
        Some(&"devices") => {
            crate::println!("List of devices attached");
            crate::println!("deix-simulator   device   (DeiX OS v0.2.1)");
        }
        Some(&"shell") => {
            let cmd = parts.get(1).cloned().unwrap_or("").to_string();
            if cmd.is_empty() {
                crate::println!("adb shell <command>");
            } else {
                crate::println!("[adb] $ {}", cmd);
                crate::cli::execute(&cmd);
            }
        }
        Some(&"push") => {
            let name = parts.get(1).cloned().unwrap_or("").to_string();
            let data = parts.get(2).cloned().unwrap_or("").to_string();
            if name.is_empty() {
                crate::println!("adb push <name> <data>");
            } else {
                match crate::ext2::write_file(&name, data.as_bytes()) {
                    Ok(()) => crate::println!("[adb] push {}: OK ({} байт)", name, data.len()),
                    Err(_) => crate::println!("[adb] push {}: ОШИБКА записи", name),
                }
            }
        }
        Some(&"pull") => {
            let name = parts.get(1).cloned().unwrap_or("").to_string();
            match crate::ext2::read_file(&name) {
                Ok(d) => {
                    crate::println!("[adb] pull {}: {} байт", name, d.len());
                    let text = core::str::from_utf8(&d).unwrap_or("<binary>");
                    crate::println!("{}", text);
                }
                Err(_) => crate::println!("[adb] pull {}: файл не найден", name),
            }
        }
        Some(&"reboot") => {
            crate::println!("[adb] reboot: перезагрузка (в модели — возврат)");
        }
        Some(&"install") => {
            let pkg = parts.get(1).cloned().unwrap_or("").to_string();
            if pkg.is_empty() {
                crate::println!("adb install <pkg>");
            } else {
                crate::cli::execute(&format!("pacman -S {}", pkg));
            }
        }
        Some(&"dev") => {
            let mode = parts.get(1).cloned().unwrap_or("").to_string();
            match mode.as_str() {
                "on" => crate::devmode::enable_dev_mode(avb),
                "off" => crate::devmode::disable_dev_mode(avb),
                _ => crate::println!("adb dev <on|off>"),
            }
        }
        _ => crate::println!("adb: неизвестная команда '{}'", parts.first().unwrap_or(&"")),
    }
}

/// REPL-режим ADB: читает команды с COM1/клавиатуры до 'exit'.
pub fn adb_repl(avb: &mut crate::avb::VerifiedBoot) {
    crate::println!("DeiX ADB shell. Введите 'exit' для выхода.");
    loop {
        crate::print!("{}", ADB_PROMPT);
        let line = read_adb_line();
        if line.trim() == "exit" {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        cmd_adb(&line, avb);
    }
}

/// Чтение строки с клавиатуры или COM1.
fn read_adb_line() -> String {
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
