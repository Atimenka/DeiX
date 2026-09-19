//! Драйвер обнаружения VBE Framebuffer из BIOS/EBDA таблиц.

use crate::vbe::Framebuffer;

#[repr(C, packed)]
struct VbeModeInfoBlock {
    mode_attributes: u16,
    win_a_attributes: u8,
    win_b_attributes: u8,
    win_granularity: u16,
    win_size: u16,
    win_a_segment: u16,
    win_b_segment: u16,
    win_func_ptr: u32,
    bytes_per_scanline: u16,  // offset 16
    x_resolution: u16,        // offset 18
    y_resolution: u16,        // offset 20
    x_char_size: u8,
    y_char_size: u8,
    number_of_planes: u8,
    bits_per_pixel: u8,       // offset 25
    number_of_banks: u8,
    memory_model: u8,
    bank_size: u8,
    number_of_image_pages: u8,
    reserved1: u8,
    red_mask_size: u8,
    red_field_position: u8,
    green_mask_size: u8,
    green_field_position: u8,
    blue_mask_size: u8,
    blue_field_position: u8,
    rsvd_mask_size: u8,
    rsvd_field_position: u8,
    direct_color_mode_info: u8,
    phys_base_ptr: u32,       // offset 40
    reserved2: u32,
    reserved3: u16,
}

pub fn detect_bios_framebuffer() -> Option<Framebuffer> {
    // Поиск VBE Mode Info Block в областях BIOS (0x500-0x600) и EBDA (0x9FC00-0xA0000)
    let search_ranges = [0x500..0x600, 0x9FC00..0xA0000];

    for range in search_ranges {
        let mut ptr = range.start as *const u8;
        let end = range.end as *const u8;

        while ptr < end {
            unsafe {
                let magic = core::ptr::read_unaligned(ptr as *const u32);
                // "VESA" (0x41534556) или "VBE2" (0x32454256)
                if magic == 0x41534556 || magic == 0x32454256 {
                    let info_ptr = ptr as *const VbeModeInfoBlock;
                    let phys_base = core::ptr::read_unaligned(core::ptr::addr_of!((*info_ptr).phys_base_ptr));
                    let width = core::ptr::read_unaligned(core::ptr::addr_of!((*info_ptr).x_resolution)) as u32;
                    let height = core::ptr::read_unaligned(core::ptr::addr_of!((*info_ptr).y_resolution)) as u32;
                    let pitch = core::ptr::read_unaligned(core::ptr::addr_of!((*info_ptr).bytes_per_scanline)) as usize;
                    let bpp = core::ptr::read_unaligned(core::ptr::addr_of!((*info_ptr).bits_per_pixel)) as u16;

                    if phys_base != 0 && width > 0 && height > 0 {
                        crate::serial_println!("[vbe_bios] Найден BIOS framebuffer: {}x{} @ {:#x}", width, height, phys_base);
                        return Some(Framebuffer {
                            addr: phys_base as usize,
                            width,
                            height,
                            bpp,
                            pitch,
                        });
                    }
                }
                ptr = ptr.add(16);
            }
        }
    }

    None
}
