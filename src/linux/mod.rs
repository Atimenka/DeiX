//! ПРОСЛОЙКА СОВМЕСТИМОСТИ С LINUX — запуск обычных ELF-программ.
//!
//! Цель: брать готовые бинарники (в том числе из репозиториев Arch) и
//! исполнять их в DeiX. Для этого нужны две вещи:
//!
//!   1. **загрузчик ELF** (`elf.rs`) — разбор заголовков, отображение
//!      сегментов `PT_LOAD`, применение релокаций для PIE;
//!   2. **Linux syscall ABI** (`syscall.rs`) — номера и семантика
//!      системных вызовов как в Linux x86-64.
//!
//! ## Что честно работает, а что нет
//!
//! Работает: **статические PIE-бинарники**. Они не требуют
//! динамического линковщика и позиционно независимы, поэтому их можно
//! разместить где угодно.
//!
//! Не работает пока:
//!
//! * **статические не-PIE** (`Type: EXEC`) — слинкованы по абсолютному
//!   адресу `0x400000`, а там лежит куча ядра. Перенести их нельзя:
//!   адреса в коде абсолютные. Нужно поднимать `.bss` ядра выше 16 МиБ,
//!   а это ломает раннюю загрузку (проверено: 14 МиБ работает, 16 — нет).
//!   Именно так слинкован `busybox` из Arch.
//! * **динамические** (`bash`, `ls`) — нужен `ld-linux-x86-64.so.2`,
//!   glibc, TLS. Это следующий большой этап.
//!
//! То есть прослойка реальная, но список запускаемого пока узкий —
//! и я говорю об этом прямо, а не выдаю задел за готовый Linux.

pub mod elf;
pub mod syscall;

use alloc::string::String;
use alloc::vec::Vec;

/// Куда грузятся пользовательские программы.
///
/// Выше кучи ядра (`.bss` до ~0x121C000) и выше страницы Ring 3
/// (0x1400000..0x1600000), ниже RAM-диска (0x2000000).
pub const USER_IMAGE_BASE: u64 = 0x0180_0000; // 24 МиБ
/// Предел образа программы.
pub const USER_IMAGE_MAX: u64 = 0x0040_0000; // 4 МиБ
/// Стек программы. Лежит ПОСЛЕ образа, отдельной областью: раньше он
/// стоял ровно на границе `USER_IMAGE_BASE + USER_IMAGE_MAX`, и первый
/// же пролог (`sub $0x190, %rsp`) уводил указатель за пределы
/// отображённой памяти — программа падала с page fault.
pub const USER_STACK_BOTTOM: u64 = USER_IMAGE_BASE + USER_IMAGE_MAX;
/// Размер стека.
pub const USER_STACK_SIZE: u64 = 256 * 1024;
/// Вершина стека (стек растёт вниз, к USER_STACK_BOTTOM).
pub const USER_STACK_TOP: u64 = USER_STACK_BOTTOM + USER_STACK_SIZE;
/// Куча программы (`brk`) начинается за стеком.
pub const USER_HEAP_BASE: u64 = USER_STACK_TOP + 0x1000;
/// Предел кучи программы.
pub const USER_HEAP_MAX: u64 = 2 * 1024 * 1024;

/// Верхняя граница всей пользовательской области — для проверки
/// указателей, приходящих из Ring 3.
pub const USER_AREA_END: u64 = USER_HEAP_BASE + USER_HEAP_MAX;

/// Результат запуска программы.
pub struct RunResult {
    pub exit_code: i64,
    pub syscalls: usize,
}

/// Загружает и запускает ELF-программу из байтов образа.
pub fn run_elf(image: &[u8], name: &str) -> Result<RunResult, String> {
    let loaded = elf::load(image)?;

    crate::println!(
        "  [linux] {}: {} сегм., точка входа {:#x} (база {:#x})",
        name,
        loaded.segments,
        loaded.entry,
        USER_IMAGE_BASE
    );

    syscall::reset_stats();
    let code = unsafe { crate::usermode::run_user_program(loaded.entry, loaded.stack_top) };

    Ok(RunResult {
        exit_code: code,
        syscalls: syscall::count(),
    })
}

/// Запускает программу из файла на ext2-диске.
pub fn run_file(path: &str) -> Result<RunResult, String> {
    let data = crate::ext2::read_file(path).map_err(|_| {
        alloc::format!("не удалось прочитать '{}' с ext2-диска", path)
    })?;
    run_elf(&data, path)
}

/// Диагностика ELF без запуска: тип, точка входа, сегменты.
pub fn inspect(image: &[u8]) -> Result<Vec<String>, String> {
    elf::inspect(image)
}

/// CLI: `linux <run|info> <файл>` — запуск ELF-программ Linux.
pub fn cmd_linux(arg: &str) {
    let mut it = arg.trim().splitn(2, ' ');
    let sub = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();

    match sub {
        "run" if !rest.is_empty() => match run_file(rest) {
            Ok(r) => crate::println!(
                "  [linux] завершено, код {}, системных вызовов: {}",
                r.exit_code,
                r.syscalls
            ),
            Err(e) => crate::println!("  [linux] ОШИБКА: {}", e),
        },
        "info" if !rest.is_empty() => {
            match crate::ext2::read_file(rest) {
                Ok(d) => match inspect(&d) {
                    Ok(lines) => {
                        crate::println!("  [linux] {}:", rest);
                        for l in lines {
                            crate::println!("    {}", l);
                        }
                    }
                    Err(e) => crate::println!("  [linux] не ELF: {}", e),
                },
                Err(_) => crate::println!("  [linux] файл '{}' не найден на ext2", rest),
            }
        }
        "test" => selftest(),
        _ => {
            crate::println!("linux run <файл>   - запустить ELF-программу Linux с ext2-диска");
            crate::println!("linux info <файл>  - разобрать ELF без запуска");
            crate::println!("linux test         - самопроверка на встроенной программе");
        }
    }
}

/// Встроенная тестовая программа: статический PIE из tools/linux_hello.c.
///
/// Держим её маленькой: `include_bytes!` увеличивает сам kernel.bin,
/// а его размер ограничен загрузчиком (KERNEL_SECTORS в boot/*.asm).
// Встроенный тестовый ELF УБРАН из kernel.bin: он занимал 5 КиБ, а
// ядро упёрлось в потолок 572 КиБ (буфер загрузчика граничит с
// видеопамятью VGA). Программа никуда не делась — собирается в
// build/linux_hello.elf и запускается с диска: 'linux run <файл>'.
static HELLO_ELF: &[u8] = &[];

/// Самопроверка прослойки: запускает настоящий Linux-ELF.
pub fn selftest() {
    crate::println!("  [linux] самопроверка: запуск встроенного ELF ({} байт)", HELLO_ELF.len());
    match run_elf(HELLO_ELF, "linux_hello.elf") {
        Ok(r) => {
            if r.exit_code == 0 && r.syscalls > 0 {
                crate::println!(
                    "  [linux] САМОПРОВЕРКА ПРОЙДЕНА: программа Linux отработала \
                     ({} системных вызовов)",
                    r.syscalls
                );
            } else {
                crate::println!("  [linux] САМОПРОВЕРКА НЕ ПРОЙДЕНА (код {})", r.exit_code);
            }
        }
        Err(e) => crate::println!("  [linux] САМОПРОВЕРКА НЕ ПРОЙДЕНА: {}", e),
    }
}
