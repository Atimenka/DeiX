//! Программный 2D-рендерер поверх линейного framebuffer (полученного через
//! драйвер Bochs VBE, см. vbe.rs).
//!
//! Включает:
//! * Двойную буферизацию (back buffer в RAM -> VBE MMIO)
//! * Поддержку Dirty Rectangle / Damage Tracking для частичной перерисовки
//! * Альфа-блендинг, размытие акрила (acrylic box blur) и мягкие тени
//! * Примитивы: линии, закруглённые прямоугольники, закрашенные круги, градиенты, векторные иконки.

use crate::vbe::Framebuffer;
use alloc::vec;
use alloc::vec::Vec;

fn isqrt(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Rect { x, y, w, h }
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.w as i32).max(other.x + other.w as i32);
        let y1 = (self.y + self.h as i32).max(other.y + other.h as i32);
        Rect {
            x: x0,
            y: y0,
            w: (x1 - x0).max(0) as u32,
            h: (y1 - y0).max(0) as u32,
        }
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.x + other.w as i32
            && self.x + self.w as i32 > other.x
            && self.y < other.y + other.h as i32
            && self.y + self.h as i32 > other.y
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color(pub u32); // формат 0x00RRGGBB

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color(((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
    }

    pub const fn from_u32(raw: u32) -> Color {
        Color(raw & 0x00FF_FFFF)
    }

    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const RED: Color = Color::rgb(239, 68, 68);
    pub const GREEN: Color = Color::rgb(34, 197, 94);
    pub const BLUE: Color = Color::rgb(59, 130, 246);
    pub const GRAY: Color = Color::rgb(148, 163, 184);
    pub const DARK_GRAY: Color = Color::rgb(30, 41, 59);
    pub const LIGHT_GRAY: Color = Color::rgb(226, 232, 240);
    pub const DESKTOP_BLUE: Color = Color::rgb(15, 23, 42);
    pub const TITLEBAR_ACTIVE: Color = Color::rgb(30, 41, 59);
    pub const TITLEBAR_INACTIVE: Color = Color::rgb(15, 23, 42);
    pub const YELLOW: Color = Color::rgb(245, 158, 11);
    pub const CYAN: Color = Color::rgb(6, 182, 212);
    pub const SHADOW: Color = Color::rgb(5, 5, 10);
    pub const ACCENT: Color = Color::rgb(99, 102, 241); // Indigo

    pub fn lerp(self, other: Color, t: u8) -> Color {
        let t = t as i32;
        let (r1, g1, b1) = self.components();
        let (r2, g2, b2) = other.components();
        let r = r1 + (r2 - r1) * t / 255;
        let g = g1 + (g2 - g1) * t / 255;
        let b = b1 + (b2 - b1) * t / 255;
        Color::rgb(r as u8, g as u8, b as u8)
    }

    pub fn alpha_blend(self, bg: Color, alpha: u8) -> Color {
        if alpha == 255 {
            return self;
        }
        if alpha == 0 {
            return bg;
        }
        bg.lerp(self, alpha)
    }

    #[inline]
    fn components(self) -> (i32, i32, i32) {
        (
            ((self.0 >> 16) & 0xFF) as i32,
            ((self.0 >> 8) & 0xFF) as i32,
            (self.0 & 0xFF) as i32,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconType {
    Terminal,
    Files,
    TaskManager,
    Display,
    About,
    Power,
    Restart,
    Settings,
    Check,
    Browser,
    Theme,
    Wifi,
    Volume,
    Search,
    Bookmark,
    Home,
    Reload,
    ArrowLeft,
    ArrowRight,
}

pub struct Renderer {
    fb: Framebuffer,
    pub back_buffer: Vec<u32>,
    damage: crate::ui::surface::DamageList,
}

impl Renderer {
    pub fn new(fb: Framebuffer) -> Self {
        let pixel_count = (fb.width as usize) * (fb.height as usize);
        let back_buffer = vec![0u32; pixel_count];
        Renderer {
            fb,
            back_buffer,
            damage: crate::ui::surface::DamageList::new(),
        }
    }

    pub fn width(&self) -> u32 {
        self.fb.width
    }

    pub fn height(&self) -> u32 {
        self.fb.height
    }

    pub fn add_damage(&mut self, rect: Rect) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        self.damage.add(rect);
    }

    pub fn clear_damage(&mut self) {
        self.damage.clear();
    }

    #[inline]
    pub fn put_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 || x as u32 >= self.fb.width || y as u32 >= self.fb.height {
            return;
        }
        let idx = y as usize * self.fb.width as usize + x as usize;
        self.back_buffer[idx] = color.0;
    }

    #[inline]
    pub fn put_pixel_alpha(&mut self, x: i32, y: i32, color: Color, alpha: u8) {
        if x < 0 || y < 0 || x as u32 >= self.fb.width || y as u32 >= self.fb.height {
            return;
        }
        let idx = y as usize * self.fb.width as usize + x as usize;
        let bg = Color(self.back_buffer[idx]);
        self.back_buffer[idx] = color.alpha_blend(bg, alpha).0;
    }

    /// Быстрый `memcpy` из RAM back buffer в MMIO Framebuffer только по списку повреждённых областей DamageList.
    pub fn present(&mut self) {
        if !self.damage.full_redraw && self.damage.count > 0 {
            for i in 0..self.damage.count {
                let r = self.damage.rects[i];
                let y0 = r.y.max(0) as usize;
                let y1 = ((r.y + r.h as i32).min(self.fb.height as i32)).max(0) as usize;
                let x0 = r.x.max(0) as usize;
                let x1 = ((r.x + r.w as i32).min(self.fb.width as i32)).max(0) as usize;

                if y0 >= y1 || x0 >= x1 {
                    continue;
                }

                for row in y0..y1 {
                    let row_offset = row * self.fb.width as usize;
                    let src = &self.back_buffer[row_offset + x0..row_offset + x1];
                    let dst = (self.fb.addr + row * self.fb.pitch + x0 * 4) as *mut u32;
                    unsafe {
                        core::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
                    }
                }
            }
            self.damage.clear();
            return;
        }

        // Полное копирование кадра при смене видеорежима или полном перерисовывании
        for row in 0..self.fb.height as usize {
            let row_offset = row * self.fb.width as usize;
            let src = &self.back_buffer[row_offset..row_offset + self.fb.width as usize];
            let dst = (self.fb.addr + row * self.fb.pitch) as *mut u32;
            unsafe {
                core::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
            }
        }

        self.damage.clear();
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w as i32).min(self.fb.width as i32);
        let y1 = (y + h as i32).min(self.fb.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let row_width = self.fb.width as usize;
        for row in y0..y1 {
            let start = row as usize * row_width + x0 as usize;
            let end = start + (x1 - x0) as usize;
            self.back_buffer[start..end].fill(color.0);
        }
        self.add_damage(Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32));
    }

    pub fn fill_rect_alpha(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color, alpha: u8) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w as i32).min(self.fb.width as i32);
        let y1 = (y + h as i32).min(self.fb.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let row_width = self.fb.width as usize;
        for row in y0..y1 {
            let start = row as usize * row_width + x0 as usize;
            let end = start + (x1 - x0) as usize;
            for px in &mut self.back_buffer[start..end] {
                let bg = Color(*px);
                *px = color.alpha_blend(bg, alpha).0;
            }
        }
        self.add_damage(Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32));
    }

    /// Оптимизированный акриловый эффекта размытия фонового изображения (Acrylic Box Blur)
    pub fn apply_blur_rect(&mut self, x: i32, y: i32, w: u32, h: u32, radius: usize) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x + w as i32).min(self.fb.width as i32)).max(0) as usize;
        let y1 = ((y + h as i32).min(self.fb.height as i32)).max(0) as usize;
        if x0 >= x1 || y0 >= y1 || radius == 0 {
            return;
        }
        let stride = self.fb.width as usize;
        let r = radius.min(3);

        for row in (y0..y1).step_by(2) {
            for col in (x0..x1).step_by(2) {
                let mut sum_r = 0u32;
                let mut sum_g = 0u32;
                let mut sum_b = 0u32;
                let mut count = 0u32;

                let nr_start = row.saturating_sub(r);
                let nr_end = (row + r + 1).min(y1);
                let nc_start = col.saturating_sub(r);
                let nc_end = (col + r + 1).min(x1);

                for nr in nr_start..nr_end {
                    for nc in nc_start..nc_end {
                        let px = self.back_buffer[nr * stride + nc];
                        sum_r += (px >> 16) & 0xFF;
                        sum_g += (px >> 8) & 0xFF;
                        sum_b += px & 0xFF;
                        count += 1;
                    }
                }
                if count > 0 {
                    let avg = ((sum_r / count) << 16) | ((sum_g / count) << 8) | (sum_b / count);
                    self.back_buffer[row * stride + col] = avg;
                    if col + 1 < x1 { self.back_buffer[row * stride + col + 1] = avg; }
                    if row + 1 < y1 { self.back_buffer[(row + 1) * stride + col] = avg; }
                    if row + 1 < y1 && col + 1 < x1 { self.back_buffer[(row + 1) * stride + col + 1] = avg; }
                }
            }
        }
        self.add_damage(Rect::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32));
    }

    pub fn fill_rect_gradient_v(&mut self, x: i32, y: i32, w: u32, h: u32, top: Color, bottom: Color) {
        if h == 0 {
            return;
        }
        for row in 0..h as i32 {
            let t = (row as u32 * 255 / h.max(1)) as u8;
            let color = top.lerp(bottom, t);
            self.draw_hline(x, y + row, w, color);
        }
    }

    pub fn shade_rect(&mut self, x: i32, y: i32, w: u32, h: u32, strength: u8) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w as i32).min(self.fb.width as i32);
        let y1 = (y + h as i32).min(self.fb.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let row_width = self.fb.width as usize;
        for row in y0..y1 {
            let start = row as usize * row_width + x0 as usize;
            let end = start + (x1 - x0) as usize;
            for px in &mut self.back_buffer[start..end] {
                let c = Color(*px);
                *px = c.lerp(Color::BLACK, strength).0;
            }
        }
        self.add_damage(Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32));
    }

    pub fn fill_rounded_rect(&mut self, x: i32, y: i32, w: u32, h: u32, radius: i32, color: Color) {
        let r = radius.min(w as i32 / 2).min(h as i32 / 2).max(0);
        if r == 0 {
            self.fill_rect(x, y, w, h, color);
            return;
        }
        self.fill_rect(x, y + r, w, h - (2 * r) as u32, color);
        for row in 0..r {
            let dy = r - row;
            let dx = isqrt((r * r - dy * dy).max(0));
            let line_w = (w as i32 - 2 * (r - dx)).max(0) as u32;
            self.draw_hline(x + (r - dx), y + row, line_w, color);
            self.draw_hline(x + (r - dx), y + h as i32 - 1 - row, line_w, color);
        }
    }

    pub fn fill_rounded_rect_alpha(&mut self, x: i32, y: i32, w: u32, h: u32, radius: i32, color: Color, alpha: u8) {
        let r = radius.min(w as i32 / 2).min(h as i32 / 2).max(0);
        if r == 0 {
            self.fill_rect_alpha(x, y, w, h, color, alpha);
            return;
        }
        self.fill_rect_alpha(x, y + r, w, h - (2 * r) as u32, color, alpha);
        for row in 0..r {
            let dy = r - row;
            let dx = isqrt((r * r - dy * dy).max(0));
            let line_w = (w as i32 - 2 * (r - dx)).max(0) as u32;
            for px in (x + (r - dx))..(x + (r - dx) + line_w as i32) {
                self.put_pixel_alpha(px, y + row, color, alpha);
                self.put_pixel_alpha(px, y + h as i32 - 1 - row, color, alpha);
            }
        }
    }

    pub fn draw_drop_shadow(&mut self, x: i32, y: i32, w: u32, h: u32, radius: u32) {
        let shadow_color = Color::rgb(0, 0, 0);
        let layers = radius as i32;
        for i in 1..=layers {
            let alpha = (50 * (layers - i + 1) / layers) as u8;
            let sx = x - i;
            let sy = y - i + 2;
            let sw = w + (i * 2) as u32;
            let sh = h + (i * 2) as u32;
            self.draw_rect_outline_alpha(sx, sy, sw, sh, shadow_color, alpha);
        }
    }

    pub fn draw_rect_outline_alpha(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color, alpha: u8) {
        for px in x..x + w as i32 {
            self.put_pixel_alpha(px, y, color, alpha);
            self.put_pixel_alpha(px, y + h as i32 - 1, color, alpha);
        }
        for py in y..y + h as i32 {
            self.put_pixel_alpha(x, py, color, alpha);
            self.put_pixel_alpha(x + w as i32 - 1, py, color, alpha);
        }
    }

    pub fn draw_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color) {
        self.draw_hline(x, y, w, color);
        self.draw_hline(x, y + h as i32 - 1, w, color);
        self.draw_vline(x, y, h, color);
        self.draw_vline(x + w as i32 - 1, y, h, color);
    }

    pub fn draw_hline(&mut self, x: i32, y: i32, w: u32, color: Color) {
        if y < 0 || y as u32 >= self.fb.height {
            return;
        }
        let x0 = x.max(0);
        let x1 = (x + w as i32).min(self.fb.width as i32);
        if x0 >= x1 {
            return;
        }
        let row_width = self.fb.width as usize;
        let start = y as usize * row_width + x0 as usize;
        let end = start + (x1 - x0) as usize;
        self.back_buffer[start..end].fill(color.0);
        self.add_damage(Rect::new(x0, y, (x1 - x0) as u32, 1));
    }

    pub fn draw_vline(&mut self, x: i32, y: i32, h: u32, color: Color) {
        for row in 0..h as i32 {
            self.put_pixel(x, y + row, color);
        }
        self.add_damage(Rect::new(x, y, 1, h));
    }

    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);

        loop {
            self.put_pixel(x, y, color);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
        let min_x = x0.min(x1);
        let min_y = y0.min(y1);
        let w = (x0 - x1).abs() as u32 + 1;
        let h = (y0 - y1).abs() as u32 + 1;
        self.add_damage(Rect::new(min_x, min_y, w, h));
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, radius: i32, color: Color) {
        for y in -radius..=radius {
            let dx = isqrt((radius * radius - y * y).max(0));
            self.draw_hline(cx - dx, cy + y, (dx * 2 + 1) as u32, color);
        }
    }

    pub fn draw_icon(&mut self, x: i32, y: i32, icon: IconType, color: Color) {
        match icon {
            IconType::Terminal => {
                self.draw_line(x + 2, y + 3, x + 7, y + 8, color);
                self.draw_line(x + 7, y + 8, x + 2, y + 13, color);
                self.draw_hline(x + 8, y + 13, 6, color);
            }
            IconType::Files => {
                self.fill_rect(x + 2, y + 3, 5, 2, color);
                self.fill_rect(x + 1, y + 5, 14, 9, color);
            }
            IconType::TaskManager => {
                self.fill_rect(x + 2, y + 10, 3, 5, color);
                self.fill_rect(x + 6, y + 6, 3, 9, color);
                self.fill_rect(x + 10, y + 3, 3, 12, color);
            }
            IconType::Display => {
                self.draw_rect(x + 1, y + 2, 14, 10, color);
                self.draw_hline(x + 5, y + 13, 6, color);
                self.draw_vline(x + 8, y + 11, 2, color);
            }
            IconType::About => {
                self.draw_rect(x + 2, y + 2, 12, 12, color);
                self.put_pixel(x + 8, y + 5, color);
                self.draw_vline(x + 8, y + 8, 4, color);
            }
            IconType::Power => {
                self.draw_vline(x + 8, y + 2, 6, color);
                self.draw_line(x + 4, y + 5, x + 3, y + 10, color);
                self.draw_line(x + 3, y + 10, x + 8, y + 13, color);
                self.draw_line(x + 8, y + 13, x + 13, y + 10, color);
                self.draw_line(x + 13, y + 10, x + 12, y + 5, color);
            }
            IconType::Restart => {
                self.draw_hline(x + 4, y + 3, 8, color);
                self.draw_vline(x + 12, y + 3, 8, color);
                self.draw_hline(x + 4, y + 11, 8, color);
                self.draw_vline(x + 4, y + 7, 4, color);
                self.draw_line(x + 2, y + 5, x + 4, y + 3, color);
            }
            IconType::Settings => {
                self.draw_rect(x + 4, y + 4, 8, 8, color);
                self.draw_vline(x + 8, y + 1, 14, color);
                self.draw_hline(x + 1, y + 8, 14, color);
            }
            IconType::Check => {
                self.draw_line(x + 2, y + 8, x + 6, y + 12, color);
                self.draw_line(x + 6, y + 12, x + 13, y + 3, color);
            }
            IconType::Browser => {
                self.draw_rect(x + 2, y + 2, 12, 12, color);
                self.draw_hline(x + 2, y + 8, 12, color);
                self.draw_vline(x + 8, y + 2, 12, color);
            }
            IconType::Theme => {
                self.fill_circle(x + 8, y + 8, 6, color);
                self.fill_circle(x + 5, y + 6, 2, Color::RED);
                self.fill_circle(x + 11, y + 6, 2, Color::GREEN);
                self.fill_circle(x + 8, y + 11, 2, Color::BLUE);
            }
            IconType::Wifi => {
                self.draw_hline(x + 2, y + 4, 12, color);
                self.draw_hline(x + 4, y + 8, 8, color);
                self.draw_hline(x + 6, y + 12, 4, color);
            }
            IconType::Volume => {
                self.fill_rect(x + 2, y + 6, 4, 4, color);
                self.draw_line(x + 6, y + 6, x + 10, y + 2, color);
                self.draw_line(x + 6, y + 9, x + 10, y + 13, color);
                self.draw_vline(x + 10, y + 2, 12, color);
            }
            IconType::Search => {
                self.draw_rect(x + 2, y + 2, 8, 8, color);
                self.draw_line(x + 9, y + 9, x + 14, y + 14, color);
            }
            IconType::Bookmark => {
                self.draw_vline(x + 4, y + 2, 12, color);
                self.draw_vline(x + 12, y + 2, 12, color);
                self.draw_line(x + 4, y + 14, x + 8, y + 10, color);
                self.draw_line(x + 8, y + 10, x + 12, y + 14, color);
            }
            IconType::Home => {
                self.draw_line(x + 2, y + 8, x + 8, y + 2, color);
                self.draw_line(x + 8, y + 2, x + 14, y + 8, color);
                self.draw_rect(x + 4, y + 8, 8, 7, color);
            }
            IconType::Reload => {
                self.draw_rect(x + 3, y + 3, 10, 10, color);
                self.draw_line(x + 1, y + 5, x + 3, y + 3, color);
            }
            IconType::ArrowLeft => {
                self.draw_line(x + 10, y + 3, x + 4, y + 8, color);
                self.draw_line(x + 4, y + 8, x + 10, y + 13, color);
            }
            IconType::ArrowRight => {
                self.draw_line(x + 4, y + 3, x + 10, y + 8, color);
                self.draw_line(x + 10, y + 8, x + 4, y + 13, color);
            }
        }
    }

    pub fn clear(&mut self, color: Color) {
        self.fill_rect(0, 0, self.fb.width, self.fb.height, color);
    }

    pub fn draw_char(&mut self, x: i32, y: i32, ch: u8, fg: Color, bg: Option<Color>) {
        if let Some(bg) = bg {
            self.fill_rect(x, y, 8, 16, bg);
        }
        let glyph = crate::font::read_glyph(ch);
        for (row, &byte) in glyph.iter().enumerate() {
            if byte == 0 {
                continue;
            }
            let py = y + row as i32;
            for col in 0..8 {
                if (byte >> (7 - col)) & 1 != 0 {
                    self.put_pixel(x + col as i32, py, fg);
                }
            }
        }
    }

    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, fg: Color, bg: Option<Color>) {
        let mut cursor_x = x;
        for byte in text.bytes() {
            self.draw_char(cursor_x, y, byte, fg, bg);
            cursor_x += 8;
        }
    }
}

// ---------------- глобальный доступ ----------------

use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;

static ACTIVE_RENDERER: SpinLock<Option<Renderer>> = SpinLock::new(None);

pub fn set_active_framebuffer(fb: Framebuffer) {
    without_interrupts(|| {
        *ACTIVE_RENDERER.lock() = Some(Renderer::new(fb));
    });
}

pub fn ensure_framebuffer() {
    if has_active_framebuffer() {
        return;
    }
    if let Some(info) = crate::gpu::detect() {
        if info.supports_bochs_vbe {
            if let Some(fb) = crate::vbe::set_mode(&info.device, 800, 600, crate::vbe::BPP_32) {
                crate::mouse::set_screen_size(800, 600);
                set_active_framebuffer(fb);
                crate::serial_println!("[renderer] framebuffer 800x600 32bpp активен (Bochs VBE)");
            }
        }
    }
}

pub fn reset_renderer() {
    *ACTIVE_RENDERER.lock() = None;
}

pub fn has_active_framebuffer() -> bool {
    without_interrupts(|| ACTIVE_RENDERER.lock().is_some())
}

pub fn with_renderer_ret<F: FnOnce(&mut Renderer) -> R, R>(f: F) -> Option<R> {
    without_interrupts(|| ACTIVE_RENDERER.lock().as_mut().map(f))
}

pub fn with_renderer<F: FnOnce(&mut Renderer)>(f: F) {
    without_interrupts(|| {
        if let Some(r) = ACTIVE_RENDERER.lock().as_mut() {
            f(r);
        }
    });
}

pub fn with_renderer_long<F: FnOnce(&mut Renderer) -> R, R>(f: F) -> Option<R> {
    let ptr: *mut Renderer = without_interrupts(|| {
        ACTIVE_RENDERER
            .lock()
            .as_mut()
            .map(|r| r as *mut Renderer)
            .unwrap_or(core::ptr::null_mut())
    });

    if ptr.is_null() {
        None
    } else {
        Some(unsafe { f(&mut *ptr) })
    }
}
