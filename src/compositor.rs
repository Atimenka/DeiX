//! Графический композитор окон DeiX OS (Back-to-Front).

use alloc::vec::Vec;
use crate::spinlock::SpinLock;

pub struct Surface {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub buffer: Vec<u32>,
    pub z_index: i32,
    pub visible: bool,
}

pub struct Compositor {
    pub surfaces: Vec<Surface>,
    pub next_id: u32,
}

impl Compositor {
    pub fn new() -> Self {
        Self {
            surfaces: Vec::new(),
            next_id: 1,
        }
    }

    pub fn create_surface(&mut self, width: u32, height: u32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let buf_size = (width * height) as usize;
        let mut buffer = Vec::with_capacity(buf_size);
        buffer.resize(buf_size, 0xFF1E1E2E); // По умолчанию цвет фона

        self.surfaces.push(Surface {
            id,
            x: 50,
            y: 50,
            width,
            height,
            buffer,
            z_index: id as i32,
            visible: true,
        });

        id
    }

    pub fn composite(&mut self, fb_addr: usize, fb_width: u32, fb_height: u32, fb_pitch: usize) {
        if fb_addr == 0 {
            return;
        }

        // Сортировка поверхностей back-to-front (по z_index)
        self.surfaces.sort_by_key(|s| s.z_index);

        let fb_ptr = fb_addr as *mut u32;

        for surface in self.surfaces.iter().filter(|s| s.visible) {
            for sy in 0..surface.height {
                let py = surface.y + sy as i32;
                if py < 0 || py >= fb_height as i32 {
                    continue;
                }
                for sx in 0..surface.width {
                    let px = surface.x + sx as i32;
                    if px < 0 || px >= fb_width as i32 {
                        continue;
                    }

                    let src_idx = (sy * surface.width + sx) as usize;
                    if src_idx < surface.buffer.len() {
                        let color = surface.buffer[src_idx];
                        let dst_offset = (py as usize * (fb_pitch / 4)) + px as usize;
                        unsafe {
                            *fb_ptr.add(dst_offset) = color;
                        }
                    }
                }
            }
        }
    }
}

pub static COMPOSITOR: SpinLock<Compositor> = SpinLock::new(Compositor {
    surfaces: Vec::new(),
    next_id: 1,
});

pub fn init() {
    let mut comp = COMPOSITOR.lock();
    *comp = Compositor::new();
}
