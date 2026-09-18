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
//! физически ограничен 572 КиБ (буфер загрузчика упирается в
//! видеопамять VGA — см. boot/stage2.asm). Поэтому лого хранится в
//! собственном формате `DXLG`: палитра до 255 цветов + RLE.
//! 256x256 из 5 цветов занимает 5 КиБ вместо 192 КиБ сырого RGB.
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

/// Встроенное лого (собирается из assets/logo.png шагом [2d/8] build.sh).
static LOGO: &[u8] = include_bytes!("../build/logo.dxlg");

const MAGIC: &[u8; 4] = b"DXLG";
/// Цвет фона загрузочного экрана.
const BG: Color = Color(0x000000);
/// Цвет строки состояния.
const FG: Color = Color(0x8FB8D8);
/// Цвет подписи под лого.
const TITLE: Color = Color(0x00D0FF);

/// Разобранный заголовок DXLG.
struct Logo<'a> {
    w: u32,
    h: u32,
    palette: &'a [u8],
    ncolors: usize,
    rle: &'a [u8],
}

fn parse(data: &[u8]) -> Option<Logo<'_>> {
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
    fn color(&self, idx: usize) -> Color {
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

/// Рисует лого по центру экрана. `scale` — целочисленное увеличение.
fn draw_logo(r: &mut crate::renderer::Renderer, logo: &Logo, scale: i32) -> i32 {
    let sw = logo.w as i32 * scale;
    let sh = logo.h as i32 * scale;
    let ox = (r.width() as i32 - sw) / 2;
    // Чуть выше центра: под лого останется место для подписи и статуса.
    let oy = (r.height() as i32 - sh) / 2 - 40;

    let mut pos: u32 = 0; // номер пикселя в распакованном потоке
    let total = logo.w * logo.h;
    let mut i = 0usize;

    while i + 1 < logo.rle.len() && pos < total {
        let count = logo.rle[i] as u32;
        let idx = logo.rle[i + 1] as usize;
        i += 2;
        if count == 0 {
            continue;
        }
        // Прозрачным считаем индекс 0 (фон лого) — не закрашиваем,
        // чтобы лого ложилось на любой фон без рамки.
        if idx != 0 {
            let color = logo.color(idx);
            for k in 0..count {
                let p = pos + k;
                if p >= total {
                    break;
                }
                let px = (p % logo.w) as i32;
                let py = (p / logo.w) as i32;
                // Один пиксель лого = квадрат scale x scale.
                r.fill_rect(
                    ox + px * scale,
                    oy + py * scale,
                    scale as u32,
                    scale as u32,
                    color,
                );
            }
        }
        pos += count;
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
fn draw_text_scaled(
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
            for col in 0..8i32 {
                if (bits >> (7 - col)) & 1 != 0 {
                    r.fill_rect(
                        x + col * scale,
                        y + row as i32 * scale,
                        scale as u32,
                        scale as u32,
                        color,
                    );
                }
            }
        }
        x += 8 * scale;
    }
}

/// Показывает загрузочный экран: лого, название и строка состояния.
///
/// Если графики нет (не удалось включить framebuffer) — молча выходит,
/// текстовый вывод продолжает работать как раньше.
pub fn show(status: &str) {
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

        // Масштаб: лого занимает ~60% высоты экрана, ниже остаётся
        // место под подпись и строку состояния.
        let scale = core::cmp::max(1, (r.height() as i32 * 6 / 10) / logo.h as i32);
        let scale = core::cmp::min(scale, 6); // не раздуваем сверх меры
        let bottom = draw_logo(r, &logo, scale);

        // Название рисуем родным шрифтом ядра: он чёткий на любом
        // разрешении, в отличие от текста, вплавленного в картинку.
        // ВАЖНО: draw_text идёт по БАЙТАМ, поэтому кириллица (UTF-8, два
        // байта на символ) рассыпается на мусор. В графическом
        // загрузочном экране используем только латиницу.
        draw_text_scaled(r, "DeiX", bottom + 24, TITLE, 4);
        draw_text_scaled(r, status, bottom + 96, FG, 2);

        r.present();
    });
}

/// Обновляет только строку состояния под лого (без перерисовки лого).
pub fn set_status(status: &str) {
    if !crate::renderer::has_active_framebuffer() {
        return;
    }
    crate::renderer::with_renderer(|r| {
        let h = r.height() as i32;
        let y = h / 2 + 170;
        // Затираем прошлую строку и пишем новую.
        r.fill_rect(0, y, r.width(), 10, BG);
        let sx = (r.width() as i32 - (status.len() as i32 * 8)) / 2;
        r.draw_text(sx, y, status, FG, None);
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
            show("нажмите Esc для выхода");
            crate::println!("  Лого показано (Esc — вернуться в текстовый режим)");
        }
    }
}
