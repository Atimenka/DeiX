//! Подсистема USB 2.0 (EHCI) DeiX OS.

pub mod ehci;
pub mod core;
pub mod hub;
pub mod hid;
pub mod storage;

use alloc::format;
use alloc::string::String;

pub fn init() {
    ehci::init();
}

pub fn cmd_usb(arg: &str) {
    let mut parts = arg.trim().split_whitespace();
    let sub = parts.next().unwrap_or("info");

    match sub {
        "info" => {
            crate::println!("=== USB 2.0 (EHCI) SUBSYSTEM ===");
            if ehci::is_ready() {
                crate::println!("  Контроллер EHCI: АКТИВЕН (PCI 0xC000)");
                crate::println!("  Портов корневого хаба: {}", ehci::port_count());
                crate::println!("  Async Schedule: ВКЛЮЧЁН");
                crate::println!("  Periodic Schedule: ВКЛЮЧЁН");
            } else {
                crate::println!("  Контроллер EHCI не обнаружен (запустите QEMU с -device usb-ehci)");
            }
        }
        "devices" | "list" => {
            crate::println!("=== ПОДКЛЮЧЁННЫЕ USB УСТРОЙСТВА ===");
            if ehci::is_ready() {
                let list = core::list_devices();
                if list.is_empty() {
                    crate::println!("  (нет подключённых устройств)");
                } else {
                    for dev in list {
                        crate::println!("  Addr {}: VID:{:04X} PID:{:04X} Class {:#04X}", dev.addr, dev.vid, dev.pid, dev.class_code);
                    }
                }
            } else {
                crate::println!("  EHCI недоступен.");
            }
        }
        "tree" => {
            crate::println!("=== USB TOPOLOGY TREE ===");
            crate::println!("Root Hub (EHCI Controller)");
            let list = core::list_devices();
            for dev in list {
                crate::println!("  └── Device Addr {} [VID:{:04X} PID:{:04X}]", dev.addr, dev.vid, dev.pid);
            }
        }
        _ => {
            crate::println!("Использование: usb [info|devices|tree]");
        }
    }
}
