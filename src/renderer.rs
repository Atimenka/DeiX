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

/// Упакованное альфа-смешивание: красный и синий каналы считаются одним
/// умножением, зелёный отдельно; деление на 255 с точным округлением.
#[inline]
fn blend_px(fg: u32, bg: u32, a: u32) -> u32 {
    let ia = 255 - a;
    let rb = (fg & 0x00FF_00FF) * a + (bg & 0x00FF_00FF) * ia + 0x0080_0080;
    let rb = ((rb + ((rb >> 8) & 0x00FF_00FF)) >> 8) & 0x00FF_00FF;
    let g = ((fg >> 8) & 0xFF) * a + ((bg >> 8) & 0xFF) * ia + 0x80;
    let g = ((g + (g >> 8)) >> 8) & 0xFF;
    rb | (g << 8)
}

/// Покрытие пикселя (px, py) скруглённым углом радиуса `r`, шкала 0..=16.
/// Центр дуги — в точке (r, r) относительно угла прямоугольника.
#[inline]
fn corner_coverage(px: i32, py: i32, r: i32) -> u32 {
    let dx = r * 16 - (px * 16 + 8);
    let dy = r * 16 - (py * 16 + 8);
    let d2 = dx * dx + dy * dy;
    let inner = r * 16 - 8;
    let outer = r * 16 + 8;
    if d2 <= inner * inner {
        return 16;
    }
    if d2 >= outer * outer {
        return 0;
    }
    (r * 16 - isqrt(d2) + 8).clamp(0, 16) as u32
}

/// Один проход скользящего «ящичного» фильтра по строке или столбцу.
/// Окно сжимается у краёв, поэтому за границы области цвет не течёт.
fn box_blur_line(
    src: &[u32],
    src_off: usize,
    src_step: usize,
    dst: &mut [u32],
    dst_off: usize,
    dst_step: usize,
    n: usize,
    r: usize,
) {
    const RECIP: [u32; 6] = [0, 65536, 32768, 21846, 16384, 13108];
    let (mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32);
    let mut count = 0usize;
    for i in 0..=r.min(n - 1) {
        let p = src[src_off + i * src_step];
        sr += (p >> 16) & 0xFF;
        sg += (p >> 8) & 0xFF;
        sb += p & 0xFF;
        count += 1;
    }
    for i in 0..n {
        if i > 0 {
            if i + r < n {
                let p = src[src_off + (i + r) * src_step];
                sr += (p >> 16) & 0xFF;
                sg += (p >> 8) & 0xFF;
                sb += p & 0xFF;
                count += 1;
            }
            if i > r {
                let p = src[src_off + (i - r - 1) * src_step];
                sr -= (p >> 16) & 0xFF;
                sg -= (p >> 8) & 0xFF;
                sb -= p & 0xFF;
                count -= 1;
            }
        }
        let m = RECIP[count];
        dst[dst_off + i * dst_step] = (((sr * m) >> 16) << 16) | (((sg * m) >> 16) << 8) | ((sb * m) >> 16);
    }
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

    /// Точка внутри прямоугольника.
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.w as i32
            && py >= self.y
            && py < self.y + self.h as i32
    }

    /// Точка внутри прямоугольника, расширенного на `pad` со всех сторон
    /// (hitbox мелких кнопок чуть больше видимой области).
    pub fn contains_pad(&self, px: i32, py: i32, pad: i32) -> bool {
        px >= self.x - pad
            && px < self.x + self.w as i32 + pad
            && py >= self.y - pad
            && py < self.y + self.h as i32 + pad
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
    pub const ACCENT: Color = Color::rgb(99, 102, 241); // Indigo

    pub fn palette() -> [Color; 6] {
        [Color::DARK_GRAY, Color::LIGHT_GRAY, Color::DESKTOP_BLUE, Color::TITLEBAR_ACTIVE, Color::TITLEBAR_INACTIVE, Color::ACCENT]
    }

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
        Color(blend_px(self.0, bg.0, alpha as u32))
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
    /// Обзор окон (Overview): четыре плитки.
    Grid,
    Bookmark,
    Home,
    Reload,
    ArrowLeft,
    ArrowRight,
    /// Установщик: стрелка вниз в диск.
    Install,
    Plus,
    NewFolder,
    Trash,
    Save,
    Cross,
    Download,
    Document,
    Drive,
    Stop,
    /// Просмотр фото: рамка с «горой» и солнцем.
    Image,
    /// Видеоплеер: экран с треугольником воспроизведения.
    Video,
    /// Аудиоплеер: восьмая нота.
    Music,
    Play,
    Pause,
    SkipNext,
    SkipPrev,
}

pub struct Renderer {
    fb: Framebuffer,
    pub back_buffer: Vec<u32>,
    pub damage: crate::ui::surface::DamageList,
    blur_scratch: Vec<u32>,
}

impl Renderer {
    pub fn new(fb: Framebuffer) -> Self {
        let pixel_count = (fb.width as usize) * (fb.height as usize);
        let back_buffer = vec![0u32; pixel_count];
        Renderer {
            fb,
            back_buffer,
            damage: crate::ui::surface::DamageList::new(),
            blur_scratch: Vec::new(),
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
        // Крайние значения альфы — без чтения фона и смешивания.
        if alpha == 0 {
            return;
        }
        let idx = y as usize * self.fb.width as usize + x as usize;
        if alpha == 255 {
            self.back_buffer[idx] = color.0;
            return;
        }
        self.back_buffer[idx] = blend_px(color.0, self.back_buffer[idx], alpha as u32);
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

        self.clear_damage();
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
        // Быстрые пути: alpha 0 — ничего не меняет, alpha 255 — обычная
        // непрозрачная заливка без чтения фона и смешивания. Профиль
        // Performance (opacity=255) целиком идёт этим путём.
        if alpha == 0 {
            return;
        }
        if alpha == 255 {
            self.fill_rect(x, y, w, h, color);
            return;
        }
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
                *px = blend_px(color.0, *px, alpha as u32);
            }
        }
        self.add_damage(Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32));
    }

    /// Смешивание горизонтального отрезка без учёта damage.
    fn blend_span(&mut self, x: i32, y: i32, w: i32, color: Color, alpha: u8) {
        if w <= 0 || alpha == 0 || y < 0 || y as u32 >= self.fb.height {
            return;
        }
        let x0 = x.max(0);
        let x1 = (x + w).min(self.fb.width as i32);
        if x0 >= x1 {
            return;
        }
        let start = y as usize * self.fb.width as usize + x0 as usize;
        let end = start + (x1 - x0) as usize;
        if alpha == 255 {
            self.back_buffer[start..end].fill(color.0);
        } else {
            let a = alpha as u32;
            for px in &mut self.back_buffer[start..end] {
                *px = blend_px(color.0, *px, a);
            }
        }
    }

    /// Вертикальный градиент прозрачности одного цвета: сверху `alpha_top`,
    /// снизу `alpha_bottom`. Один damage на всю область.
    pub fn fill_fade_v(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color, alpha_top: u8, alpha_bottom: u8) {
        if w == 0 || h == 0 {
            return;
        }
        let hi = h as i32;
        for row in 0..hi {
            let a = if hi > 1 {
                (alpha_top as i32 * (hi - 1 - row) + alpha_bottom as i32 * row) / (hi - 1)
            } else {
                alpha_top as i32
            };
            self.blend_span(x, y + row, w as i32, color, a as u8);
        }
        self.add_damage(Rect::new(x.max(0), y.max(0), w, h));
    }

    /// Акриловое размытие: область уменьшается вдвое усреднением блоков
    /// 2x2, размывается двумя проходами скользящего «ящика» и
    /// растягивается обратно. Стоимость линейна по числу пикселей.
    pub fn apply_blur_rect(&mut self, x: i32, y: i32, w: u32, h: u32, radius: usize) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x + w as i32).min(self.fb.width as i32)).max(0) as usize;
        let y1 = ((y + h as i32).min(self.fb.height as i32)).max(0) as usize;
        if x0 >= x1 || y0 >= y1 || radius == 0 {
            return;
        }
        let stride = self.fb.width as usize;
        let sw = (x1 - x0 + 1) / 2;
        let sh = (y1 - y0 + 1) / 2;
        let n = sw * sh;
        let taps = ((radius.min(4) + 1) / 2).clamp(1, 2);

        self.blur_scratch.clear();
        self.blur_scratch.resize(n * 2, 0);
        let (small, tmp) = self.blur_scratch.split_at_mut(n);

        for by in 0..sh {
            let ry = y0 + by * 2;
            let ry2 = (ry + 1).min(y1 - 1);
            for bx in 0..sw {
                let rx = x0 + bx * 2;
                let rx2 = (rx + 1).min(x1 - 1);
                let p0 = self.back_buffer[ry * stride + rx];
                let p1 = self.back_buffer[ry * stride + rx2];
                let p2 = self.back_buffer[ry2 * stride + rx];
                let p3 = self.back_buffer[ry2 * stride + rx2];
                let rb = ((p0 & 0x00FF_00FF) + (p1 & 0x00FF_00FF) + (p2 & 0x00FF_00FF) + (p3 & 0x00FF_00FF)) >> 2;
                let g = (((p0 >> 8) & 0xFF) + ((p1 >> 8) & 0xFF) + ((p2 >> 8) & 0xFF) + ((p3 >> 8) & 0xFF)) >> 2;
                small[by * sw + bx] = (rb & 0x00FF_00FF) | (g << 8);
            }
        }

        for row in 0..sh {
            box_blur_line(small, row * sw, 1, tmp, row * sw, 1, sw, taps);
        }
        for col in 0..sw {
            box_blur_line(tmp, col, sw, small, col, sw, sh, taps);
        }

        for row in y0..y1 {
            let by = (row - y0) / 2;
            let dst = row * stride;
            for col in x0..x1 {
                self.back_buffer[dst + col] = small[by * sw + (col - x0) / 2];
            }
        }
        self.add_damage(Rect::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32));
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
        self.fill_rounded_rect_alpha(x, y, w, h, radius, color, 255);
    }

    /// Скруглённый прямоугольник со сглаженными углами: покрытие каждого
    /// углового пикселя считается по расстоянию до центра дуги.
    pub fn fill_rounded_rect_alpha(&mut self, x: i32, y: i32, w: u32, h: u32, radius: i32, color: Color, alpha: u8) {
        if w == 0 || h == 0 || alpha == 0 {
            return;
        }
        let wi = w as i32;
        let hi = h as i32;
        let r = radius.min(wi / 2).min(hi / 2).max(0);
        for row in r..hi - r {
            self.blend_span(x, y + row, wi, color, alpha);
        }
        for row in 0..r {
            self.blend_span(x + r, y + row, wi - 2 * r, color, alpha);
            self.blend_span(x + r, y + hi - 1 - row, wi - 2 * r, color, alpha);
            for col in 0..r {
                let cov = corner_coverage(col, row, r);
                if cov == 0 {
                    continue;
                }
                let a = (cov * alpha as u32 / 16) as u8;
                self.blend_span(x + col, y + row, 1, color, a);
                self.blend_span(x + wi - 1 - col, y + row, 1, color, a);
                self.blend_span(x + col, y + hi - 1 - row, 1, color, a);
                self.blend_span(x + wi - 1 - col, y + hi - 1 - row, 1, color, a);
            }
        }
        self.add_damage(Rect::new(x.max(0), y.max(0), w, h));
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

    /// Мягкая тень: квадратичный спад непрозрачности на `spread` пикселей
    /// вокруг окна, смещённая вниз на `offset_y`. Вся тень должна лежать
    /// внутри области, которую восстанавливает damage (margin окна).
    pub fn draw_soft_shadow(&mut self, x: i32, y: i32, w: u32, h: u32, spread: i32, offset_y: i32, peak: u32) {
        if spread <= 0 {
            return;
        }
        for i in 1..=spread {
            let k = (spread - i + 1) as u32;
            let alpha = (peak * k * k / (spread * spread) as u32) as u8;
            if alpha == 0 {
                continue;
            }
            self.draw_rect_outline_alpha(x - i, y - i + offset_y, w + (i * 2) as u32, h + (i * 2) as u32, Color::BLACK, alpha);
        }
    }

    /// Контур прямоугольника со скруглёнными углами: прямые стороны +
    /// дуги по midpoint-алгоритму. Используется для рамок акриловых
    /// панелей и подсветки выбранного пункта меню — совпадает по форме
    /// с fill_rounded_rect того же радиуса.
    pub fn draw_rounded_rect_outline(&mut self, x: i32, y: i32, w: u32, h: u32, radius: i32, color: Color, alpha: u8) {
        let wi = w as i32;
        let hi = h as i32;
        if wi <= 0 || hi <= 0 {
            return;
        }
        let rad = radius.min(wi / 2).min(hi / 2).max(0);
        for px in (x + rad)..(x + wi - rad) {
            self.put_pixel_alpha(px, y, color, alpha);
            self.put_pixel_alpha(px, y + hi - 1, color, alpha);
        }
        for py in (y + rad)..(y + hi - rad) {
            self.put_pixel_alpha(x, py, color, alpha);
            self.put_pixel_alpha(x + wi - 1, py, color, alpha);
        }
        if rad > 0 {
            let (lx, ty) = (x + rad, y + rad);
            let (rx, by) = (x + wi - 1 - rad, y + hi - 1 - rad);
            let mut dx = rad;
            let mut dy = 0;
            let mut err = 1 - rad;
            while dx >= dy {
                self.put_pixel_alpha(rx + dx, by + dy, color, alpha);
                self.put_pixel_alpha(rx + dy, by + dx, color, alpha);
                self.put_pixel_alpha(lx - dx, by + dy, color, alpha);
                self.put_pixel_alpha(lx - dy, by + dx, color, alpha);
                self.put_pixel_alpha(lx - dx, ty - dy, color, alpha);
                self.put_pixel_alpha(lx - dy, ty - dx, color, alpha);
                self.put_pixel_alpha(rx + dx, ty - dy, color, alpha);
                self.put_pixel_alpha(rx + dy, ty - dx, color, alpha);
                dy += 1;
                if err < 0 {
                    err += 2 * dy + 1;
                } else {
                    dx -= 1;
                    err += 2 * (dy - dx) + 1;
                }
            }
        }
        self.add_damage(Rect::new(x, y, w, h));
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
        let r = radius.max(0);
        let d = (r * 2 + 1) as u32;
        self.fill_rounded_rect_alpha(cx - r, cy - r, d, d, r, color, 255);
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
            IconType::Install => {
                // Стрелка вниз в "диск" (нижняя скобка).
                self.draw_vline(x + 8, y + 2, 7, color);
                self.draw_line(x + 5, y + 6, x + 8, y + 9, color);
                self.draw_line(x + 11, y + 6, x + 8, y + 9, color);
                self.draw_vline(x + 2, y + 10, 4, color);
                self.draw_vline(x + 14, y + 10, 4, color);
                self.draw_hline(x + 2, y + 13, 13, color);
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
            IconType::Grid => {
                self.draw_rect(x + 2, y + 2, 5, 5, color);
                self.draw_rect(x + 9, y + 2, 5, 5, color);
                self.draw_rect(x + 2, y + 9, 5, 5, color);
                self.draw_rect(x + 9, y + 9, 5, 5, color);
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
            IconType::Plus => {
                self.draw_hline(x + 3, y + 8, 11, color);
                self.draw_vline(x + 8, y + 3, 11, color);
            }
            IconType::NewFolder => {
                self.draw_hline(x + 1, y + 3, 6, color);
                self.draw_rect(x + 1, y + 4, 14, 10, color);
                self.draw_hline(x + 5, y + 9, 6, color);
                self.draw_vline(x + 8, y + 6, 6, color);
            }
            IconType::Trash => {
                self.draw_hline(x + 3, y + 4, 11, color);
                self.draw_hline(x + 6, y + 2, 5, color);
                self.draw_vline(x + 4, y + 5, 9, color);
                self.draw_vline(x + 12, y + 5, 9, color);
                self.draw_hline(x + 4, y + 14, 9, color);
                self.draw_vline(x + 7, y + 6, 6, color);
                self.draw_vline(x + 9, y + 6, 6, color);
            }
            IconType::Save => {
                self.draw_rect(x + 2, y + 2, 12, 12, color);
                self.draw_rect(x + 5, y + 2, 6, 4, color);
                self.draw_rect(x + 4, y + 9, 8, 5, color);
            }
            IconType::Cross => {
                self.draw_line(x + 3, y + 3, x + 12, y + 12, color);
                self.draw_line(x + 12, y + 3, x + 3, y + 12, color);
            }
            IconType::Download => {
                self.draw_vline(x + 8, y + 2, 8, color);
                self.draw_line(x + 4, y + 6, x + 8, y + 10, color);
                self.draw_line(x + 12, y + 6, x + 8, y + 10, color);
                self.draw_hline(x + 3, y + 13, 11, color);
            }
            IconType::Document => {
                self.draw_rect(x + 3, y + 1, 10, 14, color);
                self.draw_hline(x + 5, y + 5, 6, color);
                self.draw_hline(x + 5, y + 8, 6, color);
                self.draw_hline(x + 5, y + 11, 4, color);
            }
            IconType::Drive => {
                self.draw_rect(x + 1, y + 5, 14, 7, color);
                self.draw_hline(x + 3, y + 9, 6, color);
                self.fill_rect(x + 12, y + 8, 2, 2, color);
            }
            IconType::Stop => {
                self.draw_rect(x + 2, y + 2, 12, 12, color);
                self.fill_rect(x + 5, y + 5, 6, 6, color);
            }
            IconType::Image => {
                self.draw_rect(x + 1, y + 3, 14, 10, color);
                self.fill_circle(x + 5, y + 6, 1, color);
                self.draw_line(x + 2, y + 12, x + 6, y + 8, color);
                self.draw_line(x + 6, y + 8, x + 9, y + 11, color);
                self.draw_line(x + 9, y + 11, x + 11, y + 9, color);
                self.draw_line(x + 11, y + 9, x + 14, y + 12, color);
            }
            IconType::Video => {
                self.draw_rect(x + 1, y + 3, 14, 10, color);
                self.draw_line(x + 6, y + 5, x + 11, y + 8, color);
                self.draw_line(x + 11, y + 8, x + 6, y + 11, color);
                self.draw_vline(x + 6, y + 5, 7, color);
            }
            IconType::Music => {
                self.fill_circle(x + 5, y + 11, 2, color);
                self.draw_vline(x + 7, y + 3, 9, color);
                self.draw_line(x + 7, y + 3, x + 12, y + 4, color);
                self.draw_vline(x + 12, y + 4, 4, color);
            }
            IconType::Play => {
                self.draw_line(x + 4, y + 2, x + 12, y + 8, color);
                self.draw_line(x + 12, y + 8, x + 4, y + 14, color);
                self.draw_vline(x + 4, y + 2, 13, color);
                self.draw_line(x + 6, y + 4, x + 10, y + 8, color);
                self.draw_line(x + 10, y + 8, x + 6, y + 12, color);
                self.draw_vline(x + 6, y + 4, 9, color);
            }
            IconType::Pause => {
                self.draw_rect(x + 3, y + 2, 4, 12, color);
                self.draw_rect(x + 9, y + 2, 4, 12, color);
            }
            IconType::SkipNext => {
                self.draw_line(x + 3, y + 3, x + 10, y + 8, color);
                self.draw_line(x + 10, y + 8, x + 3, y + 13, color);
                self.draw_vline(x + 3, y + 3, 11, color);
                self.draw_vline(x + 12, y + 3, 11, color);
            }
            IconType::SkipPrev => {
                self.draw_line(x + 13, y + 3, x + 6, y + 8, color);
                self.draw_line(x + 6, y + 8, x + 13, y + 13, color);
                self.draw_vline(x + 13, y + 3, 11, color);
                self.draw_vline(x + 4, y + 3, 11, color);
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
        let (fw, fh) = (self.fb.width as i32, self.fb.height as i32);
        if x >= 0 && y >= 0 && x + 8 <= fw && y + 16 <= fh {
            let stride = fw as usize;
            for (row, &byte) in glyph.iter().enumerate() {
                if byte == 0 {
                    continue;
                }
                let base = (y as usize + row) * stride + x as usize;
                let line = &mut self.back_buffer[base..base + 8];
                for (col, px) in line.iter_mut().enumerate() {
                    if (byte >> (7 - col)) & 1 != 0 {
                        *px = fg.0;
                    }
                }
            }
            return;
        }
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

    /// Рисует строку UTF-8. Разложение символов в глифы — единое для
    /// всей системы (`ui::text::glyph_cells`): ширина на экране всегда
    /// совпадает с `ui::text::text_cells(text) * 8`. Итерация по БАЙТАМ,
    /// как было раньше, превращала каждый кириллический символ в два
    /// глифа псевдографики.
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, fg: Color, bg: Option<Color>) {
        let mut cursor_x = x;
        for ch in text.chars() {
            crate::ui::text::glyph_cells(ch, &mut |b| {
                self.draw_char(cursor_x, y, b, fg, bg);
                cursor_x += 8;
            });
        }
    }
}

// ---------------- глобальный доступ ----------------

use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;

static ACTIVE_RENDERER: SpinLock<Option<Renderer>> = SpinLock::new(None);

pub fn set_active_framebuffer(fb: Framebuffer) {
    // Старый back buffer освобождается до выделения нового (на куче в
    // 16 МиБ два полноэкранных буфера одновременно не живут), а сам
    // новый Renderer строится ВНЕ лока: если аллокация запаникует,
    // обработчик паники сможет взять рендерер и нарисовать отчёт,
    // а не оставить чёрный экран.
    without_interrupts(|| {
        *ACTIVE_RENDERER.lock() = None;
    });
    let r = Renderer::new(fb);
    without_interrupts(|| {
        *ACTIVE_RENDERER.lock() = Some(r);
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
    // Дроп старого рендерера (большой Vec) — вне лока и под запретом
    // прерываний симметрично остальным операциям над ACTIVE_RENDERER.
    let old = without_interrupts(|| ACTIVE_RENDERER.lock().take());
    drop(old);
}

pub fn has_active_framebuffer() -> bool {
    without_interrupts(|| ACTIVE_RENDERER.lock().is_some())
}

pub fn with_renderer_ret<F: FnOnce(&mut Renderer) -> R, R>(f: F) -> Option<R> {
    without_interrupts(|| ACTIVE_RENDERER.lock().as_mut().map(f))
}

/// Как `with_renderer`, но не ждёт занятый рендерер.
///
/// Для обработчиков сбоев: исключение может прийти, пока блокировку держит
/// прерванная задача композитора, и обычный `lock()` в этом месте завис бы
/// навсегда. Возвращает `false`, если рендерер был занят.
pub fn try_with_renderer<F: FnOnce(&mut Renderer)>(f: F) -> bool {
    without_interrupts(|| match ACTIVE_RENDERER.try_lock() {
        Some(mut guard) => match guard.as_mut() {
            Some(r) => {
                f(r);
                true
            }
            None => false,
        },
        None => false,
    })
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
