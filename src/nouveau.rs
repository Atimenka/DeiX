//! Открытый NVIDIA-драйвер на основе документации проекта nouveau
//! (MIT/X11-лицензия, freedesktop.org) — регистровые адреса и константы
//! ниже взяты из его публичных заголовков (`nvreg.h`, `nv04_crtc.c`,
//! `dispnv04/hw.h`), которые сами nouveau собрали за 15+ лет легального
//! реверс-инжиниринга без какого-либо кода/прошивок NVIDIA. Всё в этом
//! файле either (а) честная классификация железа по общедоступному
//! регистру NV_PMC_BOOT_0 (это не секрет — она видна любому, у кого есть
//! карта и осциллограф, и полностью задокументирована), либо (б)
//! экспериментальный modesetting для АРХИТЕКТУР NV04..NV40 (карты
//! примерно 1999-2005 годов), где видеовывод всё ещё устроен как
//! классический VGA CRTC + RAMDAC с прямыми MMIO-регистрами — то есть
//! тем же способом, каким его программирует xf86-video-nv/nouveau.
//!
//! ЧЕСТНО О ГРАНИЦАХ ЭТОГО ДРАЙВЕРА (важно прочитать перед использованием):
//!
//!   1. НЕ ПРОТЕСТИРОВАНО НА РЕАЛЬНОМ ИЛИ ЭМУЛИРУЕМОМ ЖЕЛЕЗЕ. QEMU не
//!      умеет эмулировать никакой NVIDIA GPU, поэтому единственная
//!      тестовая среда, доступная при разработке этого файла, физически
//!      не может его исполнить. Регистровые адреса и битовые поля взяты
//!      из открытого кода nouveau (ссылки в комментариях к константам),
//!      но реальное поведение на конкретной карте может отличаться —
//!      как и у любого нового драйвера без цикла тестирования на живом
//!      железе. Прежде чем доверять этому modesetting на реальной
//!      машине, тестируйте на системе, где можно быстро выключить
//!      питание/перезагрузиться, если экран останется чёрным.
//!
//!   2. Карты архитектуры Kelvin/Rankine и новее (NV1x/GeForce 6-7, а тем
//!      более Tesla/Fermi/Kepler/Maxwell/Pascal/Turing+ — то есть ВСЕ
//!      GeForce 8xxx и новее, включая упомянутую GT 520M — Fermi)
//!      используют принципиально другую модель вывода видео: не прямые
//!      CRTC/RAMDAC-регистры, а EVO/NVDisplay display engine с push-
//!      буферами команд и виртуальной памятью GPU (аналог современных
//!      GPU command submission), плюс, начиная с Turing, обязательную
//!      подписанную закрытую прошивку GSP от самой NVIDIA. Это НЕ вопрос
//!      "чуть больше кода" — это принципиально другая архитектура,
//!      требующая тысяч строк и годы разработки даже у самой команды
//!      nouveau. Поэтому modesetting в этом файле сознательно
//!      ОГРАНИЧЕН архитектурами NV04..NV40 (`Architecture::CurieOrOlder`
//!      в терминах ниже — тут это "Fixed" ветка), а для всего более
//!      нового (в т.ч. вашей GT 520M/Fermi) `gpu mode`/`gpu info`
//!      честно объясняют, почему прямой вывод недоступен, и предлагают
//!      резервный путь через Bochs VBE (см. vbe.rs) — он работает только
//!      под QEMU/Bochs/VirtualBox с включённой стандартной VGA-картой,
//!      не на реальной NVIDIA.

use crate::pci;

/// NV_PMC_BOOT_0 — регистр с базовой информацией о ревизии чипа,
/// смещение 0x00000000 от BAR0. Документирован в nouveau как один из
/// первых регистров, которые вообще можно безопасно прочитать без риска
/// повесить карту (README nouveau/nvreg.h: "NV_PMC_BOOT_0 0x00000000").
const NV_PMC_BOOT_0: u32 = 0x00000000;

/// Классические архитектуры NVIDIA (до появления unified shaders в G80).
/// Деление и имена соответствуют кодовым именам nouveau
/// (nv04/nv10/nv20/nv30/nv40 в дереве drm/nouveau/dispnv04).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    /// NV04 (RIVA TNT/TNT2, ~1998-1999) — самая старая поддерживаемая
    /// генерация с классическими CRTC/RAMDAC регистрами.
    Nv04,
    /// NV10-NV1F (GeForce 256 .. GeForce4 MX, ~1999-2002).
    Nv10,
    /// NV20-NV2F (GeForce3/4 Ti, ~2001-2002).
    Nv20,
    /// NV30-NV3F (GeForce FX, ~2003-2004).
    Nv30,
    /// NV40-NV4F (GeForce 6/7, ~2004-2006) — последняя архитектура с
    /// классическим модесеттингом через прямые CRTC-регистры.
    Nv40,
    /// G80 и новее (GeForce 8xxx .. RTX) — EVO/NVDisplay, push-буферы,
    /// виртуальная память GPU. Сюда же попадает Fermi (GeForce
    /// 4xx/5xx, включая GT 520M) и все более новые архитектуры.
    /// Начиная с Turing (RTX 20xx) дополнительно требуется закрытая
    /// подписанная прошивка GSP.
    ModernUnsupported,
    /// Не удалось распознать по регистру (например, чип с 0 в BOOT_0,
    /// что означает "регистр недоступен/не читается").
    Unknown,
}

impl Architecture {
    pub fn name(&self) -> &'static str {
        match self {
            Architecture::Nv04 => "NV04 (RIVA TNT/TNT2 class)",
            Architecture::Nv10 => "NV10-NV1F (GeForce 256 .. GeForce4 MX class)",
            Architecture::Nv20 => "NV20-NV2F (GeForce3/4 Ti class)",
            Architecture::Nv30 => "NV30-NV3F (GeForce FX class)",
            Architecture::Nv40 => "NV40-NV4F (GeForce 6/7 class)",
            Architecture::ModernUnsupported => "G80+ (GeForce 8xxx and newer, incl. Fermi/Kepler/Turing+)",
            Architecture::Unknown => "unrecognized",
        }
    }

    /// true, если классический прямой modesetting (CRTC/RAMDAC-регистры)
    /// в принципе применим к этой архитектуре — то есть NV04..NV40.
    pub fn supports_legacy_modesetting(&self) -> bool {
        matches!(
            self,
            Architecture::Nv04 | Architecture::Nv10 | Architecture::Nv20 | Architecture::Nv30 | Architecture::Nv40
        )
    }
}

pub struct NouveauInfo {
    pub device: pci::PciDevice,
    pub chipset_id: u8,
    pub architecture: Architecture,
    pub bar0_mmio: u32,
}

/// Читает NV_PMC_BOOT_0 через BAR0 (MMIO) и извлекает "chipset ID" —
/// биты 20-27 (маска 0x0FF00000), задокументированные в nouveau как
/// основной идентификатор поколения чипа (например 0x04 для NV04,
/// 0x40-0x4F для семейства Curie/NV40, 0x50+ для Tesla/G80 и новее).
/// Ссылка на формат поля: drm/nouveau/nvkm/engine/device/base.c
/// (`nv_device_chipset`) и исторически более старый nv_str.c в
/// xf86-video-nv, где то же самое поле разбирается как
/// `(reg0 >> 20) & 0xff` для классических карт.
fn read_chipset_id(bar0: u32) -> u8 {
    let boot0 = unsafe { core::ptr::read_volatile((bar0 as usize + NV_PMC_BOOT_0 as usize) as *const u32) };
    ((boot0 >> 20) & 0xFF) as u8
}

fn classify_chipset(id: u8) -> Architecture {
    match id {
        0x00 => Architecture::Unknown,
        0x04 => Architecture::Nv04,
        0x05 => Architecture::Nv04, // NV05 — тот же семейство CRTC-регистров, что NV04.
        0x10..=0x1F => Architecture::Nv10,
        0x20..=0x2F => Architecture::Nv20,
        0x30..=0x3F => Architecture::Nv30,
        0x40..=0x4F => Architecture::Nv40,
        // 0x50 и выше — Tesla (G80) и все последующие архитектуры,
        // включая Fermi (GT 520M попадает именно сюда, chipset id в
        // районе 0xC0-0xCF для GF1xx-серии): EVO display engine,
        // несовместимо с классическими CRTC-регистрами ниже.
        _ => Architecture::ModernUnsupported,
    }
}

/// Определяет архитектуру уже найденного NVIDIA PCI-устройства (см.
/// gpu::detect()). Требует включённого memory space decode (bit 1
/// команды PCI) — включаем его сами перед чтением, как это делает
/// любой modesetting-код при инициализации.
pub fn detect(device: pci::PciDevice) -> NouveauInfo {
    pci::enable_memory_space(device.bus, device.slot, device.function);
    let bar0 = pci::read_bar_mmio(device.bus, device.slot, device.function, 0);
    let chipset_id = if bar0 != 0 {
        read_chipset_id(bar0)
    } else {
        0
    };
    let architecture = classify_chipset(chipset_id);

    NouveauInfo {
        device,
        chipset_id,
        architecture,
        bar0_mmio: bar0,
    }
}

// ==================== Классический modesetting (NV04..NV40) ====================
//
// Регистровые смещения ниже — это BAR0-относительные адреса блоков
// PCRTC0/PRAMDAC0/PRMVIO из открытого nouveau-заголовка
// drivers/gpu/drm/nouveau/dispnv04/nvreg.h (см. комментарии у каждой
// константы). Это НЕ проприетарная информация NVIDIA — это результат
// открытого реверс-инжиниринга, лицензированный MIT/X11.

/// PCRTC0 — блок регистров CRT-контроллера, offset 0x00600000 (nvreg.h:
/// `NV_PCRTC0_OFFSET`).
const NV_PCRTC0_OFFSET: u32 = 0x00600000;
/// PRAMDAC0 — блок регистров RAMDAC (генератор пиксельной частоты,
/// синхронизация), offset 0x00680000 (nvreg.h: `NV_PRAMDAC0_OFFSET`).
const NV_PRAMDAC0_OFFSET: u32 = 0x00680000;
/// PRMVIO — классический VGA-совместимый диапазон портов ввода-вывода,
/// отображённый в MMIO по offset 0x000C0000 (nvreg.h: `NV_PRMVIO0_OFFSET`).
/// Это те же самые логические регистры VGA Sequencer/CRTC (0x3C4/0x3D4 в
/// port I/O пространстве), но доступные через MMIO вместо портов —
/// используется, когда VGA I/O ports задизейблены/недоступны.
const NV_PRMVIO0_OFFSET: u32 = 0x000C0000;

/// Смещение регистра "CRTC framebuffer start address" внутри PCRTC0 —
/// nouveau называет его `NV_PCRTC_START` (dispnv04/nv04_crtc.c пишет в
/// него значение `fb_start` при каждом mode_set_base). У этого регистра
/// одинаковое смещение на всех архитектурах NV04..NV40.
const NV_PCRTC_START: u32 = 0x00000800;

#[inline]
unsafe fn mmio_read32(bar0: u32, offset: u32) -> u32 {
    core::ptr::read_volatile((bar0 as usize + offset as usize) as *const u32)
}

#[inline]
unsafe fn mmio_write32(bar0: u32, offset: u32, value: u32) {
    core::ptr::write_volatile((bar0 as usize + offset as usize) as *mut u32, value);
}

#[inline]
unsafe fn mmio_read8(bar0: u32, offset: u32) -> u8 {
    core::ptr::read_volatile((bar0 as usize + offset as usize) as *const u8)
}

#[inline]
unsafe fn mmio_write8(bar0: u32, offset: u32, value: u8) {
    core::ptr::write_volatile((bar0 as usize + offset as usize) as *mut u8, value);
}

/// VGA CRTC index/data пара, доступная через MMIO (PRMVIO), а не через
/// классические порты 0x3D4/0x3D5 — нужна, потому что на PCI/AGP-картах
/// (в отличие от встроенного в чипсет VGA) порты ввода-вывода могут быть
/// не смаплены на область 0x3D4 у конкретной шины, а MMIO-alias — всегда
/// доступен, если PCI Memory Space Enable включён (см. detect() выше).
/// Адреса самих индекс/дата регистров внутри PRMVIO — стандартные VGA
/// offsets 0x3D4/0x3D5, задокументированные в nvreg.h как
/// `NV_CIO_CRX__COLOR`/`NV_CIO_CR__COLOR`.
const NV_CIO_CRX_COLOR: u32 = 0x3D4;
const NV_CIO_CR_COLOR: u32 = 0x3D5;

unsafe fn write_vga_crtc(bar0: u32, index: u8, value: u8) {
    mmio_write8(bar0, NV_PRMVIO0_OFFSET + NV_CIO_CRX_COLOR, index);
    mmio_write8(bar0, NV_PRMVIO0_OFFSET + NV_CIO_CR_COLOR, value);
}

unsafe fn read_vga_crtc(bar0: u32, index: u8) -> u8 {
    mmio_write8(bar0, NV_PRMVIO0_OFFSET + NV_CIO_CRX_COLOR, index);
    mmio_read8(bar0, NV_PRMVIO0_OFFSET + NV_CIO_CR_COLOR)
}

/// Результат попытки установить видеорежим на классической архитектуре.
pub enum LegacyModesetResult {
    /// Режим "установлен" (регистры записаны без явных признаков сбоя),
    /// но, как честно указано в module-level документации, БЕЗ
    /// тестирования на реальном железе нельзя дать стопроцентную
    /// гарантию визуального результата.
    RegistersWritten,
    /// Архитектура этой карты не поддерживает классический modesetting
    /// (G80 и новее — нужен EVO/NVDisplay).
    UnsupportedArchitecture,
}

/// Экспериментальный modesetting для NV04..NV40: программирует CRTC
/// framebuffer base address на уже настроенный BIOS'ом видеорежим
/// (меняем только адрес кадрового буфера, НЕ полную последовательность
/// timings/PLL — это осознанное упрощение, т.к. настройка PLL/timings
/// требует данных VBIOS конкретной карты, которых у нас нет). Это тот
/// же принцип, которым в первые секунды после POST большинство
/// firmware-independent framebuffer-драйверов (efifb, vesafb) в принципе
/// работают: не переключать видеорежим самостоятельно с нуля, а
/// переиспользовать то, что уже установил BIOS/VBIOS, только указав
/// новый адрес буфера в системной памяти или другом месте VRAM.
///
/// # Safety
/// Вызывающий должен убедиться, что `info.architecture.supports_legacy_modesetting()`
/// и что `fb_offset` — валидный, выровненный адрес внутри VRAM карты.
pub unsafe fn set_framebuffer_start(info: &NouveauInfo, fb_offset: u32) -> LegacyModesetResult {
    if !info.architecture.supports_legacy_modesetting() {
        return LegacyModesetResult::UnsupportedArchitecture;
    }

    let bar0 = info.bar0_mmio;
    mmio_write32(bar0, NV_PCRTC0_OFFSET + NV_PCRTC_START, fb_offset);

    // "Repaint" бит в CRE_RPC1 (индекс 0x1A) должен быть согласован с
    // разрешением экрана (>=1280 по горизонтали требует другого
    // значения) — здесь читаем текущее значение и не трогаем эти биты,
    // т.к. без данных VBIOS о реальном текущем режиме безопаснее не
    // менять то, что уже настроено POST-кодом карты.
    let _ = read_vga_crtc(bar0, 0x1A);

    LegacyModesetResult::RegistersWritten
}
