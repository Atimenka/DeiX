//! Определение видеокарты через PCI (класс 0x03 = Display controller) и
//! честная классификация: что реально нашли и что с этим можно сделать.
//!
//! Важное ограничение, о котором стоит сказать прямо: настоящий закрытый
//! чип NVIDIA (или AMD) нельзя нормально поддержать без официальной
//! документации/прошивок производителя. Даже открытый проект nouveau —
//! результат более 15 лет реверс-инжиниринга большой командой; современные
//! карты (Turing и новее) требуют подписанную закрытую прошивку GSP от
//! самой NVIDIA, без которой модесеттинг невозможен в принципе. К тому же
//! QEMU (наша единственная тестовая среда) физически не эмулирует никакой
//! NVIDIA/AMD GPU.
//!
//! Поэтому здесь мы делаем то же самое честное разделение, что и с Wi-Fi:
//! реальное обнаружение и классификация оборудования плюс прозрачное
//! сообщение об ограничениях, а рабочий графический режим получаем через
//! открытый, задокументированный интерфейс Bochs VBE (см. vbe.rs) —
//! именно так поступают многие мини-ОС и загрузчики (GRUB, большинство
//! bootloader'ов, hobby OS) для гарантированно рабочей графики в разумные
//! сроки без полноценного 3D-стека под конкретное железо.

use crate::pci;

pub const PCI_CLASS_DISPLAY: u8 = 0x03;

pub const VENDOR_NVIDIA: u16 = 0x10DE;
pub const VENDOR_AMD: u16 = 0x1002;
pub const VENDOR_INTEL: u16 = 0x8086;
pub const VENDOR_QEMU_BOCHS: u16 = 0x1234;
pub const VENDOR_VIRTIO: u16 = 0x1AF4;
pub const VENDOR_VMWARE: u16 = 0x15AD;

pub const BOCHS_VGA_DEVICE_ID: u16 = 0x1111;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    QemuBochs,
    VirtIo,
    VMware,
    Unknown(u16),
}

impl GpuVendor {
    fn from_id(id: u16) -> GpuVendor {
        match id {
            VENDOR_NVIDIA => GpuVendor::Nvidia,
            VENDOR_AMD => GpuVendor::Amd,
            VENDOR_INTEL => GpuVendor::Intel,
            VENDOR_QEMU_BOCHS => GpuVendor::QemuBochs,
            VENDOR_VIRTIO => GpuVendor::VirtIo,
            VENDOR_VMWARE => GpuVendor::VMware,
            other => GpuVendor::Unknown(other),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            GpuVendor::Nvidia => "NVIDIA",
            GpuVendor::Amd => "AMD",
            GpuVendor::Intel => "Intel",
            GpuVendor::QemuBochs => "QEMU (Bochs-compatible)",
            GpuVendor::VirtIo => "VirtIO GPU",
            GpuVendor::VMware => "VMware SVGA",
            GpuVendor::Unknown(_) => "Unknown",
        }
    }
}

pub struct GpuInfo {
    pub vendor: GpuVendor,
    pub device_id: u16,
    pub device: pci::PciDevice,
    pub supports_bochs_vbe: bool,
}

/// Сканирует PCI-шину в поисках любого устройства класса "Display
/// controller" (0x03xxxx) и классифицирует найденное.
pub fn detect() -> Option<GpuInfo> {
    let device = pci::find_device_by_class(PCI_CLASS_DISPLAY)?;
    let vendor_id = pci::read_vendor_id(device.bus, device.slot, device.function);
    let device_id = pci::read_device_id(device.bus, device.slot, device.function);
    let vendor = GpuVendor::from_id(vendor_id);

    let supports_bochs_vbe = vendor == GpuVendor::QemuBochs && device_id == BOCHS_VGA_DEVICE_ID;

    Some(GpuInfo {
        vendor,
        device_id,
        device,
        supports_bochs_vbe,
    })
}

/// Честное сообщение о статусе поддержки для конкретного вендора —
/// используется в CLI командой `gpu info`.
pub fn support_status_message(vendor: GpuVendor) -> (&'static str, &'static str) {
    match vendor {
        GpuVendor::Nvidia => (
            "NVIDIA GPU detected. See 'gpu nvinfo' for the open nouveau-based chipset \
             identification and details on what this kernel can/cannot do with it.",
            "Обнаружен GPU NVIDIA. Смотри 'gpu nvinfo' для открытого определения чипа \
             (на основе nouveau) и подробностей, что это ядро может и не может с ним сделать.",
        ),
        GpuVendor::Amd => (
            "AMD GPU detected. No open modesetting driver is included in this kernel \
             (AMDGPU is a large, actively maintained project requiring firmware blobs \
             for most modern cards). Use the Bochs VBE framebuffer driver instead.",
            "Обнаружен GPU AMD. Открытый драйвер модесеттинга не включён в это ядро \
             (AMDGPU — большой, активно поддерживаемый проект, требующий файлы прошивок \
             для большинства современных карт). Используй драйвер Bochs VBE framebuffer.",
        ),
        GpuVendor::Intel => (
            "Intel GPU detected. No open modesetting driver is included in this kernel. \
             Use the Bochs VBE framebuffer driver instead.",
            "Обнаружен GPU Intel. Открытый драйвер модесеттинга не включён в это ядро. \
             Используй драйвер Bochs VBE framebuffer.",
        ),
        GpuVendor::QemuBochs => (
            "QEMU standard VGA (Bochs-compatible) detected — full support via the open, \
             documented Bochs VBE Display Interface (DISPI). This is a real, working \
             graphics driver.",
            "Обнаружена стандартная VGA-карта QEMU (совместимая с Bochs) — полная \
             поддержка через открытый, задокументированный интерфейс Bochs VBE (DISPI). \
             Это настоящий рабочий графический драйвер.",
        ),
        GpuVendor::VirtIo => (
            "VirtIO GPU detected. Not supported by this kernel yet (would need a \
             virtio-gpu driver, different protocol from Bochs VBE).",
            "Обнаружен VirtIO GPU. Пока не поддерживается этим ядром (нужен отдельный \
             драйвер virtio-gpu, другой протокол по сравнению с Bochs VBE).",
        ),
        GpuVendor::VMware => (
            "VMware SVGA detected. Not supported by this kernel yet.",
            "Обнаружен VMware SVGA. Пока не поддерживается этим ядром.",
        ),
        GpuVendor::Unknown(_) => (
            "Unknown GPU vendor. Not supported.",
            "Неизвестный производитель GPU. Не поддерживается.",
        ),
    }
}
