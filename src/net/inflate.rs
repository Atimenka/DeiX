//! Распаковка DEFLATE (RFC 1951) и gzip (RFC 1952) для HTTP-ответов
//! со сжатием `Content-Encoding: gzip`.
//!
//! Алгоритм распаковки — порт эталонной реализации puff (Mark Adler,
//! zlib/contrib): канонические коды Хаффмана декодируются побитово, без
//! больших таблиц. Поддержаны все три типа блоков (stored, fixed,
//! dynamic). Целостность проверяется по CRC-32 и ISIZE из трейлера gzip.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Предел распакованных данных — защита от «бомб» декомпрессии и
/// переполнения 16-МиБ кучи ядра. При достижении предела распаковка
/// мягко останавливается и страница показывается усечённой.
const MAX_OUT: usize = 2 * 1024 * 1024;

const MAX_BITS: usize = 15;

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bitbuf: u32,
    bitcnt: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, bitbuf: 0, bitcnt: 0 }
    }

    /// Читает need бит (LSB-first, RFC 1951 §3.1.1).
    fn bits(&mut self, need: u32) -> Result<u32, String> {
        while self.bitcnt < need {
            if self.pos >= self.data.len() {
                return Err("DEFLATE: поток оборван".into());
            }
            self.bitbuf |= (self.data[self.pos] as u32) << self.bitcnt;
            self.pos += 1;
            self.bitcnt += 8;
        }
        let val = self.bitbuf & ((1u32 << need) - 1);
        self.bitbuf >>= need;
        self.bitcnt -= need;
        Ok(val)
    }

    /// Выравнивает чтение на границу байта (для stored-блоков):
    /// целые байты из битового буфера возвращаются обратно.
    fn align_byte(&mut self) {
        self.pos -= (self.bitcnt / 8) as usize;
        self.bitbuf = 0;
        self.bitcnt = 0;
    }
}

/// Канонический код Хаффмана: counts[len] — сколько кодов длины len,
/// symbols — символы в порядке (длина, номер символа).
struct Huffman {
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
}

fn build_huffman(lengths: &[u16]) -> Huffman {
    let mut counts = [0u16; MAX_BITS + 1];
    for &l in lengths {
        counts[l as usize] += 1;
    }
    counts[0] = 0;

    let mut offs = [0u16; MAX_BITS + 1];
    for len in 1..=MAX_BITS {
        offs[len] = offs[len - 1] + counts[len - 1];
    }

    let total: usize = counts[1..].iter().map(|&c| c as usize).sum();
    let mut symbols = vec![0u16; total];
    for (sym, &l) in lengths.iter().enumerate() {
        if l != 0 {
            symbols[offs[l as usize] as usize] = sym as u16;
            offs[l as usize] += 1;
        }
    }
    Huffman { counts, symbols }
}

/// Побитовое декодирование одного символа (puff::decode).
fn decode(br: &mut BitReader, h: &Huffman) -> Result<u16, String> {
    let mut code: i32 = 0;
    let mut first: i32 = 0;
    let mut index: i32 = 0;
    for len in 1..=MAX_BITS {
        code |= br.bits(1)? as i32;
        let count = h.counts[len] as i32;
        if code - first < count {
            return Ok(h.symbols[(index + (code - first)) as usize]);
        }
        index += count;
        first += count;
        first <<= 1;
        code <<= 1;
    }
    Err("DEFLATE: некорректный код Хаффмана".into())
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59,
    67, 83, 99, 115, 131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4,
    5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513,
    769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10,
    11, 11, 12, 12, 13, 13,
];

/// Декодирует literal/length-коды блока до символа конца (256).
/// Ok(true) — блок дочитан; Ok(false) — достигнут предел MAX_OUT.
fn inflate_codes(
    br: &mut BitReader,
    litlen: &Huffman,
    dist: &Huffman,
    out: &mut Vec<u8>,
) -> Result<bool, String> {
    loop {
        let sym = decode(br, litlen)?;
        if sym < 256 {
            if out.len() >= MAX_OUT {
                return Ok(false);
            }
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(true);
        } else {
            let s = (sym - 257) as usize;
            if s >= LEN_BASE.len() {
                return Err("DEFLATE: некорректный код длины".into());
            }
            let len = LEN_BASE[s] as usize + br.bits(LEN_EXTRA[s])? as usize;
            let dsym = decode(br, dist)? as usize;
            if dsym >= DIST_BASE.len() {
                return Err("DEFLATE: некорректный код дистанции".into());
            }
            let d = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym])? as usize;
            if d > out.len() {
                return Err("DEFLATE: ссылка за начало данных".into());
            }
            for _ in 0..len {
                if out.len() >= MAX_OUT {
                    return Ok(false);
                }
                let b = out[out.len() - d];
                out.push(b);
            }
        }
    }
}

/// Stored-блок (BTYPE=00): LEN/NLEN + сырые байты.
/// Ok(true) — блок скопирован целиком; Ok(false) — предел MAX_OUT.
fn inflate_stored(br: &mut BitReader, out: &mut Vec<u8>) -> Result<bool, String> {
    br.align_byte();
    if br.pos + 4 > br.data.len() {
        return Err("DEFLATE: stored-блок оборван".into());
    }
    let len = br.data[br.pos] as usize | (br.data[br.pos + 1] as usize) << 8;
    let nlen = br.data[br.pos + 2] as usize | (br.data[br.pos + 3] as usize) << 8;
    if len != (!nlen & 0xffff) {
        return Err("DEFLATE: LEN/NLEN stored-блока не согласованы".into());
    }
    br.pos += 4;
    if br.pos + len > br.data.len() {
        return Err("DEFLATE: данные stored-блока оборваны".into());
    }
    if out.len() + len > MAX_OUT {
        let take = MAX_OUT - out.len();
        out.extend_from_slice(&br.data[br.pos..br.pos + take]);
        return Ok(false);
    }
    out.extend_from_slice(&br.data[br.pos..br.pos + len]);
    br.pos += len;
    Ok(true)
}

/// Фиксированные таблицы (BTYPE=01, RFC 1951 §3.2.6).
fn fixed_tables() -> (Huffman, Huffman) {
    let mut ll = [0u16; 288];
    for (i, l) in ll.iter_mut().enumerate() {
        *l = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let dd = [5u16; 30];
    (build_huffman(&ll), build_huffman(&dd))
}

/// Порядок длин кода длин кодов (RFC 1951 §3.2.7).
const CLC_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Динамические таблицы (BTYPE=10).
fn dynamic_tables(br: &mut BitReader) -> Result<(Huffman, Huffman), String> {
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err("DEFLATE: некорректные HLIT/HDIST".into());
    }

    let mut clc = [0u16; 19];
    for i in 0..hclen {
        clc[CLC_ORDER[i]] = br.bits(3)? as u16;
    }
    let clh = build_huffman(&clc);

    let mut lengths = vec![0u16; hlit + hdist];
    let mut i = 0;
    while i < lengths.len() {
        let sym = decode(br, &clh)?;
        let (value, repeat) = match sym {
            0..=15 => {
                lengths[i] = sym;
                i += 1;
                continue;
            }
            16 => {
                if i == 0 {
                    return Err("DEFLATE: повтор длины без предыдущей".into());
                }
                (lengths[i - 1], 3 + br.bits(2)? as usize)
            }
            17 => (0, 3 + br.bits(3)? as usize),
            18 => (0, 11 + br.bits(7)? as usize),
            _ => return Err("DEFLATE: некорректный символ длины кода".into()),
        };
        if i + repeat > lengths.len() {
            return Err("DEFLATE: повтор длин выходит за таблицу".into());
        }
        for _ in 0..repeat {
            lengths[i] = value;
            i += 1;
        }
    }
    if lengths[256] == 0 {
        return Err("DEFLATE: нет кода конца блока".into());
    }
    Ok((build_huffman(&lengths[..hlit]), build_huffman(&lengths[hlit..])))
}

/// Распаковывает сырой DEFLATE-поток.
/// Второй элемент результата — true, если вывод усечён по MAX_OUT.
fn inflate(data: &[u8]) -> Result<(Vec<u8>, bool), String> {
    let mut br = BitReader::new(data);
    let mut out = Vec::new();
    loop {
        let bfinal = br.bits(1)?;
        let btype = br.bits(2)?;
        let complete = match btype {
            0 => inflate_stored(&mut br, &mut out)?,
            1 => {
                let (ll, dd) = fixed_tables();
                inflate_codes(&mut br, &ll, &dd, &mut out)?
            }
            2 => {
                let (ll, dd) = dynamic_tables(&mut br)?;
                inflate_codes(&mut br, &ll, &dd, &mut out)?
            }
            _ => return Err("DEFLATE: некорректный тип блока (11)".into()),
        };
        if !complete {
            return Ok((out, true));
        }
        if bfinal == 1 {
            break;
        }
    }
    Ok((out, false))
}

/// CRC-32 (полином 0xEDB88320, как в gzip/zip) — побитово, без таблицы.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Распаковывает zlib-поток (RFC 1950): 2 байта заголовка CMF/FLG,
/// DEFLATE-данные, 4 байта Adler-32. Формат PNG хранит IDAT именно так.
/// Второй элемент результата — true, если вывод усечён по лимиту.
pub fn zlib_inflate(data: &[u8]) -> Result<(Vec<u8>, bool), String> {
    if data.len() < 6 {
        return Err("zlib: поток короче минимального".into());
    }
    let cmf = data[0];
    let flg = data[1];
    if cmf & 0x0F != 8 {
        return Err("zlib: метод сжатия не DEFLATE".into());
    }
    if (((cmf as u16) << 8) | flg as u16) % 31 != 0 {
        return Err("zlib: контрольная сумма заголовка не сходится".into());
    }
    if flg & 0x20 != 0 {
        return Err("zlib: preset dictionary не поддерживается".into());
    }
    let (out, truncated) = inflate(&data[2..data.len() - 4])?;
    if truncated {
        return Ok((out, true));
    }
    let tail = &data[data.len() - 4..];
    let expect = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]);
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in out.iter() {
        a += byte as u32;
        if a >= 65521 {
            a -= 65521;
        }
        b += a;
        if b >= 0x8000_0000 {
            b %= 65521;
        }
    }
    b %= 65521;
    if ((b << 16) | a) != expect {
        return Err("zlib: Adler-32 не совпал (данные повреждены)".into());
    }
    Ok((out, false))
}

/// Распаковывает gzip-поток (RFC 1952) с проверкой CRC-32 и ISIZE.
/// Второй элемент результата — true, если вывод усечён по лимиту
/// (тогда CRC/ISIZE не проверяются — конец потока не читался).
pub fn gunzip(data: &[u8]) -> Result<(Vec<u8>, bool), String> {
    if data.len() < 18 {
        return Err("gzip: поток короче минимального".into());
    }
    if data[0] != 0x1f || data[1] != 0x8b {
        return Err("gzip: нет сигнатуры 1f 8b".into());
    }
    if data[2] != 8 {
        return Err("gzip: метод сжатия не DEFLATE".into());
    }
    let flg = data[3];
    let mut off = 10;
    if flg & 0x04 != 0 {
        // FEXTRA
        if off + 2 > data.len() {
            return Err("gzip: заголовок FEXTRA оборван".into());
        }
        let xlen = data[off] as usize | (data[off + 1] as usize) << 8;
        off += 2 + xlen;
    }
    if flg & 0x08 != 0 {
        // FNAME: имя файла, завершается нулём
        while off < data.len() && data[off] != 0 {
            off += 1;
        }
        off += 1;
    }
    if flg & 0x10 != 0 {
        // FCOMMENT
        while off < data.len() && data[off] != 0 {
            off += 1;
        }
        off += 1;
    }
    if flg & 0x02 != 0 {
        off += 2; // FHCRC
    }
    if off + 8 > data.len() {
        return Err("gzip: заголовок оборван".into());
    }

    let (out, truncated) = inflate(&data[off..data.len() - 8])?;
    if truncated {
        return Ok((out, true));
    }

    let tail = &data[data.len() - 8..];
    let crc_expect = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
    let isize_expect = u32::from_le_bytes([tail[4], tail[5], tail[6], tail[7]]);
    if out.len() as u32 != isize_expect {
        return Err("gzip: размер после распаковки не совпал с ISIZE".into());
    }
    if crc32(&out) != crc_expect {
        return Err("gzip: контрольная сумма CRC-32 не совпала".into());
    }
    Ok((out, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gunzip_dynamic_huffman() {
        // Настоящий gzip-поток (zlib, уровень 9, динамический Хаффман).
        let gz: [u8; 47] = [
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x03, 0x73, 0x49,
            0xcd, 0x8c, 0x50, 0xf0, 0x0f, 0xb6, 0x52, 0x70, 0x71, 0x75, 0xf3, 0x71,
            0x0c, 0x71, 0x55, 0x28, 0xcf, 0x2f, 0xca, 0x2e, 0x56, 0x54, 0x70, 0x19,
            0x20, 0x71, 0x00, 0xdf, 0x0c, 0x4f, 0xf1, 0x90, 0x00, 0x00, 0x00,
        ];
        let (out, truncated) = gunzip(&gz).expect("valid gzip stream");
        assert!(!truncated);
        assert_eq!(out, b"DeiX OS: DEFLATE works! DeiX OS: DEFLATE works! DeiX OS: DEFLATE works! DeiX OS: DEFLATE works! DeiX OS: DEFLATE works! DeiX OS: DEFLATE works! ".to_vec());
    }

    #[test]
    fn test_gunzip_stored_block() {
        // gzip с уровнем 0 -> stored-блок (BTYPE=00).
        let gz: [u8; 40] = [
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x03, 0x01, 0x11,
            0x00, 0xee, 0xff, 0x73, 0x74, 0x6f, 0x72, 0x65, 0x64, 0x20, 0x62, 0x6c,
            0x6f, 0x63, 0x6b, 0x20, 0x74, 0x65, 0x73, 0x74, 0xb0, 0xe8, 0x2c, 0xd0,
            0x11, 0x00, 0x00, 0x00,
        ];
        let (out, truncated) = gunzip(&gz).expect("valid gzip stream");
        assert!(!truncated);
        assert_eq!(out, b"stored block test".to_vec());
    }

    #[test]
    fn test_gunzip_detects_corruption() {
        let gz: [u8; 40] = [
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x03, 0x01, 0x11,
            0x00, 0xee, 0xff, 0x73, 0x74, 0x6f, 0x72, 0x65, 0x64, 0x20, 0x62, 0x6c,
            0x6f, 0x63, 0x6b, 0x20, 0x74, 0x65, 0x73, 0x74, 0xb0, 0xe8, 0x2c, 0xd0,
            0x11, 0x00, 0x00, 0x00,
        ];
        let mut bad = gz;
        let n = bad.len();
        bad[n - 5] ^= 0xff; // портим CRC-32 в трейлере
        assert!(gunzip(&bad).is_err());
    }
}
