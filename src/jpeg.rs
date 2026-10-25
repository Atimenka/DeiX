//! Декодер JPEG (ITU T.81): baseline-последовательный (SOF0), только
//! Huffman-кодирование, 8 бит на компоненту, 1 (градации серого) или
//! 3 (YCbCr/RGB) компоненты. Возвращает тот же `crate::png::Image`, что
//! и PNG-декодер, — просмотрщики и обои не различают форматы.
//!
//! Что честно НЕ поддерживается (сообщение говорит правду):
//! progressive (SOF2), lossless (SOF3), арифметическое кодирование
//! (SOF9/SOF10), 12 бит (SOF1), CMYK/YCCK (4 компоненты), цветовые
//! профили ICC (APP2 просто пропускается).
//!
//! Поддерживаются restart-маркеры (DRI/RSTn) и многоскановые файлы
//! (каждая компонента своим SOS — так сохраняет, например, Photoshop
//! несжатые по компонентам снимки).

use crate::png::Image;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Предел площади: planes трёх компонент (i16) + итоговые пиксели (u32)
/// при 1 МПикс занимают ~10 МиБ — половина кучи ядра (16 МиБ). Большие
/// снимки отклоняются честной ошибкой, а не паникой аллокатора.
const MAX_PIXELS: usize = 1024 * 1024;
const MAX_SIDE: usize = 8192;

/// Порядок обхода коэффициентов блока (T.81, рис. B.6): zigzag → natural.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27,
    20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58,
    59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

// ---- Целочисленный IDCT -------------------------------------------------
//
// s[x] = 1/2 * Σ_u C(u)·S[u]·cos((2x+1)uπ/16),  C(0)=1/√2, иначе 1.
// Косинусы с множителем C(u) и масштабом 2048 считаются на этапе
// компиляции (const fn, ряд Тейлора); рантайм — только i32.

const fn taylor_cos(mut x: f64) -> f64 {
    // Приведение к [0, 2π).
    let two_pi = 6.283185307179586;
    let pi = 3.141592653589793;
    let half_pi = 1.5707963267948966;
    while x >= two_pi {
        x -= two_pi;
    }
    while x < 0.0 {
        x += two_pi;
    }
    // Квадрант → аргумент [0, π/2] и знак косинуса.
    let (mut arg, mut sign) = (x, 1.0);
    if arg > half_pi && arg <= pi {
        arg = pi - arg;
        sign = -1.0;
    } else if arg > pi && arg <= pi + half_pi {
        arg = arg - pi;
        sign = -1.0;
    } else if arg > pi + half_pi {
        arg = two_pi - arg;
    }
    // cos(arg) рядом Тейлора; arg ≤ π/2 — 20 членов дают точность f64.
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut n = 0u32;
    while n < 20 {
        n += 1;
        term *= -arg * arg / ((2 * n - 1) as f64 * (2 * n) as f64);
        sum += term;
    }
    sign * sum
}

const fn build_idct_table() -> [[i32; 8]; 8] {
    let pi = 3.141592653589793;
    let inv_sqrt2 = 0.7071067811865476;
    let mut t = [[0i32; 8]; 8];
    let mut u = 0usize;
    while u < 8 {
        let cu = if u == 0 { inv_sqrt2 } else { 1.0 };
        let mut x = 0usize;
        while x < 8 {
            let angle = (2 * x + 1) as f64 * u as f64 * pi / 16.0;
            t[u][x] = (taylor_cos(angle) * cu * 2048.0 + 0.5) as i32;
            x += 1;
        }
        u += 1;
    }
    t
}

/// COS[u][x] * 2048 (u=0 уже содержит 1/√2).
const COS: [[i32; 8]; 8] = build_idct_table();

/// IDCT 8×8: `src` — коэффициенты (natural order), результат (0..255)
/// пишется в `dst[0..64]`. Два раздельных прохода по строкам/столбцам;
/// сдвиги: >>11 после первого, >>11 и 1/4-множитель (итого >>13) после
/// второго, +128 и насыщение.
fn idct_block(src: &[i32; 64], dst: &mut [i32; 64]) {
    let mut tmp = [0i32; 64];
    for row in 0..8 {
        for x in 0..8 {
            let mut acc = 0i32;
            for u in 0..8 {
                let c = src[row * 8 + u];
                if c != 0 {
                    acc += c * COS[u][x];
                }
            }
            tmp[row * 8 + x] = (acc + 1024) >> 11;
        }
    }
    for x in 0..8 {
        for y in 0..8 {
            let mut acc = 0i32;
            for u in 0..8 {
                let c = tmp[u * 8 + x];
                if c != 0 {
                    acc += c * COS[u][y];
                }
            }
            let v = (acc + 4096) >> 13;
            dst[y * 8 + x] = (v + 128).clamp(0, 255);
        }
    }
}

// ---- Huffman ------------------------------------------------------------

struct HuffTable {
    /// Число кодов каждой длины (1..=16).
    counts: [u16; 17],
    /// Символы, упорядоченные по длине кода.
    symbols: Vec<u8>,
    /// min_code[len], max_code[len], val_ptr[len] (T.81 Annex F).
    min_code: [u32; 17],
    max_code: [i32; 17],
    val_ptr: [usize; 17],
}

impl HuffTable {
    fn build(counts: [u16; 17], symbols: Vec<u8>) -> Self {
        let mut t = HuffTable {
            counts,
            symbols,
            min_code: [0; 17],
            max_code: [-1; 17],
            val_ptr: [0; 17],
        };
        let mut code = 0u32;
        let mut k = 0usize;
        for len in 1..=16usize {
            t.val_ptr[len] = k;
            t.min_code[len] = code;
            code += t.counts[len] as u32;
            if t.counts[len] == 0 {
                t.max_code[len] = -1;
            } else {
                t.max_code[len] = (code - 1) as i32;
            }
            k += t.counts[len] as usize;
            code <<= 1;
        }
        t
    }
}

/// Побитовый читатель энтропийных данных: убирает byte stuffing (FF 00)
/// и останавливается, встретив настоящий маркер (FF xx, xx != 00).
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u32,
    bits: u32,
    /// Встреченный маркер (RSTn, EOI, следующий сегмент) — скан
    /// останавливается, маркер обрабатывает вызывающий.
    marker: Option<u8>,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        BitReader { data, pos, acc: 0, bits: 0, marker: None }
    }

    fn next_bit(&mut self) -> Option<u32> {
        if self.bits == 0 {
            let b = *self.data.get(self.pos)?;
            self.pos += 1;
            if b == 0xFF {
                let n = *self.data.get(self.pos)?;
                self.pos += 1;
                if n != 0 {
                    self.marker = Some(n);
                    return None;
                }
            }
            self.acc = b as u32;
            self.bits = 8;
        }
        self.bits -= 1;
        Some((self.acc >> self.bits) & 1)
    }

    /// Добиться границы байта (перед restart-маркером хвост бит — единицы).
    fn align(&mut self) {
        self.bits = 0;
    }
}

fn decode_symbol(rd: &mut BitReader, t: &HuffTable) -> Result<u8, String> {
    let mut code = 0u32;
    for len in 1..=16usize {
        let Some(b) = rd.next_bit() else {
            return Err(String::from("JPEG: файл обрывается в Huffman-коде"));
        };
        code = (code << 1) | b;
        if t.max_code[len] >= 0 && code - t.min_code[len] < t.counts[len] as u32 {
            let idx = t.val_ptr[len] + (code - t.min_code[len]) as usize;
            return t.symbols.get(idx).copied()
                .ok_or_else(|| String::from("JPEG: Huffman-таблица короче кода"));
        }
    }
    Err(String::from("JPEG: недопустимый Huffman-код"))
}

fn receive_extend(rd: &mut BitReader, size: u8) -> Result<i32, String> {
    if size == 0 {
        return Ok(0);
    }
    let mut v = 0i32;
    for _ in 0..size {
        let Some(b) = rd.next_bit() else {
            return Err(String::from("JPEG: файл обрывается в расширении коэффициента"));
        };
        v = (v << 1) | b as i32;
    }
    if v < 1 << (size - 1) {
        v -= (1 << size) - 1;
    }
    Ok(v)
}

// ---- Сегменты и структура файла -----------------------------------------

struct Component {
    id: u8,
    h: u8,
    v: u8,
    tq: u8,
    /// Плоскость после IDCT (cw × ch, cw = ceil(w*h/hmax)).
    plane: Vec<i16>,
    cw: usize,
    ch: usize,
    dc: i32,
}

struct Frame {
    w: usize,
    h: usize,
    comps: Vec<Component>,
    restart_interval: u16,
    /// Adobe APP14 transform: 0 = RGB (без YCbCr-преобразования).
    adobe_rgb: bool,
    ids_are_rgb: bool,
}

pub fn is_jpeg(data: &[u8]) -> bool {
    data.len() > 4 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF
}

/// Декодирует baseline-JPEG в `Image` (0xRRGGBB).
pub fn decode(data: &[u8]) -> Result<Image, String> {
    if !is_jpeg(data) {
        return Err(String::from("нет SOI-маркера: это не JPEG"));
    }
    let mut quant: [Option<Vec<u16>>; 4] = [None, None, None, None];
    let mut huff: [Option<HuffTable>; 8] =
        [None, None, None, None, None, None, None, None];
    let mut frame: Option<Frame> = None;
    let mut pos = 2usize;
    let mut scans_done = 0usize;

    while pos + 1 < data.len() {
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }
        // Маркеры могут разделяться цепочкой FF.
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            break;
        }
        let marker = data[pos];
        pos += 1;
        match marker {
            0xD8 | 0x01 | (0xD0..=0xD7) => {} // без содержимого
            0xD9 => break,                    // EOI
            0xC0 => {
                let f = parse_sof(data, &mut pos, false)?;
                frame = Some(f);
            }
            0xC2 => return Err(String::from(
                "JPEG: progressive (SOF2) не поддерживается — нужен baseline (SOF0)",
            )),
            0xC3 => return Err(String::from(
                "JPEG: lossless (SOF3) не поддерживается",
            )),
            0xC1 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(format!(
                    "JPEG: тип кадра SOF{:#02X} не поддерживается (только baseline 8 бит)",
                    marker & 0x0F
                ))
            }
            0xC4 => parse_dht(data, &mut pos, &mut huff)?,
            0xDB => parse_dqt(data, &mut pos, &mut quant)?,
            0xDD => {
                let (seg, _) = segment(data, &mut pos)?;
                if seg.len() >= 2 {
                    if let Some(f) = frame.as_mut() {
                        f.restart_interval = u16::from_be_bytes([seg[0], seg[1]]);
                    }
                }
            }
            0xEE => {
                let (seg, _) = segment(data, &mut pos)?;
                // Adobe APP14: последний байт — color transform
                // (0 = RGB, 1 = YCbCr, 2 = YCCK).
                if seg.len() >= 12 && &seg[..5] == b"Adobe" {
                    if let Some(f) = frame.as_mut() {
                        f.adobe_rgb = seg[11] == 0;
                    }
                }
            }
            0xDA => {
                let Some(mut f) = frame.take() else {
                    return Err(String::from("JPEG: SOS до описания кадра (SOF)"));
                };
                parse_and_decode_scan(data, &mut pos, &mut f, &quant, &huff)?;
                scans_done += 1;
                frame = Some(f);
            }
            _ => {
                // APPn/COM/DNL и прочие: сегмент пропускается целиком.
                let (_, _) = segment(data, &mut pos)?;
            }
        }
    }

    let f = frame.ok_or_else(|| {
        if scans_done == 0 {
            String::from("JPEG: нет данных изображения (SOF/SOS не найдены)")
        } else {
            String::from("JPEG: файл обрывается после сканов")
        }
    })?;
    if scans_done == 0 {
        return Err(String::from("JPEG: нет данных изображения (SOS не найден)"));
    }
    to_image(&f)
}

/// Читает содержимое сегмента (длина u16 BE включает саму длину).
fn segment<'a>(data: &'a [u8], pos: &mut usize) -> Result<(&'a [u8], usize), String> {
    if *pos + 2 > data.len() {
        return Err(String::from("JPEG: сегмент обрезан"));
    }
    let len = u16::from_be_bytes([data[*pos], data[*pos + 1]]) as usize;
    if len < 2 || *pos + len > data.len() {
        return Err(String::from("JPEG: длина сегмента вне файла"));
    }
    let body = &data[*pos + 2..*pos + len];
    *pos += len;
    Ok((body, len))
}

fn parse_sof(data: &[u8], pos: &mut usize, _extended: bool) -> Result<Frame, String> {
    let (seg, _) = segment(data, pos)?;
    if seg.len() < 6 {
        return Err(String::from("JPEG: SOF короче заголовка"));
    }
    let precision = seg[0];
    if precision != 8 {
        return Err(format!("JPEG: {} бит на компоненту не поддерживается (только 8)", precision));
    }
    let h = u16::from_be_bytes([seg[1], seg[2]]) as usize;
    let w = u16::from_be_bytes([seg[3], seg[4]]) as usize;
    let nf = seg[5] as usize;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(format!("JPEG: размер {}x{} вне пределов (1..={})", w, h, MAX_SIDE));
    }
    if w.saturating_mul(h) > MAX_PIXELS {
        return Err(format!(
            "JPEG: изображение {}x{} = {} МПикс больше предела 1 МПикс (куча ядра 16 МиБ)",
            w,
            h,
            w.saturating_mul(h) / (1024 * 1024)
        ));
    }
    if seg.len() < 6 + nf * 3 {
        return Err(String::from("JPEG: SOF обрезан в описании компонент"));
    }
    if nf != 1 && nf != 3 {
        return Err(format!(
            "JPEG: {} компонент(ы) не поддерживается — декодер понимает 1 (серый) и 3 (YCbCr/RGB); CMYK/YCCK исключён",
            nf
        ));
    }
    let mut hmax = 0u8;
    let mut vmax = 0u8;
    let mut ids_are_rgb = true;
    let want_ids = [0x52u8, 0x47, 0x42]; // "RGB"
    let mut raw: Vec<(u8, u8, u8, u8)> = Vec::with_capacity(nf);
    for i in 0..nf {
        let o = 6 + i * 3;
        let id = seg[o];
        let hv = seg[o + 1];
        let tq = seg[o + 2];
        let (hc, vc) = (hv >> 4, hv & 0x0F);
        if hc == 0 || vc == 0 || hc > 4 || vc > 4 {
            return Err(format!("JPEG: факторы выборки компоненты {} нулевые/слишком большие", id));
        }
        if tq > 3 {
            return Err(String::from("JPEG: номер таблицы квантования вне 0..3"));
        }
        hmax = hmax.max(hc);
        vmax = vmax.max(vc);
        if id != want_ids[i] {
            ids_are_rgb = false;
        }
        raw.push((id, hc, vc, tq));
    }
    let mut comps = Vec::with_capacity(nf);
    for (id, hc, vc, tq) in raw {
        let cw = (w * hc as usize + hmax as usize - 1) / hmax as usize;
        let ch = (h * vc as usize + vmax as usize - 1) / vmax as usize;
        comps.push(Component {
            id,
            h: hc,
            v: vc,
            tq,
            plane: vec![128i16; cw * ch],
            cw,
            ch,
            dc: 0,
        });
    }
    Ok(Frame { w, h, comps, restart_interval: 0, adobe_rgb: false, ids_are_rgb })
}

fn parse_dqt(data: &[u8], pos: &mut usize, quant: &mut [Option<Vec<u16>>; 4]) -> Result<(), String> {
    let (seg, _) = segment(data, pos)?;
    let mut o = 0usize;
    while o < seg.len() {
        let pq = seg[o] >> 4;
        let id = (seg[o] & 0x0F) as usize;
        o += 1;
        if id > 3 {
            return Err(String::from("JPEG: DQT — номер таблицы вне 0..3"));
        }
        if pq > 1 {
            return Err(String::from("JPEG: DQT — точность не 8 и не 16 бит"));
        }
        let n = if pq == 0 { 64 } else { 128 };
        if o + n > seg.len() {
            return Err(String::from("JPEG: DQT обрезана"));
        }
        let mut t = vec![0u16; 64];
        if pq == 0 {
            for (dst, src) in t.iter_mut().zip(seg[o..o + n].iter()) {
                *dst = *src as u16;
            }
        } else {
            for i in 0..64 {
                t[i] = u16::from_be_bytes([seg[o + i * 2], seg[o + i * 2 + 1]]);
            }
        }
        // Нулевые шаги квантования дали бы мусор — честная ошибка.
        if t.iter().any(|&q| q == 0) {
            return Err(String::from("JPEG: DQT содержит нулевой шаг квантования"));
        }
        quant[id] = Some(t);
        o += n;
    }
    Ok(())
}

fn parse_dht(data: &[u8], pos: &mut usize, huff: &mut [Option<HuffTable>; 8]) -> Result<(), String> {
    let (seg, _) = segment(data, pos)?;
    let mut o = 0usize;
    while o < seg.len() {
        let cls = (seg[o] >> 4) as usize; // 0 = DC, 1 = AC
        let id = (seg[o] & 0x0F) as usize;
        o += 1;
        if cls > 1 || id > 3 {
            return Err(String::from("JPEG: DHT — класс/номер вне диапазонов"));
        }
        if o + 16 > seg.len() {
            return Err(String::from("JPEG: DHT обрезана"));
        }
        let mut counts = [0u16; 17];
        let mut total = 0usize;
        for len in 1..=16 {
            counts[len] = seg[o + len - 1] as u16;
            total += counts[len] as usize;
        }
        o += 16;
        if o + total > seg.len() {
            return Err(String::from("JPEG: DHT — символов меньше, чем объявлено"));
        }
        huff[cls * 4 + id] = Some(HuffTable::build(counts, seg[o..o + total].to_vec()));
        o += total;
    }
    Ok(())
}

/// Один SOS: разбор заголовка скана и энтропийное декодирование всех MCU
/// в плоскости компонент кадра.
fn parse_and_decode_scan(
    data: &[u8],
    pos: &mut usize,
    f: &mut Frame,
    quant: &[Option<Vec<u16>>; 4],
    huff: &[Option<HuffTable>; 8],
) -> Result<(), String> {
    let (seg, start_after) = segment(data, pos)?;
    if seg.is_empty() {
        return Err(String::from("JPEG: SOS пуст"));
    }
    let ns = seg[0] as usize;
    if ns == 0 || ns > f.comps.len() || seg.len() < 1 + ns * 2 + 3 {
        return Err(format!("JPEG: SOS — {} компонент вне кадра", ns));
    }
    // Компоненты скана: (индекс в кадре, dc-таблица, ac-таблица).
    let mut scan: Vec<(usize, usize, usize)> = Vec::with_capacity(ns);
    for i in 0..ns {
        let id = seg[1 + i * 2];
        let tds = seg[2 + i * 2];
        let ci = f.comps.iter().position(|c| c.id == id)
            .ok_or_else(|| format!("JPEG: SOS ссылается на неизвестную компоненту {}", id))?;
        let (td, ta) = ((tds >> 4) as usize, (tds & 0x0F) as usize);
        if td > 3 || ta > 3 {
            return Err(String::from("JPEG: SOS — номер Huffman-таблицы вне 0..3"));
        }
        scan.push((ci, td, ta));
    }
    let hmax = f.comps.iter().map(|c| c.h).max().unwrap_or(1) as usize;
    let vmax = f.comps.iter().map(|c| c.v).max().unwrap_or(1) as usize;

    // Таблицы скана проверяются один раз. DC-таблицы лежат в huff[0..4],
    // AC — в huff[4..8].
    for &(ci, td, ta) in scan.iter() {
        if huff[td].is_none() || huff[4 + ta].is_none() {
            return Err(format!(
                "JPEG: для компоненты {} нет Huffman-таблиц (DC {} / AC {})",
                f.comps[ci].id, td, ta
            ));
        }
        let qt_id = f.comps[ci].tq as usize;
        if quant[qt_id].is_none() {
            return Err(format!("JPEG: для компоненты {} нет таблицы квантования {}", f.comps[ci].id, qt_id));
        }
    }

    for c in f.comps.iter_mut() {
        c.dc = 0; // предикторы DC сбрасываются на каждый скан
    }

    // Единицы обхода: interleaved-скан (ns > 1) идёт MCU кадра,
    // non-interleaved (ns == 1, многоскановые файлы) — блоками своей
    // компоненты по её собственной сетке (T.81 §A.2.2).
    let (units_x, units_y, interleaved) = if ns == 1 {
        let c = &f.comps[scan[0].0];
        ((c.cw + 7) / 8, (c.ch + 7) / 8, false)
    } else {
        ((f.w + 8 * hmax - 1) / (8 * hmax), (f.h + 8 * vmax - 1) / (8 * vmax), true)
    };
    let total_units = units_x * units_y;

    let mut rd = BitReader::new(data, start_after);
    let mut coeffs = [0i32; 64];
    let mut block = [0i32; 64];
    let mut done = 0usize;
    let mut expect_rst = 0u8;

    while done < total_units {
        // Restart-интервал: на границе — пропустить RSTn и сбросить DC.
        if f.restart_interval > 0 && done > 0 && done % f.restart_interval as usize == 0 {
            rd.align();
            skip_restart(&mut rd, data, expect_rst)?;
            expect_rst = (expect_rst + 1) & 7;
            for c in f.comps.iter_mut() {
                c.dc = 0;
            }
        }
        let ux = done % units_x;
        let uy = done / units_x;
        for &(ci, td, ta) in scan.iter() {
            let (hc, vc, tq) = {
                let c = &f.comps[ci];
                (c.h as usize, c.v as usize, c.tq as usize)
            };
            let qt = quant[tq].as_ref().unwrap();
            let dc_t = huff[td].as_ref().unwrap();
            let ac_t = huff[4 + ta].as_ref().unwrap();
            // Non-interleaved: одна единица = один блок компоненты.
            let (nx, ny) = if interleaved { (hc, vc) } else { (1, 1) };
            for by in 0..ny {
                for bx in 0..nx {
                    let px0 = if interleaved { (ux * hc + bx) * 8 } else { ux * 8 };
                    let py0 = if interleaved { (uy * vc + by) * 8 } else { uy * 8 };
                    decode_block(&mut rd, dc_t, ac_t, qt, &mut f.comps[ci].dc, &mut coeffs)?;
                    idct_block(&coeffs, &mut block);
                    // Плоскость может быть меньше сетки MCU (округление) —
                    // блоки за краем отбрасываются.
                    let (cw, ch) = (f.comps[ci].cw, f.comps[ci].ch);
                    for y in 0..8 {
                        let yy = py0 + y;
                        if yy >= ch {
                            continue;
                        }
                        for x in 0..8 {
                            let xx = px0 + x;
                            if xx >= cw {
                                continue;
                            }
                            f.comps[ci].plane[yy * cw + xx] = block[y * 8 + x] as i16;
                        }
                    }
                }
            }
        }
        done += 1;
        // Маркер внутри скана. RSTn при заданном интервале снимается на
        // границе выше; всё прочее (EOI или сегмент следующего скана)
        // означает конец энтропийных данных этого скана.
        if let Some(m) = rd.marker {
            let expected_rst = f.restart_interval > 0 && (0xD0..=0xD7).contains(&m);
            if !expected_rst {
                break;
            }
        }
    }
    if done < total_units {
        match rd.marker {
            // EOI или конец данных: изображение действительно оборвано.
            // Недодекодированные блоки остаются серыми (128) — верхняя
            // часть показывается, это честнее общей ошибки.
            Some(0xD9) | None => {
                crate::diag::notice(
                    crate::diag::NONE,
                    &format!(
                        "JPEG: энтропийные данные оборваны на блоке {}/{} — хвост кадра останется серым",
                        done, total_units
                    ),
                );
            }
            // Другой маркер — штатный конец скана многосканового файла.
            Some(_) => {}
        }
    }
    // Маркер прочитан BitReader'ом (два байта FF xx) — возвращаем курсор
    // на него, чтобы внешний цикл разобрал сегмент сам.
    *pos = if rd.marker.is_some() { rd.pos.saturating_sub(2) } else { rd.pos };
    Ok(())
}

/// Пропускает ожидаемый RSTn: маркер уже снят BitReader или лежит в
/// потоке (после fill-байтов).
fn skip_restart(rd: &mut BitReader, data: &[u8], expect_rst: u8) -> Result<(), String> {
    let want = 0xD0 + expect_rst;
    if rd.marker == Some(want) {
        rd.marker = None;
        return Ok(());
    }
    if rd.marker.is_some() {
        return Err(String::from("JPEG: вместо restart-маркера (RST) другой маркер"));
    }
    // Ищем FF want в потоке (не более 16 байт — дальше это не restart).
    let end = (rd.pos + 16).min(data.len().saturating_sub(1));
    while rd.pos < end {
        if data[rd.pos] == 0xFF && data[rd.pos + 1] == want {
            rd.pos += 2;
            return Ok(());
        }
        rd.pos += 1;
    }
    Err(String::from("JPEG: ожидаемый restart-маркер (RST) не найден"))
}

fn decode_block(
    rd: &mut BitReader,
    dc_t: &HuffTable,
    ac_t: &HuffTable,
    qt: &[u16],
    pred: &mut i32,
    coeffs: &mut [i32; 64],
) -> Result<(), String> {
    *coeffs = [0; 64];
    let t = decode_symbol(rd, dc_t)?;
    if t > 11 {
        return Err(String::from("JPEG: DC-размер коэффициента больше 11"));
    }
    let diff = receive_extend(rd, t)?;
    *pred = pred.wrapping_add(diff);
    coeffs[0] = pred.wrapping_mul(qt[0] as i32);
    let mut k = 1usize;
    while k < 64 {
        let rs = decode_symbol(rd, ac_t)?;
        let size = rs & 0x0F;
        let run = (rs >> 4) as usize;
        if size == 0 {
            if run == 15 {
                k += 16; // ZRL: 16 нулей подряд
                continue;
            }
            break; // EOB
        }
        k += run;
        if k >= 64 {
            return Err(String::from("JPEG: AC-коэффициенты вышли за блок"));
        }
        if size > 10 {
            return Err(String::from("JPEG: AC-размер коэффициента больше 10"));
        }
        let v = receive_extend(rd, size)?;
        coeffs[ZIGZAG[k]] = v.wrapping_mul(qt[k] as i32);
        k += 1;
    }
    Ok(())
}

/// Сборка пикселей: nearest-neighbor апсемпл хроматических плоскостей и
/// преобразование цвета (YCbCr→RGB, либо прямой RGB для Adobe transform=0
/// и идентификаторов компонент "RGB").
fn to_image(f: &Frame) -> Result<Image, String> {
    let (w, h) = (f.w, f.h);
    let mut pixels = vec![0u32; w * h];

    if f.comps.len() == 1 {
        let c = &f.comps[0];
        for y in 0..h {
            let sy = y * c.ch / h;
            for x in 0..w {
                let sx = x * c.cw / w;
                let v = c.plane[sy * c.cw + sx].clamp(0, 255) as u32;
                pixels[y * w + x] = (v << 16) | (v << 8) | v;
            }
        }
        return Ok(Image { w, h, pixels });
    }

    let rgb = f.adobe_rgb || f.ids_are_rgb;
    let c0 = &f.comps[0];
    let c1 = &f.comps[1];
    let c2 = &f.comps[2];
    for y in 0..h {
        let y0 = y * c0.ch / h;
        let y1 = y * c1.ch / h;
        let y2 = y * c2.ch / h;
        for x in 0..w {
            let a = c0.plane[y0 * c0.cw + x * c0.cw / w] as i32;
            let b = c1.plane[y1 * c1.cw + x * c1.cw / w] as i32;
            let c = c2.plane[y2 * c2.cw + x * c2.cw / w] as i32;
            let (r, g, bl) = if rgb {
                (a, b, c)
            } else {
                // JFIF: R = Y + 1.402(Cr-128), G = Y - 0.344136(Cb-128)
                // - 0.714136(Cr-128), B = Y + 1.772(Cb-128) — целочисленно
                // с масштабом 2^16 (те же коэффициенты, что в libjpeg-turbo).
                let cb = b - 128;
                let cr = c - 128;
                (
                    a + ((91881 * cr) >> 16),
                    a - ((22554 * cb + 46802 * cr) >> 16),
                    a + ((116130 * cb) >> 16),
                )
            };
            let r = r.clamp(0, 255) as u32;
            let g = g.clamp(0, 255) as u32;
            let bl = bl.clamp(0, 255) as u32;
            pixels[y * w + x] = (r << 16) | (g << 8) | bl;
        }
    }
    Ok(Image { w, h, pixels })
}
