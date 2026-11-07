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
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::arch::asm;

const MAX_LINE: usize = 120;
const MAX_HISTORY: usize = 32;

pub fn run() -> ! {
    // Фаза загрузки закончена: система диагностики перестаёт дублировать
    // события на консоль — дальше журнал смотрят командами log/error.
    crate::diag::log::finish_boot();
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
                // Отложенная запись журналов: события, накопленные командой,
                // уходят в /userdata/log. Дёшево: при пустом буфере выходит
                // сразу по счётчику.
                crate::diag::persist::flush();
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
        "dialog" => cmd_dialog_test(),
        "erofs" => cmd_erofs_test(),
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
        "poweroff" | "shutdown" => cmd_poweroff(),
        "halt" => cmd_halt(),
        "duil" => crate::duil::cmd_duil(rest),
        "cli" => cmd_cli(rest),
        "ds" => crate::ds::cmd_ds(rest),

        "taskmgr" => crate::sched::cmd_threads(rest),
        "ps" => crate::process::cmd_ps(),
        "kill" => crate::process::cmd_kill(rest),
        "lsmod" => cmd_lsmod(),
        "kmod" => cmd_kmod(rest),
        "log" => crate::diag::cli::cmd_log(rest),
        "error" => crate::diag::cli::cmd_error(rest),
        "panic" => crate::diag::cli::cmd_panic(rest),
        "diagnostics" => crate::diag::cli::cmd_diagnostics(rest),
        "dev" => {
            match rest.trim() {
                "on" => crate::devmode::enable_dev_mode(),
                "off" => crate::devmode::disable_dev_mode(),
                "request" => crate::devauth::cmd_request(),
                "revoke" => crate::devauth::cmd_revoke(),
                "flags" => crate::devauth::cmd_flags(),
                _ => crate::devmode::dev_status(),
            }
        }
        "sysfs" => crate::sysfs::cmd_sysfs(rest),
        "nvidia" => crate::drivers::nvidia::cmd_nvidia(rest),
        "microcode" => crate::microcode::cmd_microcode(rest),
        "hal" => crate::drivers::hal_selftest(),
        "crypt" => crate::luks::cmd_crypt(rest),
        "dinit" => crate::dinit::cmd_dinit(rest),
        "logo" => crate::bootlogo::cmd_logo(rest),
        "wallpaper" => cmd_wallpaper(rest),
        "linux" => crate::linux::cmd_linux(rest),
        "profile" => crate::userfs::cmd_profile(rest, &current_user().unwrap_or_default()),
        "lock" => crate::loginui::cmd_lock(&current_user().unwrap_or_default()),
        "threads" => crate::sched::cmd_threads(rest),
        "su" | "sudo" | "root" => {
            if crate::devmode::sudo_allowed() {
                // Dev-режим: sudo доступен, гарантии целостности сняты.
                println!("{}", t!(
                    en: "SUDO: root shell in dev-mode. System integrity guarantees are VOID.",
                    ru: "SUDO: root-оболочка в dev-режиме. Гарантии целостности системы НЕ действуют."
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
            ru: "Неизвестная команда: '{}'. Введи 'help'.",
            cmd
        ),
    }
}

/// sound [list|play <имя>|beep [hz ms]] — звуковые эффекты UI через
/// PC speaker (DPS-файлы из /system/media/audio/ui, см. src/sound.rs).
/// Без подкоманды — список эффектов с отметкой наличия в образе.
fn cmd_sound(rest: &str) {
    let mut it = rest.split_whitespace();
    match it.next() {
        None | Some("list") => {
            println!("{}", t!(en: "UI sounds (PC speaker; files live in /system/media/audio/ui):", ru: "UI-звуки (PC speaker; файлы в /system/media/audio/ui):"));
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
        Some("warn") => crate::sound::warn_triple(),
        Some("alert") => crate::sound::alert(),
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
    println!("  ifconfig [ip|dns <ip>]  - {}", t!(en: "show/change network config (IP, DNS server)", ru: "показать/сменить сетевую конфигурацию (IP, DNS-сервер)"));
    println!("  arp                     - {}", t!(en: "show ARP cache", ru: "показать ARP-кэш"));
    println!("  ping <ip>               - {}", t!(en: "send ICMP echo request", ru: "отправить ICMP echo request"));
    println!("  dns <host>              - {}", t!(en: "resolve a hostname via DNS (UDP/53)", ru: "разрешить имя хоста через DNS (UDP/53)"));
    println!("  threads [list|test]     - {}", t!(en: "preemptive multitasking: task list / selftest", ru: "вытесняющая многозадачность: список задач / самопроверка"));
    println!("  crypt <status|addpass|delpass|iter> - {}", t!(en: "volume password slots (LUKS-style)", ru: "пароли тома: слоты, как в LUKS"));
    println!("  dinit [status|services|mounts|users|audit|security|stage|reload] - {}", t!(en: "PID 1 supervisor: services, mounts, users, audit, secmon", ru: "супервизор PID 1: службы, монтирования, пользователи, аудит, монитор"));
    println!("  hal                     - {}", t!(en: "driver layer selftest on RTL8139", ru: "самопроверка прослойки драйверов на RTL8139"));
    println!("  nvidia                  - {}", t!(en: "NVIDIA hardware detection: probe and identify GPU (not an acceleration driver)", ru: "детект NVIDIA-железа: поиск и опознание карты (не драйвер ускорения)"));
    println!("  logo [show|info]        - {}", t!(en: "boot logo: show / info", ru: "загрузочное лого: показать / инфо"));
    println!("  wallpaper <url|file>    - {}", t!(en: "download PNG/BMP and set as desktop wallpaper", ru: "скачать PNG/BMP и поставить обоями рабочего стола"));
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
    println!("  sysfs [status|convert]  - {}", t!(en: "convert /system from EROFS to ext2 (dev mode only)", ru: "конвертировать /system из EROFS в ext2 (только dev-режим)"));
    println!("  dev [request|on|off|revoke] - {}", t!(en: "developer mode (server-side activation, see docs/DEVAUTH.md)", ru: "режим разработчика (серверная активация, см. docs/DEVAUTH.md)"));
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
    println!("  log [errors|boot|...]   - {}", t!(en: "system event log", ru: "журнал событий системы"));
    println!("  error [show DX-...]     - {}", t!(en: "error list and code cards", ru: "список ошибок и карточки кодов"));
    println!("  panic [last|clear]      - {}", t!(en: "last kernel panic report", ru: "последний отчёт об отказе ядра"));
    println!("  diagnostics [mode on]   - {}", t!(en: "diagnostic summary / verbose mode", ru: "сводка диагностики / подробный режим"));
    println!("  reboot                  - {}", t!(en: "reboot system", ru: "перезагрузка системы"));
    println!("  poweroff / shutdown     - {}", t!(en: "power off system (ACPI / QEMU)", ru: "выключение системы (ACPI / QEMU)"));
    println!("  halt                    - {}", t!(en: "halt CPU low-level (cli; hlt)", ru: "остановить процессор (cli; hlt)"));
    println!();
    println!("{}", t!(en: "Up/Down arrows browse command history.", ru: "Стрелки вверх/вниз - навигация по истории команд."));
}

fn cmd_about() {
    let gui = if crate::ui::compositor_status() {
        t!(en: "active", ru: "активен")
    } else {
        t!(en: "inactive", ru: "неактивен")
    };
    println!(
        "{} - mini kernel written in Rust (GUI compositor: {})",
        crate::OS_RELEASE,
        gui
    );
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
    let name_trimmed = name.trim();
    let sub = name_trimmed.split_whitespace().next().unwrap_or("");
    if matches!(
        sub,
        "opacity" | "alpha" | "transparency" | "blur" | "bg" | "background" | "valid" | "invalid" | "text" | "preset" | "reset"
    ) || sub.starts_with("0x") || sub.starts_with('#') {
        crate::ui::apps::terminal::cmd_terminal_color(name_trimmed);
        return;
    }
    let _ = crate::renderer::Color::palette();
    if let Ok(idx) = name_trimmed.parse::<u8>() {
        let c = Color::from_index(idx);
        with_writer(|w| w.set_color(c, Color::Black));
        println!("{}", t!(en: "Color changed.", ru: "Цвет изменён."));
        return;
    }
    let color = match name_trimmed {
        "black" => Some(Color::Black),
        "blue" => Some(Color::Blue),
        "green" => Some(Color::Green),
        "cyan" => Some(Color::Cyan),
        "red" => Some(Color::Red),
        "magenta" => Some(Color::Magenta),
        "brown" => Some(Color::Brown),
        "lightgray" => Some(Color::LightGray),
        "darkgray" => Some(Color::DarkGray),
        "lightblue" => Some(Color::LightBlue),
        "lightgreen" => Some(Color::LightGreen),
        "lightcyan" => Some(Color::LightCyan),
        "lightred" => Some(Color::LightRed),
        "pink" => Some(Color::Pink),
        "yellow" => Some(Color::Yellow),
        "white" => Some(Color::White),
        "" => None,
        _ => {
            crate::ui::apps::terminal::cmd_terminal_color(name_trimmed);
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
    println!("CPU vendor: {}", crate::cpuid::vendor_string());
    println!("CPU: {}", crate::cpuid::describe());
}

    match crate::net::dns::resolve_a(host) {
        Some(ip) => println!("{}: {}.{}.{}.{}", host, ip[0], ip[1], ip[2], ip[3]),
        None => println_t!(
            en: "Could not resolve '{}' (timeout, NXDOMAIN or no network).",
            ru: "Не удалось разрешить '{}' (таймаут, NXDOMAIN или нет сети).";
            host
        ),
    }
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
        en: "NVIDIA PCI device {:02x}:{:02x}.{}, BAR0 MMIO {:#010x}",
        ru: "PCI устройство NVIDIA {:02x}:{:02x}.{}, BAR0 MMIO {:#010x}";
        nv.device.bus, nv.device.slot, nv.device.function, nv.bar0_mmio
    );
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
    if let Some(fb) = crate::vbe::get_framebuffer() {
        println_t!(
            en: "Framebuffer active: {}x{} @ {} bpp",
            ru: "Активный framebuffer: {}x{} @ {} bpp";
            fb.width, fb.height, fb.bpp()
        );
    }
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

    // Back buffer старого режима освобождается до установки нового:
    // два полноэкранных буфера одновременно не помещаются в кучу при
    // переходе на большие разрешения (см. renderer::set_active_framebuffer).
    crate::renderer::reset_renderer();

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
        None => {
            // VBE уже выключен/перепрограммирован: без восстановления
            // текстового режима пользователь остался бы на чёрном
            // экране с невидимой консолью.
            crate::vbe::restore_text_mode();
            crate::font::restore_full_font();
            false
        }
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

    crate::crypto_storage::lock();
    unsafe {
        crate::port::outw(0x604, 0x2000);
        crate::port::outw(0xB004, 0x2000);
        crate::port::outw(0x4004, 0x3400);
        asm!("cli");
        loop {
            asm!("hlt");
        }
    }
}

pub fn cmd_dialog_test() {
    let ok = crate::dialog::confirm("Continue?");
    println!("confirm: {}", ok);
    let line = crate::dialog::prompt_line("Enter text");
    println!("prompt_line: {}", line);
    let masked = crate::dialog::prompt_masked("Enter password");
    println!("prompt_masked length: {}", masked.len());
    let items = [String::from("Option 1"), String::from("Option 2")];
    if let Some(idx) = crate::dialog::choose("Select item", &items) {
        println!("selected: {}", idx);
    }
}

pub fn cmd_erofs_test() {
    let dummy = crate::erofs::build_image(&[("hello.txt", b"hello world\n")]);
    if crate::erofs::is_erofs(&dummy) {
        println!("  [erofs] build_image and is_erofs OK ({} bytes)", dummy.len());
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

pub const VALID_COMMANDS: &[&str] = &[
    "help", "about", "echo", "clear", "uptime", "color", "cpuid", "mem", "lang",
    "ifconfig", "arp", "ping", "dns", "threads", "crypt", "dinit", "hal", "gpu", "sound",
    "ls", "cat", "write", "mkdir", "rm", "pkg", "dialog", "erofs", "run", "install",
    "bigfile", "useradd", "passwd", "whoami", "users", "encrypt", "crash",
    "bugreport", "dmesg", "crashlog", "duil", "ds", "taskmgr", "reboot",
    "poweroff", "shutdown", "halt", "cli", "kmod", "lsmod", "dev", "profile",
    "ps", "kill", "log", "error", "panic", "diagnostics", "wallpaper", "sysfs"
];

pub fn is_valid_command(name: &str) -> bool {
    let first = name.trim().split_whitespace().next().unwrap_or("");
    if first.is_empty() {
        return true;
    }
    VALID_COMMANDS.contains(&first)
}

fn cmd_cli(rest: &str) {
    match rest.trim() {
        "new" | "duil" => run_duil_terminal(),
        "legacy" | "text" | "old" => println!("{}", t!(en: "Already in legacy text CLI.", ru: "Уже в классическом CLI.")),
        _ => {
            println!("  cli new | duil    - {}", t!(en: "switch to DUIL syntax-highlighting terminal", ru: "переключиться в DUIL терминал с подсветкой синтаксиса"));
            println!("  cli legacy | text - {}", t!(en: "standard text CLI", ru: "классический текстовый CLI"));
        }
    }
}

pub fn run_duil_terminal() {
    println!();
    println!("{}", t!(
        en: "=== DeiX DUIL Interactive Terminal v2.0 ===",
        ru: "=== Интерактивный терминал DeiX DUIL v2.0 ==="
    ));
    println!("{}", t!(
        en: "Syntax highlighting enabled (Green = Valid command, Red = Unknown command).",
        ru: "Подсветка синтаксиса включена (Зелёный = Верная команда, Красный = Неизвестная)."
    ));
    println!("{}", t!(
        en: "Type 'color opacity|blur|bg|preset' to customize colors and transparency.",
        ru: "Введи 'color opacity|blur|bg|preset' для настройки цветов и прозрачности."
    ));
    println!("{}", t!(
        en: "Type 'cli legacy' or 'exit' to return to standard text CLI.",
        ru: "Введи 'cli legacy' или 'exit' для возврата в классический CLI."
    ));

    let mut line_buf = [0u8; MAX_LINE];
    let mut len = 0usize;
    let mut history: Vec<String> = Vec::new();
    let mut history_cursor: Option<usize> = None;
    let prev_printed_len = core::sync::atomic::AtomicUsize::new(0);

    let print_duil_prompt = |input: &str| {
        print!("\r");
        with_writer(|w| w.set_color(Color::LightCyan, Color::Black));
        print!("deix [DUIL]> ");
        let first_word = input.trim_start().split_whitespace().next().unwrap_or("");
        if first_word.is_empty() {
            with_writer(|w| w.set_color(Color::White, Color::Black));
            print!("{}", input);
        } else if is_valid_command(first_word) {
            with_writer(|w| w.set_color(Color::LightGreen, Color::Black));
            print!("{}", first_word);
            with_writer(|w| w.set_color(Color::White, Color::Black));
            if input.len() > first_word.len() {
                print!("{}", &input[first_word.len()..]);
            }
        } else {
            with_writer(|w| w.set_color(Color::LightRed, Color::Black));
            print!("{}", input);
            with_writer(|w| w.set_color(Color::White, Color::Black));
        }

        let cur_len = 13 + input.len();
        let prev = prev_printed_len.swap(cur_len, core::sync::atomic::Ordering::Relaxed);
        if prev > cur_len {
            let diff = prev - cur_len;
            for _ in 0..diff {
                print!(" ");
            }
            for _ in 0..diff {
                print!("\x08");
            }
        }
    };

    print_duil_prompt("");

    loop {
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
                with_writer(|w| w.set_color(Color::White, Color::Black));
                let cmd = core::str::from_utf8(&line_buf[..len]).unwrap_or("");
                let trimmed = cmd.trim();
                if trimmed == "exit" || trimmed == "cli legacy" || trimmed == "cli text" {
                    println!("{}", t!(en: "Returning to legacy CLI...", ru: "Возврат в классический CLI..."));
                    break;
                }
                if !trimmed.is_empty() {
                    push_history(&mut history, cmd);
                }
                execute(cmd);
                len = 0;
                history_cursor = None;
                prev_printed_len.store(0, core::sync::atomic::Ordering::Relaxed);
                print_duil_prompt("");
            }
            0x08 => {
                if len > 0 {
                    len -= 1;
                    let current = core::str::from_utf8(&line_buf[..len]).unwrap_or("");
                    print_duil_prompt(current);
                }
            }
            b'\t' => {
                let current = core::str::from_utf8(&line_buf[..len]).unwrap_or("");
                let first = current.trim_start();
                if !first.is_empty() {
                    if let Some(&match_cmd) = VALID_COMMANDS.iter().find(|&&cmd| cmd.starts_with(first)) {
                        let bytes = match_cmd.as_bytes();
                        let copy_len = bytes.len().min(MAX_LINE);
                        line_buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
                        len = copy_len;
                        print_duil_prompt(match_cmd);
                    }
                }
            }
            keyboard::ARROW_UP => {
                navigate_history(&history, &mut history_cursor, &mut line_buf, &mut len, -1);
                let current = core::str::from_utf8(&line_buf[..len]).unwrap_or("");
                print_duil_prompt(current);
            }
            keyboard::ARROW_DOWN => {
                navigate_history(&history, &mut history_cursor, &mut line_buf, &mut len, 1);
                let current = core::str::from_utf8(&line_buf[..len]).unwrap_or("");
                print_duil_prompt(current);
            }
            byte => {
                if len < MAX_LINE {
                    line_buf[len] = byte;
                    len += 1;
                    let current = core::str::from_utf8(&line_buf[..len]).unwrap_or("");
                    print_duil_prompt(current);
                }
            }
        }
    }
}
