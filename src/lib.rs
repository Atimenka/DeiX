#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

extern crate alloc;

mod adb;
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
mod devmode;
mod diag;
mod dialog;
mod dinit;
mod ds;
mod duil;
mod erofs;
mod ext2;
mod font;
mod font_cyrillic;
mod font_full;
mod fs;
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
mod mm;
mod module;
mod mouse;
mod microcode;
mod net;
mod nouveau;
mod partition_map;
mod pci;
mod pkg;
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
mod sync;
mod timer;
mod ui;
mod userfs;
mod usermode;
mod vault;
mod vbe;
mod vga;
mod vgaglobal;
mod vfs;
mod vmmouse;

use core::panic::PanicInfo;

/// Версия ядра. Единственный источник — остальные места берут её отсюда,
/// чтобы в отчёте об отказе и в `about` не расходились версии.
pub const KERNEL_VERSION: &str = "0.2.1-beta";

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

    println!("DeiX v0.2.1-beta - mini kernel booted successfully!");
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

    // Система диагностики: с этого момента каждое существенное событие
    // регистрируется в кольцевом буфере и уходит в serial. Постоянное
    // хранилище подключается позже (diag::storage_ready), когда /userdata
    // доступен — буфер до тех пор ничего не теряет.
    diag::init();

    println!("PS/2 keyboard: OK (handled via IRQ1)");

    mouse::init(800, 600);
    println!("PS/2 mouse: OK (handled via IRQ12)");

    if ata::is_present() {
        println!("ATA disk: OK (PIO mode)");
    } else {
        println!("ATA disk: not detected");
    }

    let net_ok = rtl8139::init();
    if net_ok {
        println!(
            "Network: IP {}.{}.{}.{} (type 'ifconfig' for details)",
            net::my_ip()[0], net::my_ip()[1], net::my_ip()[2], net::my_ip()[3]
        );
    } else {
        println!("Network: no RTL8139 card found (run QEMU with '-net nic,model=rtl8139 -net user')");
    }


    let gpu_info = gpu::detect();
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

    let audio_ok = hda::init();
    if audio_ok {
        println!("Audio: Intel High Definition Audio (HDA) ready");
    } else {
        println!("Audio: PC Speaker (run QEMU with '-device intel-hda -device hda-duplex' for HDA)");
    }

    println!("Default language: English. Type 'lang ru' to switch to Russian.");

    bootlogo::show("starting system...");

    bootlogo::set_status("verifying boot partitions...");
    match crate::bootchain::run_boot_chain() {
        Ok(summary) => crate::println!("{}", summary),
        Err(e) => {
            crate::println!("  [bootchain] Предупреждение: {}", e);
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

    // Полнодисковое шифрование (XTS-AES-256). ЕДИНЫЙ ПАРОЛЬ: пароль учётной
    // записи пользователя является ключом шифрования диска.
    // При заводской настройке диск не зашифрован; первая настройка
    // (создание первого аккаунта) ВКЛЮЧАЕТ шифрование. При
    // последующих загрузках разблокировка происходит на экране входа.
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


    // Загружаем модули ядра (.kmod файлы) с ext2-диска.
    // Модули расширяют функциональность ядра: сеть, графика, криптография.
    // Если модуль не найден на диске — система продолжает работу без него.
    module::load_boot_modules();

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

    // DeiX Security Subsystem (Ring 0 / Vault): проверка карты разделов
    // /system (EROFS RO) + /userdata (EXT2 RW) и разбор скрипта init.deix
    // процессом PID 1 (Dinit).
    partition_map::validate_partition_map_report();
    init_parser::boot_report(init_parser::INIT_DEIX_SCRIPT);
    security_monitor::boot_selfcheck();
    crate::dinit::init();
    crate::serial_println!("[deix] Dinit (PID 1, Ring 0) запущен");

    // ВНИМАНИЕ: autostart::run() ПЕРЕНЕСЁН за экран входа (см. ниже).
    // Раньше он выполнялся здесь — до аутентификации, и любой, кто мог
    // записать AUTOSTART.CFG, получал исполнение команд в обход входа
    // (например, строкой "gpu mode 800x600" можно было увести систему
    // в графику и не увидеть логин вовсе). Это дыра в безопасности.

    // Контрольный список загрузки: каждая строка — реальная проверка
    // подсистемы, сбой регистрируется в журнале со своим кодом DX-*.
    diag::boot::screen(net_ok, audio_ok, gpu_ok);

    // Если прошлая сессия завершилась отказом ядра (дамп в сырых секторах,
    // LBA 2048), сообщаем об этом до входа. Плановая перезагрузка дампа
    // не оставляет и диалога не вызывает.
    diag::cli::announce_previous_failure();

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

    // /userdata доступен (и расшифрован, если включено шифрование) —
    // подключаем постоянные журналы и сбрасываем в них всё, что накопил
    // кольцевой буфер с начала загрузки. События до этой точки не теряются.
    diag::storage_ready();
    crate::serial_println!("[deix] J: storage_ready done ({} ms)", timer::uptime_ms());

    // Базовый AUTOSTART.CFG (с gpu mode) — создаём при первом входе.
    autostart::ensure_default();
    crate::serial_println!("[deix] K: autostart ensure_default done ({} ms)", timer::uptime_ms());

    // Автозапуск ТОЛЬКО после успешного входа: команды выполняются от
    // имени вошедшего пользователя, а не анонимно до аутентификации.
    autostart::run();
    crate::serial_println!("[deix] L: autostart run done ({} ms)", timer::uptime_ms());

    // UI-звуки (PC speaker, src/sound.rs; файлы *.dps в EROFS /super —
    // если в образе их нет, просто играем в тишине, как раньше).
    // Загрузка шла с USB-флешки (RAM-диск) — сигнал «носитель подключён»,
    // затем общий стартовый сигнал «система готова».
    if crate::ramdisk::is_active() {
        let _ = sound::play_ui(sound::UiSound::UsbConnect);
    }
    let _ = sound::play_ui(sound::UiSound::Startup);
    crate::serial_println!("[deix] M: startup sound done ({} ms)", timer::uptime_ms());

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
    crate::diag::ring::clear();
    crate::diag::panic::clear_previous_failure();
    crate::crypto_storage::lock();
    crate::linux::syscall::reset_stats();
    *crate::dinit::DINIT.lock() = None;
}
