//! Минимальный драйвер PCI: сканирование шины через порты конфигурации
//! (0xCF8 адрес / 0xCFC данные) и поиск устройств по vendor/device ID.
//! Этого достаточно, чтобы найти сетевую карту RTL8139, которую эмулирует
//! QEMU по умолчанию для `-net nic` (или `-device rtl8139`).

use crate::port::{inl, outl};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

#[derive(Clone, Copy, Debug)]
pub struct PciDevice {
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
}

fn config_address(bus: u8, slot: u8, function: u8, offset: u8) -> u32 {
    (1 << 31)
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC)
}

pub fn read_config_u32(bus: u8, slot: u8, function: u8, offset: u8) -> u32 {
    unsafe {
        outl(CONFIG_ADDRESS, config_address(bus, slot, function, offset));
        inl(CONFIG_DATA)
    }
}

pub fn write_config_u32(bus: u8, slot: u8, function: u8, offset: u8, value: u32) {
    unsafe {
        outl(CONFIG_ADDRESS, config_address(bus, slot, function, offset));
        outl(CONFIG_DATA, value);
    }
}

fn read_config_u16(bus: u8, slot: u8, function: u8, offset: u8) -> u16 {
    let dword = read_config_u32(bus, slot, function, offset & 0xFC);
    let shift = (offset & 2) * 8;
    ((dword >> shift) & 0xFFFF) as u16
}

/// Возвращает базовый I/O адрес (BAR0), если он отображён в I/O-пространство
/// (бит 0 = 1), иначе None (тогда это MMIO-BAR, нам такой не нужен для RTL8139).
pub fn read_bar0_io(bus: u8, slot: u8, function: u8) -> Option<u16> {
    let bar0 = read_config_u32(bus, slot, function, 0x10);
    if bar0 & 0x1 == 1 {
        Some((bar0 & 0xFFFC) as u16)
    } else {
        None
    }
}

pub fn read_interrupt_line(bus: u8, slot: u8, function: u8) -> u8 {
    (read_config_u32(bus, slot, function, 0x3C) & 0xFF) as u8
}

/// Включает PCI Bus Mastering (бит 2 регистра Command в конфигурационном
/// пространстве) — без этого сетевая карта не сможет делать DMA.
pub fn enable_bus_mastering(bus: u8, slot: u8, function: u8) {
    let mut command = read_config_u32(bus, slot, function, 0x04);
    command |= 1 << 2;
    write_config_u32(bus, slot, function, 0x04, command);
}

/// Включает декодирование Memory Space (бит 1 регистра Command) — без
/// этого MMIO BAR устройства (например, framebuffer видеокарты) не
/// отвечает на чтение/запись вообще, даже если сам физический адрес в
/// BAR прописан правильно.
pub fn enable_memory_space(bus: u8, slot: u8, function: u8) {
    let mut command = read_config_u32(bus, slot, function, 0x04);
    command |= 1 << 1;
    write_config_u32(bus, slot, function, 0x04, command);
}

/// Полный перебор всех bus/slot/function в поисках устройства с заданными
/// vendor_id/device_id. Для мини-ОС достаточно простого линейного перебора
/// без рекурсии по multi-function-мостам — по факту RTL8139 в QEMU всегда
/// находится на bus 0.
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    find_device_where(|v, d| v == vendor_id && d == device_id)
}

/// Ищет первое устройство заданного класса (байт 0x0B конфигурационного
/// пространства, см. таблицу PCI Class Codes) — например 0x03 для
/// "Display controller" (видеокарта любого производителя).
pub fn find_device_by_class(class_code: u8) -> Option<PciDevice> {
    for bus in 0..=7u16 {
        let bus = bus as u8;
        let mut bus_empty = true;
        for slot in 0..32u8 {
            let vendor = read_config_u16(bus, slot, 0, 0x00);
            if vendor == 0xFFFF {
                continue;
            }
            bus_empty = false;

            let header_type = (read_config_u32(bus, slot, 0, 0x0C) >> 16) & 0xFF;
            let max_function = if header_type & 0x80 != 0 { 8 } else { 1 };

            for function in 0..max_function {
                let v = read_config_u16(bus, slot, function, 0x00);
                if v == 0xFFFF {
                    continue;
                }
                let class_reg = read_config_u32(bus, slot, function, 0x08);
                let this_class = ((class_reg >> 24) & 0xFF) as u8;
                if this_class == class_code {
                    let d = read_config_u16(bus, slot, function, 0x02);
                    return Some(PciDevice {
                        bus,
                        slot,
                        function,
                        vendor_id: v,
                        device_id: d,
                    });
                }
            }
        }
        if bus_empty && bus > 0 {
            break;
        }
    }
    None
}

/// Ищет устройство по паре Class + Subclass (например, Class 0x04 + Subclass 0x03
/// для аудиоконтроллера Intel High Definition Audio).
pub fn find_device_by_class_subclass(class_code: u8, subclass_code: u8) -> Option<PciDevice> {
    for bus in 0..=7u16 {
        let bus = bus as u8;
        let mut bus_empty = true;
        for slot in 0..32u8 {
            let vendor = read_config_u16(bus, slot, 0, 0x00);
            if vendor == 0xFFFF {
                continue;
            }
            bus_empty = false;

            let header_type = (read_config_u32(bus, slot, 0, 0x0C) >> 16) & 0xFF;
            let max_function = if header_type & 0x80 != 0 { 8 } else { 1 };

            for function in 0..max_function {
                let v = read_config_u16(bus, slot, function, 0x00);
                if v == 0xFFFF {
                    continue;
                }
                let class_reg = read_config_u32(bus, slot, function, 0x08);
                let this_class = ((class_reg >> 24) & 0xFF) as u8;
                let this_subclass = ((class_reg >> 16) & 0xFF) as u8;
                if this_class == class_code && this_subclass == subclass_code {
                    let d = read_config_u16(bus, slot, function, 0x02);
                    return Some(PciDevice {
                        bus,
                        slot,
                        function,
                        vendor_id: v,
                        device_id: d,
                    });
                }
            }
        }
        if bus_empty && bus > 0 {
            break;
        }
    }
    None
}

fn find_device_where(predicate: impl Fn(u16, u16) -> bool) -> Option<PciDevice> {
    for bus in 0..=7u16 {
        let bus = bus as u8;
        let mut bus_empty = true;
        for slot in 0..32u8 {
            let vendor = read_config_u16(bus, slot, 0, 0x00);
            if vendor == 0xFFFF {
                continue; // устройства нет
            }
            bus_empty = false;

            let header_type = (read_config_u32(bus, slot, 0, 0x0C) >> 16) & 0xFF;
            let max_function = if header_type & 0x80 != 0 { 8 } else { 1 };

            for function in 0..max_function {
                let v = read_config_u16(bus, slot, function, 0x00);
                if v == 0xFFFF {
                    continue;
                }
                let d = read_config_u16(bus, slot, function, 0x02);
                if predicate(v, d) {
                    return Some(PciDevice {
                        bus,
                        slot,
                        function,
                        vendor_id: v,
                        device_id: d,
                    });
                }
            }
        }
        if bus_empty && bus > 0 {
            break;
        }
    }
    None
}

pub fn read_vendor_id(bus: u8, slot: u8, function: u8) -> u16 {
    read_config_u16(bus, slot, function, 0x00)
}

pub fn read_device_id(bus: u8, slot: u8, function: u8) -> u16 {
    read_config_u16(bus, slot, function, 0x02)
}

/// Читает произвольный BAR (0-5) и возвращает его как "сырой" физический
/// адрес (с уже обнулёнными служебными битами флагов) — этого достаточно
/// для MMIO BAR (framebuffer видеокарты). Не поддерживает 64-битные BAR
/// (пара 32-битных регистров) — для Bochs VBE framebuffer BAR0 всегда
/// 32-битный, так что этого хватает.
pub fn read_bar_mmio(bus: u8, slot: u8, function: u8, bar_index: u8) -> u32 {
    let offset = 0x10 + bar_index * 4;
    let bar = read_config_u32(bus, slot, function, offset);
    bar & 0xFFFF_FFF0
}
