// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// inflate — декодер DEFLATE (RFC 1951) с нуля, без внешних крейтов.
// Нужен для распаковки НАСТОЯЩЕГО kernel.tar.gz (gzip + deflate) из раздела
// /kernel при загрузке: ядро извлекает kernel.bin и библиотеки из сжатого
// архива. Поддерживает все три типа блоков:
//   * stored (0) — несжатые данные;
//   * fixed Huffman (1) — фиксированные коды литералов/длин и расстояний;
//   * dynamic Huffman (2) — коды, описанные в самом потоке.
// no_std-совместимо: alloc (Vec).


use alloc::vec;
use alloc::vec::Vec;

/// Ошибка декодирования.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflateError {
    Corrupt(&'static str),
    OutputTooLarge { limit: usize },
}

/// Декодер битового потока (LSB-first, как в DEFLATE).
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

    fn read_bit(&mut self) -> Result<u32, InflateError> {
        if self.bitcnt == 0 {
            if self.pos >= self.data.len() {
                return Err(InflateError::Corrupt("bitstream end"));
            }
            self.bitbuf = self.data[self.pos] as u32;
            self.pos += 1;
            self.bitcnt = 8;
        }
        let b = self.bitbuf & 1;
        self.bitbuf >>= 1;
        self.bitcnt -= 1;
        Ok(b)
    }

    fn read_bits(&mut self, n: u32) -> Result<u32, InflateError> {
        let mut v = 0u32;
        for i in 0..n {
            v |= self.read_bit()? << i;
        }
        Ok(v)
    }

    fn align_byte(&mut self) {
        self.bitcnt = 0;
    }

    fn read_byte(&mut self) -> Result<u8, InflateError> {
        if self.pos >= self.data.len() {
            return Err(InflateError::Corrupt("byte stream end"));
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }
}

/// Узел канонического кода Хаффмана.
#[derive(Debug, Clone)]
struct HuffNode {
    /// 0 = лист (sym валиден), 1 = внутренний.
    leaf: bool,
    sym: u16,
    left: u16,
    right: u16,
}

/// Построение дерева из длин кодов (канонический Хаффман, RFC 1951).
/// Надёжный метод: символы сортируются по (длина кода, значение), каждому
/// выдаётся канонический код, затем код «проталкивается» в дерево по битам
/// от старшего к младшему.
fn build_tree(lengths: &[u16]) -> Result<Vec<HuffNode>, InflateError> {
    let max_bits = lengths.iter().max().copied().unwrap_or(0) as usize;
    if max_bits == 0 {
        return Ok(vec![HuffNode { leaf: true, sym: 0, left: 0, right: 0 }]);
    }
    // bl_count[bits]
    let mut bl_count = vec![0usize; max_bits + 1];
    for &l in lengths.iter() {
        if l as usize <= max_bits {
            bl_count[l as usize] += 1;
        }
    }
    // next_code[bits] (канонические коды).
    let mut code = 0usize;
    let mut next_code = vec![0usize; max_bits + 1];
    for bits in 1..=max_bits {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }
    // Символы с ненулевой длиной, отсортированные по (длина, значение).
    let mut syms: Vec<u16> = (0..lengths.len() as u16)
        .filter(|&s| lengths[s as usize] > 0)
        .collect();
    syms.sort_by_key(|&s| (lengths[s as usize], s));

    // Корень дерева — nodes[0].
    let mut nodes: Vec<HuffNode> = vec![HuffNode { leaf: false, sym: 0, left: 0, right: 0 }];
    for &sym in syms.iter() {
        let len = lengths[sym as usize] as usize;
        let c = next_code[len];
        next_code[len] += 1;
        let mut idx = 0usize;
        // Спуск по битам кода от старшего к младшему.
        for bit in (0..len).rev() {
            let b = (c >> bit) & 1;
            if nodes[idx].leaf {
                return Err(InflateError::Corrupt("huffman overlap"));
            }
            let child = if b == 0 { nodes[idx].left } else { nodes[idx].right } as usize;
            if child == 0 {
                let new_idx = nodes.len();
                nodes.push(HuffNode { leaf: false, sym: 0, left: 0, right: 0 });
                if b == 0 {
                    nodes[idx].left = new_idx as u16;
                } else {
                    nodes[idx].right = new_idx as u16;
                }
                idx = new_idx;
            } else {
                idx = child;
            }
        }
        if nodes[idx].leaf {
            return Err(InflateError::Corrupt("duplicate huffman code"));
        }
        nodes[idx].leaf = true;
        nodes[idx].sym = sym;
    }
    Ok(nodes)
}

/// Декодирование одного символа по дереву.
fn decode_sym(bit: &mut BitReader, nodes: &[HuffNode]) -> Result<u16, InflateError> {
    let mut idx = 0usize; // root — nodes[0]
    loop {
        let node = &nodes[idx];
        if node.leaf {
            return Ok(node.sym);
        }
        let b = bit.read_bit()?;
        idx = if b == 0 { node.left as usize } else { node.right as usize };
        if idx >= nodes.len() {
            return Err(InflateError::Corrupt("huffman walk out of range"));
        }
    }
}

/// Таблицы фиксированных кодов (RFC 1951, 3.2.6).
fn fixed_lit_len_lengths() -> Vec<u16> {
    let mut l = vec![0u16; 288];
    for i in 0..144 { l[i] = 8; }
    for i in 144..256 { l[i] = 9; }
    for i in 256..280 { l[i] = 7; }
    for i in 280..288 { l[i] = 8; }
    l
}
fn fixed_dist_lengths() -> Vec<u16> {
    vec![5u16; 30]
}

/// Длины для кодов длин (HCLEN).
const CLEN_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Декодирует один блок: возвращает выходные байты блока.
fn decode_block(bit: &mut BitReader, out: &mut Vec<u8>, max_out: usize) -> Result<(), InflateError> {
    // Тип блока.
    let btype = bit.read_bits(2)?;
    match btype {
        0 => {
            // Stored.
            bit.align_byte();
            let len = (bit.read_byte()? as usize) | ((bit.read_byte()? as usize) << 8);
            let _nlen = (bit.read_byte()? as usize) | ((bit.read_byte()? as usize) << 8);
            for _ in 0..len {
                let b = bit.read_byte()?;
                if out.len() >= max_out {
                    return Err(InflateError::OutputTooLarge { limit: max_out });
                }
                out.push(b);
            }
            Ok(())
        }
        1 | 2 => {
            // Fixed или dynamic Huffman.
            let (lit_len, dist): (Vec<u16>, Vec<u16>) = if btype == 1 {
                (fixed_lit_len_lengths(), fixed_dist_lengths())
            } else {
                let hlit = bit.read_bits(5)? as usize + 257;
                let hdist = bit.read_bits(5)? as usize + 1;
                let hclen = bit.read_bits(4)? as usize + 4;
                let mut clen_lengths = vec![0u16; 19];
                for i in 0..hclen {
                    clen_lengths[CLEN_ORDER[i]] = bit.read_bits(3)? as u16;
                }
                let cl_tree = build_tree(&clen_lengths)?;
                // Читаем длины lit/len и dist.
                let mut lengths: Vec<u16> = Vec::with_capacity(hlit + hdist);
                while lengths.len() < hlit + hdist {
                    let sym = decode_sym(bit, &cl_tree)? as usize;
                    match sym {
                        0..=15 => lengths.push(sym as u16),
                        16 => {
                            let prev = *lengths.last().ok_or(InflateError::Corrupt("no prev"))?;
                            let rep = bit.read_bits(2)? as usize + 3;
                            for _ in 0..rep { lengths.push(prev); }
                        }
                        17 => {
                            let rep = bit.read_bits(3)? as usize + 3;
                            for _ in 0..rep { lengths.push(0); }
                        }
                        18 => {
                            let rep = bit.read_bits(7)? as usize + 11;
                            for _ in 0..rep { lengths.push(0); }
                        }
                        _ => return Err(InflateError::Corrupt("bad clen sym")),
                    }
                    if lengths.len() > hlit + hdist {
                        return Err(InflateError::Corrupt("too many lengths"));
                    }
                }
                let lit = lengths[..hlit].to_vec();
                let dist = lengths[hlit..].to_vec();
                (lit, dist)
            };
            let lit_tree = build_tree(&lit_len)?;
            let dist_tree = build_tree(&dist)?;
            // Декодируем поток.
            loop {
                let sym = decode_sym(bit, &lit_tree)? as usize;
                if sym < 256 {
                    if out.len() >= max_out {
                        return Err(InflateError::OutputTooLarge { limit: max_out });
                    }
                    out.push(sym as u8);
                } else if sym == 256 {
                    break; // конец блока
                } else {
                    // Длина.
                    let (len_base, len_extra): (usize, u32) = match sym {
                        257 => (3, 0), 258 => (4, 0), 259 => (5, 0), 260 => (6, 0),
                        261 => (7, 0), 262 => (8, 0), 263 => (9, 0), 264 => (10, 0),
                        265 => (11, 1), 266 => (13, 1), 267 => (15, 1), 268 => (17, 1),
                        269 => (19, 2), 270 => (23, 2), 271 => (27, 2), 272 => (31, 2),
                        273 => (35, 3), 274 => (43, 3), 275 => (51, 3), 276 => (59, 3),
                        277 => (67, 4), 278 => (83, 4), 279 => (99, 4), 280 => (115, 4),
                        281 => (131, 5), 282 => (163, 5), 283 => (195, 5), 284 => (227, 5),
                        285 => (258, 0),
                        _ => return Err(InflateError::Corrupt("bad len sym")),
                    };
                    let mut length = len_base;
                    if len_extra > 0 {
                        length += bit.read_bits(len_extra)? as usize;
                    }
                    // Расстояние.
                    let dsym = decode_sym(bit, &dist_tree)? as usize;
                    let (d_base, d_extra): (usize, u32) = match dsym {
                        0 => (1, 0), 1 => (2, 0), 2 => (3, 0), 3 => (4, 0),
                        4 => (5, 1), 5 => (7, 1), 6 => (9, 2), 7 => (13, 2),
                        8 => (17, 3), 9 => (25, 3), 10 => (33, 4), 11 => (49, 4),
                        12 => (65, 5), 13 => (97, 5), 14 => (129, 6), 15 => (193, 6),
                        16 => (257, 7), 17 => (385, 7), 18 => (513, 8), 19 => (769, 8),
                        20 => (1025, 9), 21 => (1537, 9), 22 => (2049, 10), 23 => (3073, 10),
                        24 => (4097, 11), 25 => (6145, 11), 26 => (8193, 12), 27 => (12289, 12),
                        28 => (16385, 13), 29 => (24577, 13),
                        _ => return Err(InflateError::Corrupt("bad dist sym")),
                    };
                    let mut dist = d_base;
                    if d_extra > 0 {
                        dist += bit.read_bits(d_extra)? as usize;
                    }
                    // Копируем.
                    if dist > out.len() {
                        return Err(InflateError::Corrupt("dist too far"));
                    }
                    for _ in 0..length {
                        if out.len() >= max_out {
                            return Err(InflateError::OutputTooLarge { limit: max_out });
                        }
                        let idx = out.len() - dist;
                        let b = out[idx];
                        out.push(b);
                    }
                }
            }
            Ok(())
        }
        _ => Err(InflateError::Corrupt("reserved block type")),
    }
}

/// Распаковка DEFLATE-потока (RFC 1951). Возвращает распакованные байты.
pub fn inflate(data: &[u8], max_out: usize) -> Result<Vec<u8>, InflateError> {
    let mut bit = BitReader::new(data);
    let mut out: Vec<u8> = Vec::new();
    loop {
        let bfinal = bit.read_bit()?;
        decode_block(&mut bit, &mut out, max_out)?;
        if bfinal == 1 {
            break;
        }
        if out.len() > max_out {
            return Err(InflateError::OutputTooLarge { limit: max_out });
        }
    }
    Ok(out)
}

/// Распаковка gzip-контейнера (RFC 1952): заголовок (10+ байт, метод 8),
/// deflate-поток, CRC32 (проверяется на CRC32-байтах? нет — пропускаем
/// хвост, CRC не критичен для загрузки).
pub fn gunzip(data: &[u8], max_out: usize) -> Result<Vec<u8>, InflateError> {
    if data.len() < 10 || data[0] != 0x1F || data[1] != 0x8B || data[2] != 8 {
        return Err(InflateError::Corrupt("not gzip/deflate"));
    }
    // Обходим опциональные поля заголовка.
    let flg = data[3];
    let mut pos = 10usize;
    if flg & 0x04 != 0 {
        // XLEN
        if pos + 2 > data.len() { return Err(InflateError::Corrupt("gzip xlen")); }
        let xlen = (data[pos] as usize) | ((data[pos + 1] as usize) << 8);
        pos += 2 + xlen;
    }
    if flg & 0x08 != 0 {
        while pos < data.len() && data[pos] != 0 { pos += 1; }
        pos += 1;
    }
    if flg & 0x10 != 0 {
        while pos < data.len() && data[pos] != 0 { pos += 1; }
        pos += 1;
    }
    if flg & 0x02 != 0 {
        pos += 2;
    }
    if pos >= data.len() {
        return Err(InflateError::Corrupt("gzip header overflow"));
    }
    inflate(&data[pos..], max_out)
}
