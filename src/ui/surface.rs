//! Двумерная Surface буферизация окон и оверлеев с поддержкой Damage Region

use crate::renderer::Rect;
use alloc::vec;
use alloc::vec::Vec;

pub const MAX_DAMAGE_RECTS: usize = 32;

#[derive(Clone, Copy, Debug)]
pub struct DamageList {
    pub rects: [Rect; MAX_DAMAGE_RECTS],
    pub count: usize,
    pub full_redraw: bool,
}

impl DamageList {
    pub fn new() -> Self {
        DamageList {
            rects: [Rect::new(0, 0, 0, 0); MAX_DAMAGE_RECTS],
            count: 0,
            full_redraw: false,
        }
    }

    pub fn add(&mut self, rect: Rect) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        if self.full_redraw {
            return;
        }
        if self.count >= MAX_DAMAGE_RECTS {
            self.full_redraw = true;
            return;
        }
        self.rects[self.count] = rect;
        self.count += 1;
    }

    pub fn clear(&mut self) {
        self.count = 0;
        self.full_redraw = false;
    }
}

#[derive(Clone, Debug)]
pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub damage: DamageList,
}

impl Surface {
    pub fn new(width: u32, height: u32) -> Self {
        let mut damage = DamageList::new();
        damage.add(Rect::new(0, 0, width, height));
        Surface {
            width,
            height,
            damage,
        }
    }

    pub fn mark_dirty(&mut self, rect: Rect) {
        self.damage.add(rect);
    }

    pub fn clear_damage(&mut self) {
        self.damage.clear();
    }
}

#[derive(Clone, Debug)]
pub struct WallpaperSurface {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
    pub valid: bool,
}

impl WallpaperSurface {
    pub fn new(width: u32, height: u32) -> Self {
        WallpaperSurface {
            width,
            height,
            pixels: vec![0u32; (width as usize) * (height as usize)],
            valid: false,
        }
    }

    pub fn invalidate(&mut self) {
        self.valid = false;
    }
}
