//! RAM-ДИСК: дисковый образ, дочитанный загрузчиком в память.
//!
//! Зачем: при загрузке с USB/Ventoy ядро НЕ может читать разделы через
//! ATA PIO (порты 0x1F0) — это аппаратный контроллер внутреннего SATA,
//! а не флешки. Поэтому загрузчик (boot/ramboot.asm) до перехода в long
//! mode читает ВЕСЬ образ через BIOS int13 и кладёт его по адресу
//! RAMDISK_BASE (0x2000000 = 32 МиБ). Этот модуль даёт ядру доступ к
//! разделам как к памяти: если RAM-диск активен, все ata::read_sectors /
//! write_sectors обслуживаются из образа в RAM.
//!
//! RAM-диск активен, если по адресу RAMDISK_BASE лежит MBR с сигнатурой
//! 0xAA55 в конце первого сектора. При обычной загрузке с SATA-диска
//! (без ramboot) там нули/мусор — флаг не ставится, и ядро работает с
//! диском как раньше (ATA PIO).
//!
//! Записи в RAM-диск живут только до перезагрузки (live-сессия) — это
//! нормально для загрузки с флешки: пользовательские данные можно не
//! сохранять между сессиями (как live CD).
//!
//! no_std: только core + crate::ata (для API-совместимости).



/// Физический адрес RAM-диска (совпадает с RAMDISK_DST в boot/ramboot.asm).
pub const RAMDISK_BASE: usize = 0x2000000;
/// Сколько секторов образа скопировано загрузчиком (весь .img, 10 МиБ).
pub const RAMDISK_SECTORS: u32 = 20480;
/// Размер RAM-диска в байтах.
pub const RAMDISK_SIZE: usize = RAMDISK_SECTORS as usize * 512;

/// Размер сектора (совпадает с ATA-сектором).
const SECTOR_SIZE: usize = 512;

/// Активен ли RAM-диск (устанавливается один раз при инициализации).
static ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Инициализирует RAM-диск: проверяет сигнатуру MBR (0xAA55) в первом
/// секторе образа. Вызывается рано в kernel_main, ДО любых обращений к
/// диску (bootchain, ext2, BCB, TPM, OTA).
pub fn init() -> bool {
    // Безопасно: прерывания ещё выключены или мы не работаем с памятью,
    // которая могла бы быть выдана аллокатором (резерв в mm/phys.rs).
    let mbr = unsafe { core::slice::from_raw_parts(RAMDISK_BASE as *const u8, 512) };
    let ok = mbr[510] == 0x55 && mbr[511] == 0xAA;
    ACTIVE.store(ok, core::sync::atomic::Ordering::SeqCst);
    if ok {
        crate::println!(
            "  [ramdisk] RAM-диск активен: {} секторов ({} КиБ) по 0x{:X} — разделы читаются из памяти (USB/Ventoy).",
            RAMDISK_SECTORS,
            RAMDISK_SIZE / 1024,
            RAMDISK_BASE
        );
        crate::serial_println!("[ramdisk] active: {} sectors at 0x{:X}", RAMDISK_SECTORS, RAMDISK_BASE);
    } else {
        crate::serial_println!("[ramdisk] inactive (обычный диск)");
    }
    ok
}

/// Активен ли RAM-диск.
pub fn is_active() -> bool {
    ACTIVE.load(core::sync::atomic::Ordering::SeqCst)
}

/// Читает один сектор из RAM-диска.
pub fn read_sector(lba: u32, buf: &mut [u8]) -> Result<(), ()> {
    if buf.len() < SECTOR_SIZE {
        return Err(());
    }
    if lba >= RAMDISK_SECTORS {
        return Err(());
    }
    let src = RAMDISK_BASE + lba as usize * SECTOR_SIZE;
    unsafe {
        core::ptr::copy_nonoverlapping(src as *const u8, buf.as_mut_ptr(), SECTOR_SIZE);
    }
    Ok(())
}

/// Читает `count` секторов из RAM-диска.
pub fn read_sectors(lba: u32, count: u8, buf: &mut [u8]) -> Result<(), ()> {
    if buf.len() < count as usize * SECTOR_SIZE {
        return Err(());
    }
    for i in 0..count as u32 {
        let off = (i as usize) * SECTOR_SIZE;
        read_sector(lba + i, &mut buf[off..off + SECTOR_SIZE])?;
    }
    Ok(())
}

/// Пишет один сектор в RAM-диск (live-сессия, до перезагрузки).
pub fn write_sector(lba: u32, data: &[u8]) -> Result<(), ()> {
    if data.len() < SECTOR_SIZE {
        return Err(());
    }
    if lba >= RAMDISK_SECTORS {
        return Err(());
    }
    let dst = RAMDISK_BASE + lba as usize * SECTOR_SIZE;
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), dst as *mut u8, SECTOR_SIZE);
    }
    Ok(())
}

/// Пишет `count` секторов в RAM-диск.
pub fn write_sectors(lba: u32, count: u8, data: &[u8]) -> Result<(), ()> {
    if data.len() < count as usize * SECTOR_SIZE {
        return Err(());
    }
    for i in 0..count as u32 {
        let off = (i as usize) * SECTOR_SIZE;
        write_sector(lba + i, &data[off..off + SECTOR_SIZE])?;
    }
    Ok(())
}

// Подстраховка от неиспользуемости ata-импорта (перехват делается в ata.rs,
// но ссылка здесь держит модуль связанным).
#[allow(unused_imports)]
use crate::ata as _ata_ref;
