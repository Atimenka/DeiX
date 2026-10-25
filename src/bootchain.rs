//! bootchain — проверка системного образа после старта ядра.
//!
//! Само ядро к этому моменту уже исполняется: MBR (boot_sector) загрузил
//! stage2, stage2 — ramboot и сырой kernel.bin из служебной области диска
//! (LBA 1..=2046). Этот модуль НЕ загрузчик: он проверяет, что системный
//! раздел /system (EROFS) цел и что /system/kernel/kernel.bin совпадает
//! с реально загруженным сырым ядром (по размеру и SHA-256 из дескриптора
//! DEIXKIMG, записанного build.sh в сектор LBA 2047).
//!
//! Расхождение образов — DX-KRN-0013 (KERNEL_IMAGE_MISMATCH).
//!
//! ## Что делается до входа, а что после
//!
//! Сырое ядро читается и сверяется с дескриптором сразу: это то, что
//! реально исполняется. Сверка копии `/system/kernel/kernel.bin` с тем
//! же дескриптором — дорогая (чтение ~1 МиБ с EROFS и SHA-256) и ничего не
//! решает для самого запуска, поэтому она идёт порциями по 64 КиБ в
//! паузах экрана входа ([`pump`]) и всегда дожимается до конца сразу после
//! входа ([`finish`]), до чтения профиля и применения обновлений. Планировщик
//! к диску не привлекается: всё выполняется в потоке экрана входа, одна
//! порция за раз, поэтому обмен по ATA с другими обращениями не пересекается.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::spinlock::SpinLock;
use crate::vfs::SystemStream;

use crate::ata;
use crate::crypto::sha256::Sha256;
use crate::partition_map::{KERNEL_IMAGE_INFO_LBA, KERNEL_IMAGE_INFO_MAGIC};

/// Дескриптор образа ядра из сектора LBA 2047 (пишется build.sh).
struct KernelImageInfo {
    /// LBA первого сектора сырого kernel.bin.
    lba: u32,
    /// Точный размер kernel.bin в байтах.
    size: u32,
    /// SHA-256 файла kernel.bin, вычисленный при сборке.
    sha256: [u8; 32],
}

/// Читает и разбирает дескриптор DEIXKIMG. `Ok(None)` — дескриптора нет
/// (носитель собран старой версией build.sh).
fn read_kernel_image_info() -> Result<Option<KernelImageInfo>, String> {
    let mut sector = [0u8; 512];
    ata::read_sectors(KERNEL_IMAGE_INFO_LBA, 1, &mut sector)
        .map_err(|_| String::from("сектор дескриптора ядра не прочитан"))?;

    if sector[0..8] != KERNEL_IMAGE_INFO_MAGIC {
        return Ok(None);
    }

    let lba = u32::from_le_bytes([sector[8], sector[9], sector[10], sector[11]]);
    let size = u32::from_le_bytes([sector[12], sector[13], sector[14], sector[15]]);
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(&sector[16..48]);

    // Границы: сырое ядро лежит в служебной области до дескриптора.
    let sectors = size.div_ceil(512);
    if size == 0 || lba == 0 || lba + sectors > KERNEL_IMAGE_INFO_LBA {
        return Err(format!(
            "дескриптор ядра некорректен (LBA {}, {} байт)",
            lba, size
        ));
    }

    Ok(Some(KernelImageInfo { lba, size, sha256 }))
}

/// Читает сырой kernel.bin из служебной области по данным дескриптора.
fn read_raw_kernel(info: &KernelImageInfo) -> Result<Vec<u8>, String> {
    let sectors = info.size.div_ceil(512);
    let mut buf: Vec<u8> = vec![0u8; (sectors as usize) * 512];
    let mut cur = 0u32;
    let mut left = sectors;
    while left > 0 {
        let batch = left.min(128) as u8;
        let start = (cur as usize) * 512;
        let end = start + (batch as usize) * 512;
        ata::read_sectors(info.lba + cur, batch, &mut buf[start..end])
            .map_err(|_| String::from("сырое ядро не прочитано"))?;
        left -= batch as u32;
        cur += batch as u32;
    }
    buf.truncate(info.size as usize);
    Ok(buf)
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

/// Отложенная сверка `/system/kernel/kernel.bin` с дескриптором.
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
/// Сверка `/system/kernel/kernel.bin` с дескриптором здесь только
/// подготавливается; доводят её до конца [`pump`] и [`finish`].
pub fn run_boot_chain() -> Result<String, String> {
    let mut out = String::new();
    out.push_str("  [bootchain] Проверка системного образа (/system, EROFS):\n");

    match read_kernel_image_info() {
        Ok(Some(info)) => {
            // 1. Сырое ядро, которое реально загрузил stage2.
            let raw = read_raw_kernel(&info)?;
            crate::bootprof::mark("bootchain: raw kernel read");
            let raw_hash = crate::crypto::sha256::sha256(&raw);
            crate::bootprof::mark("bootchain: sha256 of raw kernel");
            drop(raw);

            if raw_hash != info.sha256 {
                let msg = format!(
                    "KERNEL_IMAGE_MISMATCH: сырое ядро (LBA {}) не совпадает с дескриптором сборки",
                    info.lba
                );
                report_mismatch(&msg);
                return Err(msg);
            }

            // 2. Копия в /system: порционная сверка на экране входа, если
            // /system — EROFS и файл открылся; иначе целиком прямо сейчас.
            match SystemStream::open(SYSTEM_KERNEL) {
                Some(stream) => {
                    let size = stream.size();
                    if size != info.size as usize {
                        let msg = format!(
                            "KERNEL_IMAGE_MISMATCH: {} ({} байт) не совпадает с загруженным ядром ({} байт)",
                            SYSTEM_KERNEL, size, info.size
                        );
                        report_mismatch(&msg);
                        return Err(msg);
                    }
                    *DEFERRED.lock() = Deferred::Pending(Pending {
                        stream,
                        hasher: Sha256::new(),
                        expect: info.sha256,
                        size,
                        done: 0,
                        raw_lba: info.lba,
                    });
                    out.push_str(&format!(
                        "    сырое ядро подтверждено: {} байт, sha256 {}…; сверка {} продолжится на экране входа\n",
                        info.size,
                        hex8(&info.sha256),
                        SYSTEM_KERNEL
                    ));
                }
                None => {
                    let erofs_kernel = read_system_kernel()?;
                    crate::bootprof::mark("bootchain: /system kernel read (whole)");
                    let erofs_hash = crate::crypto::sha256::sha256(&erofs_kernel);
                    if erofs_kernel.len() != info.size as usize || erofs_hash != info.sha256 {
                        let msg = mismatch_message(
                            erofs_kernel.len(),
                            &erofs_hash,
                            info.size as usize,
                            &info.sha256,
                        );
                        report_mismatch(&msg);
                        return Err(msg);
                    }
                    out.push_str(&format!(
                        "    образ ядра подтверждён: {} байт, sha256 {}… (raw LBA {} == /system)\n",
                        info.size,
                        hex8(&info.sha256),
                        info.lba
                    ));
                }
            }
        }
        Ok(None) => {
            let erofs_kernel = read_system_kernel()?;
            out.push_str(&format!("    {}: {} байт\n", SYSTEM_KERNEL, erofs_kernel.len()));
            out.push_str(
                "    дескриптор DEIXKIMG отсутствует — сверка raw/EROFS пропущена (старый образ)\n",
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
                    "образ ядра подтверждён: {} байт, sha256 {}… (raw LBA {} == {})",
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
