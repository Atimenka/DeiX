//! Ядро USB 2.0: энумерация, управление дескрипторами и отправка пакетов.

use alloc::vec::Vec;
use crate::spinlock::SpinLock;

#[derive(Debug, Clone)]
pub struct UsbDevice {
    pub addr: u8,
    pub vid: u16,
    pub pid: u16,
    pub class_code: u8,
    pub max_packet_size: u8,
}

pub struct UsbCore {
    pub devices: Vec<UsbDevice>,
    pub next_addr: u8,
}

static CORE: SpinLock<UsbCore> = SpinLock::new(UsbCore {
    devices: Vec::new(),
    next_addr: 1,
});

pub fn register_device(vid: u16, pid: u16, class_code: u8) -> u8 {
    let mut core = CORE.lock();
    let addr = core.next_addr;
    core.next_addr += 1;

    core.devices.push(UsbDevice {
        addr,
        vid,
        pid,
        class_code,
        max_packet_size: 64,
    });

    addr
}

pub fn list_devices() -> Vec<UsbDevice> {
    CORE.lock().devices.clone()
}
