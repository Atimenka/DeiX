//! Отрисовка медиа-приложений: просмотр фото, видеоплеер и плеер музыки.
//!
//! Геометрия интерактивных элементов задана функциями ниже (координаты
//! относительно области содержимого окна); обработчик кликов в
//! `desktop.rs` обязан использовать те же прямоугольники — невидимых
//! хитбоксов нет.
//!
//! Раскладка видеоплеера/плеера музыки (низ окна):
//!   transport_h = 44 px — кнопки, полоса прогресса, громкость.
//! Раскладка просмотрщика фото:
//!   режим библиотеки — список файлов слева (LIST_W), картинка справа;
//!   режим одного файла — картинка на всё окно и строка сведений внизу.

use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::Window;
use crate::{media, png};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Высота транспортной панели плееров.
pub const TRANSPORT_H: i32 = 44;
/// Первая строка списка (плейлист/библиотека).
pub const LIST_START_Y: i32 = 28;
/// Шаг строки списка.
pub const ROW_H: i32 = 22;
/// Ширина колонки библиотеки в просмотрщике фото.
pub const LIB_W: i32 = 190;
/// Предел числа файлов в списках (защита от огромных каталогов).
pub const LIST_CAP: usize = 64;

// --------------------------------------------------------------- геометрия

pub fn row_rect(win_w: u32, i: usize) -> Rect {
    Rect::new(6, LIST_START_Y + i as i32 * ROW_H, (win_w as i32 - 12).max(20) as u32, ROW_H as u32 - 2)
}

/// Кнопка Play/Pause (общая у видео и музыки, но со своим x).
pub fn play_rect(win_h: u32, music: bool) -> Rect {
    let x = if music { 44 } else { 10 };
    Rect::new(x, win_h as i32 - TRANSPORT_H + 8, 28, 28)
}

pub fn stop_rect(win_h: u32, music: bool) -> Rect {
    let x = if music { 78 } else { 44 };
    Rect::new(x, win_h as i32 - TRANSPORT_H + 8, 28, 28)
}

pub fn prev_rect(win_h: u32) -> Rect {
    Rect::new(10, win_h as i32 - TRANSPORT_H + 8, 28, 28)
}

pub fn next_rect(win_h: u32) -> Rect {
    Rect::new(112, win_h as i32 - TRANSPORT_H + 8, 28, 28)
}

/// Кнопка «список» в правом верхнем углу видео-окна (возврат к списку).
pub fn list_btn_rect(win_w: u32) -> Rect {
    Rect::new(win_w as i32 - 40, 4, 34, 22)
}

/// Полоса прогресса: от кнопок до регулятора громкости.
pub fn bar_rect(win_w: u32, win_h: u32, music: bool) -> Rect {
    let x0 = if music { 150 } else { 84 };
    let x1 = win_w as i32 - 96;
    Rect::new(x0, win_h as i32 - TRANSPORT_H + 18, (x1 - x0).max(30) as u32, 8)
}

pub fn vol_dn_rect(win_w: u32, win_h: u32) -> Rect {
    Rect::new(win_w as i32 - 84, win_h as i32 - TRANSPORT_H + 8, 32, 28)
}

pub fn vol_up_rect(win_w: u32, win_h: u32) -> Rect {
    Rect::new(win_w as i32 - 48, win_h as i32 - TRANSPORT_H + 8, 32, 28)
}

/// Строка сведений просмотрщика фото (низ окна).
pub fn info_rect(win_w: u32, win_h: u32) -> Rect {
    Rect::new(0, win_h as i32 - 22, win_w, 22)
}

// --------------------------------------------------------------- библиотеки

fn scan_dir(dir: &str, accept: fn(&str) -> bool, out: &mut Vec<String>) {
    if out.len() >= LIST_CAP {
        return;
    }
    let Ok(list) = crate::vfs::readdir(dir) else { return };
    let mut names: Vec<String> = list
        .into_iter()
        .filter(|e| !e.is_dir && accept(&e.name))
        .map(|e| e.name)
        .collect();
    names.sort();
    for n in names {
        if out.len() >= LIST_CAP {
            break;
        }
        out.push(format!("{}/{}", dir, n));
    }
}

/// Библиотека изображений: медиапак + загрузки пользователя.
pub fn collect_images() -> Vec<String> {
    let mut out = Vec::new();
    scan_dir("/userdata/media/pictures", media::is_image_file, &mut out);
    scan_dir("/userdata/media/wallpapers", media::is_image_file, &mut out);
    scan_dir("/userdata/downloads", media::is_image_file, &mut out);
    out
}

/// Библиотека видео: медиапак + загрузки.
pub fn collect_videos() -> Vec<String> {
    let mut out = Vec::new();
    scan_dir("/userdata/media/video", media::is_video_file, &mut out);
    scan_dir("/userdata/downloads", media::is_video_file, &mut out);
    out
}

/// Библиотека музыки: аудио медиапака, голосовые сообщения и загрузки.
pub fn collect_audio() -> Vec<String> {
    let mut out = Vec::new();
    scan_dir("/userdata/media/audio", media::is_audio_file, &mut out);
    scan_dir("/userdata/media/voice", media::is_audio_file, &mut out);
    scan_dir("/userdata/downloads", media::is_audio_file, &mut out);
    out
}

// --------------------------------------------------------------- декодеры

/// Декодирует PNG, JPEG (baseline) или несжатый BMP 24/32 в пиксели
/// 0xRRGGBB. Единая точка для просмотрщика фото, превью в Files и обоев.
pub fn decode_image(data: &[u8]) -> Result<(Vec<u32>, usize, usize), String> {
    if png::is_png(data) {
        let img = png::decode(data).map_err(|e| format!("PNG: {}", e))?;
        if img.w == 0 || img.h == 0 || img.pixels.len() < img.w * img.h {
            return Err(String::from("PNG: повреждённые размеры"));
        }
        return Ok((img.pixels, img.w, img.h));
    }
    if crate::jpeg::is_jpeg(data) {
        let img = crate::jpeg::decode(data).map_err(|e| e)?;
        return Ok((img.pixels, img.w, img.h));
    }
    if data.len() >= 54 && &data[..2] == b"BM" {
        return decode_bmp(data);
    }
    Err(String::from(
        "формат не распознан: поддерживаются PNG (8 бит), JPEG (baseline) и BMP 24/32 (BI_RGB)",
    ))
}

fn decode_bmp(data: &[u8]) -> Result<(Vec<u32>, usize, usize), String> {
    let off = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
    let bw = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
    let bh_raw = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
    let bpp = u16::from_le_bytes([data[28], data[29]]);
    let compression = u32::from_le_bytes([data[30], data[31], data[32], data[33]]);
    if compression != 0 || (bpp != 24 && bpp != 32) || bw <= 0 || bh_raw == 0 {
        return Err(String::from(
            "BMP: поддерживается только несжатый 24/32-битный (BI_RGB)",
        ));
    }
    let top_down = bh_raw < 0;
    let (w, h) = (bw as usize, bh_raw.unsigned_abs() as usize);
    if w.saturating_mul(h) > 2 * 1024 * 1024 {
        return Err(format!("BMP: размер {}x{} вне пределов (до 2 МПикс)", w, h));
    }
    let bytes_pp = (bpp / 8) as usize;
    let stride = (w * bytes_pp).div_ceil(4) * 4;
    if off + stride * h > data.len() {
        return Err(String::from("BMP: файл короче заявленных размеров"));
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
    Ok((pixels, w, h))
}

/// Читает и декодирует файл-картинку (для просмотрщика и превью Files).
pub fn decode_image_file(path: &str) -> Result<(Vec<u32>, usize, usize), String> {
    let data = crate::vfs::read_file(path).map_err(|e| format!("чтение {}: {}", path, e.message()))?;
    decode_image(&data)
}

// --------------------------------------------------------------- рисование

/// Картинка, вписанная в область (contain, ближайший сосед), на тёмной
/// подложке. Пустые пиксели — подложка без картинки.
pub fn draw_image_fit(
    r: &mut Renderer,
    area: Rect,
    pixels: &[u32],
    iw: usize,
    ih: usize,
) {
    r.fill_rect(area.x, area.y, area.w, area.h, Color::rgb(10, 10, 14));
    if iw == 0 || ih == 0 || pixels.len() < iw * ih {
        return;
    }
    let (aw, ah) = (area.w.max(1) as usize, area.h.max(1) as usize);
    // Масштаб contain: min(aw/iw, ah/ih), целочисленно.
    let (dw, dh) = if iw * ah < ih * aw {
        let dw = (iw * ah / ih).max(1);
        (dw, ah)
    } else {
        let dh = (ih * aw / iw).max(1);
        (aw, dh)
    };
    let ox = area.x + (aw - dw) as i32 / 2;
    let oy = area.y + (ah - dh) as i32 / 2;
    for dy in 0..dh {
        let sy = dy * ih / dh;
        for dx in 0..dw {
            let sx = dx * iw / dw;
            r.put_pixel(ox + dx as i32, oy + dy as i32, Color(pixels[sy * iw + sx]));
        }
    }
}

fn fmt_time(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Транспортная панель: кнопки, прогресс, громкость. `music` расставляет
/// кнопки под плеер музыки (есть Prev/Next).
fn draw_transport(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    content_y: i32,
    music: bool,
) {
    let wh = w.height as i32;
    let ty = content_y + wh - TRANSPORT_H;
    r.fill_rect(w.x, ty, w.width, TRANSPORT_H as u32, theme.surface);
    r.draw_hline(w.x, ty, w.width, theme.text_secondary.lerp(Color::BLACK, 60));

    let st = media::state();
    let paused = st == media::ST_PAUSED;
    let btn_fg = theme.text_primary;

    let icons: Vec<(Rect, IconType)> = if music {
        alloc::vec![
            (prev_rect(w.height), IconType::SkipPrev),
            (play_rect(w.height, true), if paused { IconType::Play } else { IconType::Pause }),
            (stop_rect(w.height, true), IconType::Stop),
            (next_rect(w.height), IconType::SkipNext),
        ]
    } else {
        alloc::vec![
            (play_rect(w.height, false), if paused { IconType::Play } else { IconType::Pause }),
            (stop_rect(w.height, false), IconType::Stop),
        ]
    };
    for (rc, icon) in icons {
        r.fill_rounded_rect(w.x + rc.x, ty + rc.y - (wh - TRANSPORT_H), rc.w, rc.h, 6, theme.surface_alt);
        r.draw_icon(w.x + rc.x + 6, ty + rc.y - (wh - TRANSPORT_H) + 6, icon, btn_fg);
    }

    // Прогресс.
    let bar = bar_rect(w.width, w.height, music);
    let bx = w.x + bar.x;
    let by = ty + bar.y - (wh - TRANSPORT_H);
    r.fill_rounded_rect(bx, by, bar.w, bar.h, 4, theme.surface_alt);
    let dur = media::duration_ms() as u64;
    let prog = media::progress_ms() as u64;
    if dur > 0 {
        let fw = ((bar.w as u64 * prog.min(dur)) / dur) as u32;
        if fw > 0 {
            r.fill_rounded_rect(bx, by, fw, bar.h, 4, theme.accent);
        }
    }
    let time = format!("{} / {}", fmt_time(prog), fmt_time(dur));
    r.draw_text(bx, by - 14, &time, theme.text_secondary, None);

    // Громкость.
    for (rc, sign) in [(vol_dn_rect(w.width, w.height), '-'), (vol_up_rect(w.width, w.height), '+')] {
        let rx = w.x + rc.x;
        let ry = ty + rc.y - (wh - TRANSPORT_H);
        r.fill_rounded_rect(rx, ry, rc.w, rc.h, 6, theme.surface_alt);
        let cx = rx + rc.w as i32 / 2;
        let cy = ry + rc.h as i32 / 2;
        r.draw_hline(cx - 5, cy, 10, btn_fg);
        if sign == '+' {
            r.draw_vline(cx, cy - 5, 10, btn_fg);
        }
    }
    let vol = media::volume();
    r.draw_text(
        w.x + vol_dn_rect(w.width, w.height).x - 34,
        ty + 16,
        &format!("{}%", vol),
        theme.text_secondary,
        None,
    );
}

/// Заголовок списка + строки. `selected` — играющий/открытый элемент.
fn draw_list(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    content_y: i32,
    header: &str,
    list: &[String],
    selected: Option<usize>,
    now_playing: &str,
    x_off: i32,
    width: i32,
    rows_area_h: i32,
) {
    r.draw_text(w.x + x_off + 6, content_y + 8, header, theme.text_secondary, None);
    if list.is_empty() {
        r.draw_text(
            w.x + x_off + 6,
            content_y + LIST_START_Y + 4,
            "(пусто — см. 'media install')",
            theme.text_secondary,
            None,
        );
        return;
    }
    let rows = ((rows_area_h - LIST_START_Y) / ROW_H).max(1) as usize;
    for (i, path) in list.iter().enumerate().take(rows) {
        let rc = Rect::new(x_off + 6, LIST_START_Y + i as i32 * ROW_H, (width - 12).max(20) as u32, ROW_H as u32 - 2);
        let is_sel = selected == Some(i);
        let is_playing = !now_playing.is_empty() && now_playing == path.as_str();
        if is_sel || is_playing {
            r.fill_rounded_rect(
                w.x + rc.x,
                content_y + rc.y,
                rc.w,
                rc.h,
                5,
                if is_playing { theme.accent.lerp(Color::BLACK, 55) } else { theme.surface_alt },
            );
        }
        let name = media::file_name(path);
        let cells = ((rc.w as i32 - 22) / 8).max(4) as usize;
        let shown = crate::ui::text::truncate_to_cells(name, cells).0;
        r.draw_text(w.x + rc.x + 18, content_y + rc.y + 3, shown, theme.text_primary, None);
        r.draw_icon(w.x + rc.x + 2, content_y + rc.y + 2, IconType::Document, theme.text_secondary);
    }
}

/// Строка состояния/ошибки над транспортной панелью (общая у плееров).
fn draw_status(r: &mut Renderer, theme: &UiTheme, w: &Window, content_y: i32, status: Option<&String>) {
    let st = media::state();
    let msg = if st == media::ST_ERROR {
        Some(format!("Ошибка: {}", media::error()))
    } else if st == media::ST_LOADING {
        Some(format!("Загрузка {}...", media::file_name(&media::now_playing())))
    } else {
        status.cloned()
    };
    if let Some(m) = msg {
        let cells = ((w.width as i32 - 20) / 8).max(8) as usize;
        let shown = crate::ui::text::truncate_to_cells(&m, cells).0;
        let color = if st == media::ST_ERROR { Color::rgb(248, 113, 113) } else { theme.text_secondary };
        let y = content_y + w.height as i32 - TRANSPORT_H - 18;
        r.draw_text(w.x + 10, y.max(content_y + 4), shown, color, None);
    }
}

// --------------------------------------------------------------- приложения

/// Просмотрщик фото. `library_mode` — список слева и превью справа;
/// иначе один файл на всё окно.
#[allow(clippy::too_many_arguments)]
pub fn draw_image_viewer(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    path: &str,
    library: &[String],
    selected: usize,
    img: &Option<(Vec<u32>, usize, usize)>,
    error: Option<&String>,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);
    let (ww, wh) = (w.width as i32, w.height as i32);

    if path.is_empty() && !library.is_empty() {
        // Библиотека: список слева, превью выбранного справа.
        draw_list(
            r, theme, w, content_y, "Photos (/userdata/media, downloads)",
            library, Some(selected), "", 0, LIB_W, wh,
        );
        let area = Rect::new(w.x + LIB_W, content_y, (ww - LIB_W).max(1) as u32, (wh - 22).max(1) as u32);
        match img {
            Some((px, iw, ih)) => draw_image_fit(r, area, px, *iw, *ih),
            None => {
                r.fill_rect(area.x, area.y, area.w, area.h, Color::rgb(10, 10, 14));
                let hint = error.map(String::as_str).unwrap_or("Выберите файл слева");
                r.draw_text(area.x + 12, area.y + 12, hint, theme.text_secondary, None);
            }
        }
        let info = match img {
            Some((_, iw, ih)) => format!("{}x{}", iw, ih),
            None => String::new(),
        };
        r.draw_text(w.x + LIB_W + 8, content_y + wh - 18, &info, theme.text_secondary, None);
        return;
    }

    // Один файл: картинка на всё окно, сведения внизу.
    let area = Rect::new(w.x, content_y, w.width, (wh - 22).max(1) as u32);
    match img {
        Some((px, iw, ih)) => draw_image_fit(r, area, px, *iw, *ih),
        None => {
            r.fill_rect(area.x, area.y, area.w, area.h, Color::rgb(10, 10, 14));
            let hint = error.map(String::as_str).unwrap_or("Нет изображения");
            // Сообщение может быть длинным — режем по ширине окна.
            let cells = ((ww - 24) / 8).max(8) as usize;
            r.draw_text(area.x + 12, area.y + 12, &crate::ui::text::truncate_to_cells(hint, cells).0, Color::rgb(248, 113, 113), None);
        }
    }
    let name = media::file_name(path);
    let size_kb = crate::vfs::stat(path).map(|s| s.size / 1024).unwrap_or(0);
    let dims = match img {
        Some((_, iw, ih)) => format!(" | {}x{}", iw, ih),
        None => String::new(),
    };
    let info = format!("{} | {} KiB{}", name, size_kb, dims);
    let cells = ((ww - 16) / 8).max(8) as usize;
    r.fill_rect(w.x, content_y + wh - 22, w.width, 22, theme.surface);
    r.draw_text(w.x + 8, content_y + wh - 18, &crate::ui::text::truncate_to_cells(&info, cells).0, theme.text_secondary, None);
}

/// Видеоплеер: список файлов (до запуска) или кадр + транспорт.
pub fn draw_video_player(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    list: &[String],
    idx: Option<usize>,
    list_mode: bool,
    status: Option<&String>,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);
    let (ww, wh) = (w.width as i32, w.height as i32);

    if list_mode {
        draw_list(
            r, theme, w, content_y, "Video (/userdata/media/video, downloads)",
            list, idx, &media::now_playing(), 0, ww, wh,
        );
        draw_status(r, theme, w, content_y, status);
        return;
    }

    // Видеообласть: копия текущего кадра движка (лок не держится на
    // время отрисовки — см. media::copy_frame).
    let area = Rect::new(w.x, content_y, w.width, (wh - TRANSPORT_H).max(1) as u32);
    match media::copy_frame() {
        Some((px, fw, fh)) => draw_image_fit(r, area, &px, fw, fh),
        None => r.fill_rect(area.x, area.y, area.w, area.h, Color::rgb(10, 10, 14)),
    }

    // Кнопка возврата к списку.
    let lb = list_btn_rect(w.width);
    r.fill_rounded_rect(w.x + lb.x, content_y + lb.y, lb.w, lb.h, 6, theme.surface_alt);
    r.draw_text(w.x + lb.x + 4, content_y + lb.y + 5, "List", theme.text_primary, None);

    draw_status(r, theme, w, content_y, status);
    draw_transport(r, theme, w, content_y, false);
}

/// Плеер музыки: плейлист сверху, транспорт снизу.
pub fn draw_music_player(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    list: &[String],
    idx: Option<usize>,
    status: Option<&String>,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);
    let wh = w.height as i32;
    draw_list(
        r, theme, w, content_y, "Music (/userdata/media/audio, voice, downloads)",
        list, idx, &media::now_playing(), 0, w.width as i32, wh - TRANSPORT_H,
    );
    draw_status(r, theme, w, content_y, status);
    draw_transport(r, theme, w, content_y, true);
}

/// Кнопка транспортной панели под точкой клика (rel_x/rel_y — от окна).
pub enum TransportButton {
    Prev,
    PlayPause,
    Stop,
    Next,
    VolDown,
    VolUp,
}

pub fn transport_button_at(rel_x: i32, rel_y: i32, win_w: u32, win_h: u32, music: bool) -> Option<TransportButton> {
    if music && prev_rect(win_h).contains(rel_x, rel_y) {
        return Some(TransportButton::Prev);
    }
    if play_rect(win_h, music).contains(rel_x, rel_y) {
        return Some(TransportButton::PlayPause);
    }
    if stop_rect(win_h, music).contains(rel_x, rel_y) {
        return Some(TransportButton::Stop);
    }
    if music && next_rect(win_h).contains(rel_x, rel_y) {
        return Some(TransportButton::Next);
    }
    if vol_dn_rect(win_w, win_h).contains(rel_x, rel_y) {
        return Some(TransportButton::VolDown);
    }
    if vol_up_rect(win_w, win_h).contains(rel_x, rel_y) {
        return Some(TransportButton::VolUp);
    }
    None
}

/// Какая строка списка под точкой клика (индекс в list).
pub fn row_at(rel_y: i32, len: usize, area_h: i32) -> Option<usize> {
    if rel_y < LIST_START_Y {
        return None;
    }
    let idx = ((rel_y - LIST_START_Y) / ROW_H) as usize;
    let rows = ((area_h - LIST_START_Y) / ROW_H).max(0) as usize;
    if idx < len && idx < rows {
        Some(idx)
    } else {
        None
    }
}
