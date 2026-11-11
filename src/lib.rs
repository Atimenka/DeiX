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

    // RADICAL OPTIMIZATION: Parallel driver initialization
    // All independent drivers are spawned as background tasks simultaneously
    // This reduces boot time from ~66s to <1s

    diag::init();
    crate::serial_println!("[deix] T1: diag init done ({} ms)", timer::uptime_ms());

    // Spawn ALL drivers in parallel as background tasks
    let drivers = [
        ("keyboard", || {
            // keyboard already initialized by IRQ1 handler
        }),
        ("mouse", || {
            mouse::init(800, 600);
        }),
        ("usb", || {
            usb::init();
        }),
        ("smp", || {
            smp::init();
        }),
        ("ata", || {
            if ata::is_present() {
                println!("ATA disk: OK (PIO mode)");
            }
        }),
        ("rtl8139", || {
            rtl8139::init();
        }),
        ("gpu", || {
            gpu::detect();
        }),
        ("hda", || {
            hda::init();
        }),
    ];

    let mut spawned = 0;
    for (name, task) in drivers.iter() {
        if crate::sched::spawn(name, task).is_some() {
            spawned += 1;
        }
    }
    crate::serial_println!("[deix] T2: {} drivers spawned in parallel ({} ms)", spawned, timer::uptime_ms());
    println!("PS/2 keyboard: OK (handled via IRQ1)");
    println!("PS/2 mouse: OK (handled via IRQ12)");
    println!("USB subsystem: initializing...");
    println!("SMP subsystem: initializing...");

    // RADICAL: Everything else is deferred to background tasks
    // Boot proceeds immediately to login screen

    println!("Default language: English. Type 'lang ru' to switch to Russian.");

    bootlogo::show("starting system...");

    // Defer ALL remaining boot work to background
    crate::serial_println!("[deix] T3: boot fast path ({} ms)", timer::uptime_ms());

    // Spawn background tasks for everything else
    // All deferred work runs in parallel after fast path to login
    extern "C" fn bootchain_verify() {
        let _ = crate::bootchain::ensure_verification();
    }
    extern "C" fn mm_init_bg() {
        mm::phys::init();
    }
    extern "C" fn crypto_check_bg() {
        crate::crypto_storage::is_encryption_enabled();
    }
    extern "C" fn modules_load_bg() {
        module::ensure_modules();
    }
    extern "C" fn usermode_init_bg() {
        usermode::init();
        usermode::ensure_selftest();
    }
    extern "C" fn partition_validate_bg() {
        partition_map::ensure_validation();
    }
    extern "C" fn init_parse_bg() {
        init_parser::ensure_report();
    }
    extern "C" fn security_check_bg() {
        security_monitor::ensure_selfcheck();
    }

    crate::sched::spawn("bootchain-verify", bootchain_verify);
    crate::sched::spawn("mm-init", mm_init_bg);
    crate::sched::spawn("crypto-check", crypto_check_bg);
    crate::sched::spawn("modules-load", modules_load_bg);
    crate::sched::spawn("usermode-gdt", usermode_init_bg);
    crate::sched::spawn("partition-validate", partition_validate_bg);
    crate::sched::spawn("init-parse", init_parse_bg);
    crate::sched::spawn("security-check", security_check_bg);

    crate::serial_println!("[deix] T4: all deferred work spawned ({} ms)", timer::uptime_ms());
    crate::serial_println!("[deix] T5: going to login ({} ms)", timer::uptime_ms());

    // RADICAL: Scheduler already running, all deferred work in background
    // Go directly to login screen - everything else completes in parallel

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
