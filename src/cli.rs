//! Мини командная строка (CLI): читает строки с клавиатуры и выполняет
//! простые встроенные команды. По умолчанию интерфейс на английском,
//! команда `lang ru`/`lang en` переключает язык сообщений на лету
//! (сами команды всегда набираются латиницей, независимо от языка).

use crate::lang::Lang;
use crate::net;
use crate::pkg;
use crate::rtl8139;
use crate::vga::Color;
use crate::vgaglobal::with_writer;
use crate::{ext2, gpu, keyboard, print, println, println_t, t, timer};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::arch::asm;

const MAX_LINE: usize = 120;
const MAX_HISTORY: usize = 32;

pub fn run() -> ! {
    println!();
    println!("{}", t!(
        en: "Welcome to DeiX CLI! Type 'help' for the command list.",
        ru: "Добро пожаловать в DeiX CLI! Введи 'help' для списка команд."
    ));
    println!("{}", t!(
        en: "Use Up/Down arrows to browse command history.",
        ru: "Стрелки вверх/вниз - навигация по истории введённых команд."
    ));
    prompt();

    let mut line_buf = [0u8; MAX_LINE];
    let mut len = 0usize;
    let mut history: Vec<String> = Vec::new();
    // None = редактируем новую (ещё не отправленную) строку;
    // Some(i) = сейчас показываем history[i] после навигации стрелками.
    let mut history_cursor: Option<usize> = None;

    loop {
        // Ввод: неблокирующий опрос клавиатуры (PS/2) И последовательного
        // порта COM1 (headless-режим QEMU: -serial stdio). Если оба пусты —
        // повторяем опрос.
        let c: u8 = if crate::serial::is_data_ready() {
            crate::serial::read_byte()
        } else {
            match keyboard::try_read_char() {
                Some(c) => c,
                None => continue,
            }
        };
        match c {
            b'\n' => {
                print!("\n");
                let cmd = core::str::from_utf8(&line_buf[..len]).unwrap_or("");
                if !cmd.trim().is_empty() {
                    push_history(&mut history, cmd);
                }
                execute(cmd);
                len = 0;
                history_cursor = None;
                prompt();
            }
            0x08 => {
                if len > 0 {
                    len -= 1;
                    print!("\u{8}");
                }
            }
            keyboard::ARROW_UP => {
                navigate_history(&history, &mut history_cursor, &mut line_buf, &mut len, -1);
            }
            keyboard::ARROW_DOWN => {
                navigate_history(&history, &mut history_cursor, &mut line_buf, &mut len, 1);
            }
            byte => {
                if len < MAX_LINE {
                    line_buf[len] = byte;
                    len += 1;
                    let s = [byte];
                    if let Ok(s) = core::str::from_utf8(&s) {
                        print!("{}", s);
                    }
                }
            }
        }
    }
}

fn push_history(history: &mut Vec<String>, cmd: &str) {
    // не дублируем подряд идущую одинаковую команду (как в bash)
    if history.last().map(|s| s.as_str()) != Some(cmd) {
        history.push(cmd.to_string());
        if history.len() > MAX_HISTORY {
            history.remove(0);
        }
    }
}

/// Стирает текущую отображённую строку с экрана и печатает новую (при
/// навигации по истории стрелками вверх/вниз).
fn navigate_history(
    history: &[String],
    cursor: &mut Option<usize>,
    line_buf: &mut [u8; MAX_LINE],
    len: &mut usize,
    direction: i32,
) {
    if history.is_empty() {
        return;
    }

    let new_index = match (*cursor, direction) {
        (None, -1) => Some(history.len() - 1),
        (None, _) => None,
        (Some(i), -1) => Some(i.saturating_sub(1)),
        (Some(i), _) if i + 1 < history.len() => Some(i + 1),
        (Some(_), _) => None, // ушли "ниже" самой новой команды — пустая строка
    };

    // стираем то, что сейчас показано на экране
    for _ in 0..*len {
        print!("\u{8}");
    }

    *cursor = new_index;
    let new_text: &str = match new_index {
        Some(i) => &history[i],
        None => "",
    };

    let bytes = new_text.as_bytes();
    let copy_len = bytes.len().min(MAX_LINE);
    line_buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
    *len = copy_len;

    if let Ok(s) = core::str::from_utf8(&line_buf[..*len]) {
        print!("{}", s);
    }
}

fn prompt() {
    print!("deix> ");
}

/// true, пока мы выполняем команду ИЗНУТРИ окна графического терминала
/// (см. ui/mod.rs::run_mini_terminal_command). Нужно, чтобы заблокировать
/// команды, которые не имеют смысла или опасны в этом контексте:
///   - "gpu mode" — попытка рекурсивно открыть ещё один
///     desktop поверх уже работающего (у нас всего один Renderer/
///     framebuffer, поддержки нескольких "экранов" нет);
///   - "reboot"/"halt"/"crash" — их результат (перезагрузка/остановка/
///     паника) одинаково фатален что из текстового, что из графического
///     режима, но выполнять их стоит осознанно из обычного CLI, а не
///     случайным кликом в окне поверх рабочего стола.
pub static IN_GRAPHICAL_TERMINAL: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Выполняет одну команду CLI (ту же самую логику, что и обычный
/// текстовый режим). Публичная — используется графическим терминалом
/// (ui/mod.rs) вместе с vgaglobal::begin_capture/end_capture, чтобы окно
/// на рабочем столе исполняло РЕАЛЬНЫЙ полный набор команд, а не свою
/// урезанную копию.
/// Текущий рабочий каталог (глобальная модель; в DeiX ФС — ext2).
static CWD: crate::spinlock::SpinLock<alloc::string::String> =
    crate::spinlock::SpinLock::new(alloc::string::String::new());


/// Устанавливает текущий рабочий каталог (для DS `cd`).
pub fn set_cwd(dir: &str) {
    *CWD.lock() = alloc::string::String::from(dir);
}

/// Возвращает текущий рабочий каталог (для DS `pwd`).
pub fn get_cwd() -> alloc::string::String {
    let cwd = CWD.lock();
    if cwd.is_empty() {
        alloc::string::String::from("/")
    } else {
        cwd.clone()
    }
}


pub fn execute(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }

    let (cmd, rest) = match line.find(char::is_whitespace) {
        Some(idx) => (&line[..idx], line[idx..].trim_start()),
        None => (line, ""),
    };

    // Некоторые команды не имеют смысла (или опасны) при вызове изнутри
    // окна графического терминала — см. документацию у
    // IN_GRAPHICAL_TERMINAL.
    if IN_GRAPHICAL_TERMINAL.load(core::sync::atomic::Ordering::Relaxed) {
        match cmd {
            "gpu" if rest.trim_start().starts_with("mode") || rest.trim_start().starts_with("demo") => {
                println!(
                    "{}",
                    t!(
                        en: "'gpu mode' can't be run from inside the graphical \
                             terminal (it's already running inside a desktop session). Use \
                             'gpu info' instead, or run 'gpu mode' from the text-mode CLI \
                             (press Esc first).",
                        ru: "'gpu mode' нельзя запустить изнутри графического \
                             терминала (мы уже внутри desktop-сессии). Используй 'gpu info', \
                             либо запусти 'gpu mode' из текстового CLI (сначала нажми Esc)."
                    )
                );
                return;
            }
            "reboot" | "halt" => {
                println!(
                    "{}",
                    t!(
                        en: "This command is disabled inside the graphical terminal window \
                             to avoid accidentally rebooting/halting the whole system from a \
                             desktop app. Press Esc to return to the text-mode CLI first.",
                        ru: "Эта команда отключена внутри окна графического терминала, чтобы \
                             случайно не перезагрузить/остановить всю систему из приложения \
                             рабочего стола. Сначала нажми Esc, чтобы вернуться в текстовый CLI."
                    )
                );
                return;
            }
            "install" => {
                println!(
                    "{}",
                    t!(
                        en: "'install' can take a while and prints a progress bar that doesn't \
                             mix well with the windowed terminal's line-based output. Press Esc \
                             and run 'install' from the text-mode CLI instead.",
                        ru: "'install' может занять время и печатает индикатор прогресса, который \
                             плохо сочетается с построчным выводом окна терминала. Нажми Esc и \
                             запусти 'install' из текстового CLI."
                    )
                );
                return;
            }
            _ => {}
        }
    }

    match cmd {
        "help" => cmd_help(),
        "about" => cmd_about(),
        "echo" => println!("{}", rest),
        "clear" => with_writer(|w| w.clear_screen()),
        "uptime" => cmd_uptime(),
        "color" => cmd_color(rest),
        "cpuid" => cmd_cpuid(),
        "mem" => cmd_mem(rest),
        "lang" => cmd_lang(rest),
        "ifconfig" => cmd_ifconfig(rest),
        "arp" => cmd_arp(),
        "ping" => cmd_ping(rest),
        "gpu" => cmd_gpu(rest),
        "sound" => cmd_sound(rest),
        "ls" => cmd_ls(),
        "cat" => cmd_cat(rest),
        "write" => cmd_write(rest),
        "rm" => cmd_rm(rest),
        "pkg" => cmd_pkg(rest),
        "run" => cmd_run(rest),
        "install" => crate::install::cmd_install(rest),
        "bigfile" => cmd_bigfile(rest),
        "useradd" => cmd_useradd(rest),
        "passwd" => cmd_passwd(rest),
        "whoami" => cmd_whoami(),
        "users" => cmd_users(),
        "encrypt" => cmd_encrypt(rest),
        "crash" => cmd_crash(rest),
        "bugreport" => crate::bugreport::cmd_bugreport(),
        "dmesg" => crate::bugreport::cmd_dmesg(),
        "crashlog" => {
            if rest.trim() == "clear" {
                crate::bugreport::cmd_crashlog_clear();
            } else {
                crate::bugreport::cmd_crashlog();
            }
        }
        "reboot" => cmd_reboot(),
        "halt" => cmd_halt(),
        "duil" => crate::duil::cmd_duil(rest),
        "ds" => crate::ds::cmd_ds(rest),

        "taskmgr" => crate::sched::cmd_threads(rest),
        "adb" => crate::adb::cmd_adb(rest),
        "adb-repl" => crate::adb::adb_repl(),
        "dev" => {
            match rest.trim() {
                "on" => crate::devmode::enable_dev_mode(),
                "off" => crate::devmode::disable_dev_mode(),
                _ => crate::devmode::dev_status(),
            }
        }
        "nvidia" => crate::drivers::nvidia::cmd_nvidia(rest),
        "microcode" => crate::microcode::cmd_microcode(rest),
        "hal" => crate::drivers::hal_selftest(),
        "crypt" => crate::luks::cmd_crypt(rest),
        "dinit" => crate::dinit::cmd_dinit(rest),
        "logo" => crate::bootlogo::cmd_logo(rest),
        "linux" => crate::linux::cmd_linux(rest),
        "profile" => crate::userfs::cmd_profile(rest, &current_user().unwrap_or_default()),
        "lock" => crate::loginui::cmd_lock(&current_user().unwrap_or_default()),
        "threads" => crate::sched::cmd_threads(rest),
        "su" | "sudo" | "root" => {
            if crate::devmode::sudo_allowed() {
                // Dev-режим: sudo доступен (гарантия OTA снята).
                println!("{}", t!(
                    en: "SUDO: root shell in dev-mode. OTA guarantee is VOID.",
                    ru: "SUDO: root-оболочка в dev-режиме. OTA-гарантия НЕ действует."
                ));
            } else {
                println!("{}", t!(
                    en: "ACCESS DENIED: privilege escalation (su/sudo) is forbidden by DeiX security policy. Enable dev-mode first.",
                    ru: "ОТКАЗАНО: эскалация привилегий (su/sudo) запрещена политикой безопасности DeiX. Включите dev-режим."
                ));
            }
        }
        _ => println_t!(
            en: "Unknown command: '{}'. Type 'help'.",
            ru: "Неизвестная команда: '{}'. Введи 'help'.";
            cmd
        ),
    }
}

/// sound [list|play <имя>|beep [hz ms]] — звуковые эффекты UI через
/// PC speaker (DPS-файлы из EROFS-раздела /super, см. src/sound.rs).
/// Без подкоманды — список эффектов с отметкой наличия в образе.
fn cmd_sound(rest: &str) {
    let mut it = rest.split_whitespace();
    match it.next() {
        None | Some("list") => {
            println!("{}", t!(en: "UI sounds (PC speaker; files live in EROFS /super):", ru: "UI-звуки (PC speaker; файлы лежат в EROFS /super):"));
            let media = crate::sound::media_files().unwrap_or_default();
            for s in crate::sound::UI_SOUNDS {
                let have = media.iter().any(|f| f == s.dps_name());
                let mark = if have { "[ok]" } else { "[--]" };
                let title = match crate::lang::current() {
                    crate::lang::Lang::Ru => s.title(),
                    crate::lang::Lang::En => s.title_en(),
                };
                println!("  {} {:<12}{}", mark, s.dps_name(), title);
            }
            if media.is_empty() {
                println!("{}", t!(en: "  (image has no sounds: rebuild with build.sh step [2e/8])", ru: "  (в образе звуков нет: пересоберите — шаг build.sh [2e/8])"));
            }
            println!("{}", t!(en: "play: sound play <start|error|lowbat|fullbat|usbcon|usbdisc>", ru: "играть: sound play <start|error|lowbat|fullbat|usbcon|usbdisc>"));
            println!("{}", t!(en: "mode: sound mode [auto|hda|speaker] - select sound output device", ru: "режим: sound mode [auto|hda|speaker] - выбор устройства вывода звука"));
            println!("{}", t!(en: "HDA:  sound hda [test|play <name>] - Intel High Definition Audio status & test", ru: "HDA:  sound hda [test|play <имя>] - статус и тест Intel High Definition Audio"));
            println!("{}", t!(en: "PC:   sound speaker [beep|play <name>] - direct PC speaker (PWM port 0x61)", ru: "PC:   sound speaker [beep|play <имя>] - прямой вывод на PC speaker (ШИМ порт 0x61)"));
        }
        Some("mode") => {
            match it.next() {
                Some("auto") => {
                    crate::sound::set_sound_mode(crate::sound::SoundMode::Auto);
                    println!("{}", t!(en: "  Sound mode: Auto (HDA if available, else PC Speaker)", ru: "  Режим звука: Auto (HDA при наличии, иначе PC Speaker)"));
                }
                Some("hda") => {
                    crate::sound::set_sound_mode(crate::sound::SoundMode::Hda);
                    println!("{}", t!(en: "  Sound mode: Forced Intel HDA", ru: "  Режим звука: принудительно Intel HDA"));
                }
                Some("speaker") => {
                    crate::sound::set_sound_mode(crate::sound::SoundMode::Speaker);
                    println!("{}", t!(en: "  Sound mode: Forced PC Speaker (PWM)", ru: "  Режим звука: принудительно PC Speaker (ШИМ)"));
                }
                _ => {
                    let cur = crate::sound::get_sound_mode();
                    println!("  {} {:?}", t!(en: "Current sound mode:", ru: "Текущий режим звука:"), cur);
                    println!("{}", t!(en: "  Usage: sound mode <auto|hda|speaker>", ru: "  Использование: sound mode <auto|hda|speaker>"));
                }
            }
        }
        Some("speaker") => {
            match it.next() {
                Some("play") => {
                    let name = it.next().unwrap_or("start");
                    match crate::sound::play_speaker_named(name) {
                        Ok(()) => println!("  [speaker] '{}' ✔", name),
                        Err(e) => println!("  [speaker] {}: {}", t!(en: "playback failed", ru: "ошибка воспроизведения"), e),
                    }
                }
                Some("beep") | Some("test") | None => {
                    let hz: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(880);
                    let ms: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(200);
                    println!("  [speaker] Beep {} Hz, {} ms (port 0x61)...", hz, ms);
                    crate::sound::beep_speaker(hz, ms);
                    println!("  [speaker] OK.");
                }
                Some(other) => {
                    match crate::sound::play_speaker_named(other) {
                        Ok(()) => println!("  [speaker] '{}' ✔", other),
                        Err(e) => println!("  [speaker] {}: {}", t!(en: "playback failed", ru: "ошибка воспроизведения"), e),
                    }
                }
            }
        }
        Some("hda") => {
            match it.next() {
                Some("test") => {
                    if !crate::hda::is_ready() {
                        println!("{}", t!(
                            en: "  [hda] Intel HDA is not detected or not initialized.\n  Run QEMU with: -device intel-hda -device hda-duplex",
                            ru: "  [hda] Intel HDA не обнаружен или не инициализирован.\n  Запустите QEMU с флагами: -device intel-hda -device hda-duplex"
                        ));
                    } else {
                        println!("{}", t!(
                            en: "  [hda] Playing 440 Hz test tone via Intel HDA (DMA 48 kHz stereo)...",
                            ru: "  [hda] Воспроизведение тестового тона 440 Гц через Intel HDA (DMA 48 кГц стерео)..."
                        ));
                        crate::hda::play_tone(440, 500);
                        println!("  [hda] OK.");
                    }
                }
                Some("play") => {
                    let name = it.next().unwrap_or("start");
                    match crate::sound::play_named(name) {
                        Ok(()) => println!("  [hda] '{}' ✔", name),
                        Err(e) => println!("  [hda] {}: {}", t!(en: "playback failed", ru: "ошибка воспроизведения"), e),
                    }
                }
                _ => {
                    if let Some(info) = crate::hda::get_info() {
                        println!("{}", info);
                        println!("{}", t!(en: "  Commands: sound hda test, sound hda play <name>", ru: "  Команды: sound hda test, sound hda play <имя>"));
                    } else {
                        println!("{}", t!(
                            en: "  Intel HDA: Not detected on PCI bus.\n  To enable in QEMU, add:\n    -device intel-hda -device hda-duplex",
                            ru: "  Intel HDA: не обнаружен на шине PCI.\n  Для включения в QEMU добавьте:\n    -device intel-hda -device hda-duplex"
                        ));
                    }
                }
            }
        }
        Some("beep") => {
            let hz: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(880);
            let ms: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(150);
            crate::sound::beep(hz, ms);
        }
        Some("play") => match it.next() {
            Some(name) => match crate::sound::play_named(name) {
                Ok(()) => println!("  [sound] {} ✔", name),
                Err(e) => println!("  [sound] {}: {}", t!(en: "playback failed", ru: "не удалось проиграть"), e),
            },
            None => println!("{}", t!(en: "usage: sound play <name>", ru: "использование: sound play <имя>")),
        },
        // Сокращение: `sound start` == `sound play start`.
        Some(name) => match crate::sound::play_named(name) {
            Ok(()) => println!("  [sound] {} ✔", name),
            Err(e) => println!("  [sound] {}: {}", t!(en: "playback failed", ru: "не удалось проиграть"), e),
        },
    }
}

fn cmd_help() {
    println!("{}", t!(en: "Available commands:", ru: "Доступные команды:"));
    println!("  help                    - {}", t!(en: "this help", ru: "эта справка"));
    println!("  about                   - {}", t!(en: "information about DeiX", ru: "информация о DeiX"));
    println!("  echo <text>             - {}", t!(en: "print text", ru: "вывести текст"));
    println!("  clear                   - {}", t!(en: "clear the screen", ru: "очистить экран"));
    println!("  uptime                  - {}", t!(en: "time since boot", ru: "время работы с момента загрузки"));
    println!("  color <name>            - {}", t!(en: "change text color", ru: "сменить цвет текста"));
    println!("                            (green/white/red/cyan/yellow)");
    println!("  cpuid                   - {}", t!(en: "print CPU vendor string", ru: "вывести vendor string процессора"));
    println!("  mem <n>                 - {}", t!(en: "heap allocator test (Vec<u32> of n elements)", ru: "тест heap-аллокатора (Vec<u32> на n элементов)"));
    println!("  lang <en|ru>            - {}", t!(en: "switch interface language", ru: "переключить язык интерфейса"));
    println!("  ifconfig [ip]           - {}", t!(en: "show/change network config", ru: "показать/сменить сетевую конфигурацию"));
    println!("  arp                     - {}", t!(en: "show ARP cache", ru: "показать ARP-кэш"));
    println!("  ping <ip>               - {}", t!(en: "send ICMP echo request", ru: "отправить ICMP echo request"));
    println!("  threads [list|test]     - {}", t!(en: "preemptive multitasking: task list / selftest", ru: "вытесняющая многозадачность: список задач / самопроверка"));
    println!("  crypt <status|addpass|delpass|iter> - {}", t!(en: "volume password slots (LUKS-style)", ru: "пароли тома: слоты, как в LUKS"));
    println!("  dinit [status|services|mounts|users|audit|security|stage|reload] - {}", t!(en: "PID 1 supervisor: services, mounts, users, audit, secmon", ru: "супервизор PID 1: службы, монтирования, пользователи, аудит, монитор"));
    println!("  hal                     - {}", t!(en: "driver layer selftest on RTL8139", ru: "самопроверка прослойки драйверов на RTL8139"));
    println!("  nvidia                  - {}", t!(en: "open NVIDIA driver: probe and identify GPU", ru: "открытый драйвер NVIDIA: поиск и опознание карты"));
    println!("  logo [show|info]        - {}", t!(en: "boot logo: show / info", ru: "загрузочное лого: показать / инфо"));
    println!("  sound [list|play <имя>|beep [hz ms]] - {}", t!(en: "UI sound effects via PC speaker", ru: "звуковые эффекты UI через PC speaker"));
    println!("  linux <run|info> <файл> - {}", t!(en: "run a Linux ELF program", ru: "запустить ELF-программу Linux"));
    println!("  gpu info                - {}", t!(en: "show detected GPU info", ru: "показать инфо об обнаруженном GPU"));
    println!("  gpu nvinfo              - {}", t!(en: "NVIDIA-specific chipset info (open nouveau-based detection)", ru: "инфо о чипе NVIDIA (открытое определение на основе nouveau)"));
    println!("  gpu mode [WxH]          - {}", t!(en: "switch to graphics mode (opens resolution picker if no WxH given)", ru: "переключиться в графику (без WxH откроет выбор разрешения)"));
    println!("  ls                      - {}", t!(en: "list files on the ext2 disk", ru: "список файлов на диске ext2"));
    println!("  cat <file>              - {}", t!(en: "print a text file", ru: "вывести содержимое текстового файла"));
    println!("  write <file> <text>     - {}", t!(en: "create/overwrite a text file", ru: "создать/перезаписать текстовый файл"));
    println!("  rm <file>               - {}", t!(en: "delete a file", ru: "удалить файл"));
    println!("  pkg <list|install|remove|info> - {}", t!(en: "local package manager (see 'pkg list')", ru: "локальный пакетный менеджер (см. 'pkg list')"));
    println!("  run <program.mex>       - {}", t!(en: "run a .mex program", ru: "запустить .mex программу"));
    println!("  install                 - {}", t!(en: "clone this disk onto a second physical disk (real HDD install)", ru: "клонировать этот диск на второй физический диск (установка на реальный HDD)"));
    println!("  bigfile <size_kb>       - {}", t!(en: "test ext2 indirect blocks with a large file", ru: "проверить indirect-блоки ext2 большим файлом"));
    println!("  useradd <user> <pass>   - {}", t!(en: "create a new user account", ru: "создать новый аккаунт пользователя"));
    println!("  passwd <u> <old> <new>  - {}", t!(en: "change a user's password", ru: "сменить пароль пользователя"));
    println!("  whoami                  - {}", t!(en: "show the currently logged-in user", ru: "показать текущего пользователя"));
    println!("  users                   - {}", t!(en: "list registered user accounts", ru: "список зарегистрированных пользователей"));
    println!("  encrypt confirm <pass>  - {}", t!(en: "enable full-disk AES-256-XTS encryption (erases data!)", ru: "включить шифрование диска AES-256-XTS (стирает данные!)"));
    println!("  crash <divzero|bp|inv>  - {}", t!(en: "demonstrate exception handling", ru: "демонстрация обработки исключений"));
    println!("  duil [run|calc]         - {}", t!(en: "DUIL declarative UI engine and calculator demo", ru: "декларативный UI-движок DUIL и калькулятор"));
    println!("  ds [script.dxs|-i|-c]   - {}", t!(en: "DeiX Script interpreter and REPL shell", ru: "интерпретатор скриптов DeiX Script и REPL"));

    println!("  taskmgr                 - {}", t!(en: "system task manager and process list", ru: "диспетчер задач и процессов"));
    println!("  reboot                  - {}", t!(en: "reboot (via keyboard controller)", ru: "перезагрузка (через контроллер клавиатуры)"));
    println!("  halt                    - {}", t!(en: "halt the CPU (cli; hlt)", ru: "остановить процессор (cli; hlt)"));
    println!();
    println!("{}", t!(en: "Up/Down arrows browse command history.", ru: "Стрелки вверх/вниз - навигация по истории команд."));
}

fn cmd_about() {
    println!("{}", t!(en: "DeiX v0.2.1-beta - mini kernel written in Rust", ru: "DeiX v0.2.1-beta - мини-ядро на Rust"));
    println!(
        "{}",
        t!(
            en: "Bootloader: classic BIOS MBR + long mode trampoline (no GRUB)",
            ru: "Загрузчик: классический BIOS MBR + long mode trampoline (без GRUB)"
        )
    );
    println!(
        "{}",
        t!(
            en: "Kernel: GDT (from bootloader), IDT, PIC 8259, PIT,",
            ru: "Компоненты ядра: GDT (из загрузчика), IDT, PIC 8259, PIT,"
        )
    );
    println!(
        "{}",
        t!(
            en: "        PS/2 keyboard, VGA text + custom Cyrillic font, heap allocator (1 MiB)",
            ru: "                  PS/2 клавиатура, VGA text + свой шрифт кириллицы, heap allocator (1 МиБ)"
        )
    );
}

fn cmd_uptime() {
    let ms = timer::uptime_ms();
    let secs = ms / 1000;
    let minutes = secs / 60;
    let secs_rem = secs % 60;
    let ms_rem = ms % 1000;
    let ticks = timer::ticks();
    println_t!(
        en: "Uptime: {}m {}.{:03}s ({} ms, {} timer ticks)",
        ru: "Время работы: {}м {}.{:03}с ({} мс, {} тиков таймера)";
        minutes, secs_rem, ms_rem, ms, ticks
    );
}

fn cmd_color(name: &str) {
    let color = match name {
        "green" => Some(Color::LightGreen),
        "white" => Some(Color::White),
        "red" => Some(Color::LightRed),
        "cyan" => Some(Color::LightCyan),
        "yellow" => Some(Color::Yellow),
        "" => None,
        _ => {
            println_t!(
                en: "Unknown color '{}'. Available: green, white, red, cyan, yellow",
                ru: "Неизвестный цвет '{}'. Доступно: green, white, red, cyan, yellow";
                name
            );
            return;
        }
    };
    match color {
        Some(c) => {
            with_writer(|w| w.set_color(c, Color::Black));
            println!("{}", t!(en: "Color changed.", ru: "Цвет изменён."));
        }
        None => println!(
            "{}",
            t!(en: "Usage: color <green|white|red|cyan|yellow>", ru: "Укажи цвет: color <green|white|red|cyan|yellow>")
        ),
    }
}

fn cmd_cpuid() {
    // rbx зарезервирован LLVM для внутренних нужд и не может быть указан
    // напрямую как выходной операнд inline asm, поэтому сохраняем/
    // восстанавливаем его вручную через стек.
    let (ebx, edx, ecx): (u32, u32, u32);
    unsafe {
        asm!(
            "mov eax, 0",
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            ebx_out = out(reg) ebx,
            out("edx") edx,
            out("ecx") ecx,
            out("eax") _,
            options(nostack)
        );
    }
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&ecx.to_le_bytes());
    let vendor_str = core::str::from_utf8(&vendor).unwrap_or("????????????");
    println!("CPU vendor: {}", vendor_str);
}

fn cmd_mem(arg: &str) {
    let n: usize = if arg.is_empty() {
        1000
    } else {
        match arg.parse() {
            Ok(v) => v,
            Err(_) => {
                println!(
                    "{}",
                    t!(en: "Usage: mem <count>, e.g. mem 5000", ru: "Укажи число элементов, например: mem 5000")
                );
                return;
            }
        }
    };

    println_t!(
        en: "Allocating Vec<u32> for {} elements via heap allocator...",
        ru: "Выделяем Vec<u32> на {} элементов через heap-аллокатор...";
        n
    );
    let mut v: Vec<u32> = Vec::with_capacity(n);
    for i in 0..n as u32 {
        v.push(i * 2);
    }
    let sum: u64 = v.iter().map(|&x| x as u64).sum();
    let bytes = n * core::mem::size_of::<u32>();
    println_t!(
        en: "Done! Allocated {} bytes, sum of all elements = {}",
        ru: "Готово! Выделено {} байт, сумма всех элементов = {}";
        bytes, sum
    );
    println!(
        "{}",
        t!(
            en: "Vec is automatically freed (dealloc) when it goes out of scope.",
            ru: "Vec автоматически освобождён (dealloc) при выходе из области видимости."
        )
    );
}

fn cmd_lang(arg: &str) {
    match arg {
        "en" => {
            crate::lang::set(Lang::En);
            println!("Language switched to English.");
        }
        "ru" => {
            crate::lang::set(Lang::Ru);
            println!("Язык переключён на русский.");
        }
        "" => {
            let current = t!(en: "English", ru: "русский");
            println_t!(
                en: "Current language: {}. Usage: lang <en|ru>",
                ru: "Текущий язык: {}. Использование: lang <en|ru>";
                current
            );
        }
        _ => println_t!(
            en: "Unknown language '{}'. Available: en, ru",
            ru: "Неизвестный язык '{}'. Доступно: en, ru";
            arg
        ),
    }
}

fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut result = [0u8; 4];
    let mut parts = s.split('.');
    for slot in result.iter_mut() {
        let part = parts.next()?;
        *slot = part.parse().ok()?;
    }
    if parts.next().is_some() {
        return None; // лишние части — не валидный адрес
    }
    Some(result)
}

fn cmd_ifconfig(arg: &str) {
    if !rtl8139::is_ready() {
        println!(
            "{}",
            t!(
                en: "No network card detected. Run QEMU with '-net nic,model=rtl8139 -net user'.",
                ru: "Сетевая карта не найдена. Запусти QEMU с '-net nic,model=rtl8139 -net user'."
            )
        );
        return;
    }

    if !arg.is_empty() {
        match parse_ipv4(arg) {
            Some(ip) => {
                net::set_my_ip(ip);
                println!("{}", t!(en: "IP address changed.", ru: "IP-адрес изменён."));
            }
            None => {
                println!(
                    "{}",
                    t!(
                        en: "Invalid IP address. Format: ifconfig 192.168.1.10",
                        ru: "Неверный IP-адрес. Формат: ifconfig 192.168.1.10"
                    )
                );
                return;
            }
        }
    }

    let mac = rtl8139::mac_address();
    let ip = net::my_ip();
    let gw = net::gateway_ip();

    println!(
        "MAC:     {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );
    println!("IP:      {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    println!("Gateway: {}.{}.{}.{}", gw[0], gw[1], gw[2], gw[3]);
}

fn cmd_arp() {
    if !rtl8139::is_ready() {
        println!(
            "{}",
            t!(en: "No network card detected.", ru: "Сетевая карта не найдена.")
        );
        return;
    }
    println!("{}", t!(en: "ARP cache:", ru: "ARP-кэш:"));
    crate::net::arp::print_cache();
}

fn cmd_ping(arg: &str) {
    if !rtl8139::is_ready() {
        println!(
            "{}",
            t!(en: "No network card detected.", ru: "Сетевая карта не найдена.")
        );
        return;
    }

    if arg.is_empty() {
        println!(
            "{}",
            t!(en: "Usage: ping <ip>, e.g. ping 10.0.2.2", ru: "Укажи адрес, например: ping 10.0.2.2")
        );
        return;
    }

    let ip = match parse_ipv4(arg) {
        Some(ip) => ip,
        None => {
            println!(
                "{}",
                t!(en: "Invalid IP address.", ru: "Неверный IP-адрес.")
            );
            return;
        }
    };

    println_t!(
        en: "PING {}.{}.{}.{}:",
        ru: "PING {}.{}.{}.{}:";
        ip[0], ip[1], ip[2], ip[3]
    );

    const PING_COUNT: u16 = 4;
    let mut received = 0u16;

    for seq in 0..PING_COUNT {
        match crate::net::icmp::ping(ip, 0x1234, seq, 2000) {
            Some(rtt_ms) => {
                received += 1;
                println_t!(
                    en: "  Reply from {}.{}.{}.{}: seq={} time={}ms",
                    ru: "  Ответ от {}.{}.{}.{}: seq={} time={}мс";
                    ip[0], ip[1], ip[2], ip[3], seq, rtt_ms
                );
            }
            None => {
                println_t!(
                    en: "  Request timed out (seq={})",
                    ru: "  Превышено время ожидания (seq={})";
                    seq
                );
            }
        }
    }

    println_t!(
        en: "{} of {} packets received.",
        ru: "Получено {} из {} пакетов.";
        received, PING_COUNT
    );
}





fn cmd_gpu(arg: &str) {
    let (subcmd, rest) = match arg.find(char::is_whitespace) {
        Some(idx) => (&arg[..idx], arg[idx..].trim_start()),
        None => (arg, ""),
    };

    match subcmd {
        "info" => cmd_gpu_info(),
        "nvinfo" => cmd_gpu_nvinfo(),
        "mode" => cmd_gpu_mode(rest),
        "" => println!(
            "{}",
            t!(en: "Usage: gpu <info|nvinfo|mode>", ru: "Использование: gpu <info|nvinfo|mode>")
        ),
        _ => println_t!(
            en: "Unknown gpu subcommand '{}'. Available: info, nvinfo, mode",
            ru: "Неизвестная подкоманда '{}'. Доступно: info, nvinfo, mode";
            subcmd
        ),
    }
}

/// `gpu nvinfo` — открытое определение архитектуры NVIDIA-чипа через
/// регистр NV_PMC_BOOT_0 (см. nouveau.rs) и честный статус, что именно
/// это ядро может сделать с найденной картой: для классических карт
/// (NV04-NV40) сообщает о наличии ЭКСПЕРИМЕНТАЛЬНОГО modesetting-кода
/// (не протестированного на реальном железе — честно предупреждаем), а
/// для всего современного (Fermi и новее, включая GT 520M) объясняет,
/// почему EVO/NVDisplay-архитектура несовместима с этим простым
/// подходом.
fn cmd_gpu_nvinfo() {
    let info = match gpu::detect() {
        Some(info) => info,
        None => {
            println!("{}", t!(en: "No display controller found on PCI bus.", ru: "Видеоконтроллер на шине PCI не найден."));
            return;
        }
    };

    if info.vendor != gpu::GpuVendor::Nvidia {
        println_t!(
            en: "Found GPU vendor is {}, not NVIDIA — 'gpu nvinfo' only applies to NVIDIA hardware.",
            ru: "Найденный GPU — {}, не NVIDIA — 'gpu nvinfo' применима только к железу NVIDIA.";
            info.vendor.name()
        );
        return;
    }

    let nv = crate::nouveau::detect(info.device);

    println_t!(
        en: "NVIDIA chipset ID: {:#04x} (read from NV_PMC_BOOT_0, an open/documented register)",
        ru: "ID чипа NVIDIA: {:#04x} (прочитан из NV_PMC_BOOT_0, открытого/задокументированного регистра)";
        nv.chipset_id
    );
    println_t!(
        en: "Architecture: {}",
        ru: "Архитектура: {}";
        nv.architecture.name()
    );

    if nv.architecture.supports_legacy_modesetting() {
        println!(
            "{}",
            t!(
                en: "This is a classic (pre-G80) architecture, whose PCRTC0/PRAMDAC0 \\
                     registers are publicly documented by the nouveau project. This \\
                     kernel does NOT include a modesetting driver for it — only the \\
                     chipset identification you see above. Use the Bochs VBE framebuffer \\
                     driver (see 'gpu mode') under QEMU/Bochs/VirtualBox.",
                ru: "Это классическая (до-G80) архитектура, регистры PCRTC0/PRAMDAC0 \\
                     которой открыто задокументированы проектом nouveau. Драйвера \\
                     модесеттинга для неё в этом ядре НЕТ — только определение чипа, \\
                     показанное выше. Используй драйвер Bochs VBE framebuffer \\
                     (см. 'gpu mode') под QEMU/Bochs/VirtualBox."
            )
        );
    } else {
        println!(
            "{}",
            t!(
                en: "This architecture (G80/Tesla and newer — includes Fermi, Kepler, \\
                     Maxwell, Pascal, Turing+) uses the EVO/NVDisplay display engine: \\
                     command push-buffers and GPU virtual memory, not simple CRTC/RAMDAC \\
                     registers. Supporting it requires thousands of lines of code (the \\
                     real nouveau driver took years to get here) and, from Turing onward, \\
                     a signed closed-source GSP firmware blob from NVIDIA itself, without \\
                     which modesetting is impossible even in principle. This kernel \\
                     cannot bundle either. Use the Bochs VBE framebuffer driver instead \\
                     (see 'gpu mode') — it works under QEMU/Bochs/VirtualBox with the \\
                     standard VGA adapter, not on real NVIDIA hardware.",
                ru: "Эта архитектура (G80/Tesla и новее — включает Fermi, Kepler, \\
                     Maxwell, Pascal, Turing+) использует display engine EVO/NVDisplay: \\
                     push-буферы команд и виртуальную память GPU, а не простые регистры \\
                     CRTC/RAMDAC. Её поддержка требует тысяч строк кода (у самого \\
                     проекта nouveau на это ушли годы) и, начиная с Turing, подписанную \\
                     закрытую прошивку GSP от самой NVIDIA, без которой модесеттинг \\
                     невозможен в принципе. Это ядро не может встроить ни то, ни другое. \\
                     Используй драйвер Bochs VBE framebuffer (см. 'gpu mode') — он \\
                     работает под QEMU/Bochs/VirtualBox со стандартной VGA-картой, но не \\
                     на реальном железе NVIDIA."
            )
        );
    }
}

fn cmd_gpu_info() {
    match gpu::detect() {
        Some(info) => {
            println_t!(
                en: "Vendor: {} (device ID {:#06x})",
                ru: "Производитель: {} (device ID {:#06x})";
                info.vendor.name(), info.device_id
            );
            let (msg_en, msg_ru) = gpu::support_status_message(info.vendor);
            println!("{}", t!(en: msg_en, ru: msg_ru));
        }
        None => {
            println!(
                "{}",
                t!(
                    en: "No display controller found on PCI bus.",
                    ru: "Видеоконтроллер на шине PCI не найден."
                )
            );
        }
    }
}

fn cmd_gpu_mode(arg: &str) {
    let info = match gpu::detect() {
        Some(info) => info,
        None => {
            println!("{}", t!(en: "No GPU detected.", ru: "GPU не найден."));
            return;
        }
    };

    if !info.supports_bochs_vbe {
        println!(
            "{}",
            t!(
                en: "This GPU doesn't support the open Bochs VBE interface. \
                     Run 'gpu info' for details on why (proprietary hardware).",
                ru: "Этот GPU не поддерживает открытый интерфейс Bochs VBE. \
                     Смотри 'gpu info' для деталей (проприетарное железо)."
            )
        );
        return;
    }

    let (width, height): (u32, u32) = if arg.is_empty() {
        (800, 600)
    } else {
        let mut parts = arg.split('x');
        let w = parts.next().and_then(|s| s.parse().ok());
        let h = parts.next().and_then(|s| s.parse().ok());
        match (w, h) {
            (Some(w), Some(h)) => (w, h),
            _ => {
                println!(
                    "{}",
                    t!(en: "Usage: gpu mode [WIDTHxHEIGHT], e.g. gpu mode 1024x768", ru: "Использование: gpu mode [ШИРИНАxВЫСОТА], например: gpu mode 1024x768")
                );
                return;
            }
        }
    };

    if !enter_graphics_mode(&info.device, width, height) {
        println!(
            "{}",
            t!(en: "Failed to set video mode.", ru: "Не удалось установить видеорежим.")
        );
        return;
    }

    // run_desktop_session сам обрабатывает переключения разрешения "на
    // лету" (окно Display settings в меню "Пуск") — каждый раз, когда
    // пользователь выбирает новое разрешение, вызывается это замыкание,
    // которое переключает видеорежим точно так же, как обычный `gpu
    // mode WxH`, но не выходя в текстовый режим между переключениями.
    let gpu_device = info.device;
    crate::ui::run_desktop_session(|w, h| enter_graphics_mode(&gpu_device, w, h));

    // Возврат сюда происходит после Esc внутри desktop-цикла —
    // возвращаем текстовый режим, переинициализируя стандартный
    // VGA text mode через BIOS-совместимый сброс, и восстанавливаем
    // полный ASCII+кириллица шрифт (он живёт в той же физической
    // VRAM, что framebuffer графического режима, и был перезаписан
    // пикселями во время работы desktop).
    crate::vbe::restore_text_mode();
    crate::font::restore_full_font();
    with_writer(|w| w.clear_screen());
    println!(
        "{}",
        t!(
            en: "Returned to text mode.",
            ru: "Возвращено в текстовый режим."
        )
    );
}

/// Устанавливает видеорежим width x height через Bochs VBE и подключает
/// его как активный framebuffer рендерера — общая логика для первого
/// входа в `gpu mode` и для переключений разрешения "на лету" из окна
/// Display settings. Возвращает false при неудаче (например, разрешение
/// не поддерживается Bochs VBE на этой видеокарте).
fn enter_graphics_mode(gpu_device: &crate::pci::PciDevice, width: u32, height: u32) -> bool {
    println_t!(
        en: "Switching to {}x{} 32bpp via Bochs VBE...",
        ru: "Переключаемся на {}x{} 32bpp через Bochs VBE...";
        width, height
    );

    match crate::vbe::set_mode(gpu_device, width, height, crate::vbe::BPP_32) {
        Some(fb) => {
            // Абсолютный курсор мыши (VMware/QEMU backdoor, см.
            // vmmouse.rs/mouse.rs) масштабирует координаты под текущее
            // разрешение экрана — сообщаем ему новое разрешение перед
            // работой desktop, иначе курсор был бы привязан к границам
            // предыдущего разрешения.
            crate::mouse::set_screen_size(width, height);
            crate::renderer::set_active_framebuffer(fb);
            true
        }
        None => false,
    }
}


fn cmd_ls() {
    if !ext2::is_formatted() {
        println!(
            "{}",
            t!(
                en: "Disk not formatted yet — it will be formatted automatically \
                     the first time a file is written (e.g. 'write test.txt hello').",
                ru: "Диск ещё не отформатирован — он будет отформатирован \
                     автоматически при первой записи файла (например, \
                     'write test.txt hello')."
            )
        );
        return;
    }
    match ext2::list_root() {
        Ok(entries) => {
            if entries.is_empty() {
                println!("{}", t!(en: "(empty)", ru: "(пусто)"));
            }
            for e in entries {
                if e.is_directory {
                    println!("  [{}]", e.name);
                } else {
                    println!("  {:<12} {} {}", e.name, e.size, t!(en: "bytes", ru: "байт"));
                }
            }
        }
        Err(_) => println!(
            "{}",
            t!(en: "Failed to read directory.", ru: "Не удалось прочитать каталог.")
        ),
    }
}

fn cmd_cat(name: &str) {
    if name.is_empty() {
        println!(
            "{}",
            t!(en: "Usage: cat <filename>", ru: "Использование: cat <имя_файла>")
        );
        return;
    }
    match ext2::read_file(name) {
        Ok(data) => match core::str::from_utf8(&data) {
            Ok(text) => println!("{}", text),
            Err(_) => println!(
                "{}",
                t!(
                    en: "File is not valid UTF-8 text (binary file?).",
                    ru: "Файл не является текстом в UTF-8 (бинарный файл?)."
                )
            ),
        },
        Err(ext2::Ext2Error::FileNotFound) => println!(
            "{}",
            t!(en: "File not found.", ru: "Файл не найден.")
        ),
        Err(_) => println!(
            "{}",
            t!(en: "Failed to read file.", ru: "Не удалось прочитать файл.")
        ),
    }
}

/// `write <filename> <text...>` — создаёт/перезаписывает текстовый файл
/// с указанным содержимым. Простая, но полностью реальная запись на
/// ext2-томе (см. ext2.rs) — форматирует диск автоматически при первом
/// использовании, если он ещё не отформатирован.
fn cmd_write(arg: &str) {
    let mut parts = arg.splitn(2, ' ');
    let name = parts.next().unwrap_or("");
    let text = parts.next().unwrap_or("");

    if name.is_empty() {
        println!(
            "{}",
            t!(
                en: "Usage: write <filename> <text>",
                ru: "Использование: write <имя_файла> <текст>"
            )
        );
        return;
    }

    if !ext2::is_formatted() {
        if let Err(_) = ext2::format() {
            println!("{}", t!(en: "Failed to format disk.", ru: "Не удалось отформатировать диск."));
            return;
        }
    }

    match ext2::write_file(name, text.as_bytes()) {
        Ok(()) => println_t!(
            en: "Wrote {} bytes to '{}'.",
            ru: "Записано {} байт в '{}'.";
            text.len(), name
        ),
        Err(_) => println!("{}", t!(en: "Failed to write file.", ru: "Не удалось записать файл.")),
    }
}

/// `bigfile <size_kb>` — тестовая команда для проверки indirect-блоков
/// ext2 (см. подробный комментарий в начале ext2.rs): генерирует файл
/// заданного размера (в КиБ) с предсказуемым псевдослучайным содержимым
/// (без настоящего RNG — линейный конгруэнтный генератор с фиксированным
/// зерном, чтобы можно было детерминированно сверить прочитанные назад
/// байты), пишет его на диск и сразу читает обратно, побайтово сверяя
/// содержимое. Не предназначена для обычного использования — это
/// диагностический инструмент, аналогичный `mem <count>` (тестирует
/// heap-аллокатор), но для файловой системы.
fn cmd_bigfile(arg: &str) {
    let size_kb: usize = match arg.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            println!(
                "{}",
                t!(
                    en: "Usage: bigfile <size_kb>, e.g. 'bigfile 64' to test a 64 KiB file \\
                         (exercises ext2 indirect blocks beyond the 12 KiB direct-only limit)",
                    ru: "Использование: bigfile <размер_кб>, например 'bigfile 64' для проверки \\
                         файла в 64 КиБ (задействует indirect-блоки ext2 сверх лимита в 12 КиБ)"
                )
            );
            return;
        }
    };

    if !ext2::is_formatted() {
        if let Err(_) = ext2::format() {
            println!("{}", t!(en: "Failed to format disk.", ru: "Не удалось отформатировать диск."));
            return;
        }
    }

    let size = size_kb * 1024;
    let mut data = Vec::with_capacity(size);
    // Простой LCG (linear congruential generator) с фиксированным
    // зерном — не криптографический RNG, просто детерминированный
    // "мусор", который легко сгенерировать заново для сверки.
    let mut state: u32 = 0x2A2A2A2A;
    for _ in 0..size {
        state = state.wrapping_mul(1103515245).wrapping_add(12345);
        data.push((state >> 16) as u8);
    }

    println_t!(
        en: "Writing {} KiB test file (name: BIGTEST.BIN)...",
        ru: "Записываем тестовый файл {} КиБ (имя: BIGTEST.BIN)...";
        size_kb
    );

    if let Err(_) = ext2::write_file("BIGTEST.BIN", &data) {
        println!("{}", t!(en: "Write failed (file too large for indirect block support?).", ru: "Ошибка записи (файл слишком большой даже для indirect-блока?)."));
        return;
    }

    match ext2::read_file("BIGTEST.BIN") {
        Ok(read_back) => {
            if read_back.len() != data.len() {
                println_t!(
                    en: "MISMATCH: wrote {} bytes but read back {} bytes!",
                    ru: "НЕСОВПАДЕНИЕ: записали {} байт, а прочитали {} байт!";
                    data.len(), read_back.len()
                );
                return;
            }
            if read_back == data {
                println_t!(
                    en: "OK: all {} bytes match after write+read-back (indirect blocks work correctly).",
                    ru: "OK: все {} байт совпадают после записи+чтения (indirect-блоки работают корректно).";
                    data.len()
                );
            } else {
                let first_diff = read_back.iter().zip(data.iter()).position(|(a, b)| a != b);
                println_t!(
                    en: "MISMATCH: data differs at byte offset {:?}!",
                    ru: "НЕСОВПАДЕНИЕ: данные отличаются со смещения байта {:?}!";
                    first_diff
                );
            }
        }
        Err(_) => println!("{}", t!(en: "Read-back failed.", ru: "Ошибка обратного чтения.")),
    }
}

// ==================== Авторизация (auth.rs) ====================

/// Имя пользователя, вошедшего в текущем сеансе — None до успешного
/// login (или до создания первого аккаунта при первом запуске, см.
/// run_login_screen() в lib.rs). Простое глобальное состояние
/// достаточно для однопользовательской ОС без параллельных сессий.
static CURRENT_USER: crate::spinlock::SpinLock<Option<String>> = crate::spinlock::SpinLock::new(None);

pub fn set_current_user(username: &str) {
    if username.is_empty() {
        // Пустое имя = сброс в None (безопасное состояние для install).
        *CURRENT_USER.lock() = None;
    } else {
        *CURRENT_USER.lock() = Some(username.to_string());
    }
}

pub fn current_user() -> Option<String> {
    CURRENT_USER.lock().clone()
}

/// `useradd <username> <password>` — создаёт новый аккаунт. Пароль
/// передаётся аргументом команды (не через отдельный скрытый
/// ввод) — то же ограничение, что и у команды `write`: наш простой
/// построчный CLI не поддерживает эхо-маскировку ввода посимвольно
/// внутри одной команды. Полноценный экран входа при загрузке (см.
/// lib.rs::run_login_screen) вводит пароль отдельным полем с маскировкой
/// звёздочками — именно там, а не в этой команде, обеспечена приватность
/// набора пароля.
fn cmd_useradd(arg: &str) {
    let mut parts = arg.splitn(2, ' ');
    let username = parts.next().unwrap_or("").trim();
    let password = parts.next().unwrap_or("").trim();

    if username.is_empty() || password.is_empty() {
        println!(
            "{}",
            t!(
                en: "Usage: useradd <username> <password>",
                ru: "Использование: useradd <имя_пользователя> <пароль>"
            )
        );
        return;
    }

    match crate::auth::create_user(username, password) {
        Ok(()) => {
            println_t!(
                en: "User '{}' created. Password is stored only as a salted SHA-256 hash.",
                ru: "Пользователь '{}' создан. Пароль хранится только как хэш SHA-256 с солью.";
                username
            );
            // Новому пользователю нужен СВОЙ слот в заголовке тома,
            // иначе он не сможет разблокировать зашифрованный диск при
            // загрузке: аккаунт будет, а ключа к данным — нет.
            if crate::luks::exists() {
                // Пароль текущего пользователя мы не знаем, но мастер-ключ
                // уже в памяти — том разблокирован. Пользуемся этим.
                match crate::luks::add_password_with_master(password) {
                    Ok(slot) => println!(
                        "  Пароль добавлен в слот {} — этот пользователь сможет открыть диск.",
                        slot
                    ),
                    Err(e) => println!("  ВНИМАНИЕ: слот не добавлен ({}). Пользователь не сможет разблокировать диск.", e),
                }
            }
        }
        Err(crate::auth::AuthError::UserAlreadyExists) => println_t!(
            en: "User '{}' already exists.",
            ru: "Пользователь '{}' уже существует.";
            username
        ),
        Err(crate::auth::AuthError::InvalidUsername) => println!(
            "{}",
            t!(
                en: "Invalid username (must be 1-32 alphanumeric/underscore/dash characters).",
                ru: "Недопустимое имя пользователя (1-32 символа: буквы/цифры/подчёркивание/дефис)."
            )
        ),
        Err(_) => println!("{}", t!(en: "Failed to create user (disk error).", ru: "Не удалось создать пользователя (ошибка диска).")),
    }
}

/// `passwd <username> <old_password> <new_password>` — смена пароля.
/// Требует знания текущего пароля (как в настоящих Unix-системах) —
/// не позволяет сменить чужой пароль без него.
fn cmd_passwd(arg: &str) {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() != 3 {
        println!(
            "{}",
            t!(
                en: "Usage: passwd <username> <old_password> <new_password>",
                ru: "Использование: passwd <имя_пользователя> <старый_пароль> <новый_пароль>"
            )
        );
        return;
    }

    match crate::auth::change_password(parts[0], parts[1], parts[2]) {
        Ok(()) => println!("{}", t!(en: "Password changed.", ru: "Пароль изменён.")),
        Err(crate::auth::AuthError::WrongPassword) => println!("{}", t!(en: "Wrong current password.", ru: "Неверный текущий пароль.")),
        Err(crate::auth::AuthError::UserNotFound) => println!("{}", t!(en: "User not found.", ru: "Пользователь не найден.")),
        Err(_) => println!("{}", t!(en: "Failed to change password.", ru: "Не удалось сменить пароль.")),
    }
}

fn cmd_whoami() {
    match current_user() {
        Some(name) => println!("{}", name),
        None => println!("{}", t!(en: "Not logged in.", ru: "Вход не выполнен.")),
    }
}

fn cmd_users() {
    match crate::auth::list_usernames() {
        Ok(names) if names.is_empty() => println!("{}", t!(en: "No users registered yet.", ru: "Пользователи ещё не зарегистрированы.")),
        Ok(names) => {
            for name in names {
                println!("  {}", name);
            }
        }
        Err(_) => println!("{}", t!(en: "Failed to read user database.", ru: "Не удалось прочитать базу пользователей.")),
    }
}

/// `encrypt confirm <password>` — включает сквозное AES-256-XTS
/// шифрование диска (byte-совместимое с `cryptsetup --type plain
/// --cipher aes-xts-plain64 --key-size 512 --hash sha512`, см.
/// crypto_storage.rs). Требует явного слова "confirm" перед паролем —
/// это НЕОБРАТИМАЯ операция, которая переформатирует диск и УНИЧТОЖАЕТ
/// все текущие файлы (они были записаны в открытом виде и не могут быть
/// "переупакованы" в зашифрованный вид без полной перезаписи), поэтому
/// требуется осознанное подтверждение, а не просто ввод пароля.
fn cmd_encrypt(arg: &str) {
    if crate::crypto_storage::is_encryption_enabled() {
        println!(
            "{}",
            t!(
                en: "Encryption is already enabled on this disk.",
                ru: "Шифрование уже включено на этом диске."
            )
        );
        return;
    }

    let mut parts = arg.splitn(2, ' ');
    let confirm = parts.next().unwrap_or("");
    let password = parts.next().unwrap_or("").trim();

    if confirm != "confirm" || password.is_empty() {
        println!(
            "{}",
            t!(
                en: "This PERMANENTLY erases all current files and enables full-disk \\
                     AES-256-XTS encryption (compatible with Linux 'cryptsetup --type plain \\
                     --cipher aes-xts-plain64 --key-size 512 --hash sha512'). This cannot be \\
                     undone from within the running session. If you understand and want to \\
                     proceed, run: encrypt confirm <password>",
                ru: "Это НЕОБРАТИМО стирает все текущие файлы и включает полное шифрование \\
                     диска AES-256-XTS (совместимо с Linux 'cryptsetup --type plain --cipher \\
                     aes-xts-plain64 --key-size 512 --hash sha512'). Отменить это в рамках \\
                     текущей сессии нельзя. Если понимаешь это и хочешь продолжить, выполни: \\
                     encrypt confirm <пароль>"
            )
        );
        return;
    }

    println!(
        "{}",
        t!(
            en: "Encrypting disk and reformatting ext2 (all previous files are now gone)...",
            ru: "Шифруем диск и переформатируем ext2 (все прежние файлы теперь стёрты)..."
        )
    );

    match crate::crypto_storage::enable_encryption(password) {
        Ok(()) => println!(
            "{}",
            t!(
                en: "Disk encryption enabled. You will need this password to unlock the disk \\
                     on every future boot (in addition to your normal user login).",
                ru: "Шифрование диска включено. Этот пароль нужно будет вводить для разблокировки \\
                     диска при каждой загрузке (в дополнение к обычному входу пользователя)."
            )
        ),
        Err(_) => println!(
            "{}",
            t!(en: "Failed to enable encryption (disk error).", ru: "Не удалось включить шифрование (ошибка диска).")
        ),
    }
}

fn cmd_rm(name: &str) {
    if name.is_empty() {
        println!("{}", t!(en: "Usage: rm <filename>", ru: "Использование: rm <имя_файла>"));
        return;
    }
    match ext2::delete_file(name) {
        Ok(()) => println_t!(en: "Deleted '{}'.", ru: "Удалено '{}'."; name),
        Err(ext2::Ext2Error::FileNotFound) => {
            println!("{}", t!(en: "File not found.", ru: "Файл не найден."))
        }
        Err(_) => println!("{}", t!(en: "Failed to delete file.", ru: "Не удалось удалить файл.")),
    }
}

fn cmd_pkg(arg: &str) {
    let (subcmd, rest) = match arg.find(char::is_whitespace) {
        Some(idx) => (&arg[..idx], arg[idx..].trim_start()),
        None => (arg, ""),
    };

    match subcmd {
        "list" => pkg::cmd_list(),
        "install" => pkg::cmd_install(rest),
        "remove" => pkg::cmd_remove(rest),
        "info" => pkg::cmd_info(rest),
        "" => println!(
            "{}",
            t!(
                en: "Usage: pkg <list|install|remove|info> [name]",
                ru: "Использование: pkg <list|install|remove|info> [имя]"
            )
        ),
        _ => println_t!(
            en: "Unknown pkg subcommand '{}'. Available: list, install, remove, info",
            ru: "Неизвестная подкоманда '{}'. Доступно: list, install, remove, info";
            subcmd
        ),
    }
}

fn cmd_run(arg: &str) {
    if arg.is_empty() {
        println!(
            "{}",
            t!(
                en: "Usage: run <program.mex> [args...]",
                ru: "Использование: run <программа.mex> [аргументы...]"
            )
        );
        return;
    }
    let mut parts = arg.splitn(2, ' ');
    let name = parts.next().unwrap_or("");
    let prog_args = parts.next().unwrap_or("");
    crate::mex::run(name, prog_args);
}

fn cmd_crash(kind: &str) {
    match kind {
        "divzero" => {
            println!("{}", t!(en: "Triggering division by zero...", ru: "Вызываем деление на ноль..."));
            unsafe {
                let a: u64 = 42;
                let b: u64 = 0;
                let _result: u64;
                asm!("div {0}", in(reg) b, inout("rax") a => _result, options(nostack));
            }
        }
        "bp" => {
            println!("{}", t!(en: "Triggering breakpoint (int3)...", ru: "Вызываем breakpoint (int3)..."));
            unsafe { asm!("int3") };
            println!(
                "{}",
                t!(
                    en: "Returned after breakpoint - handler worked correctly.",
                    ru: "Вернулись после breakpoint - обработчик отработал корректно."
                )
            );
        }
        "inv" => {
            println!("{}", t!(en: "Triggering invalid opcode (ud2)...", ru: "Вызываем неверный опкод (ud2)..."));
            unsafe { asm!("ud2") };
        }
        "panic" => {
            println!("{}", t!(en: "Triggering kernel panic!()...", ru: "Вызываем kernel panic!()..."));
            panic!("user-triggered crash (debugger test)");
        }
        "" => println!("{}", t!(en: "Usage: crash <divzero|bp|inv>", ru: "Укажи тип: crash <divzero|bp|inv>")),
        _ => println_t!(
            en: "Unknown type '{}'. Available: divzero, bp, inv",
            ru: "Неизвестный тип '{}'. Доступно: divzero, bp, inv";
            kind
        ),
    }
}

pub fn cmd_reboot() {
    println!("{}", t!(en: "Rebooting...", ru: "Перезагрузка..."));
    // Затираем мастер-ключ шифрования диска (если оно включено) через
    // write_volatile ПЕРЕД остановкой процессора — см. подробное
    // объяснение в crypto_storage.rs::lock()/clear_master_key(), почему
    // это не то же самое, что просто "перестать использовать" ключ.
    crate::crypto_storage::lock();
    unsafe {
        // 1) Классический сброс через контроллер клавиатуры 8042 (0xFE),
        //    но с ТАЙМАУТОМ: на некоторых машинах/QEMU бит 0x02 порта 0x64
        //    (input buffer full) не сбрасывается, и прежний цикл без
        //    таймаута вешал систему навсегда.
        let mut tries: u32 = 0;
        loop {
            let status = crate::port::inb(0x64);
            if status & 0x02 == 0 {
                break;
            }
            tries += 1;
            if tries > 1_000_000 {
                break;
            }
        }
        crate::port::outb(0x64, 0xFE);

        // Небольшая пауза, чтобы контроллер успел принять команду.
        for _ in 0..200_000 {
            core::hint::spin_loop();
        }

        // 2) Аппаратный reset через порт 0xCF9 (legacy/ACPI reset control):
        //    0x06 = hard reset + CPU reset. Работает в QEMU и на подавляющем
        //    большинстве PC — надёжнее 8042.
        crate::port::outb(0xCF9, 0x06);

        // 3) Если и это не помогло — бесконечный hlt (машина уже в пути).
        loop {
            asm!("hlt");
        }
    }
}

pub fn cmd_halt() {
    println!("{}", t!(en: "Halting the CPU. Goodbye!", ru: "Остановка процессора. До свидания!"));
    // См. комментарий в cmd_reboot() — та же самая гарантия зануления
    // мастер-ключа шифрования диска перед остановкой.
    crate::crypto_storage::lock();
    unsafe {
        asm!("cli");
        loop {
            asm!("hlt");
        }
    }
}
