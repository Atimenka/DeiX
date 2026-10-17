//! Минимальный декодер PNG — для загрузки обоев из интернета.
//!
//! Поддерживается честное подмножество спецификации (покрывает
//! подавляющее большинство PNG в сети): битовая глубина 8, типы цвета
//! 0 (grayscale), 2 (RGB), 3 (палитра), 4 (gray+alpha), 6 (RGBA),
//! без interlace. Adam7-interlace и глубины 1/2/4/16 бит отклоняются
//! с внятной ошибкой. IDAT распаковывается net::inflate::zlib_inflate
//! (тот же DEFLATE, что у gzip в браузере). Альфа-канал смешивается
//! с чёрным фоном: обои непрозрачны по определению.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Декодированное изображение: пиксели 0x00RRGGBB построчно.
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub pixels: Vec<u32>,
}

const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
/// Предел w*h: 4096x4096 = 64 МиБ пикселей не влезут в кучу (16 МиБ),
/// поэтому ограничиваем входное изображение ~2 МПикс (8 МиБ в u32).
const MAX_PIXELS: usize = 2 * 1024 * 1024;

/// true, если данные начинаются с PNG-сигнатуры.
pub fn is_png(data: &[u8]) -> bool {
    data.len() >= 8 && data[..8] == SIG
}

fn be32(d: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}

pub fn decode(data: &[u8]) -> Result<Image, String> {
    if !is_png(data) {
        return Err("нет PNG-сигнатуры".into());
    }

    let mut w = 0usize;
    let mut h = 0usize;
    let mut color_type = 0u8;
    let mut idat: Vec<u8> = Vec::new();
    let mut palette: Vec<u32> = Vec::new();
    let mut seen_ihdr = false;

    // Проход по чанкам: length(4) + type(4) + data + crc(4).
    let mut pos = 8usize;
    while pos + 8 <= data.len() {
        let len = be32(data, pos) as usize;
        let ctype = &data[pos + 4..pos + 8];
        let dstart = pos + 8;
        let dend = match dstart.checked_add(len) {
            Some(e) if e + 4 <= data.len() => e,
            _ => return Err("чанк оборван (файл скачан не целиком?)".into()),
        };
        let cdata = &data[dstart..dend];
        match ctype {
            b"IHDR" => {
                if len < 13 {
                    return Err("IHDR короче 13 байт".into());
                }
                w = be32(cdata, 0) as usize;
                h = be32(cdata, 4) as usize;
                let depth = cdata[8];
                color_type = cdata[9];
                let interlace = cdata[12];
                if w == 0 || h == 0 || w.saturating_mul(h) > MAX_PIXELS {
                    return Err(format!("размер {}x{} вне пределов (до 2 МПикс)", w, h));
                }
                if depth != 8 {
                    return Err(format!("битовая глубина {} не поддержана (только 8)", depth));
                }
                if !matches!(color_type, 0 | 2 | 3 | 4 | 6) {
                    return Err(format!("тип цвета {} не поддержан", color_type));
                }
                if interlace != 0 {
                    return Err("Adam7 interlace не поддержан".into());
                }
                seen_ihdr = true;
            }
            b"PLTE" => {
                palette = cdata.chunks_exact(3)
                    .map(|c| ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32)
                    .collect();
            }
            b"IDAT" => idat.extend_from_slice(cdata),
            b"IEND" => break,
            _ => {}
        }
        pos = dend + 4; // + CRC
    }

    if !seen_ihdr {
        return Err("нет чанка IHDR".into());
    }
    if idat.is_empty() {
        return Err("нет данных IDAT".into());
    }
    if color_type == 3 && palette.is_empty() {
        return Err("палитровый PNG без PLTE".into());
    }

    let (raw, truncated) = crate::net::inflate::zlib_inflate(&idat)?;
    let bpp: usize = match color_type {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        _ => 4,
    };
    let stride = w * bpp;
    let need = h * (stride + 1);
    if truncated || raw.len() < need {
        return Err(format!(
            "IDAT распакован не полностью ({} из {} байт)",
            raw.len(),
            need
        ));
    }

    // Снятие фильтров (0 none, 1 sub, 2 up, 3 average, 4 paeth) — на
    // месте, в одном буфере отфильтрованных строк.
    let mut rows = raw;
    for y in 0..h {
        let base = y * (stride + 1);
        let filter = rows[base];
        for i in 0..stride {
            let x = base + 1 + i;
            let a = if i >= bpp { rows[x - bpp] } else { 0 }; // слева
            let b = if y > 0 { rows[x - (stride + 1)] } else { 0 }; // сверху
            let c = if y > 0 && i >= bpp { rows[x - (stride + 1) - bpp] } else { 0 };
            let recon = match filter {
                0 => rows[x],
                1 => rows[x].wrapping_add(a),
                2 => rows[x].wrapping_add(b),
                3 => rows[x].wrapping_add((((a as u16) + (b as u16)) / 2) as u8),
                4 => {
                    // Paeth predictor
                    let p = a as i32 + b as i32 - c as i32;
                    let (pa, pb, pc) = (
                        (p - a as i32).abs(),
                        (p - b as i32).abs(),
                        (p - c as i32).abs(),
                    );
                    let pred = if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c };
                    rows[x].wrapping_add(pred)
                }
                f => return Err(format!("неизвестный фильтр строки {}", f)),
            };
            rows[x] = recon;
        }
    }

    // Пиксели: альфа смешивается с чёрным (c*a/255).
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        let base = y * (stride + 1) + 1;
        for xp in 0..w {
            let p = base + xp * bpp;
            let rgb = match color_type {
                0 => {
                    let g = rows[p] as u32;
                    (g << 16) | (g << 8) | g
                }
                2 => ((rows[p] as u32) << 16) | ((rows[p + 1] as u32) << 8) | rows[p + 2] as u32,
                3 => *palette.get(rows[p] as usize).unwrap_or(&0),
                4 => {
                    let g = (rows[p] as u32 * rows[p + 1] as u32) / 255;
                    (g << 16) | (g << 8) | g
                }
                _ => {
                    let a = rows[p + 3] as u32;
                    let r = (rows[p] as u32 * a) / 255;
                    let g = (rows[p + 1] as u32 * a) / 255;
                    let b = (rows[p + 2] as u32 * a) / 255;
                    (r << 16) | (g << 8) | b
                }
            };
            pixels.push(rgb);
        }
    }

    Ok(Image { w, h, pixels })
}
