//! Программный 2D-рендерер поверх линейного framebuffer (полученного через
//! драйвер Bochs VBE, см. vbe.rs). Открытый, честный software rendering —
//! без GPU-акселерации (у нас её и не может быть без проприетарного
//! драйвера конкретного чипа, см. gpu.rs), зато работает на любом
//! железе/эмуляторе, который умеет дать линейный framebuffer 32bpp.
//!
//! Примитивы: точка, линия, прямоугольник (залитый/контур), текст (через
//! уже загруженный в VGA-память битмап-шрифт, который мы читаем оттуда же,
//! где для текстового режима — переиспользуем один и тот же шрифт).

use crate::vbe::Framebuffer;
use alloc::vec;
use alloc::vec::Vec;

/// Целочисленный квадратный корень (метод Ньютона) — используется только
/// для рисования закруглённых углов окон/кнопок. В ядре нет доступа к
/// плавающей точке через libm (no_std, без FPU-рантайма), а точность
/// нужна лишь на несколько пикселей радиуса, поэтому обычная целочисленная
/// итерация полностью достаточна и работает быстро (обычно 3-5 итераций
/// на радиусы вроде 4-16, которые тут и используются).
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Color(pub u32); // формат 0x00RRGGBB, как того требует Bochs VBE 32bpp

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color(((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
    }

    /// Конструктор из сырого 0x00RRGGBB (используется DUIL/композитором).
    pub const fn from_u32(raw: u32) -> Color {
        Color(raw & 0x00FF_FFFF)
    }

    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const RED: Color = Color::rgb(220, 50, 50);
    pub const GREEN: Color = Color::rgb(50, 180, 90);
    pub const BLUE: Color = Color::rgb(40, 110, 200);
    pub const GRAY: Color = Color::rgb(160, 160, 160);
    pub const DARK_GRAY: Color = Color::rgb(60, 60, 60);
    pub const LIGHT_GRAY: Color = Color::rgb(210, 210, 210);
    pub const DESKTOP_BLUE: Color = Color::rgb(0, 90, 158);
    pub const TITLEBAR_ACTIVE: Color = Color::rgb(30, 90, 190);
    pub const TITLEBAR_INACTIVE: Color = Color::rgb(120, 120, 120);
    pub const YELLOW: Color = Color::rgb(230, 180, 40);
    pub const SHADOW: Color = Color::rgb(10, 12, 18);

    /// Линейная интерполяция между двумя цветами (0..=255 -> self..other).
    /// Используется для градиентов (см. Renderer::fill_rect_gradient_v) и
    /// для затенения/подсветки одного и того же базового цвета вместо
    /// хранения отдельной палитры под каждое состояние (hover/active).
    pub fn lerp(self, other: Color, t: u8) -> Color {
        let t = t as i32;
        let (r1, g1, b1) = self.components();
        let (r2, g2, b2) = other.components();
        let r = r1 + (r2 - r1) * t / 255;
        let g = g1 + (g2 - g1) * t / 255;
        let b = b1 + (b2 - b1) * t / 255;
        Color::rgb(r as u8, g as u8, b as u8)
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


/// Программный рендерер поверх framebuffer с ДВОЙНОЙ буферизацией: все
/// примитивы (put_pixel/fill_rect/draw_text/...) пишут в обычный массив в
/// обычной RAM (`back_buffer`), и только явный вызов `present()` в конце
/// кадра одним проходом копирует готовую картинку в реальную видеопамять
/// (MMIO framebuffer, полученный через Bochs VBE — см. vbe.rs).
///
/// Зачем это нужно: MMIO-запись в видеопамять ощутимо медленнее обычной
/// записи в RAM, а рисование "по кусочкам" прямо в framebuffer означает,
/// что монитор/VNC-клиент может отобразить кадр в процессе его отрисовки —
/// именно так проявлялся эффект "разрывов"/подтормаживания текстур окон
/// при перетаскивании: старое содержимое одной части экрана успевало
/// смешаться с новым содержимым другой части, потому что между отрисовкой
/// разных окон кадра проходило заметное (для VGA-развёртки) время. Один
/// быстрый `memcpy`-подобный проход в конце кадра устраняет этот эффект
/// (классический паттерн double buffering, как в любом настоящем оконном
/// менеджере).
pub struct Renderer {
    fb: Framebuffer,
    back_buffer: Vec<u32>,
}

impl Renderer {
    pub fn new(fb: Framebuffer) -> Self {
        let pixel_count = (fb.width as usize) * (fb.height as usize);
        let back_buffer = vec![0u32; pixel_count];
        Renderer { fb, back_buffer }
    }

    pub fn width(&self) -> u32 {
        self.fb.width
    }

    pub fn height(&self) -> u32 {
        self.fb.height
    }

    #[inline]
    pub fn put_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 || x as u32 >= self.fb.width || y as u32 >= self.fb.height {
            return;
        }
        let idx = y as usize * self.fb.width as usize + x as usize;
        self.back_buffer[idx] = color.0;
    }

    /// Копирует весь собранный кадр из back buffer (обычная RAM) в
    /// реальный MMIO framebuffer одним быстрым проходом. Вызывается один
    /// раз в конце отрисовки каждого кадра (см.
    /// ui/mod.rs::Desktop::render). Вся "сборка" картинки (стирание фона,
    /// рисование окон, текста, курсора) происходит ДО этого вызова в
    /// обычной памяти — здесь же только перенос уже готовых пикселей, что
    /// заметно быстрее, чем писать в MMIO при каждом отдельном put_pixel,
    /// как было раньше (именно это и вызывало эффект "лагающих текстур" —
    /// монитор успевал показать промежуточное, ещё не до конца
    /// перерисованное состояние экрана).
    pub fn present(&mut self) {
        // Копируем построчно через copy_nonoverlapping (обычный memcpy)
        // вместо write_volatile на каждый пиксель по отдельности.
        // Framebuffer видеопамяти — это просто линейный массив байт для
        // отображения на экран (не управляющие регистры устройства),
        // поэтому здесь не важен порядок отдельных записей внутри одной
        // строки — важно лишь, чтобы вся строка была записана до того,
        // как её начнёт сканировать видеоконтроллер, что уже гарантируется
        // тем, что copy_nonoverlapping — это одна непрерывная операция.
        // На практике это в разы быстрее покиксельного цикла и заметно
        // снижает время кадра при больших разрешениях (1024x768+).
        for row in 0..self.fb.height as usize {
            let src_start = row * self.fb.width as usize;
            let row_pixels = &self.back_buffer[src_start..src_start + self.fb.width as usize];

            let dst = (self.fb.addr + row * self.fb.pitch) as *mut u32;
            unsafe {
                core::ptr::copy_nonoverlapping(row_pixels.as_ptr(), dst, row_pixels.len());
            }
        }
    }

    /// Заливка прямоугольника. Оптимизировано через `slice::fill` вместо
    /// покиксельного put_pixel — заметно быстрее для больших областей
    /// (весь фон рабочего стола каждый кадр, тела окон), потому что
    /// LLVM компилирует `[u32]::fill` в широкие векторизованные записи
    /// вместо цикла с проверкой границ на каждый пиксель. Отсечение по
    /// границам экрана делается один раз для всего прямоугольника, а не
    /// на каждый отдельный пиксель, как раньше.
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
    }

    /// Прямоугольник с вертикальным градиентом (top -> bottom) — основа
    /// для "живого" фона рабочего стола и панели задач вместо плоской
    /// заливки одним цветом.
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

    /// Затемняет уже нарисованный прямоугольник, смешивая существующие
    /// пиксели back buffer с чёрным на заданную "силу" (0=без изменений,
    /// 255=полностью чёрный). Честная имитация полупрозрачности без
    /// настоящего альфа-канала (наш формат — 0x00RRGGBB, 32bpp без alpha)
    /// — используется для мягкой тени под окнами и для затемнения фона
    /// позади открытого меню "Пуск".
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
    }

    /// Закруглённый прямоугольник — просто обычная заливка с "срезанными"
    /// по кругу углами (радиус в пикселях). Даёт окнам/кнопкам менее
    /// "коробочный" вид без необходимости в честном антиалиасинге.
    pub fn fill_rounded_rect(&mut self, x: i32, y: i32, w: u32, h: u32, radius: i32, color: Color) {
        let r = radius.min(w as i32 / 2).min(h as i32 / 2).max(0);
        if r == 0 {
            self.fill_rect(x, y, w, h, color);
            return;
        }
        // Средняя часть без закруглений сверху/снизу.
        self.fill_rect(x, y + r, w, h - (2 * r) as u32, color);
        // Верхняя и нижняя полосы — по строкам, с сужающимся отступом
        // по окружности (целочисленная окружность через x^2+y^2<=r^2).
        for row in 0..r {
            let dy = r - row;
            let dx = isqrt((r * r - dy * dy).max(0));
            let line_w = (w as i32 - 2 * (r - dx)).max(0) as u32;
            self.draw_hline(x + (r - dx), y + row, line_w, color);
            self.draw_hline(x + (r - dx), y + h as i32 - 1 - row, line_w, color);
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
    }

    pub fn draw_vline(&mut self, x: i32, y: i32, h: u32, color: Color) {
        for row in 0..h as i32 {
            self.put_pixel(x, y + row, color);
        }
    }

    /// Алгоритм Брезенхэма для произвольных линий (нужен для курсора мыши
    /// и рамок).
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
    }

    pub fn clear(&mut self, color: Color) {
        self.fill_rect(0, 0, self.fb.width, self.fb.height, color);
    }

    /// Рисует один символ через тот же битмап-шрифт 8x16, что и текстовый
    /// VGA-режим — читаем его прямо из памяти VGA-шрифта (plane 2), куда
    /// он уже загружен (штатные ASCII-глифы) либо куда мы сами дозагрузили
    /// кириллицу (см. font.rs).
    pub fn draw_char(&mut self, x: i32, y: i32, ch: u8, fg: Color, bg: Option<Color>) {
        // Фон символа (если задан) заливаем одним fill_rect вместо 8x16
        // отдельных put_pixel — так же быстрее и для одноцветных пикселей
        // текста, потому что убирает повторную проверку границ экрана на
        // каждый отдельный бит глифа (она теперь делается один раз в
        // put_pixel только для действительно закрашиваемых точек шрифта).
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

// ---------------- глобальный доступ (аналог vgaglobal.rs) ----------------
//
// В отличие от VGA text writer, framebuffer появляется не сразу при
// старте ядра, а только после успешного gpu mode (может вообще не
// появиться, если подходящей видеокарты нет) — поэтому используем
// Option<Renderer> под тем же спинлоком с отключением прерываний.

use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;

static ACTIVE_RENDERER: SpinLock<Option<Renderer>> = SpinLock::new(None);

pub fn set_active_framebuffer(fb: Framebuffer) {
    without_interrupts(|| {
        *ACTIVE_RENDERER.lock() = Some(Renderer::new(fb));
    });
}

/// Гарантирует наличие активного framebuffer'а: если его ещё нет —
/// определяет GPU и включает режим 800x600 32bpp через Bochs VBE.
/// Используется графическими оболочками (recovery/fastbootd/DSM) при
/// старте — они запускаются из boot_flow, где видеорежим ещё не включён.
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

/// Сброс активного рендерера в None (install: framebuffer-указатель из
/// живой сессии недействителен на установленной системе).
pub fn reset_renderer() {
    *ACTIVE_RENDERER.lock() = None;
}

pub fn has_active_framebuffer() -> bool {
    without_interrupts(|| ACTIVE_RENDERER.lock().is_some())
}

/// То же, что `with_renderer`, но возвращает значение из замыкания.
/// Нужно, когда по состоянию рендерера надо что-то вычислить —
/// например, попал ли клик мыши в поле ввода.
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

/// Аналог with_renderer, но НЕ отключает прерывания на всё время работы
/// замыкания — предназначен для долгих операций (например, целого
/// интерактивного event loop UI), которые сами внутри себя блокирующе
/// ждут прерываний (таймер для кадров, клавиатура/мышь для ввода). Если
/// использовать здесь without_interrupts на весь вызов, как в
/// with_renderer, то IF навсегда останется 0 и hlt внутри замыкания
/// проснётся максимум один раз, после чего цикл зависнет навечно —
/// именно так и проявлялся баг при первой реализации `gpu demo`/
/// `gpu mode`.
///
/// Безопасно в нашем однопроцессорном ядре: единственный "конкурент" за
/// ACTIVE_RENDERER — это код рисования одного кадра, а сами обработчики
/// прерываний (таймер/клавиатура/мышь) рендерер не трогают, поэтому нет
/// риска гонки за сам SpinLock во время растянутого во времени вызова f.
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
