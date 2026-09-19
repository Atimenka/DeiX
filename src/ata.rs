//! Драйвер ATA (IDE) с поддержкой PIO и Bus Master IDE (DMA).

use crate::port::{inb, insw, outb, outsw, inl, outl};

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
const CMD_READ_DMA: u8 = 0xC8;
const CMD_CACHE_FLUSH: u8 = 0xE7;
const CMD_IDENTIFY: u8 = 0xEC;

pub const SECTOR_SIZE: usize = 512;

// Bus Master IDE BAR4 базовый порт (по умолчанию 0xC000 в QEMU)
static mut BMIDE_BASE: u16 = 0xC000;
static mut PRDT_BUFFER: [u32; 1024] = [0; 1024]; // 4 KiB PRDT буфер

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Drive {
    Master,
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

pub fn init_dma(bar4: u16) {
    if bar4 != 0 {
        unsafe {
            BMIDE_BASE = bar4;
        }
    }
}

/// Чтение секторов через Bus Master IDE DMA (fallback на PIO при недоступности)
pub fn read_sectors_dma(lba: u32, count: u8, buffer: &mut [u8]) -> Result<(), ()> {
    if crate::ramdisk::is_active() {
        return crate::ramdisk::read_sectors(lba, count, buffer);
    }

    let bmi_base = unsafe { BMIDE_BASE };
    let sector_cnt = if count == 0 { 256 } else { count as usize };
    let total_bytes = sector_cnt * SECTOR_SIZE;

    if buffer.len() < total_bytes {
        return Err(());
    }

    if !wait_bsy_clear() {
        return Err(());
    }

    unsafe {
        // Подготовка PRDT (Physical Region Descriptor)
        let phys_addr = buffer.as_ptr() as u32;
        PRDT_BUFFER[0] = phys_addr;
        PRDT_BUFFER[1] = (total_bytes as u32) | 0x80000000; // Bit 31: EOT (End of Table)

        // Адрес PRDT в регистр BMIDTPR
        let prdt_phys = PRDT_BUFFER.as_ptr() as u32;
        outl(bmi_base + 0x04, prdt_phys);

        // Установка направления передачи в BMICmd: Read (Bit 3 = 1)
        outb(bmi_base + 0x00, 0x08);

        // Очистка статуса в BMIStatus
        let stat = inb(bmi_base + 0x02);
        outb(bmi_base + 0x02, stat | 0x06);

        // Команда ATA контроллеру
        outb(CONTROL_PORT, 0x00);
        outb(DRIVE_HEAD_PORT, Drive::Master.select_byte(lba));
        outb(ERROR_PORT, 0x00);
        outb(SECTOR_COUNT_PORT, count);
        outb(LBA_LOW_PORT, (lba & 0xFF) as u8);
        outb(LBA_MID_PORT, ((lba >> 8) & 0xFF) as u8);
        outb(LBA_HIGH_PORT, ((lba >> 16) & 0xFF) as u8);
        outb(COMMAND_PORT, CMD_READ_DMA);

        // Запуск DMA (BMICmd Bit 0 = 1)
        outb(bmi_base + 0x00, 0x09);

        // Ожидание завершения передачи
        for _ in 0..1_000_000 {
            let bmicmd_stat = inb(bmi_base + 0x02);
            if bmicmd_stat & 0x01 == 0 { // Bus Master Active = 0
                // Остановка DMA
                outb(bmi_base + 0x00, 0x00);
                return Ok(());
            }
        }

        // Остановка DMA при таймауте и fallback на PIO
        outb(bmi_base + 0x00, 0x00);
    }

    read_sectors(lba, count, buffer)
}

pub fn read_sectors(lba: u32, count: u8, buffer: &mut [u8]) -> Result<(), ()> {
    if crate::ramdisk::is_active() {
        return crate::ramdisk::read_sectors(lba, count, buffer);
    }
    read_sectors_from(Drive::Master, lba, count, buffer)
}

pub fn read_sectors_from(drive: Drive, lba: u32, count: u8, buffer: &mut [u8]) -> Result<(), ()> {
    if buffer.len() < count as usize * SECTOR_SIZE {
        return Err(());
    }

    if !wait_bsy_clear() {
        return Err(());
    }

    unsafe {
        outb(CONTROL_PORT, 0x00);
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

pub fn write_sectors(lba: u32, count: u8, data: &[u8]) -> Result<(), ()> {
    if crate::ramdisk::is_active() {
        return crate::ramdisk::write_sectors(lba, count, data);
    }
    write_sectors_to(Drive::Master, lba, count, data)
}

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

pub fn is_present() -> bool {
    let mut buf = [0u8; SECTOR_SIZE];
    read_sectors(0, 1, &mut buf).is_ok()
}

pub fn is_slave_present() -> bool {
    unsafe {
        outb(DRIVE_HEAD_PORT, 0xF0);
        outb(SECTOR_COUNT_PORT, 0);
        outb(LBA_LOW_PORT, 0);
        outb(LBA_MID_PORT, 0);
        outb(LBA_HIGH_PORT, 0);
        outb(COMMAND_PORT, CMD_IDENTIFY);
    }

    let status = unsafe { inb(STATUS_PORT) };
    if status == 0x00 {
        return false;
    }

    if !wait_bsy_clear() {
        return false;
    }

    let lba_mid = unsafe { inb(LBA_MID_PORT) };
    let lba_high = unsafe { inb(LBA_HIGH_PORT) };
    if lba_mid != 0 || lba_high != 0 {
        return false;
    }

    wait_drq_or_err().is_ok()
}

pub fn slave_sector_count() -> Option<u32> {
    if !is_slave_present() {
        return None;
    }
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
