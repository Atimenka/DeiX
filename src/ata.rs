//! Драйвер ATA (IDE) в режиме PIO (Programmed I/O) — простейший, но
//! настоящий рабочий способ читать/писать сектора диска без DMA/прерываний.
//! Работает через классические порты primary ATA-контроллера (0x1F0-0x1F7,
//! 0x3F6), которые эмулирует QEMU для любого `-drive` по умолчанию (если не
//! указан `if=virtio`/`if=scsi` явно — наш собственный deix_disk.img как
//! раз подключается через `format=raw,file=...`, что даёт классический
//! IDE/ATA интерфейс).
//!
//! Наша ОС физически загружается с этого же диска (см. boot/boot_sector.asm
//! — тот самый диск, что определяется тут как "Primary Master"), поэтому
//! читаем сектора уже ПОСЛЕ данных загрузчика/ядра, отступив достаточно
//! места — см. fat16.rs, где FAT16 том создаётся начиная с фиксированного
//! LBA с большим запасом.
//!
//! Второй физический диск (Primary Slave) поддерживается отдельно (см.
//! Drive::Slave и install.rs) — это тот же ATA-контроллер (те же порты
//! 0x1F0-0x1F7), выбор master/slave делается битом 4 регистра
//! DRIVE_HEAD (0xE0=master, 0xF0=slave), как того требует спецификация
//! ATA. Нужен для команды `install` — установки DeiX на "второй HDD"
//! компьютера (в QEMU это второй флаг `-drive`), не трогая загрузочный
//! Live-диск.

use crate::port::{inb, insw, outb, outsw};

const DATA_PORT: u16 = 0x1F0;
const ERROR_PORT: u16 = 0x1F1;
const SECTOR_COUNT_PORT: u16 = 0x1F2;
const LBA_LOW_PORT: u16 = 0x1F3;
const LBA_MID_PORT: u16 = 0x1F4;
const LBA_HIGH_PORT: u16 = 0x1F5;
const DRIVE_HEAD_PORT: u16 = 0x1F6;
const COMMAND_PORT: u16 = 0x1F7;
const STATUS_PORT: u16 = 0x1F7;
const CONTROL_PORT: u16 = 0x3F6;

const STATUS_ERR: u8 = 0x01;
const STATUS_DRQ: u8 = 0x08;
const STATUS_BSY: u8 = 0x80;

const CMD_READ_SECTORS: u8 = 0x20;
const CMD_WRITE_SECTORS: u8 = 0x30;
const CMD_CACHE_FLUSH: u8 = 0xE7;
const CMD_IDENTIFY: u8 = 0xEC;

pub const SECTOR_SIZE: usize = 512;

/// Какой из двух дисков на первичном ATA-контроллере адресуем.
/// См. модульную документацию выше — оба используют одни и те же порты
/// ввода-вывода, различается только бит в DRIVE_HEAD.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Drive {
    /// "Primary Master" — тот самый диск, с которого DeiX загрузилась.
    Master,
    /// "Primary Slave" — второй физический диск (в QEMU: второй `-drive`).
    /// Используется командой `install` как место назначения установки.
    Slave,
}

impl Drive {
    fn select_byte(self, lba: u32) -> u8 {
        let base = match self {
            Drive::Master => 0xE0,
            Drive::Slave => 0xF0,
        };
        base | ((lba >> 24) & 0x0F) as u8
    }
}

fn wait_bsy_clear() -> bool {
    for _ in 0..1_000_000u32 {
        let status = unsafe { inb(STATUS_PORT) };
        if status & STATUS_BSY == 0 {
            return true;
        }
    }
    false
}

fn wait_drq_or_err() -> Result<(), ()> {
    for _ in 0..1_000_000u32 {
        let status = unsafe { inb(STATUS_PORT) };
        if status & STATUS_ERR != 0 {
            return Err(());
        }
        if status & STATUS_DRQ != 0 {
            return Ok(());
        }
    }
    Err(())
}

/// Читает `count` секторов начиная с `lba` (LBA28, максимум 256 секторов
/// за один вызов — ограничение регистра sector count) в `buffer`
/// (buffer.len() должен быть >= count * SECTOR_SIZE). Читает с диска
/// Master (см. read_sectors_from для чтения с произвольного диска).
pub fn read_sectors(lba: u32, count: u8, buffer: &mut [u8]) -> Result<(), ()> {
    read_sectors_from(Drive::Master, lba, count, buffer)
}

/// То же самое, но с явным выбором диска (Master/Slave) — нужно команде
/// `install` для чтения с загрузочного диска и записи на целевой.
pub fn read_sectors_from(drive: Drive, lba: u32, count: u8, buffer: &mut [u8]) -> Result<(), ()> {
    if buffer.len() < count as usize * SECTOR_SIZE {
        return Err(());
    }

    if !wait_bsy_clear() {
        return Err(());
    }

    unsafe {
        outb(CONTROL_PORT, 0x00); // включаем прерывания контроллера (nIEN=0), нам они не нужны, но не мешают
        outb(DRIVE_HEAD_PORT, drive.select_byte(lba));
        outb(ERROR_PORT, 0x00);
        outb(SECTOR_COUNT_PORT, count);
        outb(LBA_LOW_PORT, (lba & 0xFF) as u8);
        outb(LBA_MID_PORT, ((lba >> 8) & 0xFF) as u8);
        outb(LBA_HIGH_PORT, ((lba >> 16) & 0xFF) as u8);
        outb(COMMAND_PORT, CMD_READ_SECTORS);
    }

    let sectors = if count == 0 { 256 } else { count as usize };
    for i in 0..sectors {
        wait_drq_or_err()?;
        let offset = i * SECTOR_SIZE;
        unsafe {
            insw(DATA_PORT, &mut buffer[offset..offset + SECTOR_SIZE]);
        }
    }

    Ok(())
}

/// Пишет `count` секторов из `data` начиная с `lba` на диск Master.
pub fn write_sectors(lba: u32, count: u8, data: &[u8]) -> Result<(), ()> {
    write_sectors_to(Drive::Master, lba, count, data)
}

/// То же самое, но с явным выбором диска — см. read_sectors_from.
pub fn write_sectors_to(drive: Drive, lba: u32, count: u8, data: &[u8]) -> Result<(), ()> {
    if data.len() < count as usize * SECTOR_SIZE {
        return Err(());
    }

    if !wait_bsy_clear() {
        return Err(());
    }

    unsafe {
        outb(DRIVE_HEAD_PORT, drive.select_byte(lba));
        outb(ERROR_PORT, 0x00);
        outb(SECTOR_COUNT_PORT, count);
        outb(LBA_LOW_PORT, (lba & 0xFF) as u8);
        outb(LBA_MID_PORT, ((lba >> 8) & 0xFF) as u8);
        outb(LBA_HIGH_PORT, ((lba >> 16) & 0xFF) as u8);
        outb(COMMAND_PORT, CMD_WRITE_SECTORS);
    }

    let sectors = if count == 0 { 256 } else { count as usize };
    for i in 0..sectors {
        wait_drq_or_err()?;
        let offset = i * SECTOR_SIZE;
        unsafe {
            outsw(DATA_PORT, &data[offset..offset + SECTOR_SIZE]);
        }
    }

    unsafe {
        outb(COMMAND_PORT, CMD_CACHE_FLUSH);
    }
    wait_bsy_clear();

    Ok(())
}

/// Простая проверка "диск отвечает вообще хоть как-то" — читаем сектор 0
/// (наш собственный boot-сектор) и убеждаемся, что операция не упала.
pub fn is_present() -> bool {
    let mut buf = [0u8; SECTOR_SIZE];
    read_sectors(0, 1, &mut buf).is_ok()
}

/// Проверяет присутствие диска Slave через команду IDENTIFY DEVICE (0xEC)
/// — в отличие от read_sectors (который может "зависнуть" в ожидании
/// DRQ, если диска физически нет), IDENTIFY на несуществующем диске
/// сразу возвращает статус 0x00 (все биты статуса нулевые), что легко
/// проверить без риска зависнуть в цикле ожидания.
pub fn is_slave_present() -> bool {
    unsafe {
        outb(DRIVE_HEAD_PORT, 0xF0); // slave, LBA mode, lba=0
        outb(SECTOR_COUNT_PORT, 0);
        outb(LBA_LOW_PORT, 0);
        outb(LBA_MID_PORT, 0);
        outb(LBA_HIGH_PORT, 0);
        outb(COMMAND_PORT, CMD_IDENTIFY);
    }

    let status = unsafe { inb(STATUS_PORT) };
    if status == 0x00 {
        // Диска нет вообще — контроллер не отвечает.
        return false;
    }

    if !wait_bsy_clear() {
        return false;
    }

    // Если LBA_MID/LBA_HIGH не нулевые после IDENTIFY — это не ATA-диск
    // (скорее всего ATAPI, например CD-ROM), для наших целей установки
    // такое не подходит.
    let lba_mid = unsafe { inb(LBA_MID_PORT) };
    let lba_high = unsafe { inb(LBA_HIGH_PORT) };
    if lba_mid != 0 || lba_high != 0 {
        return false;
    }

    wait_drq_or_err().is_ok()
}

/// Возвращает общее число адресуемых секторов диска Slave, читая слова
/// 60-61 структуры IDENTIFY DEVICE (стандартное поле "Total number of
/// user addressable sectors" для LBA28-дисков). Нужно команде `install`,
/// чтобы не записать больше данных, чем реально помещается на целевой
/// диск.
pub fn slave_sector_count() -> Option<u32> {
    if !is_slave_present() {
        return None;
    }
    // is_slave_present() уже отправил IDENTIFY и дождался DRQ — данные
    // готовы к чтению, остаётся вычитать все 256 слов (512 байт) буфера.
    let mut buf = [0u8; SECTOR_SIZE];
    unsafe {
        insw(DATA_PORT, &mut buf);
    }
    let sectors = u32::from_le_bytes([buf[120], buf[121], buf[122], buf[123]]);
    if sectors == 0 {
        None
    } else {
        Some(sectors)
    }
}

