#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

extern crate alloc;

mod allocator;
mod ata;
mod ramdisk;
mod auth;
mod autostart;
mod bootlogo;
mod bootchain;
mod bugreport;
mod cli;
mod cp866;
mod cpuid;
mod crypto;
mod crypto_storage;
mod devauth;
mod devmode;
mod diag;
mod ext2;
mod font;
mod font_cyrillic;
mod font_full;
mod gpu;
mod hal;
mod hda;
mod drivers;
mod init_parser;
mod install;
mod interrupts;
mod keyboard;
mod lang;
mod linux;
mod loginui;
mod luks;
mod mex;
mod nanocore;
mod ipc;
mod mm;
mod module;
mod mouse;
mod microcode;
mod net;
mod nouveau;
mod partition_map;
mod pci;
mod pkg;
mod png;
mod port;
mod process;
mod renderer;
mod rng;
mod rtc;
mod rtl8139;
mod sched;
mod security_monitor;
mod serial;
mod sound;
mod spinlock;
mod smp;
mod sync;
mod sysfs;
mod timer;
mod ui;
mod userfs;
mod usermode;
mod vault;
mod vbe;
mod vga;
mod vgaglobal;
mod vfs;
mod usb;
mod vmmouse;

use core::panic::PanicInfo;

/// Версия ядра. Единственный источник — остальные места берут её отсюда,
/// чтобы в отчёте об отказе и в `about` не расходились версии.
pub const KERNEL_VERSION: &str = "0.3.0-beta";

/// Полная строка релиза: имя + версия + архитектура.
pub const OS_RELEASE: &str = "DeiX 0.3.0-beta x86_64";

/// Идентификатор сборки: архитектура и режим компиляции.
pub const BUILD_ID: &str = "x86_64-unknown-deix (no_std, release)";

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Отказ ядра обрабатывает система диагностики: она регистрирует событие
    // в кольцевом буфере, собирает отчёт в статический буфер (куча может быть
    // тем, что повреждено), пишет его сырыми секторами и рисует экран.
    //
    // Паник-хендлер вызывается и в ситуациях, когда diag ещё не готов
    // (отказ до его инициализации), поэтому есть запасной путь в serial.
    crate::diag::panic::rust_panic(info)
}

/// Точка входа, которую вызывает наш ассемблерный загрузчик
/// (long_mode_init.asm) после перехода в 64-битный режим.
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    // На этом этапе прерывания ещё физически выключены (interrupts::init()
    // ниже их только включит), поэтому прямой доступ безопасен.

    // NANOCORE — миниминальное ядро, запускается ПЕРВОЙ, до всего остального.
    // Оно обеспечивает базовые услуги: память, планирование, IPC.
    // Всё остальное (dinit, службы, GUI) — процессы Ring 3 под его управлением.
    nanocore::init();

    // ВАЖНО для установленного диска: install не копирует .data ядра
    // (там живое состояние SpinLock/глобалов), поэтому на старте WRITER
    // нулевой — переинициализируем его ДО первого вывода.
    vgaglobal::early_init_writer();

    // Дозагружаем кириллицу в VGA-шрифт до первого вывода на экран —
    // делаем это раньше clear_screen, чтобы не мигать штатным шрифтом.
    serial::init();
    crate::serial_println!("=== DeiX boot: serial debug active ===");

    crate::serial_println!("[deix] A: font start");
    font::install_cyrillic_font();
    crate::serial_println!("[deix] B: font done");

    vgaglobal::with_writer(|w| w.clear_screen());
    crate::serial_println!("[deix] C: clear done");

    println!("{} - mini kernel booted successfully!", OS_RELEASE);
    crate::serial_println!("[deix] D: println done");
    println!("Long mode: OK | Paging: OK | VGA text driver: OK");
    crate::serial_println!("[deix] E: long mode println");
    println!("Cyrillic VGA font: OK (loaded into plane 2)");
    crate::serial_println!("[deix] F: cyrillic println");

    // IDT СТАВИТСЯ ПЕРВОЙ — до любой другой работы.
    //
    // Раньше она инициализировалась после ramdisk и microcode, и всё
    // это время таблица прерываний была пустой. На реальном железе
    // (ноутбук ASUS, Sandy Bridge) машина уходила в TRIPLE FAULT сразу
    // после вывода строки про шрифт: чипсет присылает NMI, SMI или
    // Machine Check, вектора для них нет — процессор перезагружается.
    // В QEMU этих источников прерываний просто не существует, поэтому
    // дефект не воспроизводился ни в одном прогоне.
    println!("Boot step 1/4: interrupts");
    interrupts::init();

    // RAM-диск: при загрузке с USB/Ventoy загрузчик положил весь образ
    // в 0x2000000. Теперь чтение защищено обработчиками исключений.
    println!("Boot step 2/4: ramdisk");
    crate::ramdisk::init();

    // Микрокод: ЧТЕНИЕ версии безопасно, ПРИМЕНЕНИЕ отключено по
    // умолчанию (на реальном железе вызывало перезагрузку).
    println!("Boot step 3/4: microcode");
    crate::microcode::init();
    println!("IDT + PIC 8259: OK");
    crate::serial_println!("[deix] G: idt ok");

    println!("Boot step 4/4: timer + heap");
    timer::init();
    println!("PIT timer (~100 Hz): OK");
    crate::serial_println!("[deix] H: pit ok");

    allocator::init();
    println!("Heap allocator (16 MiB): OK");
    crate::serial_println!("[deix] I: heap ok");
    crate::serial_println!("[deix] T0: heap ok ({} ms)", timer::uptime_ms());

    // Система диагностики: с этого момента каждое существенное событие
    // регистрируется в кольцевом буфере и уходит в serial.
    diag::init();
    crate::serial_println!("[deix] T1: diag init done ({} ms)", timer::uptime_ms());

    println!("PS/2 keyboard: OK (handled via IRQ1)");

    mouse::init(800, 600);
    // USB-подсистема
    println!("USB subsystem: initializing...");
    usb::init();
    crate::serial_println!("[deix] T2: mouse+usb done ({} ms)", timer::uptime_ms());
    println!("PS/2 mouse: OK (handled via IRQ12)");

    // SMP — многоядерность
    println!("SMP subsystem: initializing...");
    smp::init();
    crate::serial_println!("[deix] T3: smp done ({} ms)", timer::uptime_ms());

    crate::serial_println!("[deix] T4: before ata ({} ms)", timer::uptime_ms());
    if ata::is_present() {
        println!("ATA disk: OK (PIO mode)");
    } else {
        println!("ATA disk: not detected");
    }
    crate::serial_println!("[deix] T5: ata done ({} ms)", timer::uptime_ms());

    crate::serial_println!("[deix] T6: before rtl8139 ({} ms)", timer::uptime_ms());
    let net_ok = rtl8139::init();
    crate::serial_println!("[deix] T7: rtl8139 done ({} ms)", timer::uptime_ms());
    if net_ok {
        println!(
            "Network: IP {}.{}.{}.{} (type 'ifconfig' for details)",
            net::my_ip()[0], net::my_ip()[1], net::my_ip()[2], net::my_ip()[3]
        );
    } else {
        println!("Network: no RTL8139 card found (run QEMU with '-net nic,model=rtl8139 -net user')");
    }

    crate::serial_println!("[deix] T8: before gpu ({} ms)", timer::uptime_ms());
    let gpu_info = gpu::detect();
    crate::serial_println!("[deix] T9: gpu done ({} ms)", timer::uptime_ms());
    let gpu_ok = gpu_info.is_some();
    match gpu_info {
        Some(info) => {
            println!(
                "GPU: {} detected (device ID {:#06x}). Type 'gpu info' for details.",
                info.vendor.name(),
                info.device_id
            );
        }
        None => println!("GPU: no display controller found on PCI bus"),
    }

    crate::serial_println!("[deix] T10: before hda ({} ms)", timer::uptime_ms());
    let audio_ok = hda::init();
    crate::serial_println!("[deix] T11: hda done ({} ms)", timer::uptime_ms());
    if audio_ok {
        println!("Audio: Intel High Definition Audio (HDA) ready");
    } else {
        println!("Audio: PC Speaker (run QEMU with '-device intel-hda -device hda-duplex' for HDA)");
    }

    println!("Default language: English. Type 'lang ru' to switch to Russian.");

    bootlogo::show("starting system...");

    bootlogo::set_status("verifying boot partitions...");
    crate::serial_println!("[deix] T12: before bootchain ({} ms)", timer::uptime_ms());
    // run_boot_chain отложено: проверка образа ядра читает весь kernel.bin
    // из EROFS (multi-MB), что добавляет секунды к загрузке.
    crate::bootchain::mark_verification_pending();
    crate::serial_println!("[deix] T13: bootchain deferred ({} ms)", timer::uptime_ms());

    bootlogo::set_status("initializing memory...");
    mm::phys::init();
    crate::serial_println!("[deix] T14: mm done ({} ms)", timer::uptime_ms());
    println!("  [mm] Physical page allocator: OK");
    mm::print_stats();

    // Полнодисковое шифрование (XTS-AES-256)
    match crate::crypto_storage::is_encryption_enabled() {
        true => {
            crate::serial_println!("[crypto] Диск зашифрован (XTS-AES-256) — разблокировка при входе.");
            crate::println!("  [crypto] Диск зашифрован (XTS-AES-256). Разблокировка — паролем аккаунта.");
        }
        false => {
            crate::serial_println!("[crypto] Диск не зашифрован.");
            crate::println!("  [crypto] Хранилище готово.");
        }
    }
    crate::serial_println!("[deix] T15: crypto done ({} ms)", timer::uptime_ms());

    // Модули ядра отложены: чтение 3 .kmod файлов с диска добавляет
    // значительную задержку. Модули загружаются лениво при первом доступе.
    module::mark_modules_pending();
    crate::serial_println!("[deix] T16: modules deferred ({} ms)", timer::uptime_ms());

    // GDT/TSS ставим ПЕРВЫМИ: планировщик кладёт в начальный кадр задачи
    usermode::init();
    // selftest отложен: переход в Ring 3 и возврат добавляют задержку.
    usermode::mark_selftest_pending();
    crate::serial_println!("[deix] T17: usermode done ({} ms)", timer::uptime_ms());

    // ПЛАНИРОВЩИК: включаем вытесняющую многозадачность.
    sched::start();
    crate::serial_println!("[deix] T18: sched done ({} ms)", timer::uptime_ms());

    // DeiX Security Subsystem (Ring 0 / Vault)
    partition_map::mark_validation_pending();
    init_parser::mark_report_pending();
    security_monitor::mark_selfcheck_pending();
    crate::dinit::init();
    crate::serial_println!("[deix] T19: dinit done ({} ms)", timer::uptime_ms());

    // Контрольный список загрузки
    diag::boot::screen(net_ok, audio_ok, gpu_ok);

    diag::cli::announce_previous_failure();

    bootlogo::set_status("system ready");

    crate::serial_println!("[deix] T20: before loginui ({} ms)", timer::uptime_ms());
    let username = match loginui::run() {
        Some(name) => {
            loginui::back_to_text();
            crate::println!("Welcome to DeiX, {}!", name);
            name
        }
        None => auth::run_login_screen(),
    };
    crate::serial_println!("[deix] T21: loginui done ({} ms)", timer::uptime_ms());

    cli::set_current_user(&username);
    let uid = if username == "root" { 0 } else { 1000 };
    if let Some(dinit) = crate::dinit::DINIT.lock().as_mut() {
        dinit.register_user(uid, &username);
    }

    // Профиль пользователя
    if let Err(e) = userfs::init_profile(&username) {
        crate::println!("  [userfs] профиль не создан: {}", e.message());
    }
    crate::serial_println!("[deix] T22: userfs done ({} ms)", timer::uptime_ms());

    // /userdata доступен — подключаем постоянные журналы
    diag::storage_ready();
    crate::serial_println!("[deix] J: storage_ready done ({} ms)", timer::uptime_ms());

    // Канонический layout /userdata
    for d in [
        "/userdata/home", "/userdata/config", "/userdata/apps", "/userdata/packages",
        "/userdata/downloads", "/userdata/cache", "/userdata/update", "/userdata/log",
    ] {
        let _ = vfs::mkdir(d);
    }

    // autostart.cfg — ленивая инициализация
    autostart::ensure_default_deferred();
    crate::serial_println!("[deix] K: autostart ensure_default done ({} ms)", timer::uptime_ms());

    // Автозапуск после входа
    autostart::run();
    crate::serial_println!("[deix] L: autostart run done ({} ms)", timer::uptime_ms());

    // Звук отложен: play_ui ждёт completion синхронно, что добавляет ~1.7s
    sound::play_ui_async(sound::UiSound::Startup);
    crate::serial_println!("[deix] M: startup sound queued ({} ms)", timer::uptime_ms());

    cli::run();
}

/// Полный сброс глобальных структур состояния (.data/.bss) перед переносом
/// ядра на установленный диск (команда `install`). Предотвращает перенос
/// живых указателей на кучу и мусорных дескрипторов сессии.
pub fn reset_all_globals() {
    crate::cli::set_current_user("");
    crate::cli::set_cwd("");
    let _ = crate::vgaglobal::end_capture();
    crate::vgaglobal::early_init_writer();
    crate::module::reset_loaded_modules();
    crate::renderer::reset_renderer();
    crate::diag::ring::clear();
    crate::diag::panic::clear_previous_failure();
    crate::crypto_storage::lock();
    crate::linux::syscall::reset_stats();
    *crate::dinit::DINIT.lock() = None;
}
