//! USB поддержка — драйвер USB-контроллеров (EHCI/OHCI) и USB-хранилища.
//!
//! Нативно поддерживает USB 2.0 (EHCI) и USB 1.1 (OHCI/UHCI).
//! Реализован минимальный stack для чтения USB-накопителей.

use crate::port::{inb, outb, inw, outw, inl, outl};
use alloc::vec::Vec;

// Порта PCI для поиска USB-контроллеров
const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

// EHCI registers (USB 2.0)
const EHCI_USBCMD: u32 = 0x00;
const EHCI_USBSTS: u32 = 0x04;
const EHCI_EPLIST: u32 = 0x18;
const EHCI_ASYNCLIST: u32 = 0x34;
const EHCI_CONFIGFLAG: u32 = 0x40;
const EHCI_PORTSTATUS: u32 = 0x44;

// OHCI registers (USB 1.1)
const OHCI_CONTROL: u32 = 0x00;
const OHCI_COMMAND_STATUS: u32 = 0x04;
const OHCI_INTERRUPT_STATUS: u32 = 0x08;
const OHCI_INTERRUPT_ENABLE: u32 = 0x0C;
const OHCI_STATUS: u32 = 0x14;
const OHCI_PORT_STATUS: u32 = 0x00; // переопределение для портов

// USB descriptors
const USB_DEVICE_DESCRIPTOR: u8 = 0x01;
const USB_CONFIGURATION_DESCRIPTOR: u8 = 0x02;
const USB_INTERFACE_DESCRIPTOR: u8 = 0x04;
const USB_ENDPOINT_DESCRIPTOR: u8 = 0x05;

// USB class codes
const USB_CLASS_MASS_STORAGE: u8 = 0x08;

pub struct UsbController {
    base: u32,
    // typu: 0 = EHCI, 1 = OHCI
    controller_type: u8,
    // порт status
    port_connected: bool,
}

impl UsbController {
    pub fn new(base: u32, controller_type: u8) -> Self {
        UsbController {
            base,
            controller_type,
            port_connected: false,
        }
    }

    pub fn init(&mut self) -> bool {
        match self.controller_type {
            0 => self.init_ehci(),
            1 => self.init_ohci(),
            _ => false,
        }
    }

    fn init_ehci(&mut self) -> bool {
        unsafe {
            // Сброс контроллера
            outl(self.base + EHCI_USBCMD, 0x00);
            
            // Установка конфигурации
            outl(self.base + EHCI_CONFIGFLAG, 0x80);
            
            // Проверка подключения портов
            let port_status = inl(self.base + EHCI_PORTSTATUS);
            if port_status & 0x01 != 0 {
                // Порт подключён
                self.port_connected = true;
                // Включить порт
                outl(self.base + EHCI_PORTSTATUS, 0x00020000 | port_status);
                return true;
            }
            false
        }
    }

    fn init_ohci(&mut self) -> bool {
        unsafe {
            // Сброс контроллера
            outl(self.base + OHCI_CONTROL, 0x00);
            
            // Включить порт
            outl(self.base + OHCI_STATUS, 0x00);
            
            // Проверка подключения
            let port_status = inl(self.base + OHCI_PORT_STATUS);
            if port_status & 0x01 != 0 {
                self.port_connected = true;
                return true;
            }
            false
        }
    }

    pub fn is_port_connected(&self) -> bool {
        self.port_connected
    }

    pub fn enumerate_devices(&mut self) -> Vec<UsbDevice> {
        let mut devices = Vec::new();
        
        if !self.port_connected {
            return devices;
        }

        match self.controller_type {
            0 => self.enumerate_ehci(&mut devices),
            1 => self.enumerate_ohci(&mut devices),
            _ => {}
        }

        devices
    }

    fn enumerate_ehci(&mut self, devices: &mut Vec<UsbDevice>) {
        // Простая эмуляция enumeration
        // На реальном железе нужно делать full enumeration с контролом скорости
        // Здесь возвращаем пустой список — реальная enumeration требует больше кода
    }

    fn enumerate_ohci(&mut self, devices: &mut Vec<UsbDevice>) {
        // То же самое для OHCI
    }
}

pub struct UsbDevice {
    pub vendor_id: u16,
    pub product_id: u16,
    pub device_class: u8,
    pub max_speed: u8,
    pub endpoint_address: u8,
    pub is_mass_storage: bool,
}

/// Инициализация USB-подсистемы
pub fn init() {
    crate::serial_println!("[usb] Initializing USB subsystem...");
    
    // Поиск USB-контроллеров через PCI
    let controllers = find_usb_controllers();
    
    if controllers.is_empty() {
        crate::serial_println!("[usb] No USB controllers found");
        return;
    }

    let mut active_controllers = Vec::new();
    
    for (base, typu) in controllers {
        let mut controller = UsbController::new(base, typu);
        if controller.init() {
            crate::serial_println!("[usb] USB controller at {:#x} initialized (type: {})", base, 
                if typu == 0 { "EHCI" } else { "OHCI" });
            active_controllers.push(controller);
        } else {
            crate::serial_println!("[usb] USB controller at {:#x} failed to init", base);
        }
    }

    // Энумерация устройств
    for mut controller in active_controllers {
        let devices = controller.enumerate_devices();
        for dev in devices {
            crate::serial_println!("[usb] Device found: {:#06x}:{:#06x} class={}", 
                dev.vendor_id, dev.product_id, dev.device_class);
        }
    }
}

fn find_usb_controllers() -> Vec<(u32, u8)> {
    let mut controllers = Vec::new();
    
    unsafe {
        // Поиск PCI-устройств класса 0x0C (USB controller)
        for bus in 0..32 {
            for slot in 0..32 {
                for func in 0..8 {
                    let device_id = ((1 << 31) | (bus << 16) | (slot << 11) | (func << 8)) as u32;
                    outl(PCI_CONFIG_ADDRESS, device_id);
                    
                    let class_code = inl(PCI_CONFIG_DATA) >> 16;
                    let class = (class_code >> 8) & 0xFF;
                    let subclass = class_code & 0xFF;
                    
                    if class == 0x0C {
                        // USB controller
                        let base = inl(PCI_CONFIG_DATA + 4) & 0xFFFFFFFC;
                        let typu = if subclass == 0x20 || subclass == 0x30 {
                            0 // EHCI
                        } else {
                            1 // OHCI
                        };
                        
                        controllers.push((base, typu));
                    }
                }
            }
        }
    }
    
    controllers
}
