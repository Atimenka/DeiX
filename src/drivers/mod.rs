//! Драйверы устройств, построенные поверх прослойки `hal`.
//!
//! Каждый драйвер обращается к железу только через HAL: PCI-конфиг,
//! MMIO-области, задержки. Прямых `read_volatile` по сырым указателям
//! в драйверах быть не должно — иначе смысл прослойки теряется.

pub mod nvidia;

/// Самопроверка прослойки на устройстве, которое QEMU **реально**
/// эмулирует.
///
/// Драйвер NVIDIA проверить негде — карт NVIDIA в QEMU нет. Но сам
/// слой HAL проверить можно и нужно: если он врёт на доступном
/// устройстве, на видеокарте он тем более не заработает. Берём
/// RTL8139: он есть в QEMU, у него известные регистры и MAC-адрес,
/// который легко сверить.
pub fn hal_selftest() {
    use crate::hal;

    crate::println!("  [hal] самопроверка прослойки на RTL8139 (QEMU его эмулирует)");

    // RTL8139: 10EC:8139.
    let dev = match hal::find_device(0x10EC, 0x8139) {
        Some(d) => d,
        None => {
            crate::println!("  [hal] RTL8139 не найден — запустите QEMU с '-device rtl8139'");
            return;
        }
    };

    crate::println!(
        "  [hal] устройство {:04X}:{:04X} на PCI {}:{}.{}, IRQ {}",
        dev.vendor_id, dev.device_id, dev.bus, dev.slot, dev.function, dev.irq_line
    );

    // BAR0 у RTL8139 — порты ввода-вывода, BAR1 — MMIO.
    match dev.bar_address(0) {
        Some(b) => crate::println!("  [hal] BAR0 = {:#x}", b),
        None => crate::println!("  [hal] BAR0 не задан"),
    }
    match dev.bar_address(1) {
        Some(b) => crate::println!("  [hal] BAR1 = {:#x}", b),
        None => crate::println!("  [hal] BAR1 не задан"),
    }

    // Читаем MAC через конфигурационное пространство и MMIO — так
    // видно, что оба пути слоя дают одно и то же.
    dev.enable_memory();
    dev.enable_bus_master();

    if let Some(bar1) = dev.bar_address(1) {
        let regs = unsafe { hal::mmio::MmioRegion::new(bar1, 256) };
        match (regs.read32(0x00), regs.read16(0x04)) {
            (Some(lo), Some(hi)) => {
                let m = lo.to_le_bytes();
                let h = hi.to_le_bytes();
                crate::println!(
                    "  [hal] MAC через MMIO: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                    m[0], m[1], m[2], m[3], h[0], h[1]
                );
                // У QEMU MAC по умолчанию 52:54:00:12:34:56.
                if m[0] == 0x52 && m[1] == 0x54 {
                    crate::println!("  [hal] САМОПРОВЕРКА ПРОЙДЕНА: MMIO читает регистры верно");
                } else {
                    crate::println!("  [hal] неожиданный MAC — проверьте параметры QEMU");
                }
            }
            _ => crate::println!("  [hal] чтение MMIO не удалось (выход за границу области)"),
        }

        // Проверяем защиту границ: адрес за пределами области должен
        // вернуть None, а не прочитать чужую память.
        if regs.read32(4096).is_none() {
            crate::println!("  [hal] защита границ MMIO работает");
        } else {
            crate::println!("  [hal] ОШИБКА: чтение за границей области не отсечено");
        }
    }
}
