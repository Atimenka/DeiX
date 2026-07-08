#![allow(dead_code)]
//! Драйвер USB 3.0 (xHCI — eXtensible Host Controller Interface).
//!
//! ## Статус: КАРКАС (skeleton)
//!
//! Полноценный драйвер xHCI — это ~8000–15000 строк кода. Здесь реализован:
//! - PCI-детектирование xHCI-контроллера
//! - Чтение/запись MMIO-регистров
//! - Базовое сброс/останов/запуск контроллера
//! - Структуры данных: TRB, Event Ring, Command Ring, Device Context
//! - Фундамент для дальнейшей реализации: порты, энумерация, transfer
//!
//! ## Спецификация
//!
//! Intel xHCI spec rev 1.2: https://www.intel.com/content/www/us/en/io/universal-serial-bus/extensible-host-controler-interface-usb-xhci.html
//!
//! ## Что НЕ сделано (честно)
//!
//! - Энумерация USB-устройств (запрос дескрипторов)
//! - Transfer ring management
//! - Bulk/Interrupt/Isochronous transfers
//! - Hub support
//! - USB 2.0 backward compatibility (через EHCI companion)

use crate::pci::{self, PciDevice, read_config_u32};
use crate::mm;


// ==================== PCI Constants ====================

const XHCI_CLASS: u8 = 0x0C;       // Serial Bus Controller
const XHCI_SUBCLASS: u8 = 0x03;    // USB 3.0
const XHCI_PROGIF: u8 = 0x30;      // xHCI

// ==================== xHCI Registers ====================

/// Capability Registers (offset from BAR0)
pub mod cap {
    pub const CAPLENGTH: u32 = 0x00;
    pub const HCSPARAMS1: u32 = 0x04;
    pub const HCSPARAMS2: u32 = 0x08;
    pub const HCSPARAMS3: u32 = 0x0C;
    pub const HCCPARAMS1: u32 = 0x10;
    pub const DBOFF: u32 = 0x14;
    pub const RTSOFF: u32 = 0x18;
}

/// Operational Registers (offset = CAPLENGTH from BAR0)
pub mod op {
    pub const USBCMD: u32 = 0x00;    // USB Command
    pub const USBSTS: u32 = 0x04;    // USB Status
    pub const PAGESIZE: u32 = 0x08;  // Page Size
    pub const DNCTRL: u32 = 0x14;    // Device Notification Control
    pub const CRCR_LO: u32 = 0x18;   // Command Ring Control (low)
    pub const CRCR_HI: u32 = 0x1C;   // Command Ring Control (high)
    pub const DCBAAP_LO: u32 = 0x30; // Device Context Base Address Array (low)
    pub const DCBAAP_HI: u32 = 0x34; // Device Context Base Address Array (high)
    pub const CONFIG: u32 = 0x38;    // Configure
}

// --- USBCMD bits ---
pub const USBCMD_RS: u32 = 1 << 0;      // Run/Stop
pub const USBCMD_HCRST: u32 = 1 << 1;   // Host Controller Reset
pub const USBCMD_INTE: u32 = 1 << 2;    // Interrupt Enable
pub const USBCMD_HSEE: u32 = 1 << 3;    // Host System Error Enable

// --- USBSTS bits ---
pub const USBSTS_HCH: u32 = 1 << 0;     // HC Halted
pub const USBSTS_HSE: u32 = 1 << 2;     // Host System Error
pub const USBSTS_EINT: u32 = 1 << 3;    // Event Interrupt
pub const USBSTS_PCD: u32 = 1 << 4;     // Port Change Detect
pub const USBSTS_CNR: u32 = 1 << 11;    // Controller Not Ready

// ==================== TRB (Transfer Request Block) ====================

/// TRB types
pub mod trb_type {
    pub const NORMAL: u32 = 1;
    pub const SETUP_STAGE: u32 = 2;
    pub const DATA_STAGE: u32 = 3;
    pub const STATUS_STAGE: u32 = 4;
    pub const LINK: u32 = 6;
    pub const EVENT_DATA: u32 = 7;
    pub const NO_OP: u32 = 8;
    pub const ENABLE_SLOT: u32 = 9;
    pub const DISABLE_SLOT: u32 = 10;
    pub const ADDRESS_DEVICE: u32 = 11;
    pub const CONFIGURE_ENDPOINT: u32 = 12;
    pub const EVALUATE_CONTEXT: u32 = 13;
    pub const RESET_ENDPOINT: u32 = 14;
    pub const STOP_ENDPOINT: u32 = 15;
    pub const SET_TR_DEQUEUE: u32 = 16;
    pub const RESET_DEVICE: u32 = 17;
    pub const FORCE_EVENT: u32 = 18;
    pub const NEGOTIATE_BW: u32 = 19;
    pub const SET_LATENCY_TOL: u32 = 20;
    pub const GET_PORT_BW: u32 = 21;
    pub const FORCE_HEADER: u32 = 22;
    pub const NO_OP_CMD: u32 = 23;
}

/// TRB (16 байт)
#[repr(C, align(16))]
struct Trb {
    parameter: u64,
    status: u32,
    cycle_and_type: u32,
    control: u32,
}

// ==================== Контекст драйвера ====================

pub struct XhciController {
    pub pci: PciDevice,
    pub mmio_base: usize,     // физический адрес MMIO (BAR0)
    pub cap_offset: usize,    // смещение capability регистров
    pub op_base: usize,       // физический адрес operational регистров
    pub rt_base: usize,       // физический адрес runtime регистров
    pub db_base: usize,       // физический адрес Doorbell Array
    pub max_ports: u8,        // максимальное число портов
    pub max_slots: u8,        // максимальное число слотов устройств
    pub page_size: u32,       // размер страницы (битовая маска)
}

// ==================== Инициализация ====================

/// Ищет xHCI-контроллер на PCI-шине.
pub fn probe() -> Option<XhciController> {
    // Ищем устройство класса 0x0C (Serial Bus), подкласс 0x03 (USB 3.0).
    let dev = pci::find_device_by_class(XHCI_CLASS)?;

    // Проверяем, что это xHCI (progif = 0x30).
    let class_reg = read_config_u32(dev.bus, dev.slot, dev.function, 0x08);
    let subclass = ((class_reg >> 16) & 0xFF) as u8;
    let progif = ((class_reg >> 8) & 0xFF) as u8;

    if subclass != XHCI_SUBCLASS || progif != XHCI_PROGIF {
        return None;
    }

    // Читаем BAR0 (MMIO).
    let bar0 = pci::read_bar_mmio(dev.bus, dev.slot, dev.function, 0);
    let mmio_base = bar0 as usize;

    // Читаем CAPLENGTH для определения границы capability/operational.
    let caplength = unsafe {
        core::ptr::read_volatile((mmio_base + cap::CAPLENGTH as usize) as *const u8)
    };

    let op_base = mmio_base + caplength as usize;

    // Читаем HCSPARAMS1 для числа портов и слотов.
    let hcsparams1 = unsafe {
        core::ptr::read_volatile((mmio_base + cap::HCSPARAMS1 as usize) as *const u32)
    };
    let max_ports = (hcsparams1 >> 24) as u8;
    let max_slots = (hcsparams1 & 0xFF) as u8;

    // Читаем HCCPARAMS1 для 64-bit addressing.
    let _hccparams1 = unsafe {
        core::ptr::read_volatile((mmio_base + cap::HCCPARAMS1 as usize) as *const u32)
    };

    // DBOFF (Doorbell offset) и RTSOFF (Runtime registers offset).
    let dboff = unsafe {
        core::ptr::read_volatile((mmio_base + cap::DBOFF as usize) as *const u32)
    } & 0xFFFF_FFFC;

    let rtsoff = unsafe {
        core::ptr::read_volatile((mmio_base + cap::RTSOFF as usize) as *const u32)
    } & 0xFFFF_FFFC;

    let db_base = mmio_base + dboff as usize;
    let rt_base = mmio_base + rtsoff as usize;

    // Включаем Bus Mastering + Memory Space.
    pci::enable_bus_mastering(dev.bus, dev.slot, dev.function);
    pci::enable_memory_space(dev.bus, dev.slot, dev.function);

    Some(XhciController {
        pci: dev,
        mmio_base,
        cap_offset: 0,
        op_base,
        rt_base,
        db_base,
        max_ports,
        max_slots,
        page_size: 4096,
    })
}

/// Сброс, останов и запуск контроллера.
pub fn init_and_start(xhc: &XhciController) -> bool {
    // Шаг 1: Остановить контроллер (если работает).
    let usbcmd = unsafe {
        core::ptr::read_volatile((xhc.op_base + op::USBCMD as usize) as *const u32)
    };
    unsafe {
        core::ptr::write_volatile(
            (xhc.op_base + op::USBCMD as usize) as *mut u32,
            usbcmd & !USBCMD_RS,
        );
    }

    // Ждём HCH (HCHalted).
    let mut timeout = 100000;
    loop {
        let sts = unsafe {
            core::ptr::read_volatile((xhc.op_base + op::USBSTS as usize) as *const u32)
        };
        if sts & USBSTS_HCH != 0 {
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            crate::println!("  [xhci] Timeout waiting for HCHalted.");
            return false;
        }
        core::hint::spin_loop();
    }

    // Шаг 2: Сброс контроллера.
    unsafe {
        core::ptr::write_volatile(
            (xhc.op_base + op::USBCMD as usize) as *mut u32,
            usbcmd | USBCMD_HCRST,
        );
    }

    // Ждём сброса (CNR и HCRST сбрасываются).
    timeout = 500000;
    loop {
        let sts = unsafe {
            core::ptr::read_volatile((xhc.op_base + op::USBSTS as usize) as *const u32)
        };
        let cmd = unsafe {
            core::ptr::read_volatile((xhc.op_base + op::USBCMD as usize) as *const u32)
        };
        if (sts & USBSTS_CNR == 0) && (cmd & USBCMD_HCRST == 0) {
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            crate::println!("  [xhci] Timeout waiting for reset completion.");
            return false;
        }
        core::hint::spin_loop();
    }

    // Шаг 3: Настройка (установка MaxSlots, DeviceContextBaseAddress, CommandRing).
    let max_slots = xhc.max_slots as u32;
    unsafe {
        core::ptr::write_volatile(
            (xhc.op_base + op::CONFIG as usize) as *mut u32,
            max_slots,
        );
    }

    // Выделяем Device Context Base Address Array (DCBAA).
    let dcbaa_phys = mm::phys::alloc_pages(1)
        .expect("xhci: out of memory for DCBAA");
    unsafe {
        core::ptr::write_volatile(
            (xhc.op_base + op::DCBAAP_LO as usize) as *mut u32,
            dcbaa_phys as u32,
        );
        core::ptr::write_volatile(
            (xhc.op_base + op::DCBAAP_HI as usize) as *mut u32,
            (dcbaa_phys >> 32) as u32,
        );
    }

    // Выделяем Command Ring.
    let cr_phys = mm::phys::alloc_pages(1)
        .expect("xhci: out of memory for Command Ring");
    unsafe {
        core::ptr::write_volatile(
            (xhc.op_base + op::CRCR_LO as usize) as *mut u32,
            (cr_phys as u32) | 1, // RCS=1 (Ring Cycle State)
        );
        core::ptr::write_volatile(
            (xhc.op_base + op::CRCR_HI as usize) as *mut u32,
            (cr_phys >> 32) as u32,
        );
    }

    // Шаг 4: Запуск контроллера.
    let cmd = unsafe {
        core::ptr::read_volatile((xhc.op_base + op::USBCMD as usize) as *const u32)
    };
    unsafe {
        core::ptr::write_volatile(
            (xhc.op_base + op::USBCMD as usize) as *mut u32,
            cmd | USBCMD_RS | USBCMD_INTE,
        );
    }

    // Ждём, пока HCH сбросится.
    timeout = 100000;
    loop {
        let sts = unsafe {
            core::ptr::read_volatile((xhc.op_base + op::USBSTS as usize) as *const u32)
        };
        if sts & USBSTS_HCH == 0 {
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            crate::println!("  [xhci] Timeout waiting for controller to start.");
            return false;
        }
        core::hint::spin_loop();
    }

    crate::println!("  [xhci] Controller ready: {} ports, {} slots.",
        xhc.max_ports, xhc.max_slots);
    true
}

/// Публичная команда для CLI.
pub fn cmd_info() {
    match probe() {
        Some(xhc) => {
            crate::println!("=== xHCI USB 3.0 Controller ===");
            crate::println!("  PCI:   {:02x}:{:02x}.{:x}",
                xhc.pci.bus, xhc.pci.slot, xhc.pci.function);
            crate::println!("  MMIO:  {:#x}", xhc.mmio_base);
            crate::println!("  Ports: {}", xhc.max_ports);
            crate::println!("  Slots: {}", xhc.max_slots);
            crate::println!("  Status: skeleton driver — controller reset works, transfers not yet implemented.");
        }
        None => {
            crate::println!("No xHCI USB 3.0 controller found.");
        }
    }
}
