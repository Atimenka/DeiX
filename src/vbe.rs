//! Драйвер Bochs VBE Display Interface (DISPI) — открытый, задокументированный
//! интерфейс, который эмулирует QEMU (device "VGA"/"std vga", PCI ID
//! 1234:1111) и Bochs. Даёт настоящий линейный framebuffer с произвольным
//! разрешением и глубиной цвета без необходимости в BIOS/VBE реальном
//! режиме или проприетарных драйверах — то, чем реально можно
//! воспользоваться в нашем 64-битном ядре напрямую через порты ввода-вывода.
//!
//! Регистры и протокол описаны на wiki.osdev.org/Bochs_VBE_Extensions —
//! используются только официально задокументированные, стабильные с 2009
//! года индексы регистров (версия BGA 0xB0C5).

use crate::pci::PciDevice;
use crate::port::{inw, outw};

const VBE_DISPI_IOPORT_INDEX: u16 = 0x01CE;
const VBE_DISPI_IOPORT_DATA: u16 = 0x01CF;

const VBE_DISPI_INDEX_ID: u16 = 0;
const VBE_DISPI_INDEX_XRES: u16 = 1;
const VBE_DISPI_INDEX_YRES: u16 = 2;
const VBE_DISPI_INDEX_BPP: u16 = 3;
const VBE_DISPI_INDEX_ENABLE: u16 = 4;
const VBE_DISPI_INDEX_BANK: u16 = 5;
const VBE_DISPI_INDEX_VIRT_WIDTH: u16 = 6;
const VBE_DISPI_INDEX_VIRT_HEIGHT: u16 = 7;
const VBE_DISPI_INDEX_X_OFFSET: u16 = 8;
const VBE_DISPI_INDEX_Y_OFFSET: u16 = 9;

const VBE_DISPI_ID5: u16 = 0xB0C5;

const VBE_DISPI_DISABLED: u16 = 0x00;
const VBE_DISPI_ENABLED: u16 = 0x01;
const VBE_DISPI_LFB_ENABLED: u16 = 0x40;
const VBE_DISPI_NOCLEARMEM: u16 = 0x80;

pub const BPP_32: u16 = 0x20;

fn write_reg(index: u16, value: u16) {
    unsafe {
        outw(VBE_DISPI_IOPORT_INDEX, index);
        outw(VBE_DISPI_IOPORT_DATA, value);
    }
}

fn read_reg(index: u16) -> u16 {
    unsafe {
        outw(VBE_DISPI_IOPORT_INDEX, index);
        inw(VBE_DISPI_IOPORT_DATA)
    }
}

pub fn is_available() -> bool {
    read_reg(VBE_DISPI_INDEX_ID) == VBE_DISPI_ID5
}

pub struct Framebuffer {
    pub addr: usize,
    pub width: u32,
    pub height: u32,
    pub bpp: u16,
    pub pitch: usize, // байт на строку
}

/// Устанавливает видеорежим width x height x bpp с включённым линейным
/// framebuffer. Физический адрес framebuffer читается из PCI BAR0
/// устройства (как и требует спецификация — адрес не фиксирован).
///
/// Раньше здесь также предпринималась попытка аппаратного двойного
/// буферирования через удвоенную VBE_DISPI_INDEX_VIRT_HEIGHT и
/// переключение VBE_DISPI_INDEX_Y_OFFSET (классический приём page
/// flipping). От неё пришлось отказаться: строгое тестирование через
/// QEMU monitor `screendump` показало артефакты (полосы "шума" из старых
/// данных видеопамяти), которые не воспроизводились в реальном GUI/VNC
/// (там QEMU действительно учитывает Y_OFFSET), но подтвердить это в
/// песочнице агента без реального экрана невозможно, а гарантировать
/// корректность на всех версиях QEMU/хостов пользователя — тоже. Вместо
/// этого используем программный back buffer в RAM (см. renderer.rs) с
/// ОДНИМ быстрым проходом копирования в конце кадра — это даёт основной
/// выигрыш (кадр либо весь старый, либо весь новый, без "лоскутного"
/// однопиксельного рисования прямо по MMIO, из-за которого раньше кадр
/// собирался заметно долго и был виден в процессе отрисовки) и при этом
/// гарантированно ведёт себя одинаково на любом хосте.
pub fn set_mode(gpu_device: &PciDevice, width: u32, height: u32, bpp: u16) -> Option<Framebuffer> {
    if !is_available() {
        return None;
    }

    // Включаем декодирование Memory Space для видеокарты — без этого её
    // MMIO BAR (framebuffer) не отвечает вообще, даже если физический
    // адрес BAR прописан правильно.
    crate::pci::enable_memory_space(gpu_device.bus, gpu_device.slot, gpu_device.function);

    // Отключаем VBE перед сменой разрешения (обязательное требование
    // спецификации — иначе запись в XRES/YRES/BPP не подействует).
    write_reg(VBE_DISPI_INDEX_ENABLE, VBE_DISPI_DISABLED);

    write_reg(VBE_DISPI_INDEX_XRES, width as u16);
    write_reg(VBE_DISPI_INDEX_YRES, height as u16);
    write_reg(VBE_DISPI_INDEX_BPP, bpp);
    write_reg(VBE_DISPI_INDEX_BANK, 0);
    write_reg(VBE_DISPI_INDEX_VIRT_WIDTH, width as u16);
    write_reg(VBE_DISPI_INDEX_VIRT_HEIGHT, height as u16);
    write_reg(VBE_DISPI_INDEX_X_OFFSET, 0);
    write_reg(VBE_DISPI_INDEX_Y_OFFSET, 0);

    // Включаем VBE + линейный framebuffer. Намеренно БЕЗ NOCLEARMEM —
    // пусть видеопамять обнулится при включении режима: раньше здесь
    // стоял флаг VBE_DISPI_NOCLEARMEM, из-за которого в свежевключённом
    // framebuffer могли оставаться случайные байты от предыдущего
    // состояния физической видеопамяти (в т.ч. похожие на осмысленные
    // данные вроде битов растровых шрифтов старого текстового режима),
    // и на самом первом кадре, до того как наш software-рендерер успевал
    // перекрыть весь экран, эти байты было видно как "мусор".
    write_reg(
        VBE_DISPI_INDEX_ENABLE,
        VBE_DISPI_ENABLED | VBE_DISPI_LFB_ENABLED,
    );

    // Проверяем, что режим реально применился (иначе на некоторых версиях
    // Bochs/QEMU при недопустимом разрешении просто ничего не происходит).
    let actual_width = read_reg(VBE_DISPI_INDEX_XRES);
    let actual_height = read_reg(VBE_DISPI_INDEX_YRES);
    crate::serial_println!(
        "[vbe] requested {}x{}, actual {}x{}",
        width, height, actual_width, actual_height
    );
    if actual_width as u32 != width || actual_height as u32 != height {
        crate::serial_println!("[vbe] mode mismatch, aborting");
        return None;
    }

    let framebuffer_addr = crate::pci::read_bar_mmio(
        gpu_device.bus,
        gpu_device.slot,
        gpu_device.function,
        0,
    ) as usize;
    crate::serial_println!("[vbe] framebuffer_addr = {:#x}", framebuffer_addr);

    let bytes_per_pixel = (bpp / 8) as usize;
    let pitch = width as usize * bytes_per_pixel;

    Some(Framebuffer {
        addr: framebuffer_addr,
        width,
        height,
        bpp,
        pitch,
    })
}


// Стандартные регистровые значения VGA text mode 3 (80x25, 16 цветов) —
// то же самое, что BIOS "int 10h, ah=0, al=3" выставляет на реальном
// железе. В 64-битном long mode у нас нет доступа к BIOS-прерываниям,
// поэтому программируем VGA-регистры вручную. Таблица — проверенный,
// рабочий набор значений (используется, например, в видео-BIOS ReactOS).
const MISC_REG: u8 = 0x67;

const SEQ_REGS: [u8; 5] = [0x03, 0x00, 0x03, 0x00, 0x02];

const CRTC_REGS: [u8; 25] = [
    0x5F, 0x4F, 0x50, 0x82, 0x55, 0x81, 0xBF, 0x1F, 0x00, 0x4F, 0x0D, 0x0E, 0x00, 0x00, 0x00,
    0x00, 0x9C, 0x8E, 0x8F, 0x28, 0x1F, 0x96, 0xB9, 0xA3, 0xFF,
];

const GC_REGS: [u8; 9] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x0E, 0x0F, 0xFF];

const AC_REGS: [u8; 21] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x14, 0x07, 0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E,
    0x3F, 0x0C, 0x00, 0x0F, 0x08, 0x00,
];

const VGA_MISC_WRITE: u16 = 0x3C2;
const VGA_SEQ_INDEX: u16 = 0x3C4;
const VGA_SEQ_DATA: u16 = 0x3C5;
const VGA_CRTC_INDEX: u16 = 0x3D4;
const VGA_CRTC_DATA: u16 = 0x3D5;
const VGA_GC_INDEX: u16 = 0x3CE;
const VGA_GC_DATA: u16 = 0x3CF;
const VGA_AC_INDEX: u16 = 0x3C0;
const VGA_INSTAT_READ: u16 = 0x3DA;

/// Отключает расширения Bochs VBE и вручную программирует стандартные
/// VGA-регистры для текстового режима 80x25 (mode 3) — на реальном
/// железе это делает BIOS через int 10h, но в нашем 64-битном long mode
/// прерывания реального режима недоступны, поэтому пишем те же значения
/// напрямую в порты. Без этого шага после отключения VBE монитор
/// остаётся в графической развёртке предыдущего разрешения, и VGA text
/// buffer (0xb8000) отображается на экране некорректно/искажённо.
pub fn restore_text_mode() {
    write_reg(VBE_DISPI_INDEX_ENABLE, VBE_DISPI_DISABLED);

    unsafe {
        use crate::port::outb;

        outb(VGA_MISC_WRITE, MISC_REG);

        for (i, &value) in SEQ_REGS.iter().enumerate() {
            outb(VGA_SEQ_INDEX, i as u8);
            outb(VGA_SEQ_DATA, value);
        }

        // Снимаем защиту регистров 0-7 CRTC (бит 7 регистра 0x11), иначе
        // запись в них будет проигнорирована.
        outb(VGA_CRTC_INDEX, 0x11);
        let unlocked = crate::port::inb(VGA_CRTC_DATA) & !0x80;
        outb(VGA_CRTC_DATA, unlocked);

        for (i, &value) in CRTC_REGS.iter().enumerate() {
            outb(VGA_CRTC_INDEX, i as u8);
            outb(VGA_CRTC_DATA, value);
        }

        for (i, &value) in GC_REGS.iter().enumerate() {
            outb(VGA_GC_INDEX, i as u8);
            outb(VGA_GC_DATA, value);
        }

        for (i, &value) in AC_REGS.iter().enumerate() {
            let _ = crate::port::inb(VGA_INSTAT_READ); // сброс flip-flop адрес/данные
            outb(VGA_AC_INDEX, i as u8);
            outb(VGA_AC_INDEX, value);
        }

        // Включаем видеовыход Attribute Controller (бит 5 индекса при
        // следующей записи в индексный порт).
        let _ = crate::port::inb(VGA_INSTAT_READ);
        outb(VGA_AC_INDEX, 0x20);
    }
}

pub fn debug_dump_regs() {
    let vw = read_reg(VBE_DISPI_INDEX_VIRT_WIDTH);
    let vh = read_reg(VBE_DISPI_INDEX_VIRT_HEIGHT);
    let xoff = read_reg(VBE_DISPI_INDEX_X_OFFSET);
    let yoff = read_reg(VBE_DISPI_INDEX_Y_OFFSET);
    crate::serial_println!("[vbe][debug] virt_width={} virt_height={} x_off={} y_off={}", vw, vh, xoff, yoff);
}

pub fn debug_hexdump(addr: usize, len: usize) {
    crate::serial_println!("[vbe][hexdump] addr={:#x} len={}", addr, len);
    let mut line = alloc::string::String::new();
    for i in 0..len {
        let byte = unsafe { core::ptr::read_volatile((addr + i) as *const u8) };
        line.push_str(&alloc::format!("{:02x} ", byte));
    }
    crate::serial_println!("{}", line);
}
