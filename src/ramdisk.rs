//! RAM-ДИСК: дисковый образ, дочитанный загрузчиком в память.
//!
//! Зачем: при загрузке с USB/Ventoy ядро НЕ может читать разделы через
//! ATA PIO (порты 0x1F0) — это аппаратный контроллер внутреннего SATA,
//! а не флешки. Поэтому загрузчик (boot/ramboot.asm) до перехода в long
//! mode читает ВЕСЬ образ через BIOS int13 и кладёт его по адресу
//! RAMDISK_BASE (0x4000000 = 64 МиБ). Этот модуль даёт ядру доступ к
//! разделам как к памяти: если RAM-диск активен, все ata::read_sectors /
//! write_sectors обслуживаются из образа в RAM.
//!
//! RAM-диск активен, если по адресу RAMDISK_BASE лежит MBR с сигнатурой
//! 0xAA55 в конце первого сектора И аппаратный ATA-диск не является
//! тем же образом: ramboot (boot/ramboot.asm) и ISO-стаб
//! (boot/boot_sector_iso.asm) всегда кладут весь образ в 0x4000000, но
//! при загрузке с диска (QEMU IDE/SATA) ядро сравнивает LBA 0 и карту
//! загрузки (LBA 2047) ATA-диска с RAM-копией — при совпадении работает
//! через ATA (данные сохраняются на диске), иначе (USB/Ventoy или
//! чужой SATA-диск) — из памяти.
//!
//! Записи в RAM-диск живут только до перезагрузки (live-сессия) — это
//! нормально для загрузки с флешки: пользовательские данные можно не
//! сохранять между сессиями (как live CD).
//!
//! no_std: только core + crate::ata (для API-совместимости).



/// Физический адрес RAM-диска (совпадает с RAMDISK_DST в boot/ramboot.asm).
pub const RAMDISK_BASE: usize = 0x4000000;
/// Сколько секторов образа скопировано загрузчиком (весь .img, 34 МиБ).
pub const RAMDISK_SECTORS: u32 = 69632;
/// Размер RAM-диска в байтах.
pub const RAMDISK_SIZE: usize = RAMDISK_SECTORS as usize * 512;

/// Размер сектора (совпадает с ATA-сектором).
const SECTOR_SIZE: usize = 512;

/// Активен ли RAM-диск (устанавливается один раз при инициализации).
static ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Инициализирует RAM-диск: проверяет сигнатуру MBR (0xAA55) в первом
/// секторе образа. Вызывается рано в kernel_main, ДО любых обращений к
/// диску (bootchain, ext2).
///
/// Активация происходит не только по сигнатуре: если аппаратный ATA-диск
/// отвечает и его LBA 0 **точно совпадает** с RAM-копией — это загрузка с
/// диска (QEMU IDE/SATA), и тогда лучше работать через ATA: данные
/// сохраняются на диске, а не в оперативной памяти. Если ATA не отвечает
/// (USB/Ventoy — контроллера к флешке нет) или содержит другую MBR (ноутбук
/// со своим SATA-диском, загрузка с USB) — RAM-диск активируется, и все
/// разделы читаются из памяти.
pub fn init() -> bool {
    // Безопасно: прерывания ещё выключены или мы не работаем с памятью,
    // которая могла бы быть выдана аллокатором (резерв в mm/phys.rs).
    let mbr = unsafe { core::slice::from_raw_parts(RAMDISK_BASE as *const u8, 512) };
    let sig_ok = mbr[510] == 0x55 && mbr[511] == 0xAA;
    let ok = if sig_ok {
        // ATA-диск — тот же образ или нет? Сравниваем LBA 0 (MBR) и
        // LBA 2047 (карта загрузки — содержит SHA-256 ядра): если оба
        // сектора совпадают с RAM-копией, ATA-диск — это тот же образ,
        // и тогда лучше работать через ATA (данные сохраняются на диске).
        // Если ATA не отвечает (USB/Ventoy) или содержит другой образ
        // (ноутбук со своим SATA-диском — другая версия/OS) — RAM-диск.
        let mut ata_sec = [0u8; 512];
        let mut ata_same = crate::ata::read_sectors_from(crate::ata::Drive::Master, 0, 1, &mut ata_sec)
            .is_ok()
            && ata_sec[..] == mbr[..];
        let mut ata_map = [0u8; 512];
        if ata_same {
            let map_off = 2047usize * 512;
            let ram_map = unsafe {
                core::slice::from_raw_parts(
                    (RAMDISK_BASE as *const u8).wrapping_add(map_off),
                    512,
                )
            };
            ata_same = crate::ata::read_sectors_from(crate::ata::Drive::Master, 2047, 1, &mut ata_map)
                .is_ok()
                && ata_map[..] == ram_map[..];
        }
        !ata_same
    } else {
        false
    };
    ACTIVE.store(ok, core::sync::atomic::Ordering::SeqCst);
    if ok {
        crate::println!(
            "  [ramdisk] RAM-диск активен: {} секторов ({} КиБ) по 0x{:X} — разделы читаются из памяти (USB/Ventoy).",
            RAMDISK_SECTORS,
            RAMDISK_SIZE / 1024,
            RAMDISK_BASE
        );
        crate::serial_println!("[ramdisk] active: {} sectors at 0x{:X}", RAMDISK_SECTORS, RAMDISK_BASE);
    } else if sig_ok {
        crate::serial_println!("[ramdisk] образ в памяти, но ATA-диск отвечает — работаем через ATA");
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

