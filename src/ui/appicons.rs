//! Цветные иконки приложений.
//!
//! Файлы DXIC лежат в `/system/media/icons/<имя>.dxic` (`tools/make_icons.py`):
//! `"DXIC"`, ширина и высота (u16 LE), затем RGBA. Иконка читается с диска один
//! раз на каждый размер, приводится к нужному размеру усреднением по площади и
//! остаётся в кеше. Нет файла или он повреждён — `draw` возвращает `false`, и
//! вызывающий код рисует штриховой глиф `Renderer::draw_icon`.

use alloc::format;
use alloc::vec::Vec;

use crate::renderer::{Color, IconType, Renderer};

const MAGIC: &[u8; 4] = b"DXIC";
const HEADER: usize = 8;
const MAX_SIDE: usize = 256;

struct Entry {
    icon: IconType,
    size: i32,
    /// ARGB, альфа в старшем байте; пусто — иконка не загрузилась.
    pixels: Vec<u32>,
}

static mut CACHE: Vec<Entry> = Vec::new();

fn file_name(icon: IconType) -> Option<&'static str> {
    Some(match icon {
        IconType::Terminal => "terminal",
        IconType::Files => "files",
        IconType::Browser => "browser",
        IconType::TaskManager => "taskmgr",
        IconType::Settings => "errors",
        IconType::Theme => "personalize",
        IconType::Display => "display",
        IconType::Install => "install",
        IconType::Reload => "updates",
        IconType::About => "about",
        _ => return None,
    })
}

fn scale_area(rgba: &[u8], w: usize, h: usize, s: usize) -> Vec<u32> {
    let mut out = Vec::with_capacity(s * s);
    for dy in 0..s {
        let y0 = dy * h / s;
        let y1 = ((dy + 1) * h / s).max(y0 + 1).min(h);
        for dx in 0..s {
            let x0 = dx * w / s;
            let x1 = ((dx + 1) * w / s).max(x0 + 1).min(w);
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = (y * w + x) * 4;
                    let al = rgba[p + 3] as u32;
                    r += rgba[p] as u32 * al;
                    g += rgba[p + 1] as u32 * al;
                    b += rgba[p + 2] as u32 * al;
                    a += al;
                    n += 1;
                }
            }
            if a == 0 {
                out.push(0);
            } else {
                out.push(((a / n) << 24) | ((r / a) << 16) | ((g / a) << 8) | (b / a));
            }
        }
    }
    out
}

fn load(name: &str, size: i32) -> Vec<u32> {
    let path = format!("/system/media/icons/{}.dxic", name);
    let Ok(data) = crate::vfs::read_file(&path) else {
        return Vec::new();
    };
    if data.len() < HEADER || &data[..4] != MAGIC {
        return Vec::new();
    }
    let w = u16::from_le_bytes([data[4], data[5]]) as usize;
    let h = u16::from_le_bytes([data[6], data[7]]) as usize;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || data.len() < HEADER + w * h * 4 {
        return Vec::new();
    }
    scale_area(&data[HEADER..HEADER + w * h * 4], w, h, size as usize)
}

/// Рисует иконку приложения квадратом `size`×`size` с левым верхним углом
/// (`x`, `y`). `false` — цветной иконки нет, нужен запасной вариант.
pub fn draw(r: &mut Renderer, x: i32, y: i32, size: i32, icon: IconType) -> bool {
    let Some(name) = file_name(icon) else {
        return false;
    };
    if size <= 0 {
        return false;
    }
    let cache = unsafe { &mut *(&raw mut CACHE) };
    let pos = match cache.iter().position(|e| e.icon == icon && e.size == size) {
        Some(p) => p,
        None => {
            cache.push(Entry { icon, size, pixels: load(name, size) });
            cache.len() - 1
        }
    };
    let pixels = &cache[pos].pixels;
    if pixels.is_empty() {
        return false;
    }
    let s = size as usize;
    for row in 0..s {
        for col in 0..s {
            let px = pixels[row * s + col];
            let alpha = (px >> 24) as u8;
            if alpha != 0 {
                r.put_pixel_alpha(x + col as i32, y + row as i32, Color(px & 0x00FF_FFFF), alpha);
            }
        }
    }
    true
}
