//! Доступ к регистрам устройств через память (MMIO).
//!
//! Драйверы NVIDIA, сетевых карт и почти всего современного железа
//! общаются с чипом не портами, а чтением и записью по физическим
//! адресам из BAR. Здесь эти операции собраны в одном месте: так
//! драйвер не рассыпает по коду `read_volatile` с ручным приведением
//! указателей, а ошибки с выравниванием и границами ловятся централизованно.
//!
//! Аналог `readl`/`writel` в Linux и LinuxKPI во FreeBSD.

/// Отображённая область регистров устройства.
///
/// Хранит физический базовый адрес и размер. Проверка границ на каждом
/// доступе не даёт драйверу случайно залезть за пределы своей области —
/// в Ring 0 такая ошибка означала бы порчу чужой памяти или зависание.
#[derive(Clone, Copy)]
pub struct MmioRegion {
    base: u64,
    size: u64,
}

impl MmioRegion {
    /// Создаёт область. Адрес должен быть получен из BAR устройства.
    ///
    /// # Safety
    /// Вызывающий отвечает за то, что диапазон действительно
    /// принадлежит устройству и отображён в адресное пространство.
    pub const unsafe fn new(base: u64, size: u64) -> Self {
        MmioRegion { base, size }
    }

    pub fn base(&self) -> u64 {
        self.base
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    fn check(&self, offset: u64, width: u64) -> bool {
        offset.checked_add(width).is_some_and(|end| end <= self.size)
    }

    /// Чтение 32 бит. `None`, если смещение выходит за границу области.
    pub fn read32(&self, offset: u64) -> Option<u32> {
        if !self.check(offset, 4) {
            return None;
        }
        // volatile обязателен: компилятор не должен кэшировать или
        // переупорядочивать обращения к регистрам устройства.
        Some(unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) })
    }

    /// Запись 32 бит. `false`, если смещение вне области.
    pub fn write32(&self, offset: u64, value: u32) -> bool {
        if !self.check(offset, 4) {
            return false;
        }
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u32, value) };
        true
    }

    pub fn read16(&self, offset: u64) -> Option<u16> {
        if !self.check(offset, 2) {
            return None;
        }
        Some(unsafe { core::ptr::read_volatile((self.base + offset) as *const u16) })
    }

    pub fn write16(&self, offset: u64, value: u16) -> bool {
        if !self.check(offset, 2) {
            return false;
        }
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u16, value) };
        true
    }

    pub fn read8(&self, offset: u64) -> Option<u8> {
        if !self.check(offset, 1) {
            return None;
        }
        Some(unsafe { core::ptr::read_volatile((self.base + offset) as *const u8) })
    }

    pub fn write8(&self, offset: u64, value: u8) -> bool {
        if !self.check(offset, 1) {
            return false;
        }
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u8, value) };
        true
    }

    /// Меняет отдельные биты регистра: читает, накладывает маску, пишет.
    ///
    /// Частая операция в драйверах: `reg = (reg & !mask) | value`.
    pub fn modify32(&self, offset: u64, mask: u32, value: u32) -> bool {
        match self.read32(offset) {
            Some(cur) => self.write32(offset, (cur & !mask) | (value & mask)),
            None => false,
        }
    }

    /// Ждёт, пока регистр по маске не примет нужное значение.
    ///
    /// Возвращает `false` по истечении таймаута. Без таймаута драйвер
    /// на неисправном железе завис бы навсегда — а диагностировать это
    /// в Ring 0 нечем.
    pub fn wait_for(&self, offset: u64, mask: u32, expected: u32, timeout_us: u64) -> bool {
        let mut waited = 0u64;
        loop {
            if let Some(v) = self.read32(offset) {
                if v & mask == expected {
                    return true;
                }
            }
            if waited >= timeout_us {
                return false;
            }
            super::udelay(10);
            waited += 10;
        }
    }
}
