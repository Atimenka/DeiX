//! Пакетный менеджер DeiX ("pkg") — устанавливает/удаляет пакеты на
//! ext2-томе (см. ext2.rs).
//!
//! Честное ограничение, как и с Wi-Fi/GPU: у DeiX нет HTTP/TLS-стека
//! (наш net/ — это Ethernet+ARP+IPv4+ICMP, без TCP), поэтому "скачивать"
//! пакеты из настоящего интернета в буквальном смысле невозможно —
//! заявлять обратное было бы нечестной заглушкой. Вместо этого pkg
//! работает с ВСТРОЕННЫМ локальным каталогом (см. CATALOG ниже):
//! `pkg install <name>` берёт содержимое пакета из этого каталога
//! (зашитого в само ядро при сборке) и реально копирует файлы на
//! ext2-диск, `pkg remove` реально удаляет их оттуда, `pkg list`
//! реально читает установленные пакеты из файла-реестра на диске.
//! Всё это самая настоящая работа с файловой системой — не имитация.

use crate::ext2;
use crate::{println, println_t, t};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Имя служебного файла-реестра, в котором pkg хранит список
/// установленных пакетов (простой текстовый формат: одно имя пакета на
/// строку). Хранится прямо на ext2-томе, как и любой другой файл.
const REGISTRY_FILE: &str = "PKG.DB";

/// Один пакет из встроенного каталога: имя, версия, описание и список
/// файлов (имя_на_диске, содержимое). Реальные, работающие файлы — не
/// заглушки: hello.mex/sysinfo.mex собраны из tools/hello.asm и
/// tools/sysinfo.asm (см. docs/MEX_FORMAT.md), остальное — обычный
/// текст.
struct Package {
    name: &'static str,
    version: &'static str,
    description_en: &'static str,
    description_ru: &'static str,
    files: &'static [(&'static str, &'static [u8])],
}

// Реальные скомпилированные .mex-программы включаются в бинарник ядра
// через include_bytes! на этапе сборки (build.sh собирает их из
// tools/*.asm ДО сборки cargo — см. шаг [2/6] в build.sh).
static HELLO_MEX: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/build/hello.mex"));
static SYSINFO_MEX: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/build/sysinfo.mex"));

static CATALOG: &[Package] = &[
    Package {
        name: "hello",
        version: "1.0",
        description_en: "Minimal demo .mex program that prints a greeting.",
        description_ru: "Минимальная демо-программа .mex, печатающая приветствие.",
        files: &[("HELLO.MEX", HELLO_MEX)],
    },
    Package {
        name: "sysinfo",
        version: "1.0",
        description_en: "Demo .mex program that prints kernel uptime via the syscall table.",
        description_ru: "Демо-программа .mex, печатающая аптайм ядра через таблицу системных функций.",
        files: &[("SYSINFO.MEX", SYSINFO_MEX)],
    },
    Package {
        name: "motd",
        version: "1.0",
        description_en: "Installs a simple text file (MOTD.TXT) — good for testing the TXT reader.",
        description_ru: "Устанавливает простой текстовый файл (MOTD.TXT) — удобно для проверки TXT reader.",
        files: &[(
            "MOTD.TXT",
            b"Welcome to DeiX!\r\n\r\nThis file was installed by the 'pkg' package manager\r\n\
              from the built-in local catalog (see src/pkg.rs). There is no real\r\n\
              network download here - DeiX has no HTTP/TLS stack (only Ethernet/\r\n\
              ARP/IPv4/ICMP) - but the file itself is 100% real and was really\r\n\
              written to the ext2 disk by pkg install.\r\n",
        )],
    },
];

fn find_package(name: &str) -> Option<&'static Package> {
    CATALOG.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

fn read_registry() -> Vec<String> {
    match ext2::read_file(REGISTRY_FILE) {
        Ok(data) => match core::str::from_utf8(&data) {
            Ok(text) => text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(),
            Err(_) => Vec::new(),
        },
        Err(_) => Vec::new(),
    }
}

fn write_registry(names: &[String]) -> Result<(), ()> {
    let mut content = String::new();
    for name in names {
        content.push_str(name);
        content.push_str("\r\n");
    }
    if !ext2::is_formatted() {
        ext2::format().map_err(|_| ())?;
    }
    ext2::write_file(REGISTRY_FILE, content.as_bytes()).map_err(|_| ())
}

/// `pkg list` — показывает и встроенный каталог доступных пакетов, и
/// реально установленные (прочитанные из PKG.DB на диске).
pub fn cmd_list() {
    println!("{}", t!(en: "Available packages (built-in local catalog):", ru: "Доступные пакеты (встроенный локальный каталог):"));
    for pkg in CATALOG {
        println!("  {:<10} {:<6} {}", pkg.name, pkg.version, t!(en: pkg.description_en, ru: pkg.description_ru));
    }

    println!();
    let installed = read_registry();
    if installed.is_empty() {
        println!("{}", t!(en: "No packages installed yet.", ru: "Пока ничего не установлено."));
    } else {
        println!("{}", t!(en: "Installed packages:", ru: "Установленные пакеты:"));
        for name in &installed {
            println!("  {}", name);
        }
    }

    println!();
    println!(
        "{}",
        t!(
            en: "Note: DeiX has no HTTP/TLS network stack (only Ethernet/ARP/IPv4/ICMP), \
                 so there is no real internet download here - packages come from a small \
                 built-in local catalog compiled into the kernel itself, but installing/ \
                 removing them does real file I/O on the ext2 disk.",
            ru: "Примечание: у DeiX нет сетевого стека HTTP/TLS (только Ethernet/ARP/IPv4/ \
                 ICMP), поэтому реального скачивания из интернета здесь нет - пакеты берутся \
                 из небольшого встроенного локального каталога, вкомпилированного в само \
                 ядро, но их установка/удаление - это настоящая работа с файлами на ext2."
        )
    );
}

/// `pkg install <name>` — реально копирует файлы пакета на ext2-диск и
/// добавляет запись в реестр.
pub fn cmd_install(name: &str) {
    if name.is_empty() {
        println!("{}", t!(en: "Usage: pkg install <name>", ru: "Использование: pkg install <имя>"));
        return;
    }

    let pkg = match find_package(name) {
        Some(p) => p,
        None => {
            println_t!(
                en: "Package '{}' not found. Run 'pkg list' to see available packages.",
                ru: "Пакет '{}' не найден. Смотри 'pkg list' для списка доступных пакетов.";
                name
            );
            return;
        }
    };

    if !ext2::is_formatted() {
        if ext2::format().is_err() {
            println!("{}", t!(en: "Failed to format disk.", ru: "Не удалось отформатировать диск."));
            return;
        }
    }

    for (filename, content) in pkg.files {
        match ext2::write_file(filename, content) {
            Ok(()) => println_t!(
                en: "  wrote {} ({} bytes)",
                ru: "  записан {} ({} байт)";
                filename, content.len()
            ),
            Err(_) => {
                println_t!(
                    en: "Failed to write '{}' — installation aborted.",
                    ru: "Не удалось записать '{}' — установка прервана.";
                    filename
                );
                return;
            }
        }
    }

    let mut installed = read_registry();
    if !installed.iter().any(|n| n.eq_ignore_ascii_case(pkg.name)) {
        installed.push(pkg.name.to_string());
        if write_registry(&installed).is_err() {
            println!(
                "{}",
                t!(
                    en: "Files were written, but failed to update the package registry.",
                    ru: "Файлы записаны, но не удалось обновить реестр пакетов."
                )
            );
            return;
        }
    }

    println_t!(
        en: "Package '{}' v{} installed successfully.",
        ru: "Пакет '{}' v{} успешно установлен.";
        pkg.name, pkg.version
    );
}

/// `pkg remove <name>` — реально удаляет файлы пакета с диска и убирает
/// запись из реестра.
pub fn cmd_remove(name: &str) {
    if name.is_empty() {
        println!("{}", t!(en: "Usage: pkg remove <name>", ru: "Использование: pkg remove <имя>"));
        return;
    }

    let pkg = match find_package(name) {
        Some(p) => p,
        None => {
            println_t!(
                en: "Unknown package '{}'.",
                ru: "Неизвестный пакет '{}'.";
                name
            );
            return;
        }
    };

    let mut installed = read_registry();
    if !installed.iter().any(|n| n.eq_ignore_ascii_case(pkg.name)) {
        println_t!(
            en: "Package '{}' is not installed.",
            ru: "Пакет '{}' не установлен.";
            pkg.name
        );
        return;
    }

    for (filename, _) in pkg.files {
        match ext2::delete_file(filename) {
            Ok(()) => println_t!(en: "  removed {}", ru: "  удалён {}"; filename),
            Err(ext2::Ext2Error::FileNotFound) => {
                println_t!(en: "  {} was already missing", ru: "  {} уже отсутствовал"; filename)
            }
            Err(_) => println_t!(
                en: "  failed to remove {}",
                ru: "  не удалось удалить {}";
                filename
            ),
        }
    }

    installed.retain(|n| !n.eq_ignore_ascii_case(pkg.name));
    let _ = write_registry(&installed);

    println_t!(
        en: "Package '{}' removed.",
        ru: "Пакет '{}' удалён.";
        pkg.name
    );
}

/// `pkg info <name>` — подробности о пакете из каталога.
pub fn cmd_info(name: &str) {
    if name.is_empty() {
        println!("{}", t!(en: "Usage: pkg info <name>", ru: "Использование: pkg info <имя>"));
        return;
    }
    let pkg = match find_package(name) {
        Some(p) => p,
        None => {
            println_t!(en: "Unknown package '{}'.", ru: "Неизвестный пакет '{}'."; name);
            return;
        }
    };

    println_t!(en: "Name: {}", ru: "Имя: {}"; pkg.name);
    println_t!(en: "Version: {}", ru: "Версия: {}"; pkg.version);
    println!("{}", t!(en: pkg.description_en, ru: pkg.description_ru));
    println!("{}", t!(en: "Files:", ru: "Файлы:"));
    for (filename, content) in pkg.files {
        println!("  {} ({} {})", filename, content.len(), t!(en: "bytes", ru: "байт"));
    }

    let installed = read_registry().iter().any(|n| n.eq_ignore_ascii_case(pkg.name));
    println!(
        "{}",
        if installed {
            t!(en: "Status: installed", ru: "Статус: установлен")
        } else {
            t!(en: "Status: not installed", ru: "Статус: не установлен")
        }
    );
}
