#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]


extern crate alloc;

mod allocator;
mod ata;
mod auth;
mod autostart;
mod browser;
mod cli;
mod cp866;
mod crypto;
mod crypto_storage;
mod dp;
mod ds;
mod duil;
mod dxinit;
mod elf;
mod ext2;
mod fat16;
mod fileman;
mod font;
mod font_cyrillic;
mod font_full;
mod fs;
mod gpu;
mod hda;
mod initrd;
mod install;
mod interrupts;
mod keyboard;
mod lang;
mod mex;
mod mexapi;
mod mm;
mod module;
mod mouse;
mod net;
mod nouveau;
mod nv3d;
mod pci;
mod pkg;
mod port;
mod renderer;
mod rng;
mod rtl8139;
mod serial;
mod spinlock;
mod sync;
mod timer;
mod ui;
mod usermode;
mod vbe;
mod vga;
mod vgaglobal;
mod vmmouse;
mod wifi;
mod xhci;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[PANIC] {}", info);
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}

/// Точка входа, которую вызывает наш ассемблерный загрузчик
/// (long_mode_init.asm) после перехода в 64-битный режим.
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    // На этом этапе прерывания ещё физически выключены (interrupts::init()
    // ниже их только включит), поэтому прямой доступ безопасен.

    // Дозагружаем кириллицу в VGA-шрифт до первого вывода на экран —
    // делаем это раньше clear_screen, чтобы не мигать штатным шрифтом.
    serial::init();
    crate::serial_println!("=== DeiX boot: serial debug active ===");

    font::install_cyrillic_font();

    vgaglobal::with_writer(|w| w.clear_screen());

    println!("DeiX v0.1 - mini kernel booted successfully!");
    println!("Long mode: OK | Paging: OK | VGA text driver: OK");
    println!("Cyrillic VGA font: OK (loaded into plane 2)");

    interrupts::init();
    println!("IDT + PIC 8259: OK");

    timer::init();
    println!("PIT timer (~100 Hz): OK");

    allocator::init();
    println!("Heap allocator (16 MiB): OK");

    println!("PS/2 keyboard: OK (handled via IRQ1)");

    mouse::init(800, 600);
    println!("PS/2 mouse: OK (handled via IRQ12)");

    if ata::is_present() {
        println!("ATA disk: OK (PIO mode)");
    } else {
        println!("ATA disk: not detected");
    }

    if rtl8139::init() {
        println!(
            "Network: IP {}.{}.{}.{} (type 'ifconfig' for details)",
            net::my_ip()[0], net::my_ip()[1], net::my_ip()[2], net::my_ip()[3]
        );
    } else {
        println!("Network: no RTL8139 card found (run QEMU with '-net nic,model=rtl8139 -net user')");
    }

    println!("Wi-Fi: protocol stack loaded (IEEE 802.11 + WPA2-PSK), no hardware detected");

    match gpu::detect() {
        Some(info) => {
            println!(
                "GPU: {} detected (device ID {:#06x}). Type 'gpu info' for details.",
                info.vendor.name(),
                info.device_id
            );
        }
        None => println!("GPU: no display controller found on PCI bus"),
    }

    println!("Default language: English. Type 'lang ru' to switch to Russian.");

    // Инициализируем менеджер памяти (физический + виртуальный).
    mm::phys::init();
    println!("  [mm] Physical page allocator: OK");
    mm::print_stats();

    // Загружаем метаданные файловой системы.
    fs::load_meta_db();
    println!("  [fs] File access control + TrustedInstaller: OK");

    // Инициализируем Ring 3 (GDT + TSS + syscall MSR).
    usermode::init();
    usermode::setup_syscall_table();

    // Загружаем модули из initrd (ранняя ФС) если вкомпилированы.
    initrd::init();

    // Загружаем модули ядра (.kmod файлы) с ext2-диска.
    // Модули расширяют функциональность ядра: сеть, графика, криптография.
    // Если модуль не найден на диске — система продолжает работу без него.
    module::load_boot_modules();

    // Выполняем скрипт автозапуска (AUTOSTART.CFG) если существует.
    autostart::run();

    // Экран входа — до первого запуска создаёт первый аккаунт, при
    // последующих запусках требует ввод логина/пароля (сверяется с
    // солёным SHA-256 хэшем, хранящимся на ext2 — см. auth.rs). Реальный
    // пароль нигде не сохраняется на диске в открытом виде.
    let username = auth::run_login_screen();
    cli::set_current_user(&username);

    cli::run();
}
