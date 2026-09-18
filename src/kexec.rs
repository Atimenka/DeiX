//! KEXEC — загрузка и запуск ядра из раздела /kernel_a|/kernel_b.
//!
//! Это то, чего не хватало, чтобы разделы `/kernel_*` перестали быть
//! просто хранилищем для OTA. Раньше `bootchain::load_kernel()` читал
//! образ, измерял его длину и выбрасывал; управление всегда оставалось
//! у ядра, загруженного stage2 с LBA 3. Теперь образ из раздела реально
//! исполняется.
//!
//! ## Как это работает
//!
//! Ядро DeiX — плоский бинарник с базой `0x100000` (см.
//! `boot/linker_kernel.ld`), точка входа `long_mode_start` совпадает с
//! началом образа. Отсюда главная сложность: **мы сами выполняемся по
//! этому адресу**. Копировать новый образ прямо на 0x100000 нельзя —
//! мы затрём собственный код на полпути.
//!
//! Поэтому используется классическая для kexec схема с трамплином:
//!
//! ```text
//!   1. образ читается в кучу (staging, внутри .bss ядра)
//!   2. крошечный позиционно-независимый трамплин копируется в
//!      свободную страницу 0x1300000 — вне и старого ядра, и .bss
//!   3. переходим на трамплин: он уже НЕ зависит от старого ядра
//!   4. трамплин копирует staging -> 0x100000 (rep movsb)
//!   5. ставит собственный стек и прыгает на 0x100000
//! ```
//!
//! Карта памяти (важно, всё проверено по `nm`/`readelf`):
//!
//! ```text
//!   0x0010_0000  код+данные ядра (~0x7F000)   <- цель копирования
//!   0x0020_0000  .bss ядра, включая кучу 16 МиБ (до ~0x121C000)
//!   0x0130_0000  трамплин kexec (эта страница)
//!   0x0135_0000  стек трамплина
//!   0x0136_0000  маркер поколения загрузки
//!   0x0140_0000  страница Ring 3
//! ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Адрес, по которому линкуется и стартует ядро.
const KERNEL_LOAD_ADDR: u64 = 0x0010_0000;
/// Свободная страница под трамплин (вне ядра и вне .bss).
const TRAMPOLINE_ADDR: u64 = 0x0130_0000;
/// Стек, который трамплин отдаёт новому ядру.
const TRAMPOLINE_STACK: u64 = 0x0135_0000;
/// Маркер «загрузились через kexec» — переживает перезапуск ядра,
/// так как лежит вне `.bss` (которую новое ядро обнуляет).
const KEXEC_MARKER: u64 = 0x0136_0000;
/// Сигнатура маркера.
const MARKER_MAGIC: u64 = 0x4B45_5845_4331; // "KEXEC1"

/// Разумный потолок размера ядра: должно влезть между 1 МиБ и .bss.
const MAX_KERNEL_SIZE: usize = 0x0010_0000; // 1 МиБ

/// Ошибки kexec.
#[derive(Debug)]
pub enum KexecError {
    /// Не удалось прочитать раздел или распаковать образ.
    Load(String),
    /// Образ пустой, слишком большой или не похож на ядро DeiX.
    Invalid(String),
}

impl KexecError {
    pub fn message(&self) -> String {
        match self {
            KexecError::Load(s) => format!("kexec: {}", s),
            KexecError::Invalid(s) => format!("kexec: образ отклонён: {}", s),
        }
    }
}

/// Извлекает `kernel.bin` из раздела слота: EROFS -> gzip -> tar.
///
/// `slot`: `None` — активный слот по BCB, `Some(0)` — A, `Some(1)` — B.
pub fn load_kernel_image(slot: Option<u8>) -> Result<Vec<u8>, KexecError> {
    let layout = match slot {
        None => crate::partition_map::active_kernel_layout(),
        Some(0) => crate::partition_map::kernel_layout_for_slot(0),
        Some(_) => crate::partition_map::kernel_layout_for_slot(1),
    };

    let image = crate::bootchain::read_partition_image(layout).map_err(KexecError::Load)?;
    let tar_gz = crate::bootchain::erofs_extract(&image, "kernel.tar.gz")
        .map_err(KexecError::Load)?;
    let tar = crate::inflate::gunzip(&tar_gz, 4 * 1024 * 1024)
        .map_err(|e| KexecError::Load(format!("gunzip: {:?}", e)))?;
    let archive = crate::kernel_loader::TarArchive::parse(tar)
        .map_err(|e| KexecError::Load(format!("tar: {}", e.message())))?;
    let kernel_bin = archive
        .extract("kernel.bin")
        .map_err(|e| KexecError::Load(e.message()))?;

    validate(&kernel_bin)?;
    Ok(kernel_bin)
}

/// Проверяет, что образ похож на ядро DeiX и его безопасно запускать.
///
/// Полноценной подписи здесь нет (её проверяет AVB на этапе прошивки),
/// но грубые ошибки — пустой файл, обрезанный образ, размер, который
/// затрёт `.bss`, — отсекаем до того, как затрём работающее ядро.
fn validate(image: &[u8]) -> Result<(), KexecError> {
    if image.is_empty() {
        return Err(KexecError::Invalid("пустой образ".into()));
    }
    if image.len() < 4096 {
        return Err(KexecError::Invalid(format!(
            "слишком мал ({} байт)",
            image.len()
        )));
    }
    if image.len() > MAX_KERNEL_SIZE {
        return Err(KexecError::Invalid(format!(
            "{} байт больше лимита {} — затрёт .bss",
            image.len(),
            MAX_KERNEL_SIZE
        )));
    }
    // Первая инструкция ядра — `mov ax, 0` (66 b8 00 00) из
    // long_mode_init.asm. Это дешёвая, но действенная проверка того, что
    // нам подсунули именно kernel.bin, а не случайный файл.
    if image[0] != 0x66 || image[1] != 0xB8 {
        return Err(KexecError::Invalid(format!(
            "не похоже на kernel.bin (первые байты {:#04x} {:#04x}, ожидалось 0x66 0xb8)",
            image[0], image[1]
        )));
    }
    Ok(())
}

// ==================== Трамплин ====================

// Позиционно-независимый код: копирует образ и передаёт управление.
// Аргументы приходят в регистрах и НЕ трогают память старого ядра:
//   rdi = адрес назначения (0x100000)
//   rsi = адрес staging-буфера
//   rcx = длина образа в байтах
//   r8  = точка входа (= адрес назначения)
//   r9  = стек для нового ядра
core::arch::global_asm!(
    ".section .text",
    ".global kexec_trampoline",
    ".global kexec_trampoline_end",
    "kexec_trampoline:",
    "  cld",
    "  rep movsb",          // копируем образ на 0x100000
    // Приводим железо в состояние «как после загрузчика». Старое ядро
    // оставило PIC размаскированным и PIT тикающим; новое ядро ставит
    // свою IDT не сразу, и первый же тик таймера ушёл бы по мусорному
    // вектору. Маскируем все IRQ на обоих PIC и глушим канал 0 PIT.
    "  mov al, 0xFF",
    "  out 0x21, al",       // PIC1: замаскировать все линии
    "  out 0xA1, al",       // PIC2: то же самое
    // PIT канал 0 переводим в режим 2 (rate generator) с максимальным
    // делителем. Режим 0 (one-shot) оставлял линию IRQ0 взведённой, и
    // после sti новое ядро получало прерывание раньше, чем успевало
    // доинициализировать таймер.
    "  mov al, 0x34",       // канал 0, lo/hi, режим 2
    "  out 0x43, al",
    "  xor al, al",
    "  out 0x40, al",       // делитель 0x0000 = 65536 (медленно)
    "  out 0x40, al",
    // Сбрасываем возможное незавершённое прерывание: посылаем EOI обоим
    // контроллерам, иначе PIC считает, что обработчик ещё не ответил.
    "  mov al, 0x20",
    "  out 0xA0, al",
    "  out 0x20, al",
    // Опустошаем буфер контроллера PS/2. Если пользователь вводил команду
    // с клавиатуры, там остались непрочитанные скан-коды: линия IRQ1
    // держится взведённой, и новое ядро попадает в шторм прерываний
    // сразу после sti.
    "  mov ecx, 32",
    "5:",
    "  in al, 0x64",
    "  test al, 1",
    "  jz 6f",
    "  in al, 0x60",
    "  loop 5b",
    "6:",
    "  mov rsp, r9",        // стек для нового ядра
    "  xor rbp, rbp",
    "  jmp r8",             // передаём управление
    "kexec_trampoline_end:",
);

extern "C" {
    static kexec_trampoline: u8;
    static kexec_trampoline_end: u8;
}

/// Записывает маркер поколения загрузки (см. `boot_generation`).
unsafe fn bump_generation() {
    let p = KEXEC_MARKER as *mut u64;
    let gen = if core::ptr::read_volatile(p) == MARKER_MAGIC {
        core::ptr::read_volatile(p.add(1)).wrapping_add(1)
    } else {
        1
    };
    core::ptr::write_volatile(p, MARKER_MAGIC);
    core::ptr::write_volatile(p.add(1), gen);
}

/// Сколько раз ядро перезапускалось через kexec (0 — обычная загрузка).
///
/// Значение лежит вне `.bss`, поэтому переживает обнуление секции новым
/// ядром — это и есть доказательство, что выполнился именно перезапуск.
pub fn boot_generation() -> u64 {
    unsafe {
        let p = KEXEC_MARKER as *const u64;
        if core::ptr::read_volatile(p) == MARKER_MAGIC {
            core::ptr::read_volatile(p.add(1))
        } else {
            0
        }
    }
}

/// ЗАПУСКАЕТ переданный образ ядра. Не возвращается.
///
/// # Safety
/// Полностью заменяет работающее ядро. Все структуры текущего ядра
/// (куча, IDT, драйверы) перестают существовать.
pub unsafe fn kexec(image: &[u8]) -> ! {
    // Останавливаем планировщик: после подмены ядра его задачи и стеки
    // перестанут существовать, а заглушка IRQ0 не должна пытаться в них
    // переключиться.
    crate::sched::stop();

    // С этого момента ни одно прерывание не должно нас перебить:
    // обработчики живут в старом ядре, которое мы вот-вот затрём.
    core::arch::asm!("cli", options(nomem, nostack));

    // Копируем трамплин в свободную страницу. Пока он копируется, мы всё
    // ещё исполняемся в старом ядре — это безопасно, страница ничем не
    // занята.
    let tramp_src = &raw const kexec_trampoline as *const u8;
    let tramp_len = (&raw const kexec_trampoline_end as usize) - (tramp_src as usize);
    core::ptr::copy_nonoverlapping(tramp_src, TRAMPOLINE_ADDR as *mut u8, tramp_len);

    bump_generation();

    // Прыгаем на трамплин. Дальше старое ядро можно затирать.
    let _tramp: extern "C" fn(u64, u64, u64, u64, u64) -> ! =
        core::mem::transmute(TRAMPOLINE_ADDR);
    core::arch::asm!(
        "jmp {t}",
        t = in(reg) TRAMPOLINE_ADDR,
        in("rdi") KERNEL_LOAD_ADDR,       // dst
        in("rsi") image.as_ptr() as u64,  // src
        in("rcx") image.len() as u64,     // len
        in("r8") KERNEL_LOAD_ADDR,        // entry
        in("r9") TRAMPOLINE_STACK,        // stack
        options(noreturn)
    );
    #[allow(unreachable_code)]
    {
        let _ = _tramp;
        loop {}
    }
}

// ==================== CLI ====================

/// `kexec [a|b|check]` — запуск ядра из раздела слота.
pub fn cmd_kexec(arg: &str) {
    let a = arg.trim();
    let (slot, only_check) = match a {
        "" | "check" => (None, a == "check"),
        "a" => (Some(0u8), false),
        "b" => (Some(1u8), false),
        _ => {
            crate::println!("kexec [a|b|check] — запустить ядро из раздела /kernel_a|b");
            crate::println!("  без аргумента — активный слот по BCB");
            crate::println!("  check         — только проверить образ, не запускать");
            return;
        }
    };

    let name = match slot {
        None => crate::partition_map::active_kernel_layout().name,
        Some(0) => "/kernel_a",
        Some(_) => "/kernel_b",
    };
    crate::println!("  [kexec] читаю ядро из {}...", name);

    let image = match load_kernel_image(slot) {
        Ok(i) => i,
        Err(e) => {
            crate::println!("  [kexec] ОШИБКА: {}", e.message());
            return;
        }
    };

    crate::println!(
        "  [kexec] образ проверен: {} байт, точка входа {:#x}",
        image.len(),
        KERNEL_LOAD_ADDR
    );

    if only_check {
        crate::println!("  [kexec] режим check — запуск не выполняется");
        return;
    }

    crate::println!(
        "  [kexec] ПЕРЕЗАПУСК ЯДРА (поколение {} -> {})",
        boot_generation(),
        boot_generation() + 1
    );
    crate::println!("  [kexec] управление уходит новому ядру, старое перестаёт существовать");

    unsafe { kexec(&image) }
}
