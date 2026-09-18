// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// ota — СИСТЕМА OTA-ОБНОВЛЕНИЙ DeiX OS (Over-The-Air).
//
// Пакет OTA (можно «послать» — смоделировать вводом команды):
//   ota apply <base64/payload>   — применить пакет
//   ota status                   — состояние гарантии и последнего обновления
//   ota generate <file>          — сформировать пакет из файла (для теста)
//
// Формат пакета:
//   MAGIC(8) "DEIXOTA1" | version(u32) | payload_len(u32) |
//   payload_sha256(32) | signature(32) | payload(...)
// Подпись в модели — SHA-256(payload || SECRET_KEY); при установке подпись
// пересчитывается и сверяется. Если OTA-гарантия снята (dev-режим) — пакет
// всё равно применяется (это ручное обновление), но статус гарантии остаётся
// «не действует».
//
// Применение OTA:
//   1) проверка подписи;
//   2) запись нового образа ядра/системы в /system (через ext2) или в
//      область ядра (через ata);
//   3) пересчёт vbmeta (avb) — новые системные файлы запечатываются;
//   4) перезагрузка.
// no_std-совместимо: alloc (String, Vec), вывод — crate::println!.


use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;

/// Магическая сигнатура OTA-пакета.
pub const OTA_MAGIC: &[u8; 8] = b"DEIXOTA1";

/// Секретный ключ подписи OTA (в модели — константа; на устройстве лежал
/// бы в TPM и был бы доступен только OTA-серверу DeiX).
const OTA_SECRET: &[u8] = b"deix_ota_secret_key_v1";

/// Версия текущей прошивки (для проверки «свежести» пакета).
pub const FIRMWARE_VERSION: u32 = 5;

/// Ошибка OTA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtaError {
    BadMagic,
    BadSignature,
    PayloadTooLarge,
    WriteFailed,
    VersionDowngrade { current: u32, pkg: u32 },
}

impl OtaError {
    pub fn message(&self) -> String {
        match self {
            OtaError::BadMagic => "некорректная магия OTA-пакета".to_string(),
            OtaError::BadSignature => "неверная подпись OTA-пакета (доверие не подтверждено)".to_string(),
            OtaError::PayloadTooLarge => "payload превышает допустимый размер".to_string(),
            OtaError::WriteFailed => "не удалось записать обновление на диск".to_string(),
            OtaError::VersionDowngrade { current, pkg } => format!(
                "попытка отката: текущая версия {} >= версии пакета {}",
                current, pkg
            ),
        }
    }
}

/// Разобранный OTA-пакет.
#[derive(Debug, Clone)]
pub struct OtaPackage {
    pub version: u32,
    pub payload: Vec<u8>,
    pub payload_sha256: [u8; 32],
    pub signature: [u8; 32],
}

/// Вычисление подписи: SHA-256(payload || secret).
fn compute_signature(payload: &[u8]) -> [u8; 32] {
    let mut blob: Vec<u8> = Vec::with_capacity(payload.len() + OTA_SECRET.len());
    blob.extend_from_slice(payload);
    blob.extend_from_slice(OTA_SECRET);
    crate::crypto::sha256::sha256(&blob)
}

impl OtaPackage {
    /// Собирает OTA-пакет из payload (для теста/отправки).
    pub fn build(version: u32, payload: Vec<u8>) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(OTA_MAGIC);
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        let sha = crate::crypto::sha256::sha256(&payload);
        out.extend_from_slice(&sha);
        let sig = compute_signature(&payload);
        out.extend_from_slice(&sig);
        out.extend_from_slice(&payload);
        out
    }

    /// Разбирает байтовый пакет и проверяет подпись.
    pub fn parse(bytes: &[u8]) -> Result<OtaPackage, OtaError> {
        let header_len = 8 + 4 + 4 + 32 + 32;
        if bytes.len() < header_len {
            return Err(OtaError::BadMagic);
        }
        if &bytes[..8] != OTA_MAGIC {
            return Err(OtaError::BadMagic);
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let payload_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let mut payload_sha256 = [0u8; 32];
        payload_sha256.copy_from_slice(&bytes[16..48]);
        let mut signature = [0u8; 32];
        signature.copy_from_slice(&bytes[48..80]);
        if bytes.len() < header_len + payload_len {
            return Err(OtaError::BadMagic);
        }
        let payload = bytes[80..80 + payload_len].to_vec();

        // Проверка подписи.
        let expected = compute_signature(&payload);
        if expected != signature {
            return Err(OtaError::BadSignature);
        }
        // Проверка SHA-256 payload.
        let sha = crate::crypto::sha256::sha256(&payload);
        if sha != payload_sha256 {
            return Err(OtaError::BadSignature);
        }

        Ok(OtaPackage {
            version,
            payload,
            payload_sha256,
            signature,
        })
    }

    /// Верификация перед установкой (без записи): подпись уже проверена в
    /// parse; здесь — проверка версии и гарантии.
    pub fn verify(&self) -> Result<(), OtaError> {
        if self.version < FIRMWARE_VERSION {
            return Err(OtaError::VersionDowngrade {
                current: FIRMWARE_VERSION,
                pkg: self.version,
            });
        }
        Ok(())
    }
}

/// ПРИМЕНЕНИЕ OTA-пакета.
///
/// Этапы:
///   1. parse + verify (подпись, версия);
///   2. запись payload как нового образа системы (в модели — в /system-том
///      как файл SYSTEM.IMG, а также обновление ядра в области ядра);
///   3. пересчёт vbmeta (avb) по новым системным файлам;
///   4. перезагрузка.
/// Возвращает Ok(имя записанного файла) или Err.
pub fn apply_ota(avb: &mut crate::avb::VerifiedBoot, bytes: &[u8]) -> Result<String, OtaError> {
    let pkg = OtaPackage::parse(bytes)?;
    pkg.verify()?;

    crate::println!("  [ota] Применение OTA v{} ({} байт payload)...", pkg.version, pkg.payload.len());

    // 1) Запись нового образа системы в /system-том.
    match crate::ext2::write_file("SYSTEM.IMG", &pkg.payload) {
        Ok(()) => {}
        Err(_) => {
            // Диск не отформатирован — пробуем записать в область ядра
            // напрямую (модель прошивки).
            crate::println!("  [ota] (ext2 недоступен) запись напрямую в область ядра...");
            let _ = write_kernel_area(&pkg.payload);
        }
    }

    // 2) Пересчёт vbmeta: новый SYSTEM.IMG запечатывается.
    let sys_files = [("SYSTEM.IMG", pkg.payload.as_slice())];
    let vbmeta = crate::avb::VbMeta::build(&sys_files);
    let state = avb.install_vbmeta(vbmeta, &sys_files);
    crate::println!("  [ota] vbmeta пересчитан, boot state: {}", state.as_str());

    // 3) Статус гарантии.
    if avb.ota_guarantee {
        crate::println!("  [ota] OTA-гарантия действует: обновление официальное.");
    } else {
        crate::println!("  [ota] OTA-гарантия НЕ действует (dev-режим): обновление неофициальное.");
    }

    crate::println!("  [ota] Готово. Перезагрузка: 'reboot'");
    Ok("SYSTEM.IMG".to_string())
}

/// Прямая запись payload в область ядра (секторы 1..N) — модель прошивки.
fn write_kernel_area(payload: &[u8]) -> Result<(), OtaError> {
    let sectors = (payload.len() + 511) / 512;
    let max = 1000; // не задеваем ext2-том (с 4096)
    if sectors > max {
        return Err(OtaError::PayloadTooLarge);
    }
    for i in 0..sectors {
        let mut buf = [0u8; 512];
        let start = i * 512;
        let end = (start + 512).min(payload.len());
        buf[..end - start].copy_from_slice(&payload[start..end]);
        if crate::ata::write_sectors(1 + i as u32, 1, &buf).is_err() {
            return Err(OtaError::WriteFailed);
        }
    }
    Ok(())
}

/// Статус OTA.
pub fn ota_status(avb: &crate::avb::VerifiedBoot) {
    crate::println!("=== OTA статус ===");
    crate::println!("  текущая прошивка: v{}", FIRMWARE_VERSION);
    crate::println!("  boot state: {}", avb.boot_state.as_str());
    crate::println!("  OTA-гарантия: {}", avb.ota_guarantee);
    if !avb.ota_guarantee {
        crate::println!("  (dev-режим: обновляйтесь вручную через рекавери/прошивальщик)");
    }
}

// CLI: `ota apply <data>` / `ota status` / `ota gen <payload>`.
// ==================== OTA ПО ВОЗДУХУ + A/B СЛОТЫ ====================

// Внешний символ конца образа ядра (linker_kernel.ld) — для сборки
// нового kernel.tar.gz из памяти.
extern "C" {
    static __image_end: u8;
}

/// Читает текущий kernel.bin из памяти (0x100000..__image_end).
fn current_kernel_bytes() -> Vec<u8> {
    let end = unsafe { &__image_end as *const u8 as usize };
    let base = 0x100000usize;
    let size = end.saturating_sub(base);
    let mut out = Vec::with_capacity(size);
    for i in 0..size {
        out.push(unsafe { core::ptr::read_volatile((base + i) as *const u8) });
    }
    out
}

/// Собирает НОВЫЙ kernel.tar.gz (stored-gzip + tar ustar) из текущего ядра
/// и библиотек — «скачанный по воздуху» образ обновления.
pub fn build_fresh_kernel_targz() -> Vec<u8> {
    let kernel = current_kernel_bytes();
    let tar = crate::kernel_loader::build_tar_archive(&[
        ("kernel.bin", &kernel),
        ("libdeix_core.so", b"DEIXLIB1\x00core\x00"),
        ("libdeix_net.so", b"DEIXLIB1\x00net\x00"),
        ("libdeix_gfx.so", b"DEIXLIB1\x00gfx\x00"),
    ]);
    build_gzip_stored(&tar)
}

/// Собирает gzip-контейнер со STORED deflate-блоком (без сжатия) —
/// наш inflate распакует его, а TarGzArchive::parse валидирует.
pub fn build_gzip_stored(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // gzip-заголовок (RFC 1952): magic, method=8, flags=0, mtime=0, xfl=0, os=3.
    out.extend_from_slice(&[0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 3]);
    // deflate: финальный stored-блок: 01 (bfinal=1, btype=00) + len + ~len + data.
    out.push(0x01);
    let len = (payload.len() & 0xFFFF) as u16;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&(!len).to_le_bytes());
    out.extend_from_slice(payload);
    // хвост (CRC32 + ISIZE) — наш декодер не проверяет, пишем нули.
    out.extend_from_slice(&[0u8; 8]);
    out
}

/// Собирает НАСТОЯЩИЙ EROFS-образ раздела с одним файлом.
///
/// Использует `crate::erofs` — формат по спецификации EROFS v1
/// (магия 0xE0F5E1E2). Такой образ читается `dump.erofs`/`fsck.erofs`
/// и монтируется ядром Linux, в отличие от прежней самодельной
/// «таблицы файлов» с выдуманной сигнатурой.
pub fn build_erofs_with_file(_label: &str, name: &str, data: &[u8], sectors: u32) -> Vec<u8> {
    let mut img = crate::erofs::build_image(&[(name, data)]);
    let want = (sectors as usize) * 512;
    if img.len() < want {
        img.resize(want, 0);
    }
    img.truncate(want);
    img
}

/// Прошивает kernel.tar.gz в НЕактивный слот ядра (A/B).
pub fn flash_kernel_to_inactive_slot() -> Result<String, ()> {
    let target = crate::partition_map::inactive_kernel_layout();
    let targz = build_fresh_kernel_targz();
    let img = build_erofs_with_file("kernel", "kernel.tar.gz", &targz, target.sectors);
    // Запись по секторам.
    let mut buf = [0u8; 512];
    for i in 0..target.sectors {
        let start = (i as usize) * 512;
        buf.copy_from_slice(&img[start..start + 512]);
        if crate::ata::write_sectors(target.start_lba + i, 1, &buf).is_err() {
            return Err(());
        }
    }
    Ok(format!(
        "  [ota] Новое ядро записано в {} ({} байт tar.gz)",
        target.name,
        targz.len()
    ))
}

/// ВСТРОЕННЫЙ АДРЕС OTA-СЕРВЕРА: система сама знает, откуда качать.
/// (в QEMU user-сетка: 10.0.2.2 = хост; порт 8080 — авто-сервер deix-ota)
pub const OTA_SERVER_URL: &str = "http://10.0.2.2:8080/TEST.OTA";

/// Скачивает OTA-пакет по HTTP с сервера (адрес встроенный).
pub fn fetch_ota(url: &str) -> Result<Vec<u8>, String> {
    if !crate::rtl8139::is_ready() {
        return Err("сеть недоступна (нет RTL8139)".into());
    }
    crate::println!("  [ota] Скачивание с сервера: {}", url);
    let body = crate::net::http::get_binary(url, 2 * 1024 * 1024)?;
    if body.len() < 32 {
        return Err(format!("слишком маленький ответ: {} байт", body.len()));
    }
    crate::println!("  [ota] Получено {} байт (сетевая доставка).", body.len());
    Ok(body)
}

/// Отмечает OTA как «ожидающее» в BCB (offset 16..20 = 1).
pub fn mark_ota_pending() {
    let mut sector = [0u8; 512];
    if crate::ata::read_sectors(crate::bcb::BCB_LBA, 1, &mut sector).is_ok() {
        if &sector[..8] != b"DEIXBCB1" {
            sector[..8].copy_from_slice(b"DEIXBCB1");
            sector[8..12].copy_from_slice(&0u32.to_le_bytes());
        }
        sector[16..20].copy_from_slice(&1u32.to_le_bytes());
        let _ = crate::ata::write_sectors(crate::bcb::BCB_LBA, 1, &sector);
    }
}

/// Сбрасывает флаг «OTA ожидает установки».
pub fn clear_ota_pending() {
    let mut sector = [0u8; 512];
    if crate::ata::read_sectors(crate::bcb::BCB_LBA, 1, &mut sector).is_ok() {
        if &sector[..8] == b"DEIXBCB1" {
            sector[16..20].copy_from_slice(&0u32.to_le_bytes());
            let _ = crate::ata::write_sectors(crate::bcb::BCB_LBA, 1, &sector);
        }
    }
}

/// true, если OTA-пакет ожидает установки (флаг в BCB).
pub fn ota_pending() -> bool {
    let mut sector = [0u8; 512];
    if crate::ata::read_sectors(crate::bcb::BCB_LBA, 1, &mut sector).is_err() {
        return false;
    }
    if &sector[..8] != b"DEIXBCB1" {
        return false;
    }
    u32::from_le_bytes([sector[16], sector[17], sector[18], sector[19]]) != 0
}


/// Полный цикл «уведомление → решение пользователя → авто-apply».
/// Вызывается при загрузке (если флаг pending в BCB) И сразу после
/// скачивания (`ota check`/`ota fetch`): система сама знает, откуда качать,
/// сама ставит флаг, а здесь ждёт 5 минут (Enter — применить, Esc/n —
/// отклонить) и применяет обновление автоматически со звуком.
pub fn ota_pending_flow() {
    crate::sound::ota_alert();
    crate::println!("  [ota] ⚠ Получено OTA-обновление (сервер активен).");
    crate::println!("  [ota]   - 'ota apply' — установить сейчас;");
    crate::println!("  [ota]   - 'ota rollback' — отклонить и вернуть слот;");
    crate::println!("  [ota]   - без ответа обновление применится автоматически через 5 минут.");
    crate::serial_println!("[ota] PENDING: сервер активен, ожидаем решение пользователя (5 мин)");

    let start = crate::timer::uptime_ms();
    let mut applied = false;
    while crate::timer::uptime_ms().saturating_sub(start) < 5 * 60 * 1000 {
        // Неблокирующий опрос клавиатуры/COM1: Enter = apply, Esc = отклонить.
        let key: Option<u8> = if crate::serial::is_data_ready() {
            Some(crate::serial::read_byte())
        } else {
            crate::keyboard::try_read_char()
        };
        match key {
            Some(b'\n') | Some(b'\r') => {
                crate::println!("  [ota] Пользователь подтвердил установку.");
                applied = true;
                break;
            }
            Some(b'\x1b') | Some(b'n') | Some(b'N') => {
                crate::println!("  [ota] Пользователь отклонил обновление (слот не меняется).");
                clear_ota_pending();
                applied = false;
                break;
            }
            _ => {}
        }
        unsafe { core::arch::asm!("nop"); }
    }
    // Применяем, если пользователь ПОДТВЕРДИЛ (Enter) или истёк таймаут 5 мин
    // (флаг всё ещё стоит). Если отклонено (Esc/n) — флаг сброшен и ничего
    // не делаем.
    if applied || ota_pending() {
        // Таймаут 5 минут: применяем автоматически, предупредив звуком.
        crate::sound::warn_triple();
        crate::println!("  [ota] ⏱ 5 минут истекло — применяю обновление автоматически.");
        // Читаем пакет из раздела /OTA.
        match crate::ota_store::load_file("TEST.OTA") {
            Ok(pkg) => {
                // pkg — kernel.tar.gz. 1) Распаковываем настоящее ядро.
                let kb = match extract_kernel_bin(&pkg) {
                    Ok(kb) => kb,
                    Err(e) => {
                        crate::println!("  [ota] ОШИБКА: пакет повреждён ({})", e);
                        return;
                    }
                };
                // 2) Пишем ядро в ЗАГРУЗОЧНЫЙ слот (LBA 2) — реально активирует.
                let secs = match flash_kernel_to_lba2(&kb) {
                    Ok(s) => s,
                    Err(()) => {
                        crate::println!("  [ota] ОШИБКА: не удалось записать ядро (LBA 2).");
                        return;
                    }
                };
                // 3) Прошиваем tar.gz в НЕактивный слот (bootchain/rollback).
                let target = crate::partition_map::inactive_kernel_layout();
                let img = crate::ota::build_erofs_with_file("kernel", "kernel.tar.gz", &pkg, target.sectors);
                let mut buf = [0u8; 512];
                let mut ok = true;
                for i in 0..target.sectors {
                    let st = (i as usize) * 512;
                    buf.copy_from_slice(&img[st..st + 512]);
                    if crate::ata::write_sectors(target.start_lba + i, 1, &buf).is_err() {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    switch_slot();
                    clear_ota_pending();
                    crate::println!("  [ota] Ядро установлено: LBA 2 ({} сект) + слот {}. Перезагрузка активирует новое ядро.", secs, crate::bcb::slot_name());
                    crate::sound::ota_alert();
                    crate::println!("  [ota] Перезагрузка...");
                    crate::cli::cmd_reboot();
                } else {
                    crate::println!("  [ota] ОШИБКА автоустановки (запись в слот).");
                }
            }
            Err(e) => crate::println!("  [ota] ОШИБКА: пакет в /OTA не найден ({})", e),
        }
    }
}


/// Распаковывает kernel.tar.gz в бинарник ядра (gzip -> tar -> kernel.bin).
/// Это то, что реально грузится с LBA 2 при следующей загрузке.
pub fn extract_kernel_bin(targz: &[u8]) -> Result<Vec<u8>, String> {
    let tar_bytes = crate::inflate::gunzip(targz, 16 * 1024 * 1024)
        .map_err(|e| format!("gunzip: {:?}", e))?;
    let tar = crate::kernel_loader::TarArchive::parse(tar_bytes)
        .map_err(|e| format!("tar: {:?}", e))?;
    tar.extract("kernel.bin").map_err(|e| format!("kernel.bin: {:?}", e))
}

/// Пишет бинарник ядра в ЗАГРУЗОЧНЫЙ слот диска (LBA 2) — реальный
/// загрузчик (stage2) читает ядро именно оттуда, поэтому после OTA
/// новое ядро активируется при перезагрузке.
pub fn flash_kernel_to_lba2(kernel_bin: &[u8]) -> Result<u32, ()> {
    const KERNEL_LBA: u32 = 2;
    let secs = ((kernel_bin.len() + 511) / 512).max(1) as u32;
    let mut buf = [0u8; 512];
    for i in 0..secs {
        let st = (i as usize) * 512;
        let n = (kernel_bin.len() - st).min(512);
        buf[..n].copy_from_slice(&kernel_bin[st..st + n]);
        if n < 512 { buf[n..].fill(0); }
        if crate::ata::write_sectors(KERNEL_LBA + i, 1, &buf).is_err() {
            return Err(());
        }
    }
    Ok(secs)
}

/// Переключает активный слот (A<->B).
pub fn switch_slot() {
    let cur = crate::bcb::read_slot();
    crate::bcb::write_slot(if cur == 0 { 1 } else { 0 });
}


pub fn cmd_ota(arg: &str, avb: &mut crate::avb::VerifiedBoot) {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    match parts.first() {
        Some(&"status") => ota_status(avb),
        Some(&"gen") => {
            // Сформировать тестовый OTA-пакет из строки payload.
            let payload = parts.get(1).cloned().unwrap_or("OTA payload").as_bytes().to_vec();
            let pkg = OtaPackage::build(FIRMWARE_VERSION + 1, payload);
            crate::println!("  [ota] Сформирован OTA-пакет ({} байт):", pkg.len());
            for b in pkg.iter().take(80) {
                crate::print!("{:02x}", b);
            }
            crate::println!("...");
        }
        Some(&"file") => {
            // Сформировать OTA-пакет и записать его файлом на /system-том
            // (для recovery Install / sideload). Payload — строка аргументов.
            let payload = parts.get(1).cloned().unwrap_or("deix-ota-payload").as_bytes().to_vec();
            let pkg = OtaPackage::build(FIRMWARE_VERSION + 1, payload);
            match crate::ext2::write_file("TEST.OTA", &pkg) {
                Ok(()) => crate::println!("  [ota] TEST.OTA записан на /system ({} байт, v{}) — готов к Install в recovery", pkg.len(), FIRMWARE_VERSION + 1),
                Err(_) => crate::println!("  [ota] не удалось записать TEST.OTA (том недоступен)"),
            }
        }
        Some(&"apply") => {
            // A/B OTA: прошиваем новое ядро в НЕактивный слот и переключаем.
            match flash_kernel_to_inactive_slot() {
                Ok(msg) => {
                    crate::println!("{}", msg);
                    let before = crate::bcb::slot_name();
                    switch_slot();
                    crate::println!("  [ota] Слот {} -> {}: перезагрузка активирует новое ядро (A/B).", before, crate::bcb::slot_name());
                    crate::println!("  [ota] Для отката: 'ota rollback' до перезагрузки.");
                }
                Err(_) => crate::println!("  [ota] ERROR: не удалось записать ядро в неактивный слот"),
            }
        }
        Some(&"check") => {
            crate::println!("  [ota] Проверка обновлений по воздуху...");
            crate::println!("  [ota] Текущая: v{}, сервер: {} — обновление {}",
                FIRMWARE_VERSION, OTA_SERVER_URL,
                if crate::ota_store::has_file("TEST.OTA") { "уже в /OTA (5 мин до авто-apply)" } else { "доступно, скачиваю..." });
            if !crate::ota_store::has_file("TEST.OTA") {
                match fetch_ota(OTA_SERVER_URL) {
                    Ok(body) => {
                        if crate::ota_store::store_file("TEST.OTA", &body).is_ok() {
                            crate::println!("  [ota] Скачано в /OTA ({} байт). Установка через 5 минут.", body.len());
                            mark_ota_pending();
                            // Сразу показываем уведомление и запускаем таймер (5 мин).
                            ota_pending_flow();
                        }
                    }
                    Err(e) => crate::println!("  [ota] Сервер недоступен: {}", e),
                }
            }
        }
        Some(&"download") => {
            // «По воздуху»: формируем новый kernel.tar.gz (образ ядра v+1)
            // и сохраняем как TEST.OTA на /system.
            let targz = build_fresh_kernel_targz();
            let pkg = OtaPackage::build(FIRMWARE_VERSION + 1, targz.clone());
            let _ = pkg;
            match crate::ext2::write_file("TEST.OTA", &targz) {
                Ok(()) => crate::println!("  [ota] OTA получен по воздуху: TEST.OTA ({} bytes, ядро v{})", targz.len(), FIRMWARE_VERSION + 1),
                Err(_) => crate::println!("  [ota] ERROR: не удалось сохранить TEST.OTA"),
            }
        }
        Some(&"rollback") => {
            let before = crate::bcb::slot_name();
            switch_slot();
            crate::println!("  [ota] Откат: слот {} -> {} (перезагрузка активирует)", before, crate::bcb::slot_name());
        }
        Some(&"fetch") => {
            let url = parts.get(1).cloned().unwrap_or("http://10.0.2.2:8080/TEST.OTA");
            match fetch_ota(&url) {
                Ok(body) => {
                    // body — kernel.tar.gz. Сохраняем в раздел /OTA.
                    let saved = crate::ota_store::store_file("TEST.OTA", &body).is_ok();
                    crate::println!("  [ota] Пакет получен и сохранён в /OTA: TEST.OTA ({} байт){}",
                        body.len(), if saved { "" } else { " (не удалось записать /OTA)" });
                    crate::println!("  [ota] Применение автоматически через 5 минут (или 'ota apply').");
                    mark_ota_pending();
                    // Сразу показываем уведомление и запускаем таймер (5 мин).
                    ota_pending_flow();
                }
                Err(e) => crate::println!("  [ota] ERROR: {}", e),
            }
        }
        _ => {
            crate::println!("ota <status|check|download|apply <payload>|rollback|gen <payload>|file <payload>> - OTA wireless update");
        }
    }
}
