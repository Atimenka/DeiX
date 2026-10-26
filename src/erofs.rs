//! EROFS (Enhanced Read-Only File System) — НАСТОЯЩАЯ реализация формата.
//!
//! Соответствует спецификации EROFS v1 (erofs.docs.kernel.org, Linux
//! `fs/erofs/erofs_fs.h`). Модуль только читает: образ /system собирает
//! `tools/make_deix_fs.py` на этапе сборки, ядро его не создаёт. Образы
//! `mkfs.erofs` читаются этим модулем так же.
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
    /// Путь или имя не найдено в образе.
    NotFound,
    /// Устройство не отдало запрошенные сектора.
    Io,
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
            ErofsError::NotFound => "EROFS: файл или каталог не найден".into(),
            ErofsError::Io => "EROFS: ошибка чтения устройства".into(),
        }
    }
}

// ==================== ИСТОЧНИК ДАННЫХ ====================

/// Произвольный доступ к байтам образа. Читатель не требует, чтобы образ
/// целиком лежал в памяти: диск отдаёт только то, что запрошено.
pub trait Source {
    /// Читает до `len` байт со смещения `off`. У конца образа возвращает
    /// меньше запрошенного (в пределе — пустой вектор).
    fn read_at(&self, off: usize, len: usize) -> Result<Vec<u8>, ErofsError>;
}

impl Source for [u8] {
    fn read_at(&self, off: usize, len: usize) -> Result<Vec<u8>, ErofsError> {
        if off >= self.len() {
            return Ok(Vec::new());
        }
        let end = off.saturating_add(len).min(self.len());
        Ok(self[off..end].to_vec())
    }
}

impl Source for Vec<u8> {
    fn read_at(&self, off: usize, len: usize) -> Result<Vec<u8>, ErofsError> {
        self.as_slice().read_at(off, len)
    }
}

/// Читает ровно `len` байт или сообщает о повреждении образа.
fn read_exact<S: Source + ?Sized>(img: &S, off: usize, len: usize) -> Result<Vec<u8>, ErofsError> {
    let data = img.read_at(off, len)?;
    if data.len() == len {
        Ok(data)
    } else {
        Err(ErofsError::Corrupt)
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
    pub meta_blkaddr: u32,
}

impl Superblock {
    pub fn block_size(&self) -> usize {
        if self.blkszbits == EROFS_BLOCK_BITS {
            EROFS_BLOCK_SIZE
        } else {
            1usize << self.blkszbits
        }
    }

    /// Смещение инода по его nid.
    pub fn inode_offset(&self, nid: u64) -> usize {
        self.meta_blkaddr as usize * self.block_size() + nid as usize * EROFS_ISLOT_SIZE
    }
}

/// Проверяет и разбирает суперблок.
pub fn parse_superblock<S: Source + ?Sized>(img: &S) -> Result<Superblock, ErofsError> {
    let raw = img.read_at(EROFS_SUPER_OFFSET, EROFS_SUPER_SIZE)?;
    if raw.len() < EROFS_SUPER_SIZE {
        return Err(ErofsError::TooSmall);
    }
    let sb = &raw[..];
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
pub fn read_inode<S: Source + ?Sized>(img: &S, sb: &Superblock, nid: u64) -> Result<Inode, ErofsError> {
    let off = sb.inode_offset(nid);
    let raw = img.read_at(off, 64)?;
    let raw = &raw[..];
    let format = rd_u16(raw, 0).ok_or(ErofsError::Corrupt)?;
    let extended = format & 1 == 1;
    let datalayout = (format >> 1) & 7;
    let xattr_icount = rd_u16(raw, 2).ok_or(ErofsError::Corrupt)? as usize;
    let mode = rd_u16(raw, 4).ok_or(ErofsError::Corrupt)?;

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
            rd_u64(raw, 8).ok_or(ErofsError::Corrupt)?,
            rd_u32(raw, 16).ok_or(ErofsError::Corrupt)?,
        )
    } else {
        (
            rd_u32(raw, 8).ok_or(ErofsError::Corrupt)? as u64,
            rd_u32(raw, 16).ok_or(ErofsError::Corrupt)?,
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
fn inode_data<S: Source + ?Sized>(img: &S, sb: &Superblock, ino: &Inode) -> Result<Vec<u8>, ErofsError> {
    let bs = sb.block_size();
    let size = ino.size as usize;
    match ino.datalayout {
        DATALAYOUT_FLAT_PLAIN => {
            if ino.raw_blkaddr == EROFS_NULL_ADDR {
                return Ok(Vec::new());
            }
            let start = ino.raw_blkaddr as usize * bs;
            read_exact(img, start, size)
        }
        DATALAYOUT_FLAT_INLINE => {
            let tail = size % bs;
            let head = size - tail;
            let mut out = if head > 0 {
                if ino.raw_blkaddr == EROFS_NULL_ADDR {
                    return Err(ErofsError::Corrupt);
                }
                let start = ino.raw_blkaddr as usize * bs;
                read_exact(img, start, head)?
            } else {
                Vec::new()
            };
            if tail > 0 {
                out.extend_from_slice(&read_exact(img, ino.inline_off, tail)?);
            }
            Ok(out)
        }
        // Сжатые раскладки (COMPRESSION_*) не поддерживаем: наши разделы
        // создаются без сжатия, а тянуть в ядро LZ4/zstd ради чтения
        // чужих образов — та самая избыточность, от которой мы уходим.
        _ => Err(ErofsError::Corrupt),
    }
}

/// Читает кусок содержимого файла `[off, off + len)`. `len` обрезается по
/// размеру файла; за концом файла возвращается пустой вектор. Позволяет
/// обработать большой файл порциями, не держа его в памяти целиком.
pub fn read_file_range<S: Source + ?Sized>(
    img: &S,
    sb: &Superblock,
    ino: &Inode,
    off: usize,
    len: usize,
) -> Result<Vec<u8>, ErofsError> {
    let size = ino.size as usize;
    if off >= size {
        return Ok(Vec::new());
    }
    let len = len.min(size - off);
    let bs = sb.block_size();
    match ino.datalayout {
        DATALAYOUT_FLAT_PLAIN => {
            if ino.raw_blkaddr == EROFS_NULL_ADDR {
                return Err(ErofsError::Corrupt);
            }
            read_exact(img, ino.raw_blkaddr as usize * bs + off, len)
        }
        DATALAYOUT_FLAT_INLINE => {
            let head = size - size % bs;
            let mut out = Vec::with_capacity(len);
            if off < head {
                if ino.raw_blkaddr == EROFS_NULL_ADDR {
                    return Err(ErofsError::Corrupt);
                }
                let n = len.min(head - off);
                out.extend_from_slice(&read_exact(img, ino.raw_blkaddr as usize * bs + off, n)?);
            }
            let done = out.len();
            if done < len {
                let tail_off = off + done - head;
                out.extend_from_slice(&read_exact(img, ino.inline_off + tail_off, len - done)?);
            }
            Ok(out)
        }
        _ => Err(ErofsError::Corrupt),
    }
}

/// Читает записи каталога.
pub fn read_dir<S: Source + ?Sized>(img: &S, sb: &Superblock, nid: u64) -> Result<Vec<DirEntry>, ErofsError> {
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

/// Ищет обычный файл по пути (например "kernel/kernel.bin" или "kernel.bin") и возвращает его инод.
pub fn lookup_path<S: Source + ?Sized>(img: &S, path: &str) -> Result<Inode, ErofsError> {
    let sb = parse_superblock(img)?;
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Err(ErofsError::Corrupt);
    }

    let mut current_nid = sb.root_nid;
    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        let entries = read_dir(img, &sb, current_nid)?;
        let mut found = false;
        for e in entries {
            if e.name == *part {
                if is_last {
                    return read_inode(img, &sb, e.nid);
                } else if e.file_type == EROFS_FT_DIR {
                    current_nid = e.nid;
                    found = true;
                    break;
                }
            }
        }
        if !found && !is_last {
            return Err(ErofsError::Corrupt);
        }
    }
    Err(ErofsError::Corrupt)
}

/// Ищет обычный файл в корне и возвращает его инод.
pub fn lookup<S: Source + ?Sized>(img: &S, name: &str) -> Result<Inode, ErofsError> {
    let sb = parse_superblock(img)?;
    for e in read_dir(img, &sb, sb.root_nid)? {
        if e.name == name && e.file_type == EROFS_FT_REG_FILE {
            return read_inode(img, &sb, e.nid);
        }
    }
    Err(ErofsError::Corrupt)
}

/// Читает содержимое файла из образа по имени или пути.
pub fn read_file<S: Source + ?Sized>(img: &S, path: &str) -> Result<Vec<u8>, ErofsError> {
    let sb = parse_superblock(img)?;
    let ino = lookup_path(img, path).or_else(|_| lookup(img, path))?;
    inode_data(img, &sb, &ino)
}

/// Запись списка каталога: имя, признак каталога, размер в байтах.
#[derive(Debug, Clone)]
pub struct DirListing {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Возвращает nid каталога по пути. Пустой путь и "/" дают корень.
fn resolve_dir_nid<S: Source + ?Sized>(img: &S, sb: &Superblock, path: &str) -> Result<u64, ErofsError> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut nid = sb.root_nid;
    for part in parts {
        let entries = read_dir(img, sb, nid)?;
        let next = entries
            .into_iter()
            .find(|e| e.name == part && e.file_type == EROFS_FT_DIR)
            .ok_or(ErofsError::NotFound)?;
        nid = next.nid;
    }
    Ok(nid)
}

/// Содержимое каталога по пути. Служебные "." и ".." отбрасываются.
pub fn list_dir<S: Source + ?Sized>(img: &S, path: &str) -> Result<Vec<DirListing>, ErofsError> {
    let sb = parse_superblock(img)?;
    let nid = resolve_dir_nid(img, &sb, path)?;
    let mut out: Vec<DirListing> = Vec::new();
    for e in read_dir(img, &sb, nid)? {
        if e.name == "." || e.name == ".." {
            continue;
        }
        let is_dir = e.file_type == EROFS_FT_DIR;
        let size = match read_inode(img, &sb, e.nid) {
            Ok(ino) => ino.size,
            Err(_) => 0,
        };
        out.push(DirListing {
            name: e.name,
            is_dir,
            size,
        });
    }
    Ok(out)
}

/// Проверяет существование пути (файла или каталога) и возвращает инод.
pub fn stat<S: Source + ?Sized>(img: &S, path: &str) -> Result<Inode, ErofsError> {
    lookup_path(img, path)
}
