#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![allow(dead_code)]

extern crate alloc;

mod adb;
mod allocator;
mod ata;
mod ramdisk;
mod auth;
mod autostart;
mod avb;
mod bcb;
mod bootlogo;
mod bootchain;
mod bugreport;
mod cli;
mod cp866;
mod crashlog;
mod crypto;
mod crypto_storage;
mod devmode;
mod dialog;
mod dinit;
mod ds;
mod duil;
mod compositor;
mod taskmgr;
mod vbe_bios;
mod proc;
mod sync_primitives;
mod usb;
mod dsm;
mod erofs;
mod ext2;
mod fastbootd_ui;
mod font;
mod font_cyrillic;
mod font_full;
mod fs;
mod gpu;
mod hal;
mod hda;
mod drivers;
mod init_parser;
mod inflate;
mod install;
mod interrupts;
mod kexec;
mod kernel_loader;
mod keyboard;
mod lang;
mod linux;
mod loginui;
mod luks;
mod mex;
mod mm;
mod module;
mod mouse;
mod microcode;
mod net;
mod ota;
mod ota_store;
mod nouveau;
mod partition_map;
mod pci;
mod pkg;
mod port;
mod recovery_flash_engine;
mod renderer;
mod rng;
mod recovery_ui;
mod rtl8139;
mod sched;
mod security_monitor;
mod serial;
mod sound;
mod syslog;
mod spinlock;
mod sync;
mod timer;
mod tpm;
mod ui;
mod userfs;
mod usermode;
mod vault;
mod vbe;
mod vga;
mod vgaglobal;
mod vmmouse;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Полный отладчик ошибок (как Android tombstone): сообщение паники
    // уходит в serial, в кольцевой журнал (dmesg) и в crash-лог на диск
    // (CRASHLOG.TXT на /system-томе), чтобы «Произошла ошибка» можно было
    // посмотреть после перезагрузки (команда `crashlog`, recovery).
    let msg = alloc::format!("[PANIC] {}", info);
    crate::serial_println!("{}", msg);
    crate::syslog::log_line(&msg);
    crate::crashlog::record_crash(&msg);
    crate::crypto_storage::lock();
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

    println!("DeiX v0.2-beta - mini kernel booted successfully!");
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

    if hda::init() {
        println!("Audio: Intel High Definition Audio (HDA) ready");
    } else {
        println!("Audio: PC Speaker (run QEMU with '-device intel-hda -device hda-duplex' for HDA)");
    }

    compositor::init();
    usb::init();

    println!("Default language: English. Type 'lang ru' to switch to Russian.");

    // BCB: одноразовый флажок загрузки. DSM > Fastbootd > Recovery > ОС.
    // Вызывается ПОСЛЕ инициализации heap/шрифтов/GPU — оболочки получают
    // полный GUI (framebuffer) и аллокатор. Если флажок установлен —
    // запускаем оболочку и НЕ продолжаем обычную загрузку (оболочка
    // «выше» ОС в запуске; AVB/логин и т.д. при этом не выполняются).
    if crate::bcb::boot_flow() {
        crate::serial_println!("[bcb] оболочка завершена — обычная ОС при следующей загрузке.");
        loop {
            unsafe { core::arch::asm!("hlt"); }
        }
    }

    // ПОЛНАЯ ЦЕПОЧКА ЗАГРУЗКИ через все разделы (не пустышки!):
    //   dsm -> init_boot -> vendor_boot -> boot -> kernel (kernel.tar.gz).
    // Каждый раздел реально читается и верифицируется.
    // КРИТИЧНО: если звено цепочки повреждено или стёрто (например,
    // `dsm erase /kernel`), загрузка ОСТАНАВЛИВАЕТСЯ — система не должна
    // стартовать без ядра (как Android RED state). Восстановление — через
    // DSM/fastbootd прошивку раздела.
    // Лого на экране, полный лог — в COM1.
    bootlogo::show("starting system...");

    bootlogo::set_status("verifying boot partitions...");
    match crate::bootchain::run_boot_chain() {
        Ok(summary) => crate::println!("{}", summary),
        Err(e) => {
            crate::serial_println!("[bootchain] КРИТИЧЕСКАЯ ОШИБКА: {}", e);
            crate::println!("  [bootchain] ЗАГРУЗКА ОСТАНОВЛЕНА: раздел загрузочной цепочки повреждён/стёрт.");
            crate::println!("  [bootchain] Восстановление: прошейте раздел через DSM/fastbootd.");
            loop {
                unsafe { core::arch::asm!("hlt"); }
            }
        }
    }


    bootlogo::set_status("initializing memory...");
    // Инициализируем менеджер памяти (физический + виртуальный).
    mm::phys::init();
    println!("  [mm] Physical page allocator: OK");
    mm::print_stats();

    // Загружаем метаданные файловой системы.
    fs::load_meta_db();
    println!("  [fs] File access control + TrustedInstaller: OK");

    bootlogo::set_status("verifying system integrity (AVB)...");
    // Verified Boot (AVB/vbmeta-аналог): проверка целостности системы.
    // Green — обычная загрузка; Orange (dev) — предупреждение + 5 сек;
    // Red — красный экран "Your device is corrupt...", загрузка запрещена.
    avb::boot_verify();

    // TPM / полнодисковое шифрование. ЕДИНЫЙ ПАРОЛЬ: пароль учётной
    // записи пользователя является ключом шифрования диска (XTS-AES-256).
    // При заводской настройке диск не зашифрован; первая настройка
    // (создание первого аккаунта в auth.rs) ВКЛЮЧАЕТ шифрование. При
    // последующих загрузках разблокировка происходит на экране входа
    // (crypto_storage::try_unlock). Скрытый раздел /TPM недоступен.
    match crate::crypto_storage::is_encryption_enabled() {
        true => {
            crate::serial_println!("[tpm] Диск зашифрован (XTS-AES-256) — разблокировка при входе.");
            crate::println!("  [tpm] Диск зашифрован (XTS-AES-256). Разблокировка — паролем аккаунта.");
        }
        false => {
            crate::serial_println!("[tpm] Заводские настройки: диск не зашифрован. Первая настройка включит шифрование.");
            crate::println!("  [tpm] Заводские настройки: диск не зашифрован. Первая настройка включит шифрование ключом = пароль аккаунта.");
        }
    }


    // Загружаем модули ядра (.kmod файлы) с ext2-диска.
    // Модули расширяют функциональность ядра: сеть, графика, криптография.
    // Если модуль не найден на диске — система продолжает работу без него.
    module::load_boot_modules();

    // Если это перезапуск через kexec — сообщаем поколение загрузки.
    // Счётчик лежит вне .bss, поэтому переживает обнуление секции.
    let gen = kexec::boot_generation();
    if gen > 0 {
        crate::println!("  [kexec] ЯДРО ЗАПУЩЕНО ИЗ РАЗДЕЛА /kernel_* (поколение {})", gen);
    }

    // GDT/TSS ставим ПЕРВЫМИ: планировщик кладёт в начальный кадр задачи
    // SS=0x10 (kernel data), а в GDT загрузчика есть только null и
    // kernel code — без полной GDT первое же переключение даёт #GP.
    usermode::init();

    // ПЛАНИРОВЩИК: включаем вытесняющую многозадачность.
    sched::start();

    // Самопроверки sched/ring3 из загрузки УБРАНЫ: они своё отработали
    // (результаты зафиксированы в REFACTOR_REPORT.md), а kernel.bin
    // упёрся в потолок 572 КиБ — буфер загрузчика на 0x11000 граничит
    // с видеопамятью VGA. Запустить вручную: 'threads test'.

    // DeiX Security Subsystem (Ring 0 / Vault): стадия init_boot — проверка
    // карты разделов и разбор скрипта init.deix процессом PID 1. Встроенный
    // эталонный скрипт имитирует содержимое защищённого раздела /init_boot
    // (EROFS, ReadOnly). Любая попытка rw-монтирования системного раздела
    // вне прошивочных контекстов (Fastbootd/EDL/Recovery) вызывает panic! —
    // ядро немедленно останавливается (см. init_parser.rs / vault.rs).
    partition_map::validate_partition_map_report();
    let init_deix_text = crate::bootchain::get_init_deix().unwrap_or_else(|| alloc::string::String::from(crate::init_parser::FALLBACK_INIT));
    init_parser::boot_report(&init_deix_text);
    crate::dinit::init_with(&init_deix_text);
    crate::serial_println!("[deix] init_boot: Dinit (PID 1, Ring 0) запущен");

    // ВНИМАНИЕ: autostart::run() ПЕРЕНЕСЁН за экран входа (см. ниже).
    // Раньше он выполнялся здесь — до аутентификации, и любой, кто мог
    // записать AUTOSTART.CFG, получал исполнение команд в обход входа
    // (например, строкой "gpu mode 800x600" можно было увести систему
    // в графику и не увидеть логин вовсе). Это дыра в безопасности.

    // Отладчик ошибок: если предыдущий сеанс завершился паникой (crash-лог
    // на диске, LBA 2048), сообщаем об этом при загрузке — как Android
    // показывает уведомление о сбое. Полный дамп: 'crashlog'.
    if crate::crashlog::has_disk_crash() {
        crate::println!("  [debugger] ⚠ В прошлом сеансе произошла ошибка (crash-лог на диске).");
        crate::println!("  [debugger] Посмотреть дамп: 'crashlog' | стереть: 'crashlog clear'.");
        crate::serial_println!("[debugger] предыдущий сеанс завершился паникой (crash-лог на диске)");
    }

    // === OTA-уведомление + автоустановка ===
    // Если OTA-пакет скачан (флаг pending в BCB) — сообщаем в терминале
    // (со звуком) и ждём 5 минут; без ответа применяем автоматически.
    // Весь цикл (уведомление, таймер, авто-apply из /OTA) — в ota.rs.
    if crate::ota::ota_pending() {
        crate::ota::ota_pending_flow();
    }

    bootlogo::set_status("system ready");

    // Экран входа — до первого запуска создаёт первый аккаунт, при
    // последующих запусках требует ввод логина/пароля (сверяется с
    // солёным SHA-256 хэшем, хранящимся на ext2 — см. auth.rs). Реальный
    // пароль нигде не сохраняется на диске в открытом виде.
    // Графический вход сменяет лого (рисуют в один framebuffer).
    let username = match loginui::run() {
        Some(name) => {
            // ОБЯЗАТЕЛЬНО возвращаем текстовый режим: иначе графика
            // остаётся на мониторе, а CLI пишет в невидимый буфер —
            // это и выглядело как "картинка не пропадает".
            loginui::back_to_text();
            crate::println!("Welcome to DeiX, {}!", name);
            name
        }
        None => auth::run_login_screen(),
    };
    cli::set_current_user(&username);
    let uid = if username == "root" { 0 } else { 1000 };
    if let Some(dinit) = crate::dinit::DINIT.lock().as_mut() {
        dinit.register_user(uid, &username);
    }

    // Профиль пользователя: /users/<имя>/files и /users/<имя>/configs.
    if let Err(e) = userfs::init_profile(&username) {
        crate::println!("  [userfs] профиль не создан: {}", e.message());
    }

    // Базовый AUTOSTART.CFG (с gpu mode) — создаём при первом входе.
    autostart::ensure_default();

    // Автозапуск ТОЛЬКО после успешного входа: команды выполняются от
    // имени вошедшего пользователя, а не анонимно до аутентификации.
    autostart::run();

    // UI-звуки (PC speaker, src/sound.rs; файлы *.dps в EROFS /super —
    // если в образе их нет, просто играем в тишине, как раньше).
    // Загрузка шла с USB-флешки (RAM-диск) — сигнал «носитель подключён»,
    // затем общий стартовый сигнал «система готова».
    if crate::ramdisk::is_active() {
        let _ = sound::play_ui(sound::UiSound::UsbConnect);
    }
    let _ = sound::play_ui(sound::UiSound::Startup);

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
    crate::fs::reset_fs_state();
    crate::renderer::reset_renderer();
    crate::avb::reset_avb();
    crate::crashlog::clear_current_crash();
    crate::crashlog::clear_disk_crash();
    crate::syslog::clear();
    crate::recovery_ui::reset_recovery_state();
    crate::fastbootd_ui::reset_fastbootd_state();
    crate::crypto_storage::lock();
    crate::linux::syscall::reset_stats();
    *crate::dinit::DINIT.lock() = None;
}
