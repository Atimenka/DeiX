//! Установка обоев из интернета или с диска.
//!
//! Поток: скачать (net::http::fetch_bytes) или прочитать файл → декодировать
//! (PNG через src/png.rs либо несжатый BMP 24/32) → при необходимости
//! уменьшить до лимита файла ext2 (268 КиБ: 12 прямых + 256 косвенных
//! блоков по 1 КиБ) → закодировать BMP 24 бит → записать /wallpaper.bmp
//! на /userdata → переключить тему на стиль Custom и сохранить конфиг.
//! Рабочий стол растягивает картинку на экран сам (load_bmp_wallpaper).

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Бюджет на итоговый BMP: чуть ниже честного максимума ext2-файла
/// (268 блоков * 1024 = 274432 байта), чтобы не упереться в границу.
const FILE_BUDGET: usize = 270_000;

struct Img {
    w: usize,
    h: usize,
    pixels: Vec<u32>,
}

/// Скачивает картинку по URL и ставит её обоями. Возвращает сообщение
/// о результате (размеры, путь) или ошибку с настоящей причиной.
pub fn set_from_url(url: &str) -> Result<String, String> {
    let resp = crate::net::http::fetch_bytes(url)?;
    if resp.status != 200 {
        return Err(format!("HTTP {} {}", resp.status, resp.reason));
    }
    set_from_bytes(&resp.body)
}

/// Ставит обоями картинку из байтов (PNG или BMP).
pub fn set_from_bytes(data: &[u8]) -> Result<String, String> {
    let img = decode_image(data)?;
    let src_dims = format!("{}x{}", img.w, img.h);
    let img = fit_to_budget(img);
    let bmp = encode_bmp24(&img);
    crate::vfs::write_file("/userdata/wallpaper.bmp", &bmp)
        .map_err(|e| format!("запись /userdata/wallpaper.bmp: {}", e.message()))?;

    // Тема: стиль Custom (индекс 4 в WALLPAPER_STYLES) + сохранение.
    let mut t = crate::ui::theme::get_theme();
    t.wallpaper_style = 4;
    crate::ui::theme::set_theme(t);
    crate::ui::theme::bump_wallpaper_epoch();
    crate::ui::config::save();

    let scaled = if img.w != 0 && src_dims != format!("{}x{}", img.w, img.h) {
        format!(", уменьшено с {}", src_dims)
    } else {
        String::new()
    };
    Ok(format!(
        "обои установлены: /userdata/wallpaper.bmp {}x{} ({} КиБ{})",
        img.w,
        img.h,
        bmp.len() / 1024,
        scaled
    ))
}

fn decode_image(data: &[u8]) -> Result<Img, String> {
    if crate::png::is_png(data) {
        let p = crate::png::decode(data).map_err(|e| format!("PNG: {}", e))?;
        return Ok(Img { w: p.w, h: p.h, pixels: p.pixels });
    }
    if crate::jpeg::is_jpeg(data) {
        let p = crate::jpeg::decode(data).map_err(|e| format!("JPEG: {}", e))?;
        return Ok(Img { w: p.w, h: p.h, pixels: p.pixels });
    }
    if data.len() >= 54 && &data[0..2] == b"BM" {
        return decode_bmp(data);
    }
    Err("формат не распознан: поддерживаются PNG (8 бит, без interlace), baseline JPEG и BMP 24/32 (BI_RGB)".into())
}

/// Несжатый BMP 24/32 бит → пиксели (тот же формат, что понимает
/// load_bmp_wallpaper в desktop.rs, но в память, для даунскейла).
fn decode_bmp(data: &[u8]) -> Result<Img, String> {
    let off = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
    let bw = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
    let bh_raw = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
    let bpp = u16::from_le_bytes([data[28], data[29]]);
    let compression = u32::from_le_bytes([data[30], data[31], data[32], data[33]]);
    if compression != 0 || (bpp != 24 && bpp != 32) || bw <= 0 || bh_raw == 0 {
        return Err("BMP: поддерживается только несжатый 24/32-битный (BI_RGB)".into());
    }
    let top_down = bh_raw < 0;
    let (w, h) = (bw as usize, bh_raw.unsigned_abs() as usize);
    if w.saturating_mul(h) > 2 * 1024 * 1024 {
        return Err(format!("BMP: размер {}x{} вне пределов (до 2 МПикс)", w, h));
    }
    let bytes_pp = (bpp / 8) as usize;
    let stride = (w * bytes_pp).div_ceil(4) * 4;
    let need = off + stride * h;
    if need > data.len() {
        return Err("BMP: файл короче заявленных размеров".into());
    }
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        let srow = if top_down { y } else { h - 1 - y };
        let row = off + srow * stride;
        for x in 0..w {
            let p = row + x * bytes_pp;
            pixels.push(
                ((data[p + 2] as u32) << 16) | ((data[p + 1] as u32) << 8) | data[p] as u32,
            );
        }
    }
    Ok(Img { w, h, pixels })
}

/// Уменьшает картинку (ближайший сосед, с сохранением пропорций) так,
/// чтобы 24-битный BMP уложился в FILE_BUDGET. Уже умещающиеся
/// картинки не трогаются.
fn fit_to_budget(img: Img) -> Img {
    if bmp24_size(img.w, img.h) <= FILE_BUDGET {
        return img;
    }
    // Пиксельный бюджет без выравнивания строк; точный размер
    // проверяется ниже и ширина при необходимости поджимается.
    let px_budget = (FILE_BUDGET - 54) / 3;
    // tw = sqrt(px_budget * w / h) целочисленно (значения < 2^32).
    let target = (px_budget as u64 * img.w as u64) / img.h.max(1) as u64;
    let mut tw = int_sqrt(target).max(16);
    let mut th = (tw * img.h as u64 / img.w.max(1) as u64).max(16);
    while tw > 16 && bmp24_size(tw as usize, th as usize) > FILE_BUDGET {
        tw -= 4;
        th = (tw * img.h as u64 / img.w.max(1) as u64).max(16);
    }
    // Вырожденные пропорции (узкая полоса в тысячи пикселей высотой):
    // ширина упёрлась в минимум, дожимаем высоту — пропорции при этом
    // честно теряются, но файл гарантированно влезает в лимит ext2.
    while th > 16 && bmp24_size(tw as usize, th as usize) > FILE_BUDGET {
        th -= 4;
    }
    let (tw, th) = (tw as usize, th as usize);
    let mut pixels = Vec::with_capacity(tw * th);
    for y in 0..th {
        let sy = y * img.h / th;
        for x in 0..tw {
            let sx = x * img.w / tw;
            pixels.push(img.pixels[sy * img.w + sx]);
        }
    }
    Img { w: tw, h: th, pixels }
}

fn bmp24_size(w: usize, h: usize) -> usize {
    54 + h * (w * 3).div_ceil(4) * 4
}

fn int_sqrt(v: u64) -> u64 {
    if v < 2 {
        return v;
    }
    // Монотонно убывающий Ньютон — та же схема, что isqrt в desktop.rs
    // (вариант с `x != last` зацикливается, см. историю фикса).
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

/// Кодирует пиксели в несжатый BMP 24 бит (bottom-up, BI_RGB).
fn encode_bmp24(img: &Img) -> Vec<u8> {
    let stride = (img.w * 3).div_ceil(4) * 4;
    let data_size = stride * img.h;
    let mut out = vec![0u8; 54 + data_size];
    out[0] = b'B';
    out[1] = b'M';
    out[2..6].copy_from_slice(&((54 + data_size) as u32).to_le_bytes());
    out[10..14].copy_from_slice(&54u32.to_le_bytes());
    out[14..18].copy_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER
    out[18..22].copy_from_slice(&(img.w as u32).to_le_bytes());
    out[22..26].copy_from_slice(&(img.h as u32).to_le_bytes()); // bottom-up
    out[26..28].copy_from_slice(&1u16.to_le_bytes()); // planes
    out[28..30].copy_from_slice(&24u16.to_le_bytes()); // bpp
    out[34..38].copy_from_slice(&(data_size as u32).to_le_bytes());
    for y in 0..img.h {
        let drow = 54 + (img.h - 1 - y) * stride;
        for x in 0..img.w {
            let px = img.pixels[y * img.w + x];
            let p = drow + x * 3;
            out[p] = px as u8; // B
            out[p + 1] = (px >> 8) as u8; // G
            out[p + 2] = (px >> 16) as u8; // R
        }
    }
    out
}
