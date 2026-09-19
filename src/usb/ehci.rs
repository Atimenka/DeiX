//! Драйвер хост-контроллера USB 2.0 EHCI (Enhanced Host Controller Interface).

use crate::pci;
use crate::spinlock::SpinLock;

const EHCI_PCI_CLASS: u8 = 0x0C;
const EHCI_PCI_SUBCLASS: u8 = 0x03;
const EHCI_PCI_PROGIF: u8 = 0x20;

pub struct EhciController {
    mmio_base: usize,
    op_regs: usize,
    ports: u8,
    ready: bool,
}

static CONTROLLER: SpinLock<Option<EhciController>> = SpinLock::new(None);

pub fn init() -> bool {
    let pci_dev = match pci::find_device_by_class_subclass(EHCI_PCI_CLASS, EHCI_PCI_SUBCLASS) {
        Some(dev) => dev,
        None => return false,
    };

    pci::enable_bus_mastering(pci_dev.bus, pci_dev.slot, pci_dev.function);
    pci::enable_memory_space(pci_dev.bus, pci_dev.slot, pci_dev.function);

    let mmio_bar = pci::read_bar_mmio(pci_dev.bus, pci_dev.slot, pci_dev.function, 0);
    if mmio_bar == 0 {
        return false;
    }

    let mmio_base = mmio_bar as usize;
    let cap_length = unsafe { core::ptr::read_volatile(mmio_base as *const u8) } as usize;
    let op_regs = mmio_base + cap_length;

    let hcsparams = unsafe { core::ptr::read_volatile((mmio_base + 0x04) as *const u32) };
    let n_ports = (hcsparams & 0x0F) as u8;

    // Инициализация EHCI
    unsafe {
        // Сброс контроллера (USBCMD HCRESET)
        let usbcmd_ptr = op_regs as *mut u32;
        let mut usbcmd = core::ptr::read_volatile(usbcmd_ptr);
        core::ptr::write_volatile(usbcmd_ptr, usbcmd | 0x02);

        for _ in 0..10_000 {
            if (core::ptr::read_volatile(usbcmd_ptr) & 0x02) == 0 {
                break;
            }
        }

        // Запуск контроллера (USBCMD Run/Stop)
        usbcmd = core::ptr::read_volatile(usbcmd_ptr);
        core::ptr::write_volatile(usbcmd_ptr, usbcmd | 0x01);

        // CONFIGFLAG = 1 (включение всех портов)
        let config_flag_ptr = (op_regs + 0x40) as *mut u32;
        core::ptr::write_volatile(config_flag_ptr, 1);
    }

    let controller = EhciController {
        mmio_base,
        op_regs,
        ports: n_ports,
        ready: true,
    };

    *CONTROLLER.lock() = Some(controller);
    crate::serial_println!("[usb:ehci] EHCI 2.0 Host Controller инициализирован (портов: {})", n_ports);
    true
}

pub fn is_ready() -> bool {
    CONTROLLER.lock().as_ref().map(|c| c.ready).unwrap_or(false)
}

pub fn port_count() -> u8 {
    CONTROLLER.lock().as_ref().map(|c| c.ports).unwrap_or(0)
}
