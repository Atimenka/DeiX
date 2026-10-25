//! bootchain — проверка системного образа после старта ядра.
//!
//! Само ядро к этому моменту уже исполняется: MBR (boot_sector) прочитал карту
//! загрузки (LBA 2047, src/bootmap.rs) и загрузил по ней stage2 и ramboot,
//! ramboot — ядро; все три лежат файлами в `/system` (`/system/boot/*`,
//! `/system/kernel/kernel.bin`). Этот модуль НЕ загрузчик: он проверяет, что
//! участки диска из карты содержат ядро, описанное картой (размер и SHA-256,
//! записанные при сборке), и что файл `/system/kernel/kernel.bin`, открытый по
//! имени через EROFS, совпадает с ним, то есть карта указывает на настоящий
//! файл.
//!
//! Расхождение образов — DX-KRN-0013 (KERNEL_IMAGE_MISMATCH).
//!
//! ## Что делается до входа, а что после
//!
//! Участки карты читаются и сверяются сразу: это то, что реально исполняется.
//! Сверка файла по имени — дорогая (чтение ~1 МиБ с EROFS и SHA-256) и ничего
//! не решает для самого запуска, поэтому она идёт порциями по 64 КиБ в паузах
//! экрана входа ([`pump`]) и всегда дожимается до конца сразу после входа
//! ([`finish`]), до чтения профиля и применения обновлений. Планировщик к
//! диску не привлекается: всё выполняется в потоке экрана входа, одна порция
//! за раз, поэтому обмен по ATA с другими обращениями не пересекается.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::spinlock::SpinLock;
use crate::vfs::SystemStream;

use crate::ata;
use crate::bootmap;
use crate::crypto::sha256::Sha256;

/// Читает ядро, как его загрузил ramboot: по участкам карты загрузки.
fn read_raw_kernel(info: &bootmap::BootMap) -> Result<Vec<u8>, String> {
    bootmap::read_extents(&info.kernel, info.kernel_size as usize, |lba, count, buf| {
        ata::read_sectors(lba, count, buf)
    })
}

fn hex8(digest: &[u8; 32]) -> String {
    let mut s = String::new();
    for b in digest.iter().take(4) {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

const SYSTEM_KERNEL: &str = "/system/kernel/kernel.bin";
/// Размер порции отложенной сверки. Не меньше порога прямого чтения EROFS.
const VERIFY_CHUNK: usize = 64 * 1024;

/// Отложенная сверка `/system/kernel/kernel.bin` с картой загрузки.
struct Pending {
    stream: SystemStream,
    hasher: Sha256,
    expect: [u8; 32],
    size: usize,
    done: usize,
    raw_lba: u32,
}

enum Deferred {
    Idle,
    Pending(Pending),
    /// Итог: строка отчёта либо (сообщение, нужно ли занести в diag как DX-KRN-0013).
    Finished(Result<String, (String, bool)>),
}

static DEFERRED: SpinLock<Deferred> = SpinLock::new(Deferred::Idle);

fn mismatch_message(size: usize, hash: &[u8; 32], info_size: usize, expect: &[u8; 32]) -> String {
    format!(
        "KERNEL_IMAGE_MISMATCH: {} ({} байт, sha256 {}…) не совпадает с загруженным ядром ({} байт, sha256 {}…)",
        SYSTEM_KERNEL,
        size,
        hex8(hash),
        info_size,
        hex8(expect)
    )
}

fn report_mismatch(msg: &str) {
    crate::diag::critical_with(
        crate::diag::ErrorCode::new(crate::diag::Subsystem::Kernel, 13),
        crate::diag::Action::DegradeSubsystem,
        msg,
    );
}

/// Читает `/system/kernel/kernel.bin` целиком (запасной путь: /system не
/// EROFS или файл не открылся потоком).
fn read_system_kernel() -> Result<Vec<u8>, String> {
    crate::vfs::read_file(SYSTEM_KERNEL).map_err(|e| format!("system: {}: {}", SYSTEM_KERNEL, e.message()))
}

/// Проверяет системный раздел и совпадение образов ядра.
/// Вызывается из kernel_main ПОСЛЕ инициализации ATA/heap, ДО экрана входа.
/// Сверка `/system/kernel/kernel.bin` с картой загрузки здесь только
/// подготавливается; доводят её до конца [`pump`] и [`finish`].
pub fn run_boot_chain() -> Result<String, String> {
    let mut out = String::new();
    out.push_str("  [bootchain] Проверка системного образа (/system, EROFS):\n");

    match bootmap::read_boot_map() {
        Ok(Some(info)) => {
            // 1. Ядро по участкам карты — то, что реально загрузил ramboot.
            let raw = read_raw_kernel(&info)?;
            crate::bootprof::mark("bootchain: raw kernel read");
            let raw_hash = crate::crypto::sha256::sha256(&raw);
            crate::bootprof::mark("bootchain: sha256 of raw kernel");
            drop(raw);

            if raw_hash != info.kernel_sha256 {
                let msg = format!(
                    "KERNEL_IMAGE_MISMATCH: ядро по карте загрузки (LBA {}) не совпадает с описанием сборки",
                    info.kernel[0].lba
                );
                report_mismatch(&msg);
                return Err(msg);
            }

            // 2. Файл /system/kernel/kernel.bin по имени: порционная сверка
            // на экране входа, если /system — EROFS и файл открылся; иначе
            // целиком прямо сейчас.
            match SystemStream::open(SYSTEM_KERNEL) {
                Some(stream) => {
                    let size = stream.size();
                    if size != info.kernel_size as usize {
                        let msg = format!(
                            "KERNEL_IMAGE_MISMATCH: {} ({} байт) не совпадает с загруженным ядром ({} байт)",
                            SYSTEM_KERNEL, size, info.kernel_size
                        );
                        report_mismatch(&msg);
                        return Err(msg);
                    }
                    *DEFERRED.lock() = Deferred::Pending(Pending {
                        stream,
                        hasher: Sha256::new(),
                        expect: info.kernel_sha256,
                        size,
                        done: 0,
                        raw_lba: info.kernel[0].lba,
                    });
                    out.push_str(&format!(
                        "    ядро по карте загрузки подтверждено: {} байт, sha256 {}…; сверка {} продолжится на экране входа\n",
                        info.kernel_size,
                        hex8(&info.kernel_sha256),
                        SYSTEM_KERNEL
                    ));
                }
                None => {
                    let erofs_kernel = read_system_kernel()?;
                    crate::bootprof::mark("bootchain: /system kernel read (whole)");
                    let erofs_hash = crate::crypto::sha256::sha256(&erofs_kernel);
                    if erofs_kernel.len() != info.kernel_size as usize || erofs_hash != info.kernel_sha256 {
                        let msg = mismatch_message(
                            erofs_kernel.len(),
                            &erofs_hash,
                            info.kernel_size as usize,
                            &info.kernel_sha256,
                        );
                        report_mismatch(&msg);
                        return Err(msg);
                    }
                    out.push_str(&format!(
                        "    образ ядра подтверждён: {} байт, sha256 {}… (LBA {} == /system)\n",
                        info.kernel_size,
                        hex8(&info.kernel_sha256),
                        info.kernel[0].lba
                    ));
                }
            }
        }
        Ok(None) => {
            let erofs_kernel = read_system_kernel()?;
            out.push_str(&format!("    {}: {} байт\n", SYSTEM_KERNEL, erofs_kernel.len()));
            out.push_str(
                "    карта загрузки DEIXKIMG отсутствует — сверка ядра пропущена (старый образ)\n",
            );
        }
        Err(e) => {
            let erofs_kernel = read_system_kernel()?;
            out.push_str(&format!("    {}: {} байт\n", SYSTEM_KERNEL, erofs_kernel.len()));
            crate::diag::error(
                crate::diag::ErrorCode::new(crate::diag::Subsystem::Kernel, 13),
                &e,
            );
            out.push_str(&format!("    сверка образов ядра не выполнена: {}\n", e));
        }
    }

    out.push_str("  [bootchain] Системный образ проверен.\n");
    Ok(out)
}

/// Выполняет одну порцию отложенной сверки. `true` — порция обработана
/// (вызывающему стоит сразу опросить ввод и позвать снова); `false` —
/// делать нечего (сверка не нужна или уже закончена).
pub fn pump() -> bool {
    let pending = {
        let mut guard = DEFERRED.lock();
        match core::mem::replace(&mut *guard, Deferred::Idle) {
            Deferred::Pending(p) => Some(p),
            other => {
                *guard = other;
                None
            }
        }
    };
    let mut p = match pending {
        Some(p) => p,
        None => return false,
    };

    // Блокировка отпущена: чтение диска ждёт IRQ и не должно держать лок.
    let next = match p.stream.next_chunk(VERIFY_CHUNK) {
        Ok(chunk) if !chunk.is_empty() => {
            p.hasher.update(&chunk);
            p.done += chunk.len();
            None
        }
        Ok(_) => Some(Err((
            format!("system: {}: файл короче заявленного ({} из {} байт)", SYSTEM_KERNEL, p.done, p.size),
            false,
        ))),
        Err(e) => Some(Err((format!("system: {}: {}", SYSTEM_KERNEL, e), false))),
    };

    let state = match next {
        Some(failed) => Deferred::Finished(failed),
        None if p.done >= p.size => {
            let hash = p.hasher.finalize();
            if hash == p.expect {
                Deferred::Finished(Ok(format!(
                    "образ ядра подтверждён: {} байт, sha256 {}… (LBA {} == {})",
                    p.size,
                    hex8(&p.expect),
                    p.raw_lba,
                    SYSTEM_KERNEL
                )))
            } else {
                Deferred::Finished(Err((mismatch_message(p.size, &hash, p.size, &p.expect), true)))
            }
        }
        None => Deferred::Pending(p),
    };
    *DEFERRED.lock() = state;
    true
}

/// Дожимает отложенную сверку и сообщает итог. Вызывается сразу после
/// входа. Если сверка не откладывалась, ничего не делает.
pub fn finish() {
    while pump() {}
    let verdict = {
        let mut guard = DEFERRED.lock();
        match core::mem::replace(&mut *guard, Deferred::Idle) {
            Deferred::Finished(v) => Some(v),
            other => {
                *guard = other;
                None
            }
        }
    };
    match verdict {
        Some(Ok(line)) => crate::println!("  [bootchain] {}", line),
        Some(Err((msg, critical))) => {
            if critical {
                report_mismatch(&msg);
            }
            crate::println!("  [bootchain] Предупреждение: {}", msg);
        }
        None => return,
    }
    crate::bootprof::mark("bootchain: /system kernel verified (deferred)");
}
