//! Драйвер USB Mass Storage (BOT/SCSI).

pub trait BlockDevice {
    fn read(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), ()>;
    fn write(&mut self, lba: u64, count: u32, data: &[u8]) -> Result<(), ()>;
    fn capacity(&self) -> u64;
}

pub struct UsbDisk {
    pub lba_count: u64,
}

impl BlockDevice for UsbDisk {
    fn read(&mut self, _lba: u64, _count: u32, _buf: &mut [u8]) -> Result<(), ()> {
        Ok(())
    }

    fn write(&mut self, _lba: u64, _count: u32, _data: &[u8]) -> Result<(), ()> {
        Ok(())
    }

    fn capacity(&self) -> u64 {
        self.lba_count
    }
}
