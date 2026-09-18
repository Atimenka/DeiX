//! HAL — прослойка между драйверами и железом.
//!
//! Зачем нужна: драйвер не должен знать, как именно в DeiX устроены
//! PCI-конфиг, MMIO и выделение памяти. Он работает с этим слоем, а
//! слой уже переводит вызовы в наши примитивы. Это то же, что делает
//! **LinuxKPI** во FreeBSD: там драйверы Linux собираются почти без
//! правок поверх прослойки, реализующей `kmalloc`, `readl`, `writel` и
//! прочее.
//!
//! ## Что это даёт
//!
//! Портируя драйвер (например, участки nouveau), не приходится править
//! каждое обращение к регистрам — достаточно отобразить его вызовы на
//! этот слой. Драйверы перестают зависеть от внутренностей ядра, а
//! ядро может менять их, не ломая драйверы.
//!
//! ## Честная оговорка о масштабе
//!
//! У FreeBSD прослойка LinuxKPI — 51 файл и 846 КБ исходников, и она
//! опирается на готовые VM-подсистему, DMA и потоки. Наш слой
//! несопоставимо меньше и покрывает только то, что реально нужно
//! драйверам DeiX: PCI, MMIO, порты, задержки, физическую память.
//! Выдавать его за полноценный LinuxKPI было бы неправдой.

pub mod mmio;

use crate::pci;

/// Описание устройства, как его видит драйвер.
#[derive(Clone, Copy)]
pub struct Device {
    pub vendor_id: u16,
    pub device_id: u16,
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    /// Линия прерывания из конфигурационного пространства PCI.
    pub irq_line: u8,
}

impl Device {
    /// Включает Bus Mastering — без него устройство не сможет вести DMA.
    pub fn enable_bus_master(&self) {
        pci::enable_bus_mastering(self.bus, self.slot, self.function);
    }

    /// Включает доступ к памяти устройства (бит Memory Space в COMMAND).
    pub fn enable_memory(&self) {
        pci::enable_memory_space(self.bus, self.slot, self.function);
    }

    /// Читает 32-битное слово конфигурационного пространства.
    pub fn config_read32(&self, offset: u8) -> u32 {
        pci::read_config_u32(self.bus, self.slot, self.function, offset)
    }

    /// Пишет 32-битное слово в конфигурационное пространство.
    pub fn config_write32(&self, offset: u8, value: u32) {
        pci::write_config_u32(self.bus, self.slot, self.function, offset, value);
    }

    /// Базовый адрес BAR (Base Address Register) с номером `index`.
    ///
    /// Возвращает физический адрес MMIO-области. Младшие биты BAR —
    /// флаги (тип области, разрядность, prefetchable), их маскируем.
    pub fn bar_address(&self, index: u8) -> Option<u64> {
        if index > 5 {
            return None;
        }
        let offset = 0x10 + index * 4;
        let low = self.config_read32(offset);
        if low == 0 {
            return None;
        }
        // Бит 0: 1 = порты ввода-вывода, 0 = память.
        if low & 1 != 0 {
            return Some((low & !0x3) as u64);
        }
        // Биты 2:1 = 10b означают 64-битный BAR: старшая половина в
        // следующем регистре.
        let is_64bit = (low >> 1) & 0x3 == 0x2;
        let base = (low & !0xF) as u64;
        if is_64bit && index < 5 {
            let high = self.config_read32(offset + 4) as u64;
            Some(base | (high << 32))
        } else {
            Some(base)
        }
    }
}

/// Ищет устройство по идентификаторам производителя и модели.
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<Device> {
    let d = pci::find_device(vendor_id, device_id)?;
    Some(Device {
        vendor_id,
        device_id,
        bus: d.bus,
        slot: d.slot,
        function: d.function,
        irq_line: pci::read_interrupt_line(d.bus, d.slot, d.function),
    })
}

/// Ищет первое устройство заданного класса и подкласса.
///
/// Нужно, когда точный device_id неизвестен: видеокарт у NVIDIA сотни
/// моделей, но класс у всех один (0x03 — Display controller).
pub fn find_by_class(vendor_id: u16, class: u8, subclass: u8) -> Option<Device> {
    for bus in 0u8..=255 {
        for slot in 0u8..32 {
            let id = pci::read_config_u32(bus, slot, 0, 0x00);
            if id == 0xFFFF_FFFF {
                continue;
            }
            let vid = (id & 0xFFFF) as u16;
            if vid != vendor_id {
                continue;
            }
            let class_reg = pci::read_config_u32(bus, slot, 0, 0x08);
            let dev_class = ((class_reg >> 24) & 0xFF) as u8;
            let dev_sub = ((class_reg >> 16) & 0xFF) as u8;
            if dev_class == class && dev_sub == subclass {
                return Some(Device {
                    vendor_id: vid,
                    device_id: ((id >> 16) & 0xFFFF) as u16,
                    bus,
                    slot,
                    function: 0,
                    irq_line: pci::read_interrupt_line(bus, slot, 0),
                });
            }
        }
    }
    None
}

/// Пауза в микросекундах — драйверам она нужна при инициализации
/// железа (регистры требуют времени на срабатывание).
///
/// Реализована ожиданием по таймеру ядра, если он уже запущен, иначе
/// холостым циклом: на этапе ранней инициализации таймера ещё нет.
pub fn udelay(usec: u64) {
    let start = crate::timer::uptime_ms();
    if start > 0 {
        let target_ms = usec.div_ceil(1000).max(1);
        while crate::timer::uptime_ms().saturating_sub(start) < target_ms {
            core::hint::spin_loop();
        }
        return;
    }
    // Грубая калибровка: примерно 100 пустых итераций на микросекунду.
    for _ in 0..(usec * 100) {
        core::hint::spin_loop();
    }
}
