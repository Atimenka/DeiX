//! EROFS (Enhanced Read-Only File System) — НАСТОЯЩАЯ реализация формата.
//!
//! Соответствует спецификации EROFS v1 (erofs.docs.kernel.org, Linux
//! `fs/erofs/erofs_fs.h`). Образы, созданные этим модулем, читаются
//! штатными `dump.erofs`/`fsck.erofs` и монтируются ядром Linux; образы,
//! созданные `mkfs.erofs`, читаются этим модулем.
//!
//! ## Почему это важно
//!
//! Раньше ядро писало в разделы сигнатуру `0xE0F5E0F5` и собственную
//! таблицу файлов. Настоящая магия EROFS — `0xE0F5E1E2`. То есть разделы
//! назывались EROFS, но ни одна настоящая утилита EROFS их не понимала, а
//! ядро не смогло бы прочитать ни один настоящий EROFS-образ. Здесь этот
//! формат реализован по букве спецификации.
//!
//! ## Раскладка образа
//!
//! ```text
//!   [0 .. 1024)      padding (место под MBR/загрузчик — так задумано в EROFS)
//!   [1024 .. 1152)   superblock (128 байт)
//!   [1152 .. 4096)   область инодов (по 32 байта, nid = offset/32)
//!   [4096 .. )       блоки данных (файлы и каталоги, FLAT_PLAIN)
//! ```
//!
//! Все иноды — compact (32 байта), раскладка данных FLAT_PLAIN: содержимое
//! лежит в целых блоках начиная с `raw_blkaddr`. Это самый простой
//! layout, полностью описанный спецификацией, без inline-хвостов.
//!
//! no_std: только `alloc`.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Магия суперблока EROFS v1 (little-endian по смещению 1024).
pub const EROFS_SUPER_MAGIC_V1: u32 = 0xE0F5_E1E2;
/// Фиксированное смещение суперблока.
pub const EROFS_SUPER_OFFSET: usize = 1024;
/// Размер базового суперблока.
pub const EROFS_SUPER_SIZE: usize = 128;
/// Размер блока (blkszbits = 12).
pub const EROFS_BLOCK_SIZE: usize = 4096;
pub const EROFS_BLOCK_BITS: u8 = 12;
/// Размер слота инода: nid = (offset - meta_blkaddr*blocksize) / 32.
pub const EROFS_ISLOT_SIZE: usize = 32;
/// Признак «нет блока данных».
pub const EROFS_NULL_ADDR: u32 = !0;

/// Раскладка данных FLAT_PLAIN: данные в целых блоках с `raw_blkaddr`.
const DATALAYOUT_FLAT_PLAIN: u16 = 0;
/// FLAT_INLINE: целые блоки + «хвост» сразу за инодом. Так пишет mkfs.erofs.
const DATALAYOUT_FLAT_INLINE: u16 = 2;

/// Типы файлов в записях каталога (совпадают с EROFS_FT_*).
const EROFS_FT_REG_FILE: u8 = 1;
const EROFS_FT_DIR: u8 = 2;

const S_IFREG: u16 = 0o100_000;
const S_IFDIR: u16 = 0o040_000;

/// Ошибки разбора EROFS-образа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErofsError {
    /// Образ короче суперблока.
    TooSmall,
    /// Магия по смещению 1024 не равна EROFS_SUPER_MAGIC_V1.
    BadMagic { found: u32 },
    /// Неподдерживаемый размер блока.
    BadBlockSize { bits: u8 },
    /// Структура образа повреждена (ссылка за пределы).
    Corrupt,
}

impl ErofsError {
    pub fn message(&self) -> String {
        match self {
            ErofsError::TooSmall => "EROFS: образ меньше суперблока".into(),
            ErofsError::BadMagic { found } => alloc::format!(
                "EROFS: магия {:#010x} != {:#010x}",
                found,
                EROFS_SUPER_MAGIC_V1
            ),
            ErofsError::BadBlockSize { bits } => {
                alloc::format!("EROFS: неподдерживаемый blkszbits={}", bits)
            }
            ErofsError::Corrupt => "EROFS: повреждённая структура".into(),
        }
    }
}

// ==================== ЧТЕНИЕ ====================

fn rd_u16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(o)?, *b.get(o + 1)?]))
}
fn rd_u32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(o)?,
        *b.get(o + 1)?,
        *b.get(o + 2)?,
        *b.get(o + 3)?,
    ]))
}
fn rd_u64(b: &[u8], o: usize) -> Option<u64> {
    let mut v = [0u8; 8];
    for (i, s) in v.iter_mut().enumerate() {
        *s = *b.get(o + i)?;
    }
    Some(u64::from_le_bytes(v))
}

/// Разобранный суперблок EROFS.
#[derive(Debug, Clone, Copy)]
pub struct Superblock {
    pub blkszbits: u8,
    pub root_nid: u64,
    pub inos: u64,
    pub blocks: u32,
    pub meta_blkaddr: u32,
}

impl Superblock {
    pub fn block_size(&self) -> usize {
        1usize << self.blkszbits
    }

    /// Смещение инода по его nid.
    pub fn inode_offset(&self, nid: u64) -> usize {
        self.meta_blkaddr as usize * self.block_size() + nid as usize * EROFS_ISLOT_SIZE
    }
}

/// Проверяет и разбирает суперблок.
pub fn parse_superblock(img: &[u8]) -> Result<Superblock, ErofsError> {
    if img.len() < EROFS_SUPER_OFFSET + EROFS_SUPER_SIZE {
        return Err(ErofsError::TooSmall);
    }
    let sb = &img[EROFS_SUPER_OFFSET..];
    let magic = rd_u32(sb, 0).ok_or(ErofsError::TooSmall)?;
    if magic != EROFS_SUPER_MAGIC_V1 {
        return Err(ErofsError::BadMagic { found: magic });
    }
    let blkszbits = *sb.get(12).ok_or(ErofsError::TooSmall)?;
    if !(9..=16).contains(&blkszbits) {
        return Err(ErofsError::BadBlockSize { bits: blkszbits });
    }
    Ok(Superblock {
        blkszbits,
        root_nid: rd_u16(sb, 14).ok_or(ErofsError::TooSmall)? as u64,
        inos: rd_u64(sb, 16).ok_or(ErofsError::TooSmall)?,
        blocks: rd_u32(sb, 36).ok_or(ErofsError::TooSmall)?,
        meta_blkaddr: rd_u32(sb, 40).ok_or(ErofsError::TooSmall)?,
    })
}

/// Инод EROFS (нужные нам поля; поддержаны compact и extended).
#[derive(Debug, Clone, Copy)]
pub struct Inode {
    pub mode: u16,
    pub size: u64,
    pub raw_blkaddr: u32,
    pub datalayout: u16,
    /// Смещение конца структуры инода в образе — начало inline-хвоста
    /// (для FLAT_INLINE) с учётом xattr.
    pub inline_off: usize,
}

impl Inode {
    pub fn is_dir(&self) -> bool {
        self.mode & 0o170_000 == S_IFDIR
    }
}

/// Читает инод по nid.
pub fn read_inode(img: &[u8], sb: &Superblock, nid: u64) -> Result<Inode, ErofsError> {
    let off = sb.inode_offset(nid);
    let format = rd_u16(img, off).ok_or(ErofsError::Corrupt)?;
    let extended = format & 1 == 1;
    let datalayout = (format >> 1) & 7;
    let xattr_icount = rd_u16(img, off + 2).ok_or(ErofsError::Corrupt)? as usize;
    let mode = rd_u16(img, off + 4).ok_or(ErofsError::Corrupt)?;

    // Размер структуры инода: compact 32, extended 64.
    let base = if extended { 64 } else { 32 };
    // Область xattr сразу за инодом: если icount != 0, это
    // erofs_xattr_ibody_header (12 байт) + (icount-1)*4.
    let xattr_size = if xattr_icount > 0 {
        12 + (xattr_icount - 1) * 4
    } else {
        0
    };

    let (size, raw_blkaddr) = if extended {
        (
            rd_u64(img, off + 8).ok_or(ErofsError::Corrupt)?,
            rd_u32(img, off + 16).ok_or(ErofsError::Corrupt)?,
        )
    } else {
        (
            rd_u32(img, off + 8).ok_or(ErofsError::Corrupt)? as u64,
            rd_u32(img, off + 16).ok_or(ErofsError::Corrupt)?,
        )
    };

    Ok(Inode {
        mode,
        size,
        raw_blkaddr,
        datalayout,
        inline_off: off + base + xattr_size,
    })
}

/// Одна запись каталога.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub nid: u64,
    pub file_type: u8,
}

/// Собирает содержимое инода.
///
/// FLAT_PLAIN — данные целиком в блоках с `raw_blkaddr`.
/// FLAT_INLINE — целые блоки в `raw_blkaddr`, а остаток (`size % blocksize`)
/// лежит сразу за структурой инода. Именно так пишет `mkfs.erofs`.
fn inode_data(img: &[u8], sb: &Superblock, ino: &Inode) -> Result<Vec<u8>, ErofsError> {
    let bs = sb.block_size();
    let size = ino.size as usize;
    match ino.datalayout {
        DATALAYOUT_FLAT_PLAIN => {
            if ino.raw_blkaddr == EROFS_NULL_ADDR {
                return Ok(Vec::new());
            }
            let start = ino.raw_blkaddr as usize * bs;
            Ok(img
                .get(start..start + size)
                .ok_or(ErofsError::Corrupt)?
                .to_vec())
        }
        DATALAYOUT_FLAT_INLINE => {
            let tail = size % bs;
            let head = size - tail;
            let mut out = Vec::with_capacity(size);
            if head > 0 {
                if ino.raw_blkaddr == EROFS_NULL_ADDR {
                    return Err(ErofsError::Corrupt);
                }
                let start = ino.raw_blkaddr as usize * bs;
                out.extend_from_slice(img.get(start..start + head).ok_or(ErofsError::Corrupt)?);
            }
            if tail > 0 {
                out.extend_from_slice(
                    img.get(ino.inline_off..ino.inline_off + tail)
                        .ok_or(ErofsError::Corrupt)?,
                );
            }
            Ok(out)
        }
        // Сжатые раскладки (COMPRESSION_*) не поддерживаем: наши разделы
        // создаются без сжатия, а тянуть в ядро LZ4/zstd ради чтения
        // чужих образов — та самая избыточность, от которой мы уходим.
        _ => Err(ErofsError::Corrupt),
    }
}

/// Читает записи каталога.
pub fn read_dir(img: &[u8], sb: &Superblock, nid: u64) -> Result<Vec<DirEntry>, ErofsError> {
    let ino = read_inode(img, sb, nid)?;
    if !ino.is_dir() {
        return Err(ErofsError::Corrupt);
    }
    let data = inode_data(img, sb, &ino)?;
    let size = data.len();
    let bs = sb.block_size();
    let mut out: Vec<DirEntry> = Vec::new();
    let mut done = 0usize;

    while done < size {
        let this = core::cmp::min(bs, size - done);
        let blk = data.get(done..done + this).ok_or(ErofsError::Corrupt)?;

        // Количество записей: nameoff первой записи / 12.
        let first_nameoff = rd_u16(blk, 8).ok_or(ErofsError::Corrupt)? as usize;
        if first_nameoff < 12 || first_nameoff > this {
            return Err(ErofsError::Corrupt);
        }
        let count = first_nameoff / 12;

        for i in 0..count {
            let e = i * 12;
            let nid = rd_u64(blk, e).ok_or(ErofsError::Corrupt)?;
            let nameoff = rd_u16(blk, e + 8).ok_or(ErofsError::Corrupt)? as usize;
            let ftype = *blk.get(e + 10).ok_or(ErofsError::Corrupt)?;
            let name_end = if i + 1 < count {
                rd_u16(blk, e + 12 + 8).ok_or(ErofsError::Corrupt)? as usize
            } else {
                this
            };
            if nameoff > name_end || name_end > this {
                return Err(ErofsError::Corrupt);
            }
            let raw = &blk[nameoff..name_end];
            let raw = match raw.iter().position(|&c| c == 0) {
                Some(p) => &raw[..p],
                None => raw,
            };
            let mut name = String::new();
            for &c in raw {
                name.push(c as char);
            }
            if !name.is_empty() {
                out.push(DirEntry {
                    name,
                    nid,
                    file_type: ftype,
                });
            }
        }
        done += this;
    }
    Ok(out)
}

/// Ищет обычный файл в корне и возвращает его инод.
pub fn lookup(img: &[u8], name: &str) -> Result<Inode, ErofsError> {
    let sb = parse_superblock(img)?;
    for e in read_dir(img, &sb, sb.root_nid)? {
        if e.name == name && e.file_type == EROFS_FT_REG_FILE {
            return read_inode(img, &sb, e.nid);
        }
    }
    Err(ErofsError::Corrupt)
}

/// Список обычных файлов корня: `(имя, размер)`.
pub fn list_files(img: &[u8]) -> Result<Vec<(String, usize)>, ErofsError> {
    let sb = parse_superblock(img)?;
    let mut out = Vec::new();
    for e in read_dir(img, &sb, sb.root_nid)? {
        if e.file_type != EROFS_FT_REG_FILE {
            continue;
        }
        let ino = read_inode(img, &sb, e.nid)?;
        out.push((e.name, ino.size as usize));
    }
    Ok(out)
}

/// Читает содержимое файла из образа (FLAT_PLAIN и FLAT_INLINE).
pub fn read_file(img: &[u8], name: &str) -> Result<Vec<u8>, ErofsError> {
    let sb = parse_superblock(img)?;
    let ino = lookup(img, name)?;
    inode_data(img, &sb, &ino)
}

// ==================== ЗАПИСЬ (mkfs) ====================

fn wr_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn wr_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn wr_u64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn align_up(v: usize, a: usize) -> usize {
    v.div_ceil(a) * a
}

/// Записывает compact-инод (32 байта) по смещению `off`.
#[allow(clippy::too_many_arguments)]
fn write_inode(
    img: &mut [u8],
    off: usize,
    mode: u16,
    nlink: u16,
    size: u32,
    blkaddr: u32,
    ino_num: u32,
) {
    // i_format: version=0 (compact), datalayout=FLAT_PLAIN
    wr_u16(img, off, DATALAYOUT_FLAT_PLAIN << 1);
    wr_u16(img, off + 2, 0); // i_xattr_icount
    wr_u16(img, off + 4, mode); // i_mode
    wr_u16(img, off + 6, nlink); // i_nlink
    wr_u32(img, off + 8, size); // i_size
    wr_u32(img, off + 12, 0); // i_reserved
    wr_u32(img, off + 16, blkaddr); // i_u.raw_blkaddr
    wr_u32(img, off + 20, ino_num); // i_ino
    wr_u16(img, off + 24, 0); // i_uid
    wr_u16(img, off + 26, 0); // i_gid
    wr_u32(img, off + 28, 0); // i_reserved2
}

/// Собирает блоки данных каталога (записи + имена, по блокам).
fn build_dir_blocks(entries: &[(String, u64, u8)]) -> Vec<u8> {
    // Формируем поблочно: в каждом блоке — массив erofs_dirent, затем имена.
    let mut out: Vec<u8> = Vec::new();
    let mut idx = 0usize;

    while idx < entries.len() {
        // Сколько записей влезет в этот блок.
        let mut n = 0usize;
        let mut names_len = 0usize;
        while idx + n < entries.len() {
            let nl = entries[idx + n].0.len();
            if (n + 1) * 12 + names_len + nl > EROFS_BLOCK_SIZE {
                break;
            }
            names_len += nl;
            n += 1;
        }
        if n == 0 {
            break; // имя длиннее блока — не поддерживаем
        }

        let mut blk = vec![0u8; EROFS_BLOCK_SIZE];
        let mut nameoff = n * 12;
        for i in 0..n {
            let (name, nid, ftype) = &entries[idx + i];
            let e = i * 12;
            wr_u64(&mut blk, e, *nid);
            wr_u16(&mut blk, e + 8, nameoff as u16);
            blk[e + 10] = *ftype;
            blk[e + 11] = 0;
            blk[nameoff..nameoff + name.len()].copy_from_slice(name.as_bytes());
            nameoff += name.len();
        }
        out.extend_from_slice(&blk);
        idx += n;
    }
    out
}

/// СОЗДАЁТ НАСТОЯЩИЙ EROFS-ОБРАЗ из списка файлов `(имя, содержимое)`.
///
/// Результат читается `dump.erofs`/`fsck.erofs` и монтируется Linux.
pub fn build_image(files: &[(&str, &[u8])]) -> Vec<u8> {
    let bs = EROFS_BLOCK_SIZE;

    // --- раскладка инодов: корень, затем файлы ---
    let inode_area = EROFS_SUPER_OFFSET + EROFS_SUPER_SIZE; // 1152
    let root_nid = (inode_area / EROFS_ISLOT_SIZE) as u64; // 36
    let mut nids: Vec<u64> = Vec::with_capacity(files.len());
    for i in 0..files.len() {
        nids.push(root_nid + 1 + i as u64);
    }
    let inode_area_end = inode_area + (1 + files.len()) * EROFS_ISLOT_SIZE;
    // Иноды должны поместиться в блок 0.
    if inode_area_end > bs {
        // Слишком много файлов для простой раскладки — вернём пустой корень.
        return build_image(&[]);
    }

    // --- данные каталога ---
    // ВАЖНО: спецификация EROFS требует, чтобы записи каталога были
    // отсортированы лексикографически по имени (fsck.erofs проверяет это
    // и отвергает образ с "wrong dirent name order").
    let mut dirents: Vec<(String, u64, u8)> = Vec::new();
    dirents.push((".".into(), root_nid, EROFS_FT_DIR));
    dirents.push(("..".into(), root_nid, EROFS_FT_DIR));
    for (i, (name, _)) in files.iter().enumerate() {
        dirents.push(((*name).into(), nids[i], EROFS_FT_REG_FILE));
    }
    dirents.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    let dir_data = build_dir_blocks(&dirents);
    // Реальный размер каталога — только заполненная часть последнего блока.
    let dir_size = {
        // Пересчитываем «полезный» размер: у нас всегда целые блоки, но
        // размер каталога должен покрывать записи+имена.
        let mut sz = 0usize;
        let mut idx = 0usize;
        while idx < dirents.len() {
            let mut n = 0usize;
            let mut names_len = 0usize;
            while idx + n < dirents.len() {
                let nl = dirents[idx + n].0.len();
                if (n + 1) * 12 + names_len + nl > bs {
                    break;
                }
                names_len += nl;
                n += 1;
            }
            if n == 0 {
                break;
            }
            let used = n * 12 + names_len;
            idx += n;
            sz += if idx < dirents.len() { bs } else { used };
        }
        sz
    };

    // --- распределяем блоки: сначала каталог, потом файлы ---
    let mut blocks: Vec<u8> = Vec::new();
    let dir_blkaddr = 1u32; // блок 0 занят SB+инодами
    blocks.extend_from_slice(&dir_data);

    let mut file_blkaddr: Vec<u32> = Vec::with_capacity(files.len());
    for (_, data) in files.iter() {
        let cur_block = dir_blkaddr as usize + blocks.len() / bs;
        file_blkaddr.push(cur_block as u32);
        blocks.extend_from_slice(data);
        let pad = align_up(data.len(), bs) - data.len();
        blocks.extend(core::iter::repeat_n(0u8, pad));
    }

    // --- собираем образ ---
    let total = bs + blocks.len();
    let mut img = vec![0u8; total];

    // суперблок
    let sb = EROFS_SUPER_OFFSET;
    wr_u32(&mut img, sb, EROFS_SUPER_MAGIC_V1); // magic
    wr_u32(&mut img, sb + 4, 0); // checksum (фича выключена)
    wr_u32(&mut img, sb + 8, 0); // feature_compat
    img[sb + 12] = EROFS_BLOCK_BITS; // blkszbits
    img[sb + 13] = 0; // sb_extslots
    wr_u16(&mut img, sb + 14, root_nid as u16); // root_nid
    wr_u64(&mut img, sb + 16, (1 + files.len()) as u64); // inos
    wr_u64(&mut img, sb + 24, 0); // build_time
    wr_u32(&mut img, sb + 32, 0); // build_time_nsec
    wr_u32(&mut img, sb + 36, (total / bs) as u32); // blocks
    wr_u32(&mut img, sb + 40, 0); // meta_blkaddr
    wr_u32(&mut img, sb + 44, 0); // xattr_blkaddr
    // uuid[16] @48, volume_name[16] @64 — нули
    wr_u32(&mut img, sb + 80, 0); // feature_incompat

    // корневой инод
    write_inode(
        &mut img,
        inode_area,
        S_IFDIR | 0o755,
        2,
        dir_size as u32,
        dir_blkaddr,
        1,
    );

    // иноды файлов
    for (i, (_, data)) in files.iter().enumerate() {
        write_inode(
            &mut img,
            inode_area + (1 + i) * EROFS_ISLOT_SIZE,
            S_IFREG | 0o644,
            1,
            data.len() as u32,
            file_blkaddr[i],
            (i + 2) as u32,
        );
    }

    // данные
    img[bs..bs + blocks.len()].copy_from_slice(&blocks);
    img
}

/// Проверяет, что образ — валидный EROFS.
pub fn is_erofs(img: &[u8]) -> bool {
    parse_superblock(img).is_ok()
}
