//! Загрузка кастомного VGA-шрифта: дозаписываем битмапы кириллических
//! букв (кодировка CP866, коды 0x80-0xAF и 0xE0-0xF1) поверх штатного
//! VGA-шрифта, не трогая латиницу/цифры/псевдографику.
//!
//! В текстовом режиме VGA символьные шрифты хранятся в plane 2 видеопамяти
//! (обычная "текстовая" память 0xB8000 в этот момент отображает planes 0
//! и 1). Чтобы дозаписать свои глифы, нужно временно переключить видеокарту
//! так, чтобы 0xB8000 указывал на plane 2, записать байты глифов, а потом
//! вернуть карту в обычный текстовый режим. Это классическая, десятилетиями
//! проверенная процедура — алгоритм и значения регистров взяты из
//! стандартного рецепта VGA Fonts (см. wiki.osdev.org/VGA_Fonts и форум
//! OSDev, тема "Change VGA font").

use crate::font_cyrillic::CYRILLIC_GLYPHS;
use crate::font_full::FULL_GLYPHS;
use crate::port::{inb, outb};

/// Возвращает 16-байтную битовую маску глифа для программного графического
/// рендерера (renderer.rs) — читает из статической таблицы, сгенерированной
/// tools/gen_font_full.py, а не из VGA text-mode памяти (быстрее, не
/// требует переключения plane на каждый символ).
pub fn read_glyph(byte: u8) -> [u8; 16] {
    FULL_GLYPHS[byte as usize]
}

const VGA_SEQ_INDEX: u16 = 0x3C4;
const VGA_SEQ_DATA: u16 = 0x3C5;
const VGA_GC_INDEX: u16 = 0x3CE;
const VGA_GC_DATA: u16 = 0x3CF;

const VGA_SEQ_MAP_MASK_REG: u8 = 0x02;
const VGA_SEQ_CHARSET_REG: u8 = 0x03;
const VGA_SEQ_MEMORY_MODE_REG: u8 = 0x04;
const VGA_GC_READ_MAP_SELECT_REG: u8 = 0x04;
const VGA_GC_GRAPHICS_MODE_REG: u8 = 0x05;
const VGA_GC_MISC_REG: u8 = 0x06;

const FONT_MEMORY_BASE: usize = 0xB8000;
const BYTES_PER_GLYPH: usize = 16;
const BYTES_SKIP: usize = 16; // VGA резервирует 32 байта на глиф, используем первые 16

unsafe fn vga_write_reg(index_port: u16, data_port: u16, reg: u8, val: u8) {
    outb(index_port, reg);
    outb(data_port, val);
}

unsafe fn vga_read_reg(index_port: u16, data_port: u16, reg: u8) -> u8 {
    outb(index_port, reg);
    inb(data_port)
}

pub fn install_cyrillic_font() {
    unsafe {
        with_plane2_access(|| {
            // Записываем только нужные нам глифы кириллицы, не трогая
            // остальные — используется при обычной загрузке ядра, когда
            // штатный ASCII-шрифт BIOS ещё цел в памяти карты.
            for &(code, glyph) in CYRILLIC_GLYPHS {
                write_glyph(code, &glyph);
            }
        });
    }
}

/// Полностью перезаписывает все 256 глифов (ASCII + кириллица) из
/// FULL_GLYPHS. Нужна после возврата из графического режима Bochs VBE:
/// пока карта работала в 32bpp linear framebuffer, память VGA-шрифта
/// (та же физическая VRAM, что видна как plane 2 в текстовом режиме)
/// была перезаписана произвольными пиксельными данными нашего рендерера
/// — старый ASCII-шрифт, once загруженный BIOS при самом первом старте,
/// без этого восстановления был бы безвозвратно утерян, и текст в 80x25
/// режиме отображался бы пустыми/мусорными клетками.
pub fn restore_full_font() {
    unsafe {
        with_plane2_access(|| {
            for code in 0..256usize {
                write_glyph(code as u8, &FULL_GLYPHS[code]);
            }
        });
    }
}

unsafe fn write_glyph(code: u8, glyph: &[u8; 16]) {
    let base = FONT_MEMORY_BASE + (code as usize) * (BYTES_PER_GLYPH + BYTES_SKIP);
    for (row, &byte) in glyph.iter().take(BYTES_PER_GLYPH).enumerate() {
        core::ptr::write_volatile((base + row) as *mut u8, byte);
    }
}

unsafe fn with_plane2_access<F: FnOnce()>(f: F) {
    // --- Открываем доступ к plane 2 (там живут шрифты) ---
    vga_write_reg(VGA_SEQ_INDEX, VGA_SEQ_DATA, VGA_SEQ_MAP_MASK_REG, 0x04);
    vga_write_reg(VGA_SEQ_INDEX, VGA_SEQ_DATA, VGA_SEQ_CHARSET_REG, 0x00);

    let mem_mode = vga_read_reg(VGA_SEQ_INDEX, VGA_SEQ_DATA, VGA_SEQ_MEMORY_MODE_REG);
    vga_write_reg(VGA_SEQ_INDEX, VGA_SEQ_DATA, VGA_SEQ_MEMORY_MODE_REG, 0x06);

    vga_write_reg(VGA_GC_INDEX, VGA_GC_DATA, VGA_GC_READ_MAP_SELECT_REG, 0x02);
    let graphics_mode = vga_read_reg(VGA_GC_INDEX, VGA_GC_DATA, VGA_GC_GRAPHICS_MODE_REG);
    vga_write_reg(VGA_GC_INDEX, VGA_GC_DATA, VGA_GC_GRAPHICS_MODE_REG, 0x00);
    vga_write_reg(VGA_GC_INDEX, VGA_GC_DATA, VGA_GC_MISC_REG, 0x0C);

    f();

    // --- Возвращаем нормальный текстовый режим доступа к видеопамяти ---
    vga_write_reg(VGA_SEQ_INDEX, VGA_SEQ_DATA, VGA_SEQ_MAP_MASK_REG, 0x03);
    vga_write_reg(VGA_SEQ_INDEX, VGA_SEQ_DATA, VGA_SEQ_MEMORY_MODE_REG, mem_mode);
    vga_write_reg(VGA_GC_INDEX, VGA_GC_DATA, VGA_GC_READ_MAP_SELECT_REG, 0x00);
    vga_write_reg(VGA_GC_INDEX, VGA_GC_DATA, VGA_GC_GRAPHICS_MODE_REG, graphics_mode);
    vga_write_reg(VGA_GC_INDEX, VGA_GC_DATA, VGA_GC_MISC_REG, 0x0C);
}
