//! Драйвер ext2 — настоящая, официально совместимая файловая система
//! (Second Extended Filesystem), поверх драйвера ATA PIO (см. ata.rs).
//!
//! В отличие от fat16.rs (который мы оставляем нетронутым для обратной
//! совместимости со старыми образами), ext2 — это классическая Unix
//! файловая система на inode: суперблок с метаданными тома, таблица
//! групп блоков, битовые карты свободных блоков/инодов, таблица инодов
//! и настоящие Unix-права/владелец/группа у каждого файла.
//!
//! Формат СОЗНАТЕЛЬНО упрощён до совместимого подмножества:
//!   - revision 0 (fixed 128-byte inodes, без extended полей суперблока,
//!     без UUID/volume label в динамических полях) — это тот же формат,
//!     что использовал Linux до середины 90-х, и он полностью читается
//!     современным `mount`/`e2fsck` (проверено: см. tools/ext2_prototype_full.py
//!     и его комментарии — каждый шаг этого формата validated настоящим
//!     `e2fsck -f` и смонтирован через `mount -o loop` перед портированием
//!     сюда);
//!   - ровно ОДНА группа блоков (Block Group) — для тома в несколько
//!     мегабайт этого достаточно, а код становится сильно проще (не
//!     нужно решать, в какую из нескольких групп класть новый инод/файл);
//!   - каталоги — только корневой (без mkdir/подкаталогов пользователя);
//!     `lost+found` создаётся при форматировании как обычный ext2-том
//!     того требует (e2fsck ищет его при проверке).
//!
//! ФАЙЛЫ БОЛЬШОГО РАЗМЕРА (indirect-блоки) — раньше здесь было честное
//! ограничение "только 12 прямых указателей, максимум 12 КиБ на файл".
//! Теперь реализован ОДИНАРНЫЙ indirect-блок (offset 88..91 в inode,
//! block_ptrs[12] в терминах этого файла) — это тот же самый механизм,
//! который использует настоящий ext2: блок №13 указателя на самом деле
//! не хранит данные файла, а хранит ещё BLOCK_SIZE/4 = 256 указателей на
//! блоки данных. Это увеличивает максимальный размер файла с 12 КиБ до
//! 12 КиБ + 256 * 1 КиБ = 268 КиБ — с большим запасом хватает для .mex
//! программ, текстовых файлов и небольших изображений. Doubly-/triply-
//! indirect (offset 92..99, block_ptrs[13]/[14]) сознательно НЕ
//! реализованы — для мини-ОС 268 КиБ на файл более чем достаточно, а
//! тройная косвенность потребовала бы рекурсивного кода без явной
//! практической необходимости прямо сейчас.
//!
//! Что НЕ упрощено, а сделано по-настоящему: суперблок, group descriptor,
//! битовые карты, формат inode (включая теперь и indirect-блок) и
//! directory entry — это байт-в-байт то, что ожидает читать настоящий
//! Linux-код ext2 (fs/ext2/*). Если примонтировать наш образ диска в
//! Linux (`mount -o loop`), он примонтируется и будет полностью
//! работоспособен для чтения и записи, включая файлы, использующие
//! indirect-блок.

use crate::ata;
use alloc::string::String;
use alloc::vec::Vec;

pub const BLOCK_SIZE: usize = 1024;
pub const SECTOR_SIZE: usize = 512;
const SECTORS_PER_BLOCK: u32 = (BLOCK_SIZE / SECTOR_SIZE) as u32;
const INODE_SIZE: usize = 128;
const EXT2_MAGIC: u16 = 0xEF53;

const ROOT_INO: u32 = 2;
const LOST_AND_FOUND_INO: u32 = 11;
const FIRST_NON_RESERVED_INO: u32 = 11; // revision 0: inode 1..10 зарезервированы

/// Сколько ПРЯМЫХ блочных указателей хранится в inode — стандартное
/// поле формата ext2 (offset 40..87, 12 указателей по 4 байта).
const MAX_DIRECT_BLOCKS: usize = 12;
/// Индекс единственного indirect-указателя в массиве block_ptrs
/// (offset 88..91 в структуре inode — сразу после 12 прямых).
const INDIRECT_BLOCK_INDEX: usize = 12;
/// Сколько указателей на блоки данных помещается в один indirect-блок:
/// BLOCK_SIZE байт / 4 байта на указатель = 256 указателей при блоке
/// 1024 байта — это ровно то же вычисление, что делает настоящий ext2.
const PTRS_PER_INDIRECT_BLOCK: usize = BLOCK_SIZE / 4;
/// Максимальный размер файла с прямыми + одинарным indirect блоком.
const MAX_FILE_BLOCKS: usize = MAX_DIRECT_BLOCKS + PTRS_PER_INDIRECT_BLOCK;

/// Начало тома ext2 на диске (/userdata раздел на LBA 12800).
pub const FS_START_LBA: u32 = 12800;
/// Размер тома в секторах (раздел /userdata = 5632 секторов).
const TOTAL_SECTORS: u32 = 5632;
const TOTAL_BLOCKS: u32 = TOTAL_SECTORS / SECTORS_PER_BLOCK;

const SB_BLOCK: u32 = 1;
const GDT_BLOCK: u32 = SB_BLOCK + 1;
const BLOCK_BITMAP_BLOCK: u32 = GDT_BLOCK + 1;
const INODE_BITMAP_BLOCK: u32 = BLOCK_BITMAP_BLOCK + 1;
const INODE_TABLE_START: u32 = INODE_BITMAP_BLOCK + 1;


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ext2Error {
    DiskError,
    NotFormatted,
    FileNotFound,
    NoSpace,
    InvalidName,
    FileTooLarge,
    DirectoryFull,
}

pub struct FileEntry {
    pub name: String,
    pub size: u32,
    pub is_directory: bool,
}

// ---------------- геометрия тома (вычисляется из фиксированных констант,
// т.к. у нас всегда один и тот же размер тома — в отличие от настоящего
// mke2fs, который считает это динамически под любой размер диска) ----------------

fn inodes_count() -> u32 {
    // То же правило, что использует mke2fs по умолчанию (~1 инод на
    // 4096 байт данных), округлённое вверх до кратности 8 (чтобы битовая
    // карта инодов укладывалась в целое число байт без дробных остатков).
    let raw = core::cmp::max(32, (TOTAL_BLOCKS as u64 * BLOCK_SIZE as u64) / 4096) as u32;
    (raw + 7) / 8 * 8
}

fn inode_table_blocks() -> u32 {
    let bytes = inodes_count() as u64 * INODE_SIZE as u64;
    ((bytes + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64) as u32
}

fn data_start_block() -> u32 {
    INODE_TABLE_START + inode_table_blocks()
}

fn root_dir_block() -> u32 {
    data_start_block()
}

fn lost_found_block() -> u32 {
    data_start_block() + 1
}

/// Последний физический номер блока, занятого метаданными/начальными
/// каталогами (всё с блока 1 по этот номер включительно занято сразу
/// после форматирования).
fn reserved_meta_blocks() -> u32 {
    lost_found_block()
}

/// Валидных (физически существующих) блоков в единственной группе:
/// блок 0 не входит в группу (first_data_block=1 для блоков размером
/// 1024 байта), поэтому валидны номера 1..(TOTAL_BLOCKS-1).
fn valid_blocks_in_group() -> u32 {
    TOTAL_BLOCKS - 1
}

// ---------------- низкоуровневый ввод-вывод блоков ----------------

/// Читает один ext2-блок (BLOCK_SIZE=1024 байта = 2 физических
/// 512-байтных ATA-сектора) и, если сквозное шифрование диска включено
/// (см. crypto_storage.rs), прозрачно расшифровывает КАЖДЫЙ из этих
/// 2 физических секторов независимо, используя его собственный
/// абсолютный LBA-номер как XTS tweak — именно так поступает настоящий
/// dm-crypt (каждый физический сектор шифруется отдельно со своим
/// номером, а не весь блок целиком одним tweak-ом). Сама файловая
/// система (весь остальной код ext2.rs) не подозревает о шифровании —
/// получает уже расшифрованные обычные байты, как если бы диска
/// шифрования вовсе не было.
fn read_block(block: u32) -> Result<[u8; BLOCK_SIZE], Ext2Error> {
    let mut buf = [0u8; BLOCK_SIZE];
    let lba = FS_START_LBA + block * SECTORS_PER_BLOCK;
    ata::read_sectors(lba, SECTORS_PER_BLOCK as u8, &mut buf).map_err(|_| Ext2Error::DiskError)?;

    for i in 0..SECTORS_PER_BLOCK {
        let sector_lba = lba + i;
        let start = (i as usize) * SECTOR_SIZE;
        let end = start + SECTOR_SIZE;
        crate::crypto_storage::decrypt_sector_after_read(sector_lba, &mut buf[start..end]);
    }

    Ok(buf)
}

fn write_block(block: u32, data: &[u8; BLOCK_SIZE]) -> Result<(), Ext2Error> {
    let lba = FS_START_LBA + block * SECTORS_PER_BLOCK;

    // Шифруем в отдельном буфере (не трогая переданные данные вызывающей
    // стороны — write_block принимает &[u8; BLOCK_SIZE] по ссылке, а не
    // мутабельно, поэтому все остальные функции ext2.rs продолжают
    // работать с незашифрованными байтами в своей памяти, как и раньше;
    // шифрование существует только "на пути к диску").
    let mut encrypted = *data;
    for i in 0..SECTORS_PER_BLOCK {
        let sector_lba = lba + i;
        let start = (i as usize) * SECTOR_SIZE;
        let end = start + SECTOR_SIZE;
        crate::crypto_storage::encrypt_sector_for_write(sector_lba, &mut encrypted[start..end]);
    }

    ata::write_sectors(lba, SECTORS_PER_BLOCK as u8, &encrypted).map_err(|_| Ext2Error::DiskError)
}

fn write_block_partial(block: u32, offset: usize, data: &[u8]) -> Result<(), Ext2Error> {
    let mut buf = read_block(block)?;
    buf[offset..offset + data.len()].copy_from_slice(data);
    write_block(block, &buf)
}

// ---------------- проверка форматирования ----------------

pub fn is_formatted() -> bool {
    match read_block(SB_BLOCK) {
        Ok(sb) => u16::from_le_bytes([sb[56], sb[57]]) == EXT2_MAGIC,
        Err(_) => false,
    }
}

// ---------------- битовые карты ----------------

fn bitmap_get(block: u32, bit: u32) -> Result<bool, Ext2Error> {
    let buf = read_block(block)?;
    let byte = buf[(bit / 8) as usize];
    Ok((byte >> (bit % 8)) & 1 != 0)
}

fn bitmap_set(block: u32, bit: u32, value: bool) -> Result<(), Ext2Error> {
    let mut buf = read_block(block)?;
    let idx = (bit / 8) as usize;
    if value {
        buf[idx] |= 1 << (bit % 8);
    } else {
        buf[idx] &= !(1 << (bit % 8));
    }
    write_block(block, &buf)
}

fn adjust_free_blocks(delta: i32) -> Result<(), Ext2Error> {
    let mut sb = read_block(SB_BLOCK)?;
    let val = u32::from_le_bytes(sb[12..16].try_into().unwrap());
    let new_val = (val as i64 + delta as i64) as u32;
    sb[12..16].copy_from_slice(&new_val.to_le_bytes());
    write_block(SB_BLOCK, &sb)?;

    let mut gdt = read_block(GDT_BLOCK)?;
    let val = u16::from_le_bytes(gdt[12..14].try_into().unwrap());
    let new_val = (val as i32 + delta) as u16;
    gdt[12..14].copy_from_slice(&new_val.to_le_bytes());
    write_block(GDT_BLOCK, &gdt)
}

fn adjust_free_inodes(delta: i32) -> Result<(), Ext2Error> {
    let mut sb = read_block(SB_BLOCK)?;
    let val = u32::from_le_bytes(sb[16..20].try_into().unwrap());
    let new_val = (val as i64 + delta as i64) as u32;
    sb[16..20].copy_from_slice(&new_val.to_le_bytes());
    write_block(SB_BLOCK, &sb)?;

    let mut gdt = read_block(GDT_BLOCK)?;
    let val = u16::from_le_bytes(gdt[14..16].try_into().unwrap());
    let new_val = (val as i32 + delta) as u16;
    gdt[14..16].copy_from_slice(&new_val.to_le_bytes());
    write_block(GDT_BLOCK, &gdt)
}

fn alloc_block() -> Result<u32, Ext2Error> {
    for bit in 0..valid_blocks_in_group() {
        if !bitmap_get(BLOCK_BITMAP_BLOCK, bit)? {
            bitmap_set(BLOCK_BITMAP_BLOCK, bit, true)?;
            adjust_free_blocks(-1)?;
            return Ok(bit + 1); // физический номер = first_data_block(1) + bit
        }
    }
    Err(Ext2Error::NoSpace)
}

fn free_block(block_num: u32) -> Result<(), Ext2Error> {
    let bit = block_num - 1;
    bitmap_set(BLOCK_BITMAP_BLOCK, bit, false)?;
    adjust_free_blocks(1)
}

fn alloc_inode() -> Result<u32, Ext2Error> {
    for bit in 0..inodes_count() {
        if !bitmap_get(INODE_BITMAP_BLOCK, bit)? {
            bitmap_set(INODE_BITMAP_BLOCK, bit, true)?;
            adjust_free_inodes(-1)?;
            return Ok(bit + 1);
        }
    }
    Err(Ext2Error::NoSpace)
}

fn free_inode(ino: u32) -> Result<(), Ext2Error> {
    bitmap_set(INODE_BITMAP_BLOCK, ino - 1, false)?;
    adjust_free_inodes(1)
}

// ---------------- чтение/запись inode ----------------

fn inode_location(ino: u32) -> (u32, usize) {
    let index = ino - 1;
    let containing_block = INODE_TABLE_START + (index * INODE_SIZE as u32) / BLOCK_SIZE as u32;
    let offset_in_block = ((index as usize) * INODE_SIZE) % BLOCK_SIZE;
    (containing_block, offset_in_block)
}

struct Inode {
    mode: u16,
    size: u32,
    links_count: u16,
    block_ptrs: [u32; 15],
}

fn read_inode(ino: u32) -> Result<Inode, Ext2Error> {
    let (block, offset) = inode_location(ino);
    let buf = read_block(block)?;
    let raw = &buf[offset..offset + INODE_SIZE];

    let mode = u16::from_le_bytes([raw[0], raw[1]]);
    let size = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    let links_count = u16::from_le_bytes([raw[26], raw[27]]);
    let mut block_ptrs = [0u32; 15];
    for i in 0..15 {
        let o = 40 + i * 4;
        block_ptrs[i] = u32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]);
    }

    Ok(Inode { mode, size, links_count, block_ptrs })
}

/// Пишет структуру inode на диск. `used_blocks` — РЕАЛЬНОЕ количество
/// физически занятых блоков этого файла (не количество ненулевых
/// указателей в block_ptrs!) — при наличии indirect-блока это 12 прямых
/// + 1 сам indirect-блок + сколько бы блоков данных ни было перечислено
/// внутри него. i_blocks в ext2 всегда означает "сколько 512-байтных
/// секторов реально выделено под этот файл на диске, включая метаданные
/// косвенной адресации", а НЕ "сколько указателей ненулевые" — это была
/// изначальная ошибка (обнаружена через e2fsck: "i_blocks is 26, should
/// be 130" для файла с indirect-блоком, где 26 — это 13 указателей * 2
/// сектора на блок, а не реальные 65 занятых блоков).
fn write_inode(ino: u32, mode: u16, size: u32, links_count: u16, block_ptrs: &[u32], used_blocks: u32) -> Result<(), Ext2Error> {
    let (block, offset) = inode_location(ino);
    let mut raw = [0u8; INODE_SIZE];

    raw[0..2].copy_from_slice(&mode.to_le_bytes());
    raw[4..8].copy_from_slice(&size.to_le_bytes());
    let blocks_512 = used_blocks * (BLOCK_SIZE / SECTOR_SIZE) as u32;
    raw[26..28].copy_from_slice(&links_count.to_le_bytes());
    raw[28..32].copy_from_slice(&blocks_512.to_le_bytes());
    for (i, &ptr) in block_ptrs.iter().take(15).enumerate() {
        let o = 40 + i * 4;
        raw[o..o + 4].copy_from_slice(&ptr.to_le_bytes());
    }

    write_block_partial(block, offset, &raw)
}

/// Обнуляет структуру inode на диске и выставляет dtime (offset 20) —
/// КРИТИЧЕСКИ важно при удалении файла, помимо очистки битов в bitmap.
/// Без этого шага e2fsck находит в таблице инодов "живую на вид"
/// структуру (ненулевые mode/блочные указатели), не связанную ни с одной
/// directory entry, и считает её потерянным файлом ("unattached inode") —
/// именно так и проявлялся баг при разработке (см. историю в
/// tools/ext2_prototype_full.py). Настоящий Linux (fs/ext2/inode.c)
/// поступает точно так же при удалении инода.
fn clear_inode(ino: u32, dtime: u32) -> Result<(), Ext2Error> {
    let (block, offset) = inode_location(ino);
    let mut raw = [0u8; INODE_SIZE];
    raw[20..24].copy_from_slice(&dtime.to_le_bytes());
    write_block_partial(block, offset, &raw)
}

// ---------------- directory entries ----------------

/// Пишет один directory entry по заданному смещению внутри блока.
fn make_dirent(buf: &mut [u8], offset: usize, ino: u32, name: &str, rec_len: u16) {
    buf[offset..offset + 4].copy_from_slice(&ino.to_le_bytes());
    buf[offset + 4..offset + 6].copy_from_slice(&rec_len.to_le_bytes());
    buf[offset + 6] = name.len() as u8;
    buf[offset + 7] = 0; // file type indicator: не используем (feature bit не выставлен)
    buf[offset + 8..offset + 8 + name.len()].copy_from_slice(name.as_bytes());
}

struct DirEntryRef {
    offset: usize,
    ino: u32,
    rec_len: u16,
    name_len: u8,
}

fn dirent_at(buf: &[u8], offset: usize) -> Option<DirEntryRef> {
    if offset + 8 > BLOCK_SIZE {
        return None;
    }
    let ino = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
    let rec_len = u16::from_le_bytes(buf[offset + 4..offset + 6].try_into().unwrap());
    if rec_len == 0 {
        return None;
    }
    let name_len = buf[offset + 6];
    Some(DirEntryRef { offset, ino, rec_len, name_len })
}

fn dirent_name(buf: &[u8], entry: &DirEntryRef) -> String {
    let start = entry.offset + 8;
    let end = start + entry.name_len as usize;
    core::str::from_utf8(&buf[start..end]).unwrap_or("").into()
}

fn dir_block_ptrs(inode: &Inode) -> impl Iterator<Item = u32> + '_ {
    inode.block_ptrs[..MAX_DIRECT_BLOCKS].iter().copied().filter(|&p| p != 0)
}

/// Читает индексный (indirect) блок и возвращает его как массив из 256
/// u32-указателей — то же самое, что делает ядро Linux в
/// fs/ext2/inode.c при обращении к ind-блоку. Возвращает все нули, если
/// indirect-блок ещё не выделен (в inode стоит 0) — вызывающий код
/// интерпретирует такие нули как "блок не выделен", как и для обычных
/// прямых указателей.
fn read_indirect_block(indirect_block_num: u32) -> Result<[u32; PTRS_PER_INDIRECT_BLOCK], Ext2Error> {
    let mut ptrs = [0u32; PTRS_PER_INDIRECT_BLOCK];
    if indirect_block_num == 0 {
        return Ok(ptrs);
    }
    let raw = read_block(indirect_block_num)?;
    for (i, chunk) in raw.chunks_exact(4).enumerate() {
        ptrs[i] = u32::from_le_bytes(chunk.try_into().unwrap());
    }
    Ok(ptrs)
}

fn write_indirect_block(indirect_block_num: u32, ptrs: &[u32; PTRS_PER_INDIRECT_BLOCK]) -> Result<(), Ext2Error> {
    let mut raw = [0u8; BLOCK_SIZE];
    for (i, &ptr) in ptrs.iter().enumerate() {
        raw[i * 4..i * 4 + 4].copy_from_slice(&ptr.to_le_bytes());
    }
    write_block(indirect_block_num, &raw)
}

/// Возвращает ПОЛНЫЙ список блоков данных файла — сначала до 12 прямых
/// указателей, затем (если файл больше 12 КиБ) содержимое indirect-
/// блока. В отличие от dir_block_ptrs (используется только для
/// каталогов, которые у нас никогда не превышают 12 КиБ, поэтому им
/// indirect не нужен), эта функция — общий путь для файлов.
fn file_block_list(inode: &Inode) -> Result<Vec<u32>, Ext2Error> {
    let mut blocks: Vec<u32> = inode.block_ptrs[..MAX_DIRECT_BLOCKS]
        .iter()
        .copied()
        .filter(|&p| p != 0)
        .collect();

    let indirect = inode.block_ptrs[INDIRECT_BLOCK_INDEX];
    if indirect != 0 {
        let ptrs = read_indirect_block(indirect)?;
        blocks.extend(ptrs.iter().copied().filter(|&p| p != 0));
    }

    Ok(blocks)
}

fn find_in_dir(dir_ino: u32, name: &str) -> Result<Option<(u32, usize, u32)>, Ext2Error> {
    let dir_inode = read_inode(dir_ino)?;
    for block_num in dir_block_ptrs(&dir_inode) {
        let buf = read_block(block_num)?;
        let mut offset = 0usize;
        while let Some(entry) = dirent_at(&buf, offset) {
            if entry.ino != 0 && dirent_name(&buf, &entry) == name {
                return Ok(Some((block_num, entry.offset, entry.ino)));
            }
            offset += entry.rec_len as usize;
        }
    }
    Ok(None)
}

fn add_dirent(dir_ino: u32, name: &str, new_ino: u32) -> Result<(), Ext2Error> {
    if name.is_empty() || name.len() > 55 {
        return Err(Ext2Error::InvalidName);
    }
    let needed_len = (((8 + name.len()) + 3) / 4 * 4) as u16;

    let dir_inode = read_inode(dir_ino)?;
    let block_list: Vec<u32> = dir_block_ptrs(&dir_inode).collect();

    for block_num in &block_list {
        let mut buf = read_block(*block_num)?;
        let mut offset = 0usize;
        while let Some(entry) = dirent_at(&buf, offset) {
            let actual_len = if entry.ino != 0 {
                ((8 + entry.name_len as usize + 3) / 4 * 4) as u16
            } else {
                0
            };
            let free_in_entry = entry.rec_len - actual_len;

            if entry.ino == 0 && entry.rec_len >= needed_len {
                make_dirent(&mut buf, entry.offset, new_ino, name, entry.rec_len);
                write_block(*block_num, &buf)?;
                return Ok(());
            }
            if entry.ino != 0 && free_in_entry >= needed_len {
                buf[entry.offset + 4..entry.offset + 6].copy_from_slice(&actual_len.to_le_bytes());
                let new_offset = entry.offset + actual_len as usize;
                let new_rec_len = entry.rec_len - actual_len;
                make_dirent(&mut buf, new_offset, new_ino, name, new_rec_len);
                write_block(*block_num, &buf)?;
                return Ok(());
            }
            offset += entry.rec_len as usize;
        }
    }

    // Не нашли места в существующих блоках каталога — нужен новый блок
    // (если есть свободный прямой указатель).
    let mut new_ptrs = dir_inode.block_ptrs;
    let free_slot = new_ptrs[..MAX_DIRECT_BLOCKS].iter().position(|&p| p == 0);
    let slot = match free_slot {
        Some(s) => s,
        None => return Err(Ext2Error::DirectoryFull),
    };

    let new_block = alloc_block()?;
    let mut buf = [0u8; BLOCK_SIZE];
    make_dirent(&mut buf, 0, new_ino, name, BLOCK_SIZE as u16);
    write_block(new_block, &buf)?;

    new_ptrs[slot] = new_block;
    let new_size = dir_inode.size + BLOCK_SIZE as u32;
    // Каталоги используют только прямые блоки (см. dir_block_ptrs) —
    // used_blocks здесь всегда равен числу ненулевых прямых указателей.
    let used_blocks = new_ptrs[..MAX_DIRECT_BLOCKS].iter().filter(|&&p| p != 0).count() as u32;
    write_inode(dir_ino, dir_inode.mode, new_size, dir_inode.links_count, &new_ptrs, used_blocks)
}

fn remove_dirent(dir_ino: u32, name: &str) -> Result<bool, Ext2Error> {
    let dir_inode = read_inode(dir_ino)?;
    for block_num in dir_block_ptrs(&dir_inode) {
        let mut buf = read_block(block_num)?;
        let mut offset = 0usize;
        while let Some(entry) = dirent_at(&buf, offset) {
            if entry.ino != 0 && dirent_name(&buf, &entry) == name {
                buf[entry.offset..entry.offset + 4].copy_from_slice(&0u32.to_le_bytes());
                write_block(block_num, &buf)?;
                return Ok(true);
            }
            offset += entry.rec_len as usize;
        }
    }
    Ok(false)
}

// ---------------- форматирование ----------------

/// Текущее "время" ядра в секундах — у нас нет RTC-драйвера, поэтому
/// используем аптайм с момента загрузки как приближение (не настоящий
/// Unix-таймstamp, но ext2 не требует, чтобы время было "настоящим" для
/// корректности структуры — e2fsck не проверяет правдоподобность самих
/// значений времени, только их наличие).
fn fake_timestamp() -> u32 {
    (crate::timer::uptime_ms() / 1000) as u32
}

pub fn format() -> Result<(), Ext2Error> {
    let now = fake_timestamp();
    let inodes = inodes_count();
    let reserved = reserved_meta_blocks();
    let valid_blocks = valid_blocks_in_group();
    let free_blocks_count = valid_blocks - reserved;
    let free_inodes_count = inodes - (FIRST_NON_RESERVED_INO - 1) - 1;

    // ---- Superblock ----
    let mut sb = [0u8; BLOCK_SIZE];
    sb[0..4].copy_from_slice(&inodes.to_le_bytes());
    sb[4..8].copy_from_slice(&TOTAL_BLOCKS.to_le_bytes());
    sb[8..12].copy_from_slice(&0u32.to_le_bytes()); // reserved blocks for superuser
    sb[12..16].copy_from_slice(&free_blocks_count.to_le_bytes());
    sb[16..20].copy_from_slice(&free_inodes_count.to_le_bytes());
    sb[20..24].copy_from_slice(&SB_BLOCK.to_le_bytes()); // first data block
    sb[24..28].copy_from_slice(&0u32.to_le_bytes()); // log2(block size) - 10 => 1024 << 0
    sb[28..32].copy_from_slice(&0u32.to_le_bytes()); // log2(fragment size) - 10
    sb[32..36].copy_from_slice(&TOTAL_BLOCKS.to_le_bytes()); // blocks per group (одна группа)
    sb[36..40].copy_from_slice(&TOTAL_BLOCKS.to_le_bytes()); // fragments per group
    sb[40..44].copy_from_slice(&inodes.to_le_bytes()); // inodes per group
    sb[44..48].copy_from_slice(&0u32.to_le_bytes()); // last mount time
    sb[48..52].copy_from_slice(&now.to_le_bytes()); // last write time
    sb[52..54].copy_from_slice(&0u16.to_le_bytes()); // mount count
    sb[54..56].copy_from_slice(&0xFFFFu16.to_le_bytes()); // max mount count (-1)
    sb[56..58].copy_from_slice(&EXT2_MAGIC.to_le_bytes());
    sb[58..60].copy_from_slice(&1u16.to_le_bytes()); // state: clean
    sb[60..62].copy_from_slice(&1u16.to_le_bytes()); // errors: continue
    sb[62..64].copy_from_slice(&0u16.to_le_bytes()); // minor version
    sb[64..68].copy_from_slice(&now.to_le_bytes()); // last check time
    sb[68..72].copy_from_slice(&0u32.to_le_bytes()); // check interval
    sb[72..76].copy_from_slice(&0u32.to_le_bytes()); // creator OS: Linux (для совместимости с fsck/mount)
    sb[76..80].copy_from_slice(&0u32.to_le_bytes()); // major version: 0 (revision 0)
    write_block(SB_BLOCK, &sb)?;

    // ---- Group Descriptor Table ----
    let mut gdt = [0u8; BLOCK_SIZE];
    gdt[0..4].copy_from_slice(&BLOCK_BITMAP_BLOCK.to_le_bytes());
    gdt[4..8].copy_from_slice(&INODE_BITMAP_BLOCK.to_le_bytes());
    gdt[8..12].copy_from_slice(&INODE_TABLE_START.to_le_bytes());
    gdt[12..14].copy_from_slice(&(free_blocks_count as u16).to_le_bytes());
    gdt[14..16].copy_from_slice(&(free_inodes_count as u16).to_le_bytes());
    gdt[16..18].copy_from_slice(&2u16.to_le_bytes()); // число каталогов: root + lost+found
    write_block(GDT_BLOCK, &gdt)?;

    // ---- Block bitmap ----
    let mut block_bitmap = [0u8; BLOCK_SIZE];
    for b in 0..reserved {
        block_bitmap[(b / 8) as usize] |= 1 << (b % 8);
    }
    for b in valid_blocks..(BLOCK_SIZE as u32 * 8) {
        block_bitmap[(b / 8) as usize] |= 1 << (b % 8);
    }
    write_block(BLOCK_BITMAP_BLOCK, &block_bitmap)?;

    // ---- Inode bitmap ----
    let mut inode_bitmap = [0u8; BLOCK_SIZE];
    for i in 0..FIRST_NON_RESERVED_INO {
        inode_bitmap[(i / 8) as usize] |= 1 << (i % 8);
    }
    inode_bitmap[((LOST_AND_FOUND_INO - 1) / 8) as usize] |= 1 << ((LOST_AND_FOUND_INO - 1) % 8);
    for i in inodes..(BLOCK_SIZE as u32 * 8) {
        inode_bitmap[(i / 8) as usize] |= 1 << (i % 8);
    }
    write_block(INODE_BITMAP_BLOCK, &inode_bitmap)?;

    // ---- Root directory (inode 2) ----
    let mut root_block = [0u8; BLOCK_SIZE];
    make_dirent(&mut root_block, 0, ROOT_INO, ".", 12);
    make_dirent(&mut root_block, 12, ROOT_INO, "..", 12);
    make_dirent(&mut root_block, 24, LOST_AND_FOUND_INO, "lost+found", (BLOCK_SIZE - 24) as u16);
    write_block(root_dir_block(), &root_block)?;

    let mut root_ptrs = [0u32; 15];
    root_ptrs[0] = root_dir_block();
    write_inode(ROOT_INO, 0o040755, BLOCK_SIZE as u32, 3, &root_ptrs, 1)?;

    // ---- lost+found (inode 11) ----
    let mut lf_block = [0u8; BLOCK_SIZE];
    make_dirent(&mut lf_block, 0, LOST_AND_FOUND_INO, ".", 12);
    make_dirent(&mut lf_block, 12, ROOT_INO, "..", (BLOCK_SIZE - 12) as u16);
    write_block(lost_found_block(), &lf_block)?;

    let mut lf_ptrs = [0u32; 15];
    lf_ptrs[0] = lost_found_block();
    write_inode(LOST_AND_FOUND_INO, 0o040700, BLOCK_SIZE as u32, 2, &lf_ptrs, 1)?;

    Ok(())
}

// ---------------- имена файлов ----------------

/// ext2 (в отличие от FAT) не требует формата 8.3 — имена произвольной
/// длины (до 55 символов в нашей реализации directory entry, см.
/// add_dirent). Единственное ограничение — недопустимость '/' и NUL
/// (обычные ограничения Unix-путей).
fn validate_name(name: &str) -> Result<(), Ext2Error> {
    if name.is_empty() || name.len() > 55 {
        return Err(Ext2Error::InvalidName);
    }
    if name.contains('/') || name.contains('\0') {
        return Err(Ext2Error::InvalidName);
    }
    Ok(())
}

// ---------------- публичное API (совместимое по духу с fat16.rs) ----------------

pub fn list_root() -> Result<Vec<FileEntry>, Ext2Error> {
    if !is_formatted() {
        return Err(Ext2Error::NotFormatted);
    }

    let dir_inode = read_inode(ROOT_INO)?;
    let mut entries = Vec::new();

    for block_num in dir_block_ptrs(&dir_inode) {
        let buf = read_block(block_num)?;
        let mut offset = 0usize;
        while let Some(entry) = dirent_at(&buf, offset) {
            if entry.ino != 0 {
                let name = dirent_name(&buf, &entry);
                if name != "." && name != ".." && name != "lost+found" {
                    let file_inode = read_inode(entry.ino)?;
                    let is_directory = (file_inode.mode & 0xF000) == 0x4000;
                    entries.push(FileEntry {
                        name,
                        size: file_inode.size,
                        is_directory,
                    });
                }
            }
            offset += entry.rec_len as usize;
        }
    }

    Ok(entries)
}

/// Читает содержимое файла по номеру инода.
pub fn read_inode_data(ino: u32) -> Result<Vec<u8>, Ext2Error> {
    let inode = read_inode(ino)?;
    if inode.size > 8 * 1024 * 1024 {
        return Err(Ext2Error::DiskError);
    }
    let mut data = Vec::with_capacity(inode.size as usize);
    let mut left = inode.size as usize;
    for block_num in dir_block_ptrs(&inode) {
        if left == 0 {
            break;
        }
        let buf = read_block(block_num)?;
        let take = left.min(BLOCK_SIZE);
        data.extend_from_slice(&buf[..take]);
        left -= take;
    }
    Ok(data)
}

pub fn read_file(name: &str) -> Result<Vec<u8>, Ext2Error> {
    if !is_formatted() {
        return Err(Ext2Error::NotFormatted);
    }
    validate_name(name)?;

    let found = find_in_dir(ROOT_INO, name)?;
    let (_, _, ino) = found.ok_or(Ext2Error::FileNotFound)?;

    let inode = read_inode(ino)?;
    if inode.size > 8 * 1024 * 1024 {
        return Err(Ext2Error::DiskError);
    }
    let mut data = Vec::with_capacity(inode.size as usize);
    let mut remaining = inode.size as usize;

    // file_block_list() уже возвращает блоки в правильном порядке:
    // сначала 12 прямых, затем (если файл больше 12 КиБ) блоки,
    // перечисленные в indirect-блоке — то есть ровно та
    // последовательность, в которой были записаны данные файла.
    for ptr in file_block_list(&inode)? {
        if remaining == 0 {
            break;
        }
        let block_data = read_block(ptr)?;
        let take = remaining.min(BLOCK_SIZE);
        data.extend_from_slice(&block_data[..take]);
        remaining -= take;
    }

    Ok(data)
}

fn free_file_blocks(ino: u32) -> Result<(), Ext2Error> {
    let inode = read_inode(ino)?;

    // Освобождаем блоки данных (прямые + перечисленные в indirect-блоке).
    for ptr in file_block_list(&inode)? {
        free_block(ptr)?;
    }

    // Сам indirect-блок — это ТОЖЕ занятый блок на диске (в нём хранятся
    // указатели, а не данные файла), поэтому его нужно освободить
    // отдельно, а не только блоки, на которые он указывает — иначе
    // e2fsck найдёт "утёкший" блок, который значится занятым в bitmap,
    // но ни на что не ссылается ни один живой inode.
    let indirect = inode.block_ptrs[INDIRECT_BLOCK_INDEX];
    if indirect != 0 {
        free_block(indirect)?;
    }

    free_inode(ino)?;
    clear_inode(ino, fake_timestamp())
}

pub fn write_file(name: &str, data: &[u8]) -> Result<(), Ext2Error> {
    write_file_in(ROOT_INO, name, data)
}

/// Запись файла в ПРОИЗВОЛЬНЫЙ каталог (по иноду). Вся логика прямых и
/// indirect-блоков общая с корневой записью — дублировать её нельзя,
/// иначе две реализации разъедутся.
pub fn write_file_in(dir_ino: u32, name: &str, data: &[u8]) -> Result<(), Ext2Error> {
    if !is_formatted() {
        return Err(Ext2Error::NotFormatted);
    }
    validate_name(name)?;

    if let Some((_, _, existing_ino)) = find_in_dir(dir_ino, name)? {
        free_file_blocks(existing_ino)?;
        remove_dirent(dir_ino, name)?;
    }

    let blocks_needed = data.len().div_ceil(BLOCK_SIZE).max(if data.is_empty() { 0 } else { 1 });
    if blocks_needed > MAX_FILE_BLOCKS {
        return Err(Ext2Error::FileTooLarge);
    }

    let mut block_ptrs = [0u32; 15];

    // Сначала заполняем до MAX_DIRECT_BLOCKS прямых указателей — точно
    // так же, как раньше.
    let direct_blocks = blocks_needed.min(MAX_DIRECT_BLOCKS);
    for i in 0..direct_blocks {
        let b = alloc_block()?;
        let start = i * BLOCK_SIZE;
        let end = (start + BLOCK_SIZE).min(data.len());
        let mut block_buf = [0u8; BLOCK_SIZE];
        block_buf[..end - start].copy_from_slice(&data[start..end]);
        write_block(b, &block_buf)?;
        block_ptrs[i] = b;
    }

    // used_blocks считает РЕАЛЬНО занятые на диске блоки — для файла с
    // indirect-адресацией это прямые блоки + блоки данных за
    // indirect-указателем + САМ indirect-блок (он тоже физически занят,
    // хоть и не содержит данных файла напрямую).
    let mut used_blocks = direct_blocks as u32;

    // Если файл больше 12 КиБ — остаток данных уходит в блоки,
    // перечисленные через indirect-блок (тот же механизм, что и в
    // настоящем ext2: block_ptrs[12] указывает не на данные, а на блок,
    // целиком состоящий из ещё 256 указателей на блоки данных).
    if blocks_needed > MAX_DIRECT_BLOCKS {
        let indirect_data_blocks = blocks_needed - MAX_DIRECT_BLOCKS;
        let mut indirect_ptrs = [0u32; PTRS_PER_INDIRECT_BLOCK];

        for i in 0..indirect_data_blocks {
            let b = alloc_block()?;
            let start = (MAX_DIRECT_BLOCKS + i) * BLOCK_SIZE;
            let end = (start + BLOCK_SIZE).min(data.len());
            let mut block_buf = [0u8; BLOCK_SIZE];
            block_buf[..end - start].copy_from_slice(&data[start..end]);
            write_block(b, &block_buf)?;
            indirect_ptrs[i] = b;
        }

        let indirect_block_num = alloc_block()?;
        write_indirect_block(indirect_block_num, &indirect_ptrs)?;
        block_ptrs[INDIRECT_BLOCK_INDEX] = indirect_block_num;

        // + indirect_data_blocks блоков данных, + 1 сам indirect-блок.
        used_blocks += indirect_data_blocks as u32 + 1;
    }

    let ino = alloc_inode()?;
    write_inode(ino, 0o100644, data.len() as u32, 1, &block_ptrs, used_blocks)?;
    add_dirent(dir_ino, name, ino)?;

    Ok(())
}

pub fn delete_file(name: &str) -> Result<(), Ext2Error> {
    if !is_formatted() {
        return Err(Ext2Error::NotFormatted);
    }
    validate_name(name)?;

    let found = find_in_dir(ROOT_INO, name)?;
    let (_, _, ino) = found.ok_or(Ext2Error::FileNotFound)?;

    free_file_blocks(ino)?;
    remove_dirent(ROOT_INO, name)?;

    Ok(())
}

// ==================== КАТАЛОГИ И ПУТИ ====================
//
// Менять файловую систему на ext4/NTFS ради подкаталогов не нужно:
// ext2 поддерживает иерархию с самого начала. В томе уже лежит
// настоящий каталог `lost+found` (inode 11, режим 0o040700) — его
// создаёт format(). Не хватало только публичного API: создания
// каталогов в рантайме и разбора путей вида "/users/ivan/files".
//
// ext4 отличается от ext2 экстентами, журналом и 64-битными полями —
// для наших задач это не даёт ничего, кроме объёма кода. NTFS вообще
// закрытый формат, его свободные реализации (ntfs-3g) — это десятки
// тысяч строк под FUSE.

/// Максимальная глубина вложенности — защита от циклов и переполнения
/// стека при разборе пути.
const MAX_PATH_DEPTH: usize = 16;

/// Создаёт подкаталог `name` внутри каталога с инодом `parent_ino`.
///
/// Делает ровно то же, что format() делает для `lost+found`:
/// выделяет инод и блок, кладёт в блок записи "." и "..", отмечает
/// тип каталога в режиме инода и увеличивает счётчик ссылок родителя.
pub fn mkdir_in(parent_ino: u32, name: &str) -> Result<u32, Ext2Error> {
    if !is_formatted() {
        return Err(Ext2Error::NotFormatted);
    }
    validate_name(name)?;

    // Уже существует — возвращаем его инод (идемпотентность).
    if let Some((_, _, ino)) = find_in_dir(parent_ino, name)? {
        let existing = read_inode(ino)?;
        if (existing.mode & 0xF000) == 0x4000 {
            return Ok(ino);
        }
        return Err(Ext2Error::InvalidName); // имя занято обычным файлом
    }

    let new_ino = alloc_inode()?;
    let block = alloc_block()?;

    // Тело каталога: "." на себя, ".." на родителя.
    let mut buf = [0u8; BLOCK_SIZE];
    make_dirent(&mut buf, 0, new_ino, ".", 12);
    make_dirent(&mut buf, 12, parent_ino, "..", (BLOCK_SIZE - 12) as u16);
    write_block(block, &buf)?;

    let mut ptrs = [0u32; 15];
    ptrs[0] = block;
    // 0o040755 — бит 0x4000 помечает инод как каталог.
    write_inode(new_ino, 0o040755, BLOCK_SIZE as u32, 2, &ptrs, 1)?;

    // Ссылка из родителя + его nlink растёт из-за ".." в потомке.
    add_dirent(parent_ino, name, new_ino)?;
    let parent = read_inode(parent_ino)?;
    let pptrs: Vec<u32> = parent.block_ptrs.to_vec();
    let pused = pptrs.iter().filter(|p| **p != 0).count() as u32;
    write_inode(
        parent_ino,
        parent.mode,
        parent.size,
        parent.links_count.saturating_add(1),
        &pptrs,
        pused,
    )?;

    // В группе стало на один каталог больше.
    if let Ok(mut gdt) = read_block(GDT_BLOCK) {
        let dirs = u16::from_le_bytes([gdt[16], gdt[17]]).saturating_add(1);
        gdt[16..18].copy_from_slice(&dirs.to_le_bytes());
        let _ = write_block(GDT_BLOCK, &gdt);
    }

    Ok(new_ino)
}

/// Разбирает путь и возвращает инод каталога и имя последнего элемента.
///
/// `"/users/ivan/notes.txt"` -> `(инод каталога /users/ivan, "notes.txt")`.
/// Если `create` = true, отсутствующие промежуточные каталоги создаются
/// (поведение `mkdir -p`).
pub fn resolve_parent(path: &str, create: bool) -> Result<(u32, alloc::string::String), Ext2Error> {
    let trimmed = path.trim_start_matches('/');
    let parts: Vec<&str> = trimmed.split('/').filter(|p| !p.is_empty()).collect();

    if parts.is_empty() {
        return Err(Ext2Error::InvalidName);
    }
    if parts.len() > MAX_PATH_DEPTH {
        return Err(Ext2Error::InvalidName);
    }

    let mut dir = ROOT_INO;
    for component in &parts[..parts.len() - 1] {
        match find_in_dir(dir, component)? {
            Some((_, _, ino)) => {
                let node = read_inode(ino)?;
                if (node.mode & 0xF000) != 0x4000 {
                    // На пути оказался обычный файл — дальше идти некуда.
                    return Err(Ext2Error::InvalidName);
                }
                dir = ino;
            }
            None => {
                if !create {
                    return Err(Ext2Error::FileNotFound);
                }
                dir = mkdir_in(dir, component)?;
            }
        }
    }
    Ok((dir, alloc::string::String::from(parts[parts.len() - 1])))
}

/// `mkdir -p`: создаёт всю цепочку каталогов пути.
pub fn mkdir_p(path: &str) -> Result<u32, Ext2Error> {
    let (parent, last) = resolve_parent(path, true)?;
    mkdir_in(parent, &last)
}

/// `mkdir`: алиас для `mkdir_p`.
pub fn mkdir(path: &str) -> Result<u32, Ext2Error> {
    mkdir_p(path)
}

/// Список содержимого каталога по пути (`"/"` — корень).
pub fn list_dir_path(path: &str) -> Result<Vec<FileEntry>, Ext2Error> {
    if !is_formatted() {
        return Err(Ext2Error::NotFormatted);
    }
    let dir_ino = if path.trim_matches('/').is_empty() {
        ROOT_INO
    } else {
        let (parent, last) = resolve_parent(path, false)?;
        match find_in_dir(parent, &last)? {
            Some((_, _, ino)) => ino,
            None => return Err(Ext2Error::FileNotFound),
        }
    };

    let dir_inode = read_inode(dir_ino)?;
    let mut entries = Vec::new();
    for block_num in dir_block_ptrs(&dir_inode) {
        let buf = read_block(block_num)?;
        let mut offset = 0usize;
        while let Some(entry) = dirent_at(&buf, offset) {
            if entry.ino != 0 {
                let name = dirent_name(&buf, &entry);
                if name != "." && name != ".." {
                    let node = read_inode(entry.ino)?;
                    entries.push(FileEntry {
                        name,
                        size: node.size,
                        is_directory: (node.mode & 0xF000) == 0x4000,
                    });
                }
            }
            if entry.rec_len == 0 {
                break;
            }
            offset += entry.rec_len as usize;
            if offset >= BLOCK_SIZE {
                break;
            }
        }
    }
    Ok(entries)
}

// ---------------- файловые операции по ПУТИ ----------------
//
// Старые read_file/write_file/delete_file работают только с корнем.
// Эти версии принимают путь вида "/users/ivan/files/notes.txt" и
// выполняют операцию в нужном подкаталоге.

/// Записывает файл по пути, создавая недостающие каталоги (mkdir -p).
pub fn write_file_path(path: &str, data: &[u8]) -> Result<(), Ext2Error> {
    let (dir_ino, name) = resolve_parent(path, true)?;
    if dir_ino == ROOT_INO {
        return write_file(&name, data);
    }
    write_file_in(dir_ino, &name, data)
}

/// Читает файл по пути.
pub fn read_file_path(path: &str) -> Result<Vec<u8>, Ext2Error> {
    let (dir_ino, name) = resolve_parent(path, false)?;
    if dir_ino == ROOT_INO {
        return read_file(&name);
    }
    let found = find_in_dir(dir_ino, &name)?;
    let (_, _, ino) = found.ok_or(Ext2Error::FileNotFound)?;
    read_inode_data(ino)
}

/// Удаляет файл по пути.
pub fn delete_file_path(path: &str) -> Result<(), Ext2Error> {
    let (dir_ino, name) = resolve_parent(path, false)?;
    if dir_ino == ROOT_INO {
        return delete_file(&name);
    }
    let found = find_in_dir(dir_ino, &name)?;
    let (_, _, ino) = found.ok_or(Ext2Error::FileNotFound)?;

    let inode = read_inode(ino)?;
    for b in dir_block_ptrs(&inode) {
        let _ = free_block(b);
    }
    free_inode(ino)?;
    remove_dirent(dir_ino, &name)?;
    Ok(())
}
