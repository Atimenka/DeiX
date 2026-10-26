//! ГРАФИЧЕСКИЙ ЗАГРУЗОЧНЫЙ ЭКРАН — лого вместо простыни логов.
//!
//! Логи никуда не деваются: они по-прежнему целиком идут в COM1
//! (`serial_println!`). Меняется только то, что видит человек на
//! мониторе — лого и короткая строка состояния вместо потока отладки.
//! Так сделано намеренно: весь цикл отладки в headless-QEMU держится на
//! `-serial stdio`, и если убрать вывод совсем, диагностировать поломки
//! станет нечем.
//!
//! ## Формат картинки
//!
//! Декодера PNG в ядре нет, а сырой RGB не влезает: `kernel.bin`
//! физически ограничен (буфер загрузчика упирается в память).
//! Поэтому лого хранится в собственном формате `DXLG`: палитра до 255
//! цветов + RLE. 256x256 из 16 цветов (со сглаженными краями) занимает ~7 КиБ вместо 192 КиБ сырого RGB.
//!
//! ```text
//!   0   4   магия "DXLG"
//!   4   2   ширина      (u16 LE)
//!   6   2   высота      (u16 LE)
//!   8   1   число цветов N
//!   9   3   резерв
//!   12  N*3 палитра RGB
//!   ..      RLE: пары [count 1..255][индекс]
//! ```

use crate::renderer::Color;
use core::sync::atomic::{AtomicU32, Ordering};

/// Встроенное лого (assets/logo.dxlg, собирается tools/make_logo_aa.py; шаг [2d/8] build.sh).
static LOGO: &[u8] = include_bytes!("../build/logo.dxlg");

const MAGIC: &[u8; 4] = b"DXLG";
/// Цвет фона загрузочного экрана.
pub const BG: Color = Color(0x000000);
/// Цвет строки состояния.
pub const FG: Color = Color(0x8FB8D8);
/// Цвет подписи под лого.
pub const TITLE: Color = Color(0x00D0FF);

/// Число этапов загрузки, о которых сообщается на экране: старт,
/// проверка разделов, память, готовность системы.
const BOOT_STEPS: u32 = 4;
static BOOT_STEP: AtomicU32 = AtomicU32::new(0);

/// Разобранный заголовок DXLG.
pub struct Logo<'a> {
    pub w: u32,
    pub h: u32,
    pub palette: &'a [u8],
    pub ncolors: usize,
    pub rle: &'a [u8],
}

pub fn parse(data: &[u8]) -> Option<Logo<'_>> {
    if data.len() < 12 || &data[..4] != MAGIC {
        return None;
    }
    let w = u16::from_le_bytes([data[4], data[5]]) as u32;
    let h = u16::from_le_bytes([data[6], data[7]]) as u32;
    let n = data[8] as usize;
    let pal_end = 12 + n * 3;
    if n == 0 || data.len() < pal_end {
        return None;
    }
    Some(Logo {
        w,
        h,
        palette: &data[12..pal_end],
        ncolors: n,
        rle: &data[pal_end..],
    })
}

impl Logo<'_> {
    pub fn color(&self, idx: usize) -> Color {
        if idx >= self.ncolors {
            return BG;
        }
        let o = idx * 3;
        Color(
            ((self.palette[o] as u32) << 16)
                | ((self.palette[o + 1] as u32) << 8)
                | self.palette[o + 2] as u32,
        )
    }
}

/// Ширина встроенного лого в пикселях (0, если оно повреждено).
pub fn size() -> u32 {
    parse(LOGO).map_or(0, |l| l.w)
}

/// Рисует лого по координатам (ox, oy) с заданным целочисленным масштабом.
/// Индекс 0 считается фоном (прозрачным) и не закрашивается.
/// Возвращает координату нижней границы (oy + sh).
pub fn draw_logo_at(r: &mut crate::renderer::Renderer, ox: i32, oy: i32, scale: i32) -> i32 {
    draw_logo_div(r, ox, oy, scale, 1)
}

/// То же, но берёт каждый `div`-й пиксель по обеим осям: лого 256x256 при
/// `div` = 2 занимает 128x128 (экран входа, где места меньше).
pub fn draw_logo_div(r: &mut crate::renderer::Renderer, ox: i32, oy: i32, scale: i32, div: u32) -> i32 {
    let logo = match parse(LOGO) {
        Some(l) => l,
        None => return oy,
    };
    let scale = scale.max(1);
    let div = div.max(1);
    let sh = logo.h.div_ceil(div) as i32 * scale;
    let total = logo.w * logo.h;

    let mut pos: u32 = 0;
    let mut i = 0usize;

    while i + 1 < logo.rle.len() && pos < total {
        let count = logo.rle[i] as u32;
        let idx = logo.rle[i + 1] as usize;
        i += 2;
        if count == 0 {
            continue;
        }

        let mut rem = count;
        while rem > 0 && pos < total {
            let px = pos % logo.w;
            let py = pos / logo.w;
            let chunk = core::cmp::min(rem, logo.w - px);
            if idx != 0 && py % div == 0 {
                let first = px.div_ceil(div);
                let last = (px + chunk).div_ceil(div);
                if last > first {
                    r.fill_rect(
                        ox + first as i32 * scale,
                        oy + (py / div) as i32 * scale,
                        (last - first) * scale as u32,
                        scale as u32,
                        logo.color(idx),
                    );
                }
            }
            rem -= chunk;
            pos += chunk;
        }
    }
    oy + sh
}

/// Рисует строку увеличенным шрифтом: каждый пиксель глифа —
/// квадрат scale x scale. Родной draw_text даёт 8x16, что на 800x600
/// мелковато для загрузочного экрана.
///
/// ВАЖНО: идём по БАЙТАМ, как и draw_text, поэтому кириллица (UTF-8,
/// два байта на символ) здесь рассыпется. На загрузочном экране
/// используем только латиницу.
pub fn draw_text_scaled(
    r: &mut crate::renderer::Renderer,
    text: &str,
    y: i32,
    color: Color,
    scale: i32,
) {
    let scale = scale.max(1);
    let w = text.len() as i32 * 8 * scale;
    let mut x = (r.width() as i32 - w) / 2;
    for byte in text.bytes() {
        let glyph = crate::font::read_glyph(byte);
        for (row, &bits) in glyph.iter().enumerate() {
            if bits == 0 {
                continue;
            }
            let mut col = 0i32;
            while col < 8 {
                if (bits >> (7 - col)) & 1 == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < 8 && (bits >> (7 - col)) & 1 != 0 {
                    col += 1;
                }
                r.fill_rect(
                    x + start * scale,
                    y + row as i32 * scale,
                    ((col - start) * scale) as u32,
                    scale as u32,
                    color,
                );
            }
        }
        x += 8 * scale;
    }
}

/// Полоса прогресса под строкой состояния: заполнена на долю
/// пройденных этапов загрузки.
fn draw_progress(r: &mut crate::renderer::Renderer, bottom: i32, step: u32) {
    let bar_w = (r.width() as i32 * 4 / 10).clamp(120, 420);
    let bar_h = 8;
    let x = (r.width() as i32 - bar_w) / 2;
    let y = bottom + 140;
    r.fill_rounded_rect(x, y, bar_w as u32, bar_h as u32, 4, Color(0x16222E));
    let filled = bar_w * step.min(BOOT_STEPS) as i32 / BOOT_STEPS as i32;
    if filled > 0 {
        r.fill_rounded_rect(x, y, filled as u32, bar_h as u32, 4, TITLE);
        r.fill_fade_v(x + 4, y, (filled - 8).max(0) as u32, bar_h as u32 / 2, Color::WHITE, 70, 0);
    }
}

/// Вычисляет масштаб и вертикальную позицию загрузочного экрана.
fn compute_layout(screen_w: u32, screen_h: u32, logo_w: u32, logo_h: u32) -> (i32, i32, i32, i32) {
    let scale = core::cmp::max(1, (screen_h as i32 * 5 / 10) / logo_h as i32).min(4);
    let sw = logo_w as i32 * scale;
    let sh = logo_h as i32 * scale;
    let ox = (screen_w as i32 - sw) / 2;
    let oy = (screen_h as i32 - sh) / 2 - 50;
    (scale, ox, oy, oy + sh)
}

/// Показывает загрузочный экран: лого, название и строка состояния.
///
/// Если графики нет (не удалось включить framebuffer) — молча выходит,
/// текстовый вывод продолжает работать как раньше.
pub fn show(status: &str) {
    show_screen(status, true);
}

fn show_screen(status: &str, with_bar: bool) {
    crate::renderer::ensure_framebuffer();
    if !crate::renderer::has_active_framebuffer() {
        return;
    }

    let logo = match parse(LOGO) {
        Some(l) => l,
        None => {
            crate::serial_println!("[bootlogo] встроенное лого повреждено");
            return;
        }
    };

    crate::renderer::with_renderer(|r| {
        r.clear(BG);

        let (scale, ox, oy, bottom) = compute_layout(r.width(), r.height(), logo.w, logo.h);
        draw_logo_at(r, ox, oy, scale);

        draw_text_scaled(r, "DeiX", bottom + 24, TITLE, 4);
        draw_text_scaled(r, status, bottom + 88, FG, 2);
        if with_bar {
            BOOT_STEP.store(1, Ordering::Relaxed);
            draw_progress(r, bottom, 1);
        }

        r.present();
    });
}

/// Обновляет только строку состояния под лого (без перерисовки лого).
pub fn set_status(status: &str) {
    if !crate::renderer::has_active_framebuffer() {
        return;
    }
    let logo = match parse(LOGO) {
        Some(l) => l,
        None => return,
    };
    crate::renderer::with_renderer(|r| {
        let (_scale, _ox, _oy, bottom) = compute_layout(r.width(), r.height(), logo.w, logo.h);
        let y = bottom + 88;
        // Затираем область статуса и пишем новую строку
        r.fill_rect(0, y - 4, r.width(), 44, BG);
        draw_text_scaled(r, status, y, FG, 2);
        let step = (BOOT_STEP.load(Ordering::Relaxed) + 1).min(BOOT_STEPS);
        BOOT_STEP.store(step, Ordering::Relaxed);
        draw_progress(r, bottom, step);
        r.present();
    });
}

/// Размер встроенного лого в байтах — для `logo info`.
pub fn blob_size() -> usize {
    LOGO.len()
}

/// CLI: `logo [show|info]`.
pub fn cmd_logo(arg: &str) {
    match arg.trim() {
        "info" => {
            crate::println!("  Встроенное лого: {} байт", blob_size());
            match parse(LOGO) {
                Some(l) => crate::println!(
                    "  {}x{}, палитра {} цв., RLE {} байт",
                    l.w,
                    l.h,
                    l.ncolors,
                    l.rle.len()
                ),
                None => crate::println!("  формат повреждён"),
            }
        }
        _ => {
            show_screen("Press Esc, Enter or Space to return", false);
            loop {
                if let Some(ch) = crate::keyboard::try_read_char() {
                    if ch == 0x1b || ch == b'\n' || ch == b'\r' || ch == b' ' || ch == b'q' || ch == b'Q' {
                        break;
                    }
                }
                if crate::serial::is_data_ready() {
                    let b = crate::serial::read_byte();
                    if b == 0x1b || b == b'\n' || b == b'\r' || b == b' ' || b == b'q' || b == b'Q' {
                        break;
                    }
                }
                crate::timer::pit_sleep_ms(20);
            }
            crate::loginui::back_to_text();
        }
    }
}
