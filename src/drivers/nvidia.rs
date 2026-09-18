//! Открытый драйвер NVIDIA поверх HAL — определение чипа и раскладка
//! регистров по документации проекта nouveau.
//!
//! ## Почему без прошивки
//!
//! Мейнтейнер nouveau Ilia Mirkin формулирует это так: *«Across the
//! board, modesetting works without any additional firmware»* —
//! установка видеорежима работает на всех картах NVIDIA без блобов.
//! Прошивка нужна только для:
//!
//! * видеодекодирования (G84 и новее) — нам не требуется;
//! * 2D/3D-ускорения, начиная с **GM20x** (Maxwell 2, 2014) — там
//!   подписи проверяет сам чип;
//! * GSP — только **Turing** (2018) и новее.
//!
//! Fermi (GF1xx, 2011) старше всех трёх порогов, поэтому для него
//! modesetting реализуем полностью открытым кодом. Искать GSP-прошивку
//! для GT 520M бессмысленно: процессора GSP в этом чипе физически нет.
//!
//! ## Что этот код делает и чего не делает
//!
//! Делает: находит карту на шине PCI, отображает BAR0, читает
//! `NV_PMC_BOOT_0`, определяет поколение и печатает состояние
//! дисплейного движка.
//!
//! **Не делает: не переключает видеорежим.** Для Fermi это требует
//! программирования EVO/NVDisplay через канал команд и таблиц VBIOS
//! конкретной платы. Писать это вслепую нельзя — QEMU не эмулирует ни
//! одной карты NVIDIA, проверить будет негде. Поэтому здесь
//! заканчивается проверяемая часть, и я не выдаю задел за готовый
//! драйвер.

use crate::hal::{self, mmio::MmioRegion, Device};

/// Идентификатор производителя NVIDIA на шине PCI.
pub const VENDOR_NVIDIA: u16 = 0x10DE;
/// Класс «видеоконтроллер», подкласс «VGA-совместимый».
const CLASS_DISPLAY: u8 = 0x03;
const SUBCLASS_VGA: u8 = 0x00;

/// Главный регистр идентификации чипа. Один из немногих, читаемых
/// сразу после включения, без всякой инициализации.
const NV_PMC_BOOT_0: u64 = 0x0000_0000;
/// Регистр разрешения подсистем (PMC_ENABLE).
const NV_PMC_ENABLE: u64 = 0x0000_0200;
/// Состояние прерываний PMC.
const NV_PMC_INTR_0: u64 = 0x0000_0100;

/// Размер области регистров: у всех карт начиная с NV50 это 16 МиБ.
const BAR0_SIZE: u64 = 16 * 1024 * 1024;

/// Поколение чипа.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arch {
    /// NV04..NV40 — простые регистры CRTC/RAMDAC.
    Curie,
    /// NV50/G80 — первое поколение с движком EVO.
    Tesla,
    /// GF1xx — сюда относится GT 520M (GF119).
    Fermi,
    /// GK1xx.
    Kepler,
    /// GM1xx/GM2xx — с GM20x требуется подписанная прошивка для 3D.
    Maxwell,
    /// GP1xx.
    Pascal,
    /// TU1xx и новее — появился процессор GSP.
    TuringPlus,
    Unknown,
}

impl Arch {
    pub fn name(&self) -> &'static str {
        match self {
            Arch::Curie => "Curie (NV04-NV40)",
            Arch::Tesla => "Tesla (NV50/G80)",
            Arch::Fermi => "Fermi (GF1xx)",
            Arch::Kepler => "Kepler (GK1xx)",
            Arch::Maxwell => "Maxwell (GM1xx/GM2xx)",
            Arch::Pascal => "Pascal (GP1xx)",
            Arch::TuringPlus => "Turing и новее (TU1xx+)",
            Arch::Unknown => "неизвестно",
        }
    }

    /// Нужна ли подписанная прошивка для 2D/3D-ускорения.
    ///
    /// Порог — GM20x: с него чип проверяет криптографические подписи.
    pub fn needs_firmware_for_accel(&self) -> bool {
        matches!(self, Arch::Maxwell | Arch::Pascal | Arch::TuringPlus)
    }

    /// Есть ли в чипе процессор GSP (Turing и новее).
    pub fn has_gsp(&self) -> bool {
        matches!(self, Arch::TuringPlus)
    }
}

/// Определяет поколение по старшему байту идентификатора чипа.
fn classify(chipset: u32) -> Arch {
    match chipset {
        0x04..=0x4F => Arch::Curie,
        0x50 | 0x80..=0xAF => Arch::Tesla,
        0xC0..=0xD9 => Arch::Fermi,
        0xE0..=0xF1 | 0x108 => Arch::Kepler,
        0x110..=0x137 => Arch::Maxwell,
        0x130..=0x13F => Arch::Pascal,
        0x140.. => Arch::TuringPlus,
        _ => Arch::Unknown,
    }
}

/// Найденная и опознанная видеокарта.
pub struct NvidiaGpu {
    pub device: Device,
    pub regs: MmioRegion,
    pub boot0: u32,
    pub chipset: u32,
    pub arch: Arch,
}

impl NvidiaGpu {
    /// Ищет карту NVIDIA и читает её идентификатор.
    pub fn probe() -> Option<NvidiaGpu> {
        let device = hal::find_by_class(VENDOR_NVIDIA, CLASS_DISPLAY, SUBCLASS_VGA)?;

        // Без этого бита обращения к BAR не дойдут до чипа.
        device.enable_memory();

        let bar0 = device.bar_address(0)?;
        if bar0 == 0 {
            return None;
        }
        let regs = unsafe { MmioRegion::new(bar0, BAR0_SIZE) };

        let boot0 = regs.read32(NV_PMC_BOOT_0)?;
        // 0xFFFFFFFF означает, что чтение не дошло до устройства:
        // BAR не отображён или карта не отвечает.
        if boot0 == 0xFFFF_FFFF || boot0 == 0 {
            return None;
        }

        // Раскладка NV_PMC_BOOT_0: биты 20:20 и выше кодируют чип.
        // Для NV50+ идентификатор чипа лежит в битах 19:16 и 23:20.
        let chipset = (boot0 >> 20) & 0x1FF;
        let arch = classify(chipset);

        Some(NvidiaGpu {
            device,
            regs,
            boot0,
            chipset,
            arch,
        })
    }

    /// Читает регистр разрешения подсистем.
    pub fn pmc_enable(&self) -> Option<u32> {
        self.regs.read32(NV_PMC_ENABLE)
    }

    /// Читает регистр состояния прерываний.
    pub fn pmc_intr(&self) -> Option<u32> {
        self.regs.read32(NV_PMC_INTR_0)
    }

    /// Человекочитаемый отчёт о карте.
    pub fn report(&self) {
        crate::println!("  [nvidia] карта найдена: {:04X}:{:04X}",
            self.device.vendor_id, self.device.device_id);
        crate::println!("  [nvidia] PCI {}:{}.{}, IRQ {}",
            self.device.bus, self.device.slot, self.device.function, self.device.irq_line);
        crate::println!("  [nvidia] BAR0 = {:#x} ({} МиБ регистров)",
            self.regs.base(), self.regs.size() / (1024 * 1024));
        crate::println!("  [nvidia] NV_PMC_BOOT_0 = {:#010x}", self.boot0);
        crate::println!("  [nvidia] чип {:#05x} -> {}", self.chipset, self.arch.name());

        if let Some(e) = self.pmc_enable() {
            crate::println!("  [nvidia] PMC_ENABLE = {:#010x}", e);
        }
        if let Some(i) = self.pmc_intr() {
            crate::println!("  [nvidia] PMC_INTR_0 = {:#010x}", i);
        }

        crate::println!("");
        crate::println!("  Прошивка для установки видеорежима: НЕ ТРЕБУЕТСЯ");
        crate::println!("  (modesetting работает без блобов на всех картах NVIDIA)");

        if self.arch.needs_firmware_for_accel() {
            crate::println!("  Прошивка для 2D/3D-ускорения: требуется (GM20x и новее)");
        } else {
            crate::println!("  Прошивка для 2D/3D-ускорения: не требуется на этом поколении");
        }
        if self.arch.has_gsp() {
            crate::println!("  Процессор GSP: есть, нужен gsp_*.bin от NVIDIA");
        } else {
            crate::println!("  Процессор GSP: отсутствует (появился в Turing)");
        }
    }
}

/// Снимает дамп ключевых регистров и пишет его в файл на диске.
///
/// Нужен при проверке на реальном железе: экран может не работать, а
/// дамп остаётся на носителе и его можно изучить потом.
fn dump_to_file(gpu: &NvidiaGpu) {
    use alloc::format;
    let mut out = alloc::string::String::new();

    out.push_str(&format!("DeiX NVIDIA dump\n"));
    out.push_str(&format!("PCI {:04X}:{:04X} bus {} slot {} fn {} irq {}\n",
        gpu.device.vendor_id, gpu.device.device_id,
        gpu.device.bus, gpu.device.slot, gpu.device.function, gpu.device.irq_line));
    out.push_str(&format!("BAR0 {:#x}\n", gpu.regs.base()));
    out.push_str(&format!("BOOT_0 {:#010x} chipset {:#05x} arch {}\n",
        gpu.boot0, gpu.chipset, gpu.arch.name()));

    // Регистры, безопасные для чтения на любом поколении: они не
    // меняют состояние чипа, только сообщают его.
    const PROBES: [(&str, u64); 6] = [
        ("PMC_BOOT_0",   0x0000_0000),
        ("PMC_INTR_0",   0x0000_0100),
        ("PMC_ENABLE",   0x0000_0200),
        ("PBUS_DEBUG_1", 0x0000_1084),
        ("PFB_CFG0",     0x0010_0200),
        ("PDISP_CAPS",   0x0061_0000),
    ];
    for (name, off) in PROBES {
        match gpu.regs.read32(off) {
            Some(v) => out.push_str(&format!("{:<14} {:#08x} = {:#010x}\n", name, off, v)),
            None => out.push_str(&format!("{:<14} {:#08x} = <вне области>\n", name, off)),
        }
    }

    match crate::ext2::write_file("NVIDIA.TXT", out.as_bytes()) {
        Ok(()) => crate::println!("  [nvidia] дамп сохранён: NVIDIA.TXT ('cat NVIDIA.TXT')"),
        Err(_) => crate::println!("  [nvidia] дамп не сохранён (диск недоступен)"),
    }
}

/// CLI: `nvidia [dump]` — поиск и опознание видеокарты.
pub fn cmd_nvidia(arg: &str) {
    match NvidiaGpu::probe() {
        Some(gpu) => {
            gpu.report();
            if arg.trim() == "dump" {
                dump_to_file(&gpu);
            } else {
                crate::println!("  ('nvidia dump' — сохранить дамп регистров в файл)");
            }
        }
        None => {
            crate::println!("  [nvidia] видеокарта NVIDIA не найдена на шине PCI.");
            crate::println!("  В QEMU это ожидаемо: эмуляции карт NVIDIA не существует,");
            crate::println!("  проверить драйвер можно только на реальном железе.");
        }
    }
}
