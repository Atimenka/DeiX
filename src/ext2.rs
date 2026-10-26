// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// ext2 — драйвер файловой системы ext2 revision 0 (чтение и запись).
//
// Формат на диске байт-в-байт совместим с настоящим Linux-кодом ext2
// (fs/ext2/*): суперблок, таблица дескрипторов групп, битовые карты,
// inode (включая одинарный indirect-блок) и directory entry. Образ,
// отформатированный этим драйвером, монтируется в Linux (`mount -o loop`)
// и проходит e2fsck.
//
// ## Тома
//
// Драйвер обслуживает НЕСКОЛЬКО томов ext2 одновременно:
//   * /userdata (VolId::Userdata) — раздел P2, LBA 24576, чтение/запись,
//     прозрачное шифрование XTS (crypto_storage.rs);
//   * /system   (VolId::System)   — раздел P1, LBA 4096; используется
//     ТОЛЬКО после конверсии EROFS→ext2 (утилита sysfs, dev-режим),
//     без шифрования (загрузчик читает раздел открытым);
//   * произвольный том на другом диске (установщик: format_raw /
//     write_file_raw) — прямой доступ к целевому диску мимо RAM-диска
//     и шифрования.
//
// ## Геометрия
//
// Геометрия тома (размер, блоков на группу, инодов на группу) читается
// из суперблока при монтировании, а не зашита константами — поэтому один
// и тот же драйвер работает и со штатным томом 8 МиБ (одна группа), и с
// томом произвольного размера, созданным установщиком (много групп).
//
// Раскладка группы g (блок 1024 байта, first_data_block = 1):
//   base(g) = 1 + g*blocks_per_group
//   base+0                — суперблок (g=0: основной, g>0: резервная копия)
//   base+1 .. base+gdtb   — таблица дескрипторов групп (копия в каждой группе)
//   base+1+gdtb           — битовая карта блоков (1 блок, поэтому bpg<=8192)
//   base+2+gdtb           — битовая карта инодов
//   base+3+gdtb ..        — таблица инодов (itb блоков)
//   далее                 — данные
//
// Резервные копии суперблока и GDT пишутся при форматировании в КАЖДОЙ
// группе (как положено revision 0) и далее не обновляются на каждую
// операцию — так же поступает настоящий ext2 (копии освежает e2fsck).

use crate::ata::{self, Drive};
use alloc::string::String;
use alloc::vec::Vec;

use crate::spinlock::SpinLock;
use core::sync::atomic::{AtomicU32, Ordering};

pub const BLOCK_SIZE: usize = 1024;
const SECTOR_SIZE: usize = 512;
const SECTORS_PER_BLOCK: u32 = (BLOCK_SIZE / SECTOR_SIZE) as u32;
const INODE_SIZE: usize = 128;
const EXT2_MAGIC: u16 = 0xEF53;

const ROOT_INO: u32 = 2;
const LOST_AND_FOUND_INO: u32 = 11;
const FIRST_NON_RESERVED_INO: u32 = 11; // revision 0: inode 1..10 зарезервированы

/// Сколько ПРЯМЫХ блочных указателей хранится в inode — стандартное
/// поле формата ext2 (offset 40..87, 12 указателей по 4 байта).
const MAX_DIRECT_BLOCKS: usize = 12;
/// Индекс одинарного indirect-указателя в массиве block_ptrs
/// (offset 88..91 в структуре inode — сразу после 12 прямых).
const INDIRECT_BLOCK_INDEX: usize = 12;
/// Индекс двойного indirect-указателя (offset 92..95): блок, содержащий
/// 256 указателей на indirect-блоки — стандартное поле i_block[13].
const DOUBLE_INDIRECT_BLOCK_INDEX: usize = 13;
/// Сколько указателей на блоки данных помещается в один indirect-блок:
/// BLOCK_SIZE байт / 4 байта на указатель = 256 указателей при блоке
/// 1024 байта — это ровно то же вычисление, что делает настоящий ext2.
const PTRS_PER_INDIRECT_BLOCK: usize = BLOCK_SIZE / 4;
/// Максимальный размер файла: 12 прямых + 256 через одинарный indirect
/// + 256*256 через двойной (~64 МиБ) — хватает для kernel.bin и любых
/// файлов /system при конверсии EROFS→ext2.
const MAX_FILE_BLOCKS: usize =
    MAX_DIRECT_BLOCKS + PTRS_PER_INDIRECT_BLOCK + PTRS_PER_INDIRECT_BLOCK * PTRS_PER_INDIRECT_BLOCK;

/// Начало тома ext2 /userdata на диске (раздел P2).
pub const FS_START_LBA: u32 = 36864;
/// Размер тома /userdata по умолчанию (штатный образ: 32768 секторов =
/// 16 МиБ). Реальный размер читается из таблицы разделов MBR; эта
/// константа — запасной вариант, когда MBR не читается.
const TOTAL_SECTORS_DEFAULT: u32 = 32768;

/// Номер блока суперблока (при блоке 1024 суперблок лежит в блоке 1).
const SB_BLOCK: u32 = 1;
/// Первый блок основной таблицы дескрипторов групп.
const GDT_BLOCK: u32 = SB_BLOCK + 1;
/// Блоков на группу: битовая карта блоков занимает ровно один блок
/// (1024 байта * 8 бит), поэтому больше 8192 блоков в группе не бывает.
const BLOCKS_PER_GROUP: u32 = 8192;

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

// ==================== тома ====================

/// Какой из смонтированных томов использовать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolId {
    /// /userdata (P2): чтение/запись, с шифрованием (если включено).
    Userdata,
    /// /system (P1) ПОСЛЕ конверсии в ext2: без шифрования.
    System,
}

/// Путь ввода-вывода тома.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VolIo {
    /// ata::read_sectors / write_sectors — Master с учётом RAM-диска.
    Standard,
    /// Прямой доступ к конкретному диску (установщик пишет на целевой
    /// диск мимо RAM-диска и шифрования).
    Raw(Drive),
}

/// Смонтированный том: геометрия прочитана из суперблока.
#[derive(Debug, Clone, Copy)]
struct Vol {
    io: VolIo,
    /// Пропускать ли секторы через crypto_storage (только /userdata).
    encrypted: bool,
    start_lba: u32,
    total_blocks: u32,
    /// Блоков на группу (s_blocks_per_group).
    bpg: u32,
    /// Инодов на группу (s_inodes_per_group).
    ipg: u32,
    groups: u32,
    /// Блоков в таблице дескрипторов групп.
    gdt_blocks: u32,
}

/// Кеш смонтированных стандартных томов (индекс = VolId).
/// Сбрасывается при форматировании/конверсии.
static VOL_CACHE: SpinLock<[Option<Vol>; 2]> = SpinLock::new([None, None]);

fn cache_idx(id: VolId) -> usize {
    match id {
        VolId::Userdata => 0,
        VolId::System => 1,
    }
}

/// Сбрасывает закешированную геометрию тома (после format/convert).
pub fn invalidate_cache(id: VolId) {
    VOL_CACHE.lock()[cache_idx(id)] = None;
    clear_block_cache();
}

/// Размер раздела /userdata в секторах из таблицы разделов MBR.
/// Установщик может создать /userdata любого размера — реальный размер
/// знает только MBR; суперблок затем обязан с ним согласовываться.
fn userdata_partition_sectors() -> u32 {
    let mut sec = [0u8; SECTOR_SIZE];
    if ata::read_sectors(0, 1, &mut sec).is_ok() && sec[510] == 0x55 && sec[511] == 0xAA {
        // Запись P2 — смещение 462 в MBR.
        let start = u32::from_le_bytes([sec[470], sec[471], sec[472], sec[473]]);
        let count = u32::from_le_bytes([sec[474], sec[475], sec[476], sec[477]]);
        if start == FS_START_LBA && count >= 1024 {
            return count;
        }
    }
    TOTAL_SECTORS_DEFAULT
}

/// Параметры стандартного тома: (io, шифрование, начальный LBA,
/// максимальный размер в секторах).
fn vol_params(id: VolId) -> (VolIo, bool, u32, u32) {
    match id {
        VolId::Userdata => (VolIo::Standard, true, FS_START_LBA, userdata_partition_sectors()),
        VolId::System => {
            let layout = crate::partition_map::lookup_layout("/system");
            let (start, sectors) = match layout {
                Some(l) => (l.start_lba, l.sectors),
                None => (4096, 20480),
            };
            (VolIo::Standard, false, start, sectors)
        }
    }
}

/// Монтирует том: читает суперблок и проверяет, что геометрия
/// соответствует раскладке, которую понимает драйвер.
fn load_vol(io: VolIo, encrypted: bool, start_lba: u32, max_sectors: u32) -> Result<Vol, Ext2Error> {
    let sb = blk_read(io, encrypted, start_lba, SB_BLOCK)?;
    if u16::from_le_bytes([sb[56], sb[57]]) != EXT2_MAGIC {
        return Err(Ext2Error::NotFormatted);
    }

    let inodes = u32::from_le_bytes(sb[0..4].try_into().unwrap());
    let total_blocks = u32::from_le_bytes(sb[4..8].try_into().unwrap());
    let first_data_block = u32::from_le_bytes(sb[20..24].try_into().unwrap());
    let log_block_size = u32::from_le_bytes(sb[24..28].try_into().unwrap());
    let bpg = u32::from_le_bytes(sb[32..36].try_into().unwrap());
    let ipg = u32::from_le_bytes(sb[40..44].try_into().unwrap());

    // Поддерживается только блок 1024 и first_data_block = 1 — ровно то,
    // что пишут format()/установщик/tools/make_deix_fs.py.
    if log_block_size != 0 || first_data_block != 1 {
        return Err(Ext2Error::NotFormatted);
    }
    if total_blocks < 8 || total_blocks.saturating_mul(SECTORS_PER_BLOCK) > max_sectors {
        return Err(Ext2Error::NotFormatted);
    }
    if bpg == 0 || bpg > BLOCKS_PER_GROUP || ipg < 8 || ipg > BLOCK_SIZE as u32 * 8 {
        return Err(Ext2Error::NotFormatted);
    }

    let groups = (total_blocks - 1).div_ceil(bpg);
    if groups == 0 || groups > 1024 || inodes != groups.saturating_mul(ipg) {
        return Err(Ext2Error::NotFormatted);
    }
    let gdt_blocks = (groups * 32).div_ceil(BLOCK_SIZE as u32);

    let vol = Vol { io, encrypted, start_lba, total_blocks, bpg, ipg, groups, gdt_blocks };

    // Дескриптор группы 0 обязан указывать туда, где драйвер ожидает
    // метаданные (раскладка из заголовка файла) — иначе том создан не
    // нашим форматтером и работать с ним по арифметической раскладке
    // нельзя.
    let gdt = read_block(&vol, GDT_BLOCK)?;
    let bbm = u32::from_le_bytes(gdt[0..4].try_into().unwrap());
    let ibm = u32::from_le_bytes(gdt[4..8].try_into().unwrap());
    let itable = u32::from_le_bytes(gdt[8..12].try_into().unwrap());
    let (want_bbm, want_ibm, want_it) = group_meta(&vol, 0);
    if bbm != want_bbm || ibm != want_ibm || itable != want_it {
        return Err(Ext2Error::NotFormatted);
    }

    Ok(vol)
}

/// Возвращает смонтированный стандартный том (из кеша или с диска).
fn vol(id: VolId) -> Result<Vol, Ext2Error> {
    if let Some(v) = VOL_CACHE.lock()[cache_idx(id)] {
        return Ok(v);
    }
    let (io, encrypted, start_lba, max_sectors) = vol_params(id);
    let v = load_vol(io, encrypted, start_lba, max_sectors)?;
    VOL_CACHE.lock()[cache_idx(id)] = Some(v);
    Ok(v)
}

// ==================== геометрия группы ====================

/// Первый блок группы g (в группе 0 это суперблок, блок 1).
fn group_base(v: &Vol, g: u32) -> u32 {
    1 + g * v.bpg
}

/// Блоки метаданных группы g: (битовая карта блоков, битовая карта
/// инодов, первый блок таблицы инодов). Раскладка одинаковая во всех
/// группах: копия суперблока + копия GDT есть в каждой.
fn group_meta(v: &Vol, g: u32) -> (u32, u32, u32) {
    let bbm = group_base(v, g) + 1 + v.gdt_blocks;
    (bbm, bbm + 1, bbm + 2)
}

/// Сколько блоков реально принадлежит группе g (последняя группа может
/// быть неполной).
fn group_block_count(v: &Vol, g: u32) -> u32 {
    let base = group_base(v, g);
    core::cmp::min(v.bpg, v.total_blocks - base)
}

/// Положение дескриптора группы g в ОСНОВНОЙ таблице (блок, смещение).
fn gdt_locate(g: u32) -> (u32, usize) {
    let byte = g as usize * 32;
    (GDT_BLOCK + (byte / BLOCK_SIZE) as u32, byte % BLOCK_SIZE)
}

// ---------------- кеш расшифрованных блоков ----------------

/// Сколько блоков держит кеш (по 1 КиБ). Прямое отображение по номеру блока.
const BCACHE_SLOTS: usize = 512;

/// Расшифрованные блоки зашифрованного тома. XTS-AES в программной
/// реализации стоит десятки тысяч инструкций на сектор, а ext2 читает одни и
/// те же служебные блоки (суперблок, дескрипторы групп, битовые карты,
/// таблицы инодов) десятки раз за операцию. Кеш сквозной: запись идёт на
/// диск сразу, в кеше обновляется копия. Содержит открытый текст, поэтому
/// затирается при блокировке тома и смене ключа.
struct BlockCache {
    tags: Vec<u32>,
    data: Vec<u8>,
}

static BCACHE: SpinLock<Option<BlockCache>> = SpinLock::new(None);
/// Поколение кеша: растёт при каждой записи и сбросе. Чтение, начатое до
/// записи, не имеет права положить в кеш устаревший блок.
static BCACHE_GEN: AtomicU32 = AtomicU32::new(0);

fn bcache_slot(lba: u32) -> usize {
    ((lba / SECTORS_PER_BLOCK) as usize) % BCACHE_SLOTS
}

fn bcache_get(lba: u32) -> Option<[u8; BLOCK_SIZE]> {
    let guard = BCACHE.lock();
    let cache = guard.as_ref()?;
    let slot = bcache_slot(lba);
    if cache.tags[slot] != lba {
        return None;
    }
    let mut out = [0u8; BLOCK_SIZE];
    out.copy_from_slice(&cache.data[slot * BLOCK_SIZE..(slot + 1) * BLOCK_SIZE]);
    Some(out)
}

/// Кладёт блок в кеш. `generation` — поколение, снятое до чтения с диска; если оно
/// изменилось, блок мог устареть и не сохраняется. `None` — запись на диск
/// (безусловная).
fn bcache_put(lba: u32, data: &[u8; BLOCK_SIZE], generation: Option<u32>) {
    let mut guard = BCACHE.lock();
    match generation {
        Some(g) => {
            if BCACHE_GEN.load(Ordering::SeqCst) != g {
                return;
            }
        }
        None => {
            BCACHE_GEN.fetch_add(1, Ordering::SeqCst);
        }
    }
    let cache = guard.get_or_insert_with(|| BlockCache {
        tags: alloc::vec![u32::MAX; BCACHE_SLOTS],
        data: alloc::vec![0u8; BCACHE_SLOTS * BLOCK_SIZE],
    });
    let slot = bcache_slot(lba);
    cache.tags[slot] = lba;
    cache.data[slot * BLOCK_SIZE..(slot + 1) * BLOCK_SIZE].copy_from_slice(data);
}

fn bcache_forget(lba: u32) {
    let mut guard = BCACHE.lock();
    BCACHE_GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(cache) = guard.as_mut() {
        let slot = bcache_slot(lba);
        if cache.tags[slot] == lba {
            cache.tags[slot] = u32::MAX;
        }
    }
}

/// Сбрасывает кеш и затирает открытый текст в памяти (блокировка тома,
/// смена ключа).
pub fn clear_block_cache() {
    let mut guard = BCACHE.lock();
    BCACHE_GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(cache) = guard.as_mut() {
        for b in cache.data.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
    }
    *guard = None;
}

// ---------------- низкоуровневый ввод-вывод блоков ----------------

/// Читает один ext2-блок (2 физических сектора). Для шифрованного тома
/// каждый 512-байтный сектор расшифровывается независимо, с его
/// абсолютным LBA как XTS tweak — так же, как делает dm-crypt. Остальной
/// код драйвера о шифровании не знает: получает открытые байты.
fn blk_read(io: VolIo, encrypted: bool, start_lba: u32, block: u32) -> Result<[u8; BLOCK_SIZE], Ext2Error> {
    let mut buf = [0u8; BLOCK_SIZE];
    let lba = start_lba + block * SECTORS_PER_BLOCK;
    let cacheable = encrypted && io == VolIo::Standard;
    if cacheable {
        if let Some(hit) = bcache_get(lba) {
            return Ok(hit);
        }
    }
    let generation = BCACHE_GEN.load(Ordering::SeqCst);
    let res = match io {
        VolIo::Standard => ata::read_sectors(lba, SECTORS_PER_BLOCK as u8, &mut buf),
        VolIo::Raw(drive) => ata::read_sectors_from(drive, lba, SECTORS_PER_BLOCK as u8, &mut buf),
    };
    res.map_err(|_| Ext2Error::DiskError)?;

    if encrypted {
        for i in 0..SECTORS_PER_BLOCK {
            let sector_lba = lba + i;
            let start = (i as usize) * SECTOR_SIZE;
            let end = start + SECTOR_SIZE;
            crate::crypto_storage::decrypt_sector_after_read(sector_lba, &mut buf[start..end]);
        }
        if cacheable && crate::crypto_storage::is_unlocked() {
            bcache_put(lba, &buf, Some(generation));
        }
    }

    Ok(buf)
}

fn blk_write(io: VolIo, encrypted: bool, start_lba: u32, block: u32, data: &[u8; BLOCK_SIZE]) -> Result<(), Ext2Error> {
    let lba = start_lba + block * SECTORS_PER_BLOCK;

    // Шифруем в отдельном буфере: вызывающая сторона продолжает работать
    // с открытыми байтами, шифрование существует только "на пути к диску".
    let mut out = *data;
    if encrypted {
        for i in 0..SECTORS_PER_BLOCK {
            let sector_lba = lba + i;
            let start = (i as usize) * SECTOR_SIZE;
            let end = start + SECTOR_SIZE;
            crate::crypto_storage::encrypt_sector_for_write(sector_lba, &mut out[start..end]);
        }
    }

    let res = match io {
        VolIo::Standard => ata::write_sectors(lba, SECTORS_PER_BLOCK as u8, &out),
        VolIo::Raw(drive) => ata::write_sectors_to(drive, lba, SECTORS_PER_BLOCK as u8, &out),
    };
    let cacheable = encrypted && io == VolIo::Standard && crate::crypto_storage::is_unlocked();
    if cacheable {
        if res.is_ok() {
            bcache_put(lba, data, None);
        } else {
            bcache_forget(lba);
        }
    }
    res.map_err(|_| Ext2Error::DiskError)
}

fn read_block(v: &Vol, block: u32) -> Result<[u8; BLOCK_SIZE], Ext2Error> {
    blk_read(v.io, v.encrypted, v.start_lba, block)
}

fn write_block(v: &Vol, block: u32, data: &[u8; BLOCK_SIZE]) -> Result<(), Ext2Error> {
    blk_write(v.io, v.encrypted, v.start_lba, block, data)
}

fn write_block_partial(v: &Vol, block: u32, offset: usize, data: &[u8]) -> Result<(), Ext2Error> {
    let mut buf = read_block(v, block)?;
    buf[offset..offset + data.len()].copy_from_slice(data);
    write_block(v, block, &buf)
}

// ---------------- проверка форматирования ----------------

fn probe_formatted(id: VolId) -> bool {
    let (io, encrypted, start_lba, _) = vol_params(id);
    match blk_read(io, encrypted, start_lba, SB_BLOCK) {
        Ok(sb) => u16::from_le_bytes([sb[56], sb[57]]) == EXT2_MAGIC,
        Err(_) => false,
    }
}

pub fn is_formatted() -> bool {
    probe_formatted(VolId::Userdata)
}

pub fn is_formatted_vol(id: VolId) -> bool {
    probe_formatted(id)
}

// ---------------- битовые карты и счётчики ----------------

fn bitmap_set(v: &Vol, block: u32, bit: u32, value: bool) -> Result<(), Ext2Error> {
    let mut buf = read_block(v, block)?;
    let idx = (bit / 8) as usize;
    if value {
        buf[idx] |= 1 << (bit % 8);
    } else {
        buf[idx] &= !(1 << (bit % 8));
    }
    write_block(v, block, &buf)
}

/// Правит 16-битный счётчик в дескрипторе группы g (field — смещение
/// внутри 32-байтного дескриптора: 12 = свободные блоки, 14 = свободные
/// иноды, 16 = каталоги).
fn gdt_adjust_u16(v: &Vol, g: u32, field: usize, delta: i32) -> Result<(), Ext2Error> {
    let (blk, off) = gdt_locate(g);
    let mut buf = read_block(v, blk)?;
    let cur = u16::from_le_bytes(buf[off + field..off + field + 2].try_into().unwrap());
    let newv = (cur as i32 + delta) as u16;
    buf[off + field..off + field + 2].copy_from_slice(&newv.to_le_bytes());
    write_block(v, blk, &buf)
}

/// Правит 32-битный счётчик в суперблоке (field: 12 = свободные блоки,
/// 16 = свободные иноды).
fn sb_adjust_u32(v: &Vol, field: usize, delta: i32) -> Result<(), Ext2Error> {
    let mut sb = read_block(v, SB_BLOCK)?;
    let val = u32::from_le_bytes(sb[field..field + 4].try_into().unwrap());
    let new_val = (val as i64 + delta as i64) as u32;
    sb[field..field + 4].copy_from_slice(&new_val.to_le_bytes());
    write_block(v, SB_BLOCK, &sb)
}

fn adjust_free_blocks(v: &Vol, g: u32, delta: i32) -> Result<(), Ext2Error> {
    sb_adjust_u32(v, 12, delta)?;
    gdt_adjust_u16(v, g, 12, delta)
}

fn adjust_free_inodes(v: &Vol, g: u32, delta: i32) -> Result<(), Ext2Error> {
    sb_adjust_u32(v, 16, delta)?;
    gdt_adjust_u16(v, g, 14, delta)
}

/// Выделяет свободный блок. Группы перебираются по счётчику свободных
/// блоков в дескрипторе; битовая карта выбранной группы читается ОДИН
/// раз и сканируется в памяти.
fn alloc_block(v: &Vol) -> Result<u32, Ext2Error> {
    for g in 0..v.groups {
        let (gblk, goff) = gdt_locate(g);
        let gbuf = read_block(v, gblk)?;
        let free = u16::from_le_bytes(gbuf[goff + 12..goff + 14].try_into().unwrap());
        if free == 0 {
            continue;
        }
        let (bbm, _, _) = group_meta(v, g);
        let mut buf = read_block(v, bbm)?;
        for bit in 0..group_block_count(v, g) {
            let idx = (bit / 8) as usize;
            let mask = 1u8 << (bit % 8);
            if buf[idx] & mask == 0 {
                buf[idx] |= mask;
                write_block(v, bbm, &buf)?;
                adjust_free_blocks(v, g, -1)?;
                return Ok(group_base(v, g) + bit);
            }
        }
    }
    Err(Ext2Error::NoSpace)
}

fn free_block(v: &Vol, block_num: u32) -> Result<(), Ext2Error> {
    if block_num < 1 || block_num >= v.total_blocks {
        return Err(Ext2Error::DiskError);
    }
    let g = (block_num - 1) / v.bpg;
    let bit = block_num - group_base(v, g);
    let (bbm, _, _) = group_meta(v, g);
    bitmap_set(v, bbm, bit, false)?;
    adjust_free_blocks(v, g, 1)
}

fn alloc_inode(v: &Vol) -> Result<u32, Ext2Error> {
    for g in 0..v.groups {
        let (gblk, goff) = gdt_locate(g);
        let gbuf = read_block(v, gblk)?;
        let free = u16::from_le_bytes(gbuf[goff + 14..goff + 16].try_into().unwrap());
        if free == 0 {
            continue;
        }
        let (_, ibm, _) = group_meta(v, g);
        let mut buf = read_block(v, ibm)?;
        for bit in 0..v.ipg {
            let idx = (bit / 8) as usize;
            let mask = 1u8 << (bit % 8);
            if buf[idx] & mask == 0 {
                buf[idx] |= mask;
                write_block(v, ibm, &buf)?;
                adjust_free_inodes(v, g, -1)?;
                return Ok(g * v.ipg + bit + 1);
            }
        }
    }
    Err(Ext2Error::NoSpace)
}

fn free_inode(v: &Vol, ino: u32) -> Result<(), Ext2Error> {
    let g = (ino - 1) / v.ipg;
    let bit = (ino - 1) % v.ipg;
    let (_, ibm, _) = group_meta(v, g);
    bitmap_set(v, ibm, bit, false)?;
    adjust_free_inodes(v, g, 1)
}

// ---------------- чтение/запись inode ----------------

fn inode_location(v: &Vol, ino: u32) -> (u32, usize) {
    let g = (ino - 1) / v.ipg;
    let idx = ((ino - 1) % v.ipg) as usize;
    let (_, _, itable) = group_meta(v, g);
    let containing_block = itable + ((idx * INODE_SIZE) / BLOCK_SIZE) as u32;
    let offset_in_block = (idx * INODE_SIZE) % BLOCK_SIZE;
    (containing_block, offset_in_block)
}

struct Inode {
    mode: u16,
    size: u32,
    links_count: u16,
    block_ptrs: [u32; 15],
}

fn read_inode(v: &Vol, ino: u32) -> Result<Inode, Ext2Error> {
    let (block, offset) = inode_location(v, ino);
    let buf = read_block(v, block)?;
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
/// косвенной адресации" (обнаружено через e2fsck: "i_blocks is 26,
/// should be 130" для файла с indirect-блоком).
fn write_inode(v: &Vol, ino: u32, mode: u16, size: u32, links_count: u16, block_ptrs: &[u32], used_blocks: u32) -> Result<(), Ext2Error> {
    let (block, offset) = inode_location(v, ino);
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

    write_block_partial(v, block, offset, &raw)
}

/// Обнуляет структуру inode на диске и выставляет dtime (offset 20) —
/// обязательный шаг при удалении файла, помимо очистки битов в bitmap:
/// без него e2fsck находит в таблице инодов "живую на вид" структуру,
/// не связанную ни с одной directory entry ("unattached inode").
/// Настоящий Linux (fs/ext2/inode.c) поступает так же.
fn clear_inode(v: &Vol, ino: u32, dtime: u32) -> Result<(), Ext2Error> {
    let (block, offset) = inode_location(v, ino);
    let mut raw = [0u8; INODE_SIZE];
    raw[20..24].copy_from_slice(&dtime.to_le_bytes());
    write_block_partial(v, block, offset, &raw)
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
/// u32-указателей. Возвращает все нули, если indirect-блок ещё не
/// выделен (в inode стоит 0).
fn read_indirect_block(v: &Vol, indirect_block_num: u32) -> Result<[u32; PTRS_PER_INDIRECT_BLOCK], Ext2Error> {
    let mut ptrs = [0u32; PTRS_PER_INDIRECT_BLOCK];
    if indirect_block_num == 0 {
        return Ok(ptrs);
    }
    let raw = read_block(v, indirect_block_num)?;
    for (i, chunk) in raw.chunks_exact(4).enumerate() {
        ptrs[i] = u32::from_le_bytes(chunk.try_into().unwrap());
    }
    Ok(ptrs)
}

fn write_indirect_block(v: &Vol, indirect_block_num: u32, ptrs: &[u32; PTRS_PER_INDIRECT_BLOCK]) -> Result<(), Ext2Error> {
    let mut raw = [0u8; BLOCK_SIZE];
    for (i, &ptr) in ptrs.iter().enumerate() {
        raw[i * 4..i * 4 + 4].copy_from_slice(&ptr.to_le_bytes());
    }
    write_block(v, indirect_block_num, &raw)
}

/// Возвращает ПОЛНЫЙ список блоков ДАННЫХ файла в порядке записи:
/// 12 прямых указателей, затем содержимое одинарного indirect-блока,
/// затем блоки, перечисленные в indirect-блоках двойного. Сами
/// индексные блоки (indirect/dindirect) в список не входят — они
/// метаданные, их освобождает free_file_blocks отдельно.
fn file_block_list(v: &Vol, inode: &Inode) -> Result<Vec<u32>, Ext2Error> {
    let mut blocks: Vec<u32> = inode.block_ptrs[..MAX_DIRECT_BLOCKS]
        .iter()
        .copied()
        .filter(|&p| p != 0)
        .collect();

    let indirect = inode.block_ptrs[INDIRECT_BLOCK_INDEX];
    if indirect != 0 {
        let ptrs = read_indirect_block(v, indirect)?;
        blocks.extend(ptrs.iter().copied().filter(|&p| p != 0));
    }

    let dind = inode.block_ptrs[DOUBLE_INDIRECT_BLOCK_INDEX];
    if dind != 0 {
        let ind_list = read_indirect_block(v, dind)?;
        for &ind in ind_list.iter().filter(|&&p| p != 0) {
            let ptrs = read_indirect_block(v, ind)?;
            blocks.extend(ptrs.iter().copied().filter(|&p| p != 0));
        }
    }

    Ok(blocks)
}

fn find_in_dir(v: &Vol, dir_ino: u32, name: &str) -> Result<Option<(u32, usize, u32)>, Ext2Error> {
    let dir_inode = read_inode(v, dir_ino)?;
    for block_num in dir_block_ptrs(&dir_inode) {
        let buf = read_block(v, block_num)?;
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

fn add_dirent(v: &Vol, dir_ino: u32, name: &str, new_ino: u32) -> Result<(), Ext2Error> {
    if name.is_empty() || name.len() > 55 {
        return Err(Ext2Error::InvalidName);
    }
    let needed_len = (((8 + name.len()) + 3) / 4 * 4) as u16;

    let dir_inode = read_inode(v, dir_ino)?;
    let block_list: Vec<u32> = dir_block_ptrs(&dir_inode).collect();

    for block_num in &block_list {
        let mut buf = read_block(v, *block_num)?;
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
                write_block(v, *block_num, &buf)?;
                return Ok(());
            }
            if entry.ino != 0 && free_in_entry >= needed_len {
                buf[entry.offset + 4..entry.offset + 6].copy_from_slice(&actual_len.to_le_bytes());
                let new_offset = entry.offset + actual_len as usize;
                let new_rec_len = entry.rec_len - actual_len;
                make_dirent(&mut buf, new_offset, new_ino, name, new_rec_len);
                write_block(v, *block_num, &buf)?;
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

    let new_block = alloc_block(v)?;
    let mut buf = [0u8; BLOCK_SIZE];
    make_dirent(&mut buf, 0, new_ino, name, BLOCK_SIZE as u16);
    write_block(v, new_block, &buf)?;

    new_ptrs[slot] = new_block;
    let new_size = dir_inode.size + BLOCK_SIZE as u32;
    // Каталоги используют только прямые блоки (см. dir_block_ptrs) —
    // used_blocks здесь всегда равен числу ненулевых прямых указателей.
    let used_blocks = new_ptrs[..MAX_DIRECT_BLOCKS].iter().filter(|&&p| p != 0).count() as u32;
    write_inode(v, dir_ino, dir_inode.mode, new_size, dir_inode.links_count, &new_ptrs, used_blocks)
}

fn remove_dirent(v: &Vol, dir_ino: u32, name: &str) -> Result<bool, Ext2Error> {
    let dir_inode = read_inode(v, dir_ino)?;
    for block_num in dir_block_ptrs(&dir_inode) {
        let mut buf = read_block(v, block_num)?;
        let mut offset = 0usize;
        while let Some(entry) = dirent_at(&buf, offset) {
            if entry.ino != 0 && dirent_name(&buf, &entry) == name {
                buf[entry.offset..entry.offset + 4].copy_from_slice(&0u32.to_le_bytes());
                write_block(v, block_num, &buf)?;
                return Ok(true);
            }
            offset += entry.rec_len as usize;
        }
    }
    Ok(false)
}

// ==================== форматирование ====================

/// Текущее "время" ядра в секундах — RTC-драйвера для таймстампов нет,
/// используется аптайм с момента загрузки. e2fsck не проверяет
/// правдоподобность значений времени, только их наличие.
fn fake_timestamp() -> u32 {
    (crate::timer::uptime_ms() / 1000) as u32
}

/// План форматирования: геометрия будущего тома. Считается один раз,
/// затем группы записываются по одной (format_group) — это позволяет
/// установщику показывать честный прогресс по группам.
pub struct FormatPlan {
    io: VolIo,
    encrypted: bool,
    start_lba: u32,
    total_blocks: u32,
    bpg: u32,
    ipg: u32,
    pub groups: u32,
    gdt_blocks: u32,
    itb_blocks: u32,
}

impl FormatPlan {
    /// Служебные блоки одной группы: SB(копия) + GDT + 2 битовые карты +
    /// таблица инодов.
    fn overhead(&self) -> u32 {
        1 + self.gdt_blocks + 2 + self.itb_blocks
    }

    fn as_vol(&self) -> Vol {
        Vol {
            io: self.io,
            encrypted: self.encrypted,
            start_lba: self.start_lba,
            total_blocks: self.total_blocks,
            bpg: self.bpg,
            ipg: self.ipg,
            groups: self.groups,
            gdt_blocks: self.gdt_blocks,
        }
    }

    fn group_count_blocks(&self, g: u32) -> u32 {
        let base = 1 + g * self.bpg;
        core::cmp::min(self.bpg, self.total_blocks - base)
    }

    fn group_free_blocks(&self, g: u32) -> u32 {
        let mut free = self.group_count_blocks(g) - self.overhead();
        if g == 0 {
            free -= 2; // корневой каталог + lost+found
        }
        free
    }

    fn group_free_inodes(&self, g: u32) -> u32 {
        if g == 0 {
            self.ipg - FIRST_NON_RESERVED_INO
        } else {
            self.ipg
        }
    }
}

fn plan_format(io: VolIo, encrypted: bool, start_lba: u32, total_sectors: u32) -> Result<FormatPlan, Ext2Error> {
    let mut total_blocks = total_sectors / SECTORS_PER_BLOCK;
    if total_blocks < 64 {
        return Err(Ext2Error::NoSpace);
    }
    let bpg = BLOCKS_PER_GROUP;
    // Плотность инодов как у mke2fs по умолчанию: ~1 инод на 4096 байт,
    // кратно 8 (битовая карта — целое число байт). Для полной группы
    // 8192 блоков это 2048 инодов на группу.
    let ipg = ((core::cmp::max(32, core::cmp::min(total_blocks, bpg) / 4) + 7) / 8) * 8;
    let itb_blocks = (ipg * INODE_SIZE as u32).div_ceil(BLOCK_SIZE as u32);

    loop {
        let groups = (total_blocks - 1).div_ceil(bpg);
        let gdt_blocks = (groups * 32).div_ceil(BLOCK_SIZE as u32);
        let overhead = 1 + gdt_blocks + 2 + itb_blocks;
        let last_base = 1 + (groups - 1) * bpg;
        let last_count = total_blocks - last_base;
        // Хвостовая группа, в которую не помещаются даже метаданные и
        // немного данных, отрезается (mke2fs делает так же).
        if groups > 1 && last_count < overhead + 16 {
            total_blocks = last_base;
            continue;
        }
        if last_count < overhead + 2 {
            return Err(Ext2Error::NoSpace);
        }
        return Ok(FormatPlan { io, encrypted, start_lba, total_blocks, bpg, ipg, groups, gdt_blocks, itb_blocks });
    }
}

/// Собирает содержимое суперблока. В резервных копиях (g>0) поле
/// s_block_group_nr (offset 90) хранит номер группы копии.
fn build_superblock(plan: &FormatPlan, group_nr: u32, free_blocks: u32, free_inodes: u32, now: u32) -> [u8; BLOCK_SIZE] {
    let inodes = plan.groups * plan.ipg;
    let mut sb = [0u8; BLOCK_SIZE];
    sb[0..4].copy_from_slice(&inodes.to_le_bytes());
    sb[4..8].copy_from_slice(&plan.total_blocks.to_le_bytes());
    sb[8..12].copy_from_slice(&0u32.to_le_bytes()); // reserved blocks for superuser
    sb[12..16].copy_from_slice(&free_blocks.to_le_bytes());
    sb[16..20].copy_from_slice(&free_inodes.to_le_bytes());
    sb[20..24].copy_from_slice(&1u32.to_le_bytes()); // first data block
    sb[24..28].copy_from_slice(&0u32.to_le_bytes()); // log2(block size) - 10 => 1024
    sb[28..32].copy_from_slice(&0u32.to_le_bytes()); // log2(fragment size) - 10
    sb[32..36].copy_from_slice(&plan.bpg.to_le_bytes());
    sb[36..40].copy_from_slice(&plan.bpg.to_le_bytes()); // fragments per group
    sb[40..44].copy_from_slice(&plan.ipg.to_le_bytes());
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
    sb[72..76].copy_from_slice(&0u32.to_le_bytes()); // creator OS: Linux
    sb[76..80].copy_from_slice(&0u32.to_le_bytes()); // major version: 0 (revision 0)
    sb[90..92].copy_from_slice(&(group_nr as u16).to_le_bytes());
    sb
}

/// Собирает полную таблицу дескрипторов групп (одинакова во всех копиях).
fn build_gdt(plan: &FormatPlan) -> Vec<u8> {
    let mut gdt = alloc::vec![0u8; plan.gdt_blocks as usize * BLOCK_SIZE];
    for g in 0..plan.groups {
        let base = 1 + g * plan.bpg;
        let bbm = base + 1 + plan.gdt_blocks;
        let off = g as usize * 32;
        gdt[off..off + 4].copy_from_slice(&bbm.to_le_bytes());
        gdt[off + 4..off + 8].copy_from_slice(&(bbm + 1).to_le_bytes());
        gdt[off + 8..off + 12].copy_from_slice(&(bbm + 2).to_le_bytes());
        gdt[off + 12..off + 14].copy_from_slice(&(plan.group_free_blocks(g) as u16).to_le_bytes());
        gdt[off + 14..off + 16].copy_from_slice(&(plan.group_free_inodes(g) as u16).to_le_bytes());
        let dirs: u16 = if g == 0 { 2 } else { 0 }; // root + lost+found
        gdt[off + 16..off + 18].copy_from_slice(&dirs.to_le_bytes());
    }
    gdt
}

/// Записывает метаданные одной группы: суперблок (основной или копию),
/// GDT, битовые карты и обнулённую таблицу инодов.
pub fn format_group(plan: &FormatPlan, g: u32) -> Result<(), Ext2Error> {
    let _batch = ata::FlushBatch::begin();
    let v = plan.as_vol();
    let now = fake_timestamp();
    let base = group_base(&v, g);
    let count = plan.group_count_blocks(g);
    let total_free_blocks: u32 = (0..plan.groups).map(|gg| plan.group_free_blocks(gg)).sum();
    let total_free_inodes: u32 = (0..plan.groups).map(|gg| plan.group_free_inodes(gg)).sum();

    // Суперблок (в группе 0 — основной).
    let sb = build_superblock(plan, g, total_free_blocks, total_free_inodes, now);
    write_block(&v, base, &sb)?;

    // Таблица дескрипторов групп.
    let gdt = build_gdt(plan);
    for i in 0..plan.gdt_blocks {
        let mut blk = [0u8; BLOCK_SIZE];
        let s = i as usize * BLOCK_SIZE;
        blk.copy_from_slice(&gdt[s..s + BLOCK_SIZE]);
        write_block(&v, base + 1 + i, &blk)?;
    }

    let (bbm, ibm, itable) = group_meta(&v, g);

    // Битовая карта блоков: заняты метаданные (в группе 0 — ещё корневой
    // каталог и lost+found), биты за пределами группы — единицы.
    let mut used = plan.overhead();
    if g == 0 {
        used += 2;
    }
    let mut block_bitmap = [0u8; BLOCK_SIZE];
    for b in 0..used {
        block_bitmap[(b / 8) as usize] |= 1 << (b % 8);
    }
    for b in count..(BLOCK_SIZE as u32 * 8) {
        block_bitmap[(b / 8) as usize] |= 1 << (b % 8);
    }
    write_block(&v, bbm, &block_bitmap)?;

    // Битовая карта инодов: в группе 0 заняты зарезервированные 1..10 и
    // lost+found (11); биты за пределами ipg — единицы.
    let mut inode_bitmap = [0u8; BLOCK_SIZE];
    if g == 0 {
        for i in 0..FIRST_NON_RESERVED_INO {
            inode_bitmap[(i / 8) as usize] |= 1 << (i % 8);
        }
    }
    for i in plan.ipg..(BLOCK_SIZE as u32 * 8) {
        inode_bitmap[(i / 8) as usize] |= 1 << (i % 8);
    }
    write_block(&v, ibm, &inode_bitmap)?;

    // Таблица инодов: нули.
    let zero = [0u8; BLOCK_SIZE];
    for i in 0..plan.itb_blocks {
        write_block(&v, itable + i, &zero)?;
    }

    Ok(())
}

/// Завершает форматирование: корневой каталог и lost+found в группе 0.
pub fn format_finish(plan: &FormatPlan) -> Result<(), Ext2Error> {
    let _batch = ata::FlushBatch::begin();
    let v = plan.as_vol();
    let (_, _, itable) = group_meta(&v, 0);
    let root_block = itable + plan.itb_blocks;
    let lf_block = root_block + 1;

    let mut root = [0u8; BLOCK_SIZE];
    make_dirent(&mut root, 0, ROOT_INO, ".", 12);
    make_dirent(&mut root, 12, ROOT_INO, "..", 12);
    make_dirent(&mut root, 24, LOST_AND_FOUND_INO, "lost+found", (BLOCK_SIZE - 24) as u16);
    write_block(&v, root_block, &root)?;

    let mut root_ptrs = [0u32; 15];
    root_ptrs[0] = root_block;
    write_inode(&v, ROOT_INO, 0o040755, BLOCK_SIZE as u32, 3, &root_ptrs, 1)?;

    let mut lf = [0u8; BLOCK_SIZE];
    make_dirent(&mut lf, 0, LOST_AND_FOUND_INO, ".", 12);
    make_dirent(&mut lf, 12, ROOT_INO, "..", (BLOCK_SIZE - 12) as u16);
    write_block(&v, lf_block, &lf)?;

    let mut lf_ptrs = [0u32; 15];
    lf_ptrs[0] = lf_block;
    write_inode(&v, LOST_AND_FOUND_INO, 0o040700, BLOCK_SIZE as u32, 2, &lf_ptrs, 1)?;

    Ok(())
}

fn format_volume(io: VolIo, encrypted: bool, start_lba: u32, total_sectors: u32) -> Result<(), Ext2Error> {
    let _batch = ata::FlushBatch::begin();
    let plan = plan_format(io, encrypted, start_lba, total_sectors)?;
    for g in 0..plan.groups {
        format_group(&plan, g)?;
    }
    format_finish(&plan)
}

/// Форматирует том /userdata. Размер берётся из таблицы разделов MBR —
/// на диске, подготовленном установщиком, /userdata может быть любого
/// размера; на штатном образе это 16384 секторов.
pub fn format() -> Result<(), Ext2Error> {
    let sectors = userdata_partition_sectors();
    let r = format_volume(VolIo::Standard, true, FS_START_LBA, sectors);
    invalidate_cache(VolId::Userdata);
    r
}

/// Форматирует раздел /system в ext2 (утилита sysfs, dev-режим).
/// Без шифрования: загрузчик и vfs читают раздел открытым.
pub fn format_system() -> Result<(), Ext2Error> {
    let (io, encrypted, start_lba, sectors) = vol_params(VolId::System);
    let r = format_volume(io, encrypted, start_lba, sectors);
    invalidate_cache(VolId::System);
    r
}

/// План форматирования тома на ПРОИЗВОЛЬНОМ диске (установщик) —
/// прямой доступ, без шифрования и RAM-диска.
pub fn plan_format_raw(drive: Drive, start_lba: u32, total_sectors: u32) -> Result<FormatPlan, Ext2Error> {
    plan_format(VolIo::Raw(drive), false, start_lba, total_sectors)
}

/// Записывает файл в корень ext2-тома на произвольном диске (установщик:
/// USERS.DB на целевой диск). Том должен быть уже отформатирован.
pub fn write_file_raw(drive: Drive, start_lba: u32, max_sectors: u32, name: &str, data: &[u8]) -> Result<(), Ext2Error> {
    let v = load_vol(VolIo::Raw(drive), false, start_lba, max_sectors)?;
    validate_name(name)?;
    let _batch = ata::FlushBatch::begin();
    write_file_at(&v, ROOT_INO, name, data)
}

/// Записывает файл по пути на том произвольного диска, создавая
/// недостающие каталоги (mkdir -p) — установщик переносит на новый
/// /userdata обои и настройки, лежащие в подкаталогах.
pub fn write_file_path_raw(drive: Drive, start_lba: u32, max_sectors: u32, path: &str, data: &[u8]) -> Result<(), Ext2Error> {
    let v = load_vol(VolIo::Raw(drive), false, start_lba, max_sectors)?;
    let _batch = ata::FlushBatch::begin();
    write_file_path_at(&v, path, data)
}

// ---------------- имена файлов ----------------

/// ext2 не требует формата 8.3 — имена произвольной длины (до 55
/// символов в нашей реализации directory entry, см. add_dirent).
/// Ограничение — недопустимость '/' и NUL.
fn validate_name(name: &str) -> Result<(), Ext2Error> {
    if name.is_empty() || name.len() > 55 {
        return Err(Ext2Error::InvalidName);
    }
    if name.contains('/') || name.contains('\0') {
        return Err(Ext2Error::InvalidName);
    }
    Ok(())
}

// ==================== операции внутри тома ====================

fn list_dir_ino(v: &Vol, dir_ino: u32, skip_lost_found: bool) -> Result<Vec<FileEntry>, Ext2Error> {
    let dir_inode = read_inode(v, dir_ino)?;
    let mut entries = Vec::new();
    for block_num in dir_block_ptrs(&dir_inode) {
        let buf = read_block(v, block_num)?;
        let mut offset = 0usize;
        while let Some(entry) = dirent_at(&buf, offset) {
            if entry.ino != 0 {
                let name = dirent_name(&buf, &entry);
                if name != "." && name != ".." && !(skip_lost_found && name == "lost+found") {
                    let node = read_inode(v, entry.ino)?;
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

/// Читает содержимое файла по номеру инода.
fn read_inode_data(v: &Vol, ino: u32) -> Result<Vec<u8>, Ext2Error> {
    let inode = read_inode(v, ino)?;
    if inode.size > 8 * 1024 * 1024 {
        return Err(Ext2Error::DiskError);
    }
    let mut data = Vec::with_capacity(inode.size as usize);
    let mut remaining = inode.size as usize;
    // file_block_list возвращает блоки в порядке записи: 12 прямых,
    // затем блоки из indirect-блока.
    for ptr in file_block_list(v, &inode)? {
        if remaining == 0 {
            break;
        }
        let block_data = read_block(v, ptr)?;
        let take = remaining.min(BLOCK_SIZE);
        data.extend_from_slice(&block_data[..take]);
        remaining -= take;
    }
    Ok(data)
}

fn free_file_blocks(v: &Vol, ino: u32) -> Result<(), Ext2Error> {
    let inode = read_inode(v, ino)?;

    // Блоки данных (прямые + перечисленные в indirect-блоке).
    for ptr in file_block_list(v, &inode)? {
        free_block(v, ptr)?;
    }

    // Индексные блоки — тоже занятые блоки на диске (в них хранятся
    // указатели): без их освобождения e2fsck найдёт "утёкшие" блоки.
    let indirect = inode.block_ptrs[INDIRECT_BLOCK_INDEX];
    if indirect != 0 {
        free_block(v, indirect)?;
    }
    let dind = inode.block_ptrs[DOUBLE_INDIRECT_BLOCK_INDEX];
    if dind != 0 {
        let ind_list = read_indirect_block(v, dind)?;
        for &ind in ind_list.iter().filter(|&&p| p != 0) {
            free_block(v, ind)?;
        }
        free_block(v, dind)?;
    }

    free_inode(v, ino)?;
    clear_inode(v, ino, fake_timestamp())
}

/// Запись файла в каталог (по иноду). Общая логика прямых и indirect-
/// блоков для всех томов.
fn write_file_at(v: &Vol, dir_ino: u32, name: &str, data: &[u8]) -> Result<(), Ext2Error> {
    validate_name(name)?;

    if let Some((_, _, existing_ino)) = find_in_dir(v, dir_ino, name)? {
        free_file_blocks(v, existing_ino)?;
        remove_dirent(v, dir_ino, name)?;
    }

    let blocks_needed = data.len().div_ceil(BLOCK_SIZE).max(if data.is_empty() { 0 } else { 1 });
    if blocks_needed > MAX_FILE_BLOCKS {
        return Err(Ext2Error::FileTooLarge);
    }

    let mut block_ptrs = [0u32; 15];

    let direct_blocks = blocks_needed.min(MAX_DIRECT_BLOCKS);
    for i in 0..direct_blocks {
        let b = alloc_block(v)?;
        let start = i * BLOCK_SIZE;
        let end = (start + BLOCK_SIZE).min(data.len());
        let mut block_buf = [0u8; BLOCK_SIZE];
        block_buf[..end - start].copy_from_slice(&data[start..end]);
        write_block(v, b, &block_buf)?;
        block_ptrs[i] = b;
    }

    // used_blocks считает РЕАЛЬНО занятые на диске блоки — для файла с
    // indirect-адресацией это прямые блоки + блоки данных за indirect-
    // указателем + сам indirect-блок.
    let mut used_blocks = direct_blocks as u32;

    if blocks_needed > MAX_DIRECT_BLOCKS {
        let indirect_data_blocks =
            (blocks_needed - MAX_DIRECT_BLOCKS).min(PTRS_PER_INDIRECT_BLOCK);
        let mut indirect_ptrs = [0u32; PTRS_PER_INDIRECT_BLOCK];

        for i in 0..indirect_data_blocks {
            let b = alloc_block(v)?;
            let start = (MAX_DIRECT_BLOCKS + i) * BLOCK_SIZE;
            let end = (start + BLOCK_SIZE).min(data.len());
            let mut block_buf = [0u8; BLOCK_SIZE];
            block_buf[..end - start].copy_from_slice(&data[start..end]);
            write_block(v, b, &block_buf)?;
            indirect_ptrs[i] = b;
        }

        let indirect_block_num = alloc_block(v)?;
        write_indirect_block(v, indirect_block_num, &indirect_ptrs)?;
        block_ptrs[INDIRECT_BLOCK_INDEX] = indirect_block_num;

        used_blocks += indirect_data_blocks as u32 + 1;
    }

    // Файл больше 12 + 256 блоков: остаток уходит через двойной
    // indirect-блок (i_block[13]) — блок из 256 указателей на обычные
    // indirect-блоки, каждый из которых адресует до 256 блоков данных.
    let single_limit = MAX_DIRECT_BLOCKS + PTRS_PER_INDIRECT_BLOCK;
    if blocks_needed > single_limit {
        let mut remaining = blocks_needed - single_limit;
        let mut next_block = single_limit; // индекс следующего блока данных в файле
        let mut dind_ptrs = [0u32; PTRS_PER_INDIRECT_BLOCK];
        let mut chunk = 0usize;

        while remaining > 0 {
            let in_chunk = remaining.min(PTRS_PER_INDIRECT_BLOCK);
            let mut indirect_ptrs = [0u32; PTRS_PER_INDIRECT_BLOCK];
            for i in 0..in_chunk {
                let b = alloc_block(v)?;
                let start = (next_block + i) * BLOCK_SIZE;
                let end = (start + BLOCK_SIZE).min(data.len());
                let mut block_buf = [0u8; BLOCK_SIZE];
                block_buf[..end - start].copy_from_slice(&data[start..end]);
                write_block(v, b, &block_buf)?;
                indirect_ptrs[i] = b;
            }
            let ind_block = alloc_block(v)?;
            write_indirect_block(v, ind_block, &indirect_ptrs)?;
            dind_ptrs[chunk] = ind_block;

            used_blocks += in_chunk as u32 + 1;
            next_block += in_chunk;
            remaining -= in_chunk;
            chunk += 1;
        }

        let dind_block = alloc_block(v)?;
        write_indirect_block(v, dind_block, &dind_ptrs)?;
        block_ptrs[DOUBLE_INDIRECT_BLOCK_INDEX] = dind_block;
        used_blocks += 1;
    }

    let ino = alloc_inode(v)?;
    write_inode(v, ino, 0o100644, data.len() as u32, 1, &block_ptrs, used_blocks)?;
    add_dirent(v, dir_ino, name, ino)?;

    Ok(())
}

// ==================== каталоги и пути ====================
//
// ext2 поддерживает иерархию с самого начала: lost+found (inode 11,
// режим 0o040700) создаёт уже format(). Здесь — создание каталогов в
// рантайме и разбор путей вида "/users/ivan/files".

/// Максимальная глубина вложенности — защита от циклов и переполнения
/// стека при разборе пути.
const MAX_PATH_DEPTH: usize = 16;

/// Создаёт подкаталог `name` внутри каталога с инодом `parent_ino`.
fn mkdir_at(v: &Vol, parent_ino: u32, name: &str) -> Result<u32, Ext2Error> {
    validate_name(name)?;

    // Уже существует — возвращаем его инод (идемпотентность).
    if let Some((_, _, ino)) = find_in_dir(v, parent_ino, name)? {
        let existing = read_inode(v, ino)?;
        if (existing.mode & 0xF000) == 0x4000 {
            return Ok(ino);
        }
        return Err(Ext2Error::InvalidName); // имя занято обычным файлом
    }

    let new_ino = alloc_inode(v)?;
    let block = alloc_block(v)?;

    // Тело каталога: "." на себя, ".." на родителя.
    let mut buf = [0u8; BLOCK_SIZE];
    make_dirent(&mut buf, 0, new_ino, ".", 12);
    make_dirent(&mut buf, 12, parent_ino, "..", (BLOCK_SIZE - 12) as u16);
    write_block(v, block, &buf)?;

    let mut ptrs = [0u32; 15];
    ptrs[0] = block;
    // 0o040755 — бит 0x4000 помечает инод как каталог.
    write_inode(v, new_ino, 0o040755, BLOCK_SIZE as u32, 2, &ptrs, 1)?;

    // Ссылка из родителя + его nlink растёт из-за ".." в потомке.
    add_dirent(v, parent_ino, name, new_ino)?;
    let parent = read_inode(v, parent_ino)?;
    let pptrs: Vec<u32> = parent.block_ptrs.to_vec();
    let pused = pptrs.iter().filter(|p| **p != 0).count() as u32;
    write_inode(
        v,
        parent_ino,
        parent.mode,
        parent.size,
        parent.links_count.saturating_add(1),
        &pptrs,
        pused,
    )?;

    // В группе нового инода стало на один каталог больше.
    let g = (new_ino - 1) / v.ipg;
    let _ = gdt_adjust_u16(v, g, 16, 1);

    Ok(new_ino)
}

/// Разбирает путь и возвращает инод каталога и имя последнего элемента.
/// `"/users/ivan/notes.txt"` -> `(инод каталога /users/ivan, "notes.txt")`.
/// Если `create` = true, отсутствующие промежуточные каталоги создаются
/// (поведение `mkdir -p`).
fn resolve_parent_at(v: &Vol, path: &str, create: bool) -> Result<(u32, String), Ext2Error> {
    let trimmed = path.trim_start_matches('/');
    let parts: Vec<&str> = trimmed.split('/').filter(|p| !p.is_empty()).collect();

    if parts.is_empty() || parts.len() > MAX_PATH_DEPTH {
        return Err(Ext2Error::InvalidName);
    }

    let mut dir = ROOT_INO;
    for component in &parts[..parts.len() - 1] {
        match find_in_dir(v, dir, component)? {
            Some((_, _, ino)) => {
                let node = read_inode(v, ino)?;
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
                dir = mkdir_at(v, dir, component)?;
            }
        }
    }
    Ok((dir, String::from(parts[parts.len() - 1])))
}

fn list_dir_path_at(v: &Vol, path: &str) -> Result<Vec<FileEntry>, Ext2Error> {
    let dir_ino = if path.trim_matches('/').is_empty() {
        ROOT_INO
    } else {
        let (parent, last) = resolve_parent_at(v, path, false)?;
        match find_in_dir(v, parent, &last)? {
            Some((_, _, ino)) => ino,
            None => return Err(Ext2Error::FileNotFound),
        }
    };
    list_dir_ino(v, dir_ino, false)
}

fn read_file_path_at(v: &Vol, path: &str) -> Result<Vec<u8>, Ext2Error> {
    let (dir_ino, name) = resolve_parent_at(v, path, false)?;
    let found = find_in_dir(v, dir_ino, &name)?;
    let (_, _, ino) = found.ok_or(Ext2Error::FileNotFound)?;
    read_inode_data(v, ino)
}

fn write_file_path_at(v: &Vol, path: &str, data: &[u8]) -> Result<(), Ext2Error> {
    let (dir_ino, name) = resolve_parent_at(v, path, true)?;
    write_file_at(v, dir_ino, &name, data)
}

fn delete_file_path_at(v: &Vol, path: &str) -> Result<(), Ext2Error> {
    let (dir_ino, name) = resolve_parent_at(v, path, false)?;
    let found = find_in_dir(v, dir_ino, &name)?;
    let (_, _, ino) = found.ok_or(Ext2Error::FileNotFound)?;

    let node = read_inode(v, ino)?;
    if (node.mode & 0xF000) == 0x4000 {
        // Каталог: освобождаем его блоки и инод.
        for b in dir_block_ptrs(&node) {
            let _ = free_block(v, b);
        }
        free_inode(v, ino)?;
        clear_inode(v, ino, fake_timestamp())?;
        remove_dirent(v, dir_ino, &name)?;
        return Ok(());
    }

    free_file_blocks(v, ino)?;
    remove_dirent(v, dir_ino, &name)?;
    Ok(())
}

// ==================== публичное API: /userdata ====================

pub fn list_root() -> Result<Vec<FileEntry>, Ext2Error> {
    let v = vol(VolId::Userdata)?;
    list_dir_ino(&v, ROOT_INO, true)
}

pub fn read_file(name: &str) -> Result<Vec<u8>, Ext2Error> {
    let v = vol(VolId::Userdata)?;
    validate_name(name)?;
    let found = find_in_dir(&v, ROOT_INO, name)?;
    let (_, _, ino) = found.ok_or(Ext2Error::FileNotFound)?;
    read_inode_data(&v, ino)
}

pub fn write_file(name: &str, data: &[u8]) -> Result<(), Ext2Error> {
    let v = vol(VolId::Userdata)?;
    let _batch = ata::FlushBatch::begin();
    write_file_at(&v, ROOT_INO, name, data)
}

pub fn delete_file(name: &str) -> Result<(), Ext2Error> {
    let v = vol(VolId::Userdata)?;
    let _batch = ata::FlushBatch::begin();
    validate_name(name)?;
    let found = find_in_dir(&v, ROOT_INO, name)?;
    let (_, _, ino) = found.ok_or(Ext2Error::FileNotFound)?;
    free_file_blocks(&v, ino)?;
    remove_dirent(&v, ROOT_INO, name)?;
    Ok(())
}

/// `mkdir -p`: создаёт всю цепочку каталогов пути.
pub fn mkdir_p(path: &str) -> Result<u32, Ext2Error> {
    let v = vol(VolId::Userdata)?;
    let _batch = ata::FlushBatch::begin();
    let (parent, last) = resolve_parent_at(&v, path, true)?;
    mkdir_at(&v, parent, &last)
}

/// Список содержимого каталога по пути (`"/"` — корень).
pub fn list_dir_path(path: &str) -> Result<Vec<FileEntry>, Ext2Error> {
    let v = vol(VolId::Userdata)?;
    list_dir_path_at(&v, path)
}

/// Записывает файл по пути, создавая недостающие каталоги (mkdir -p).
pub fn write_file_path(path: &str, data: &[u8]) -> Result<(), Ext2Error> {
    let v = vol(VolId::Userdata)?;
    let _batch = ata::FlushBatch::begin();
    write_file_path_at(&v, path, data)
}

/// Читает файл по пути.
pub fn read_file_path(path: &str) -> Result<Vec<u8>, Ext2Error> {
    let v = vol(VolId::Userdata)?;
    read_file_path_at(&v, path)
}

/// Удаляет файл по пути.
pub fn delete_file_path(path: &str) -> Result<(), Ext2Error> {
    let v = vol(VolId::Userdata)?;
    let _batch = ata::FlushBatch::begin();
    delete_file_path_at(&v, path)
}

// ==================== публичное API: произвольный том ====================

pub fn list_dir_path_vol(id: VolId, path: &str) -> Result<Vec<FileEntry>, Ext2Error> {
    let v = vol(id)?;
    list_dir_path_at(&v, path)
}

pub fn read_file_path_vol(id: VolId, path: &str) -> Result<Vec<u8>, Ext2Error> {
    let v = vol(id)?;
    read_file_path_at(&v, path)
}

pub fn write_file_path_vol(id: VolId, path: &str, data: &[u8]) -> Result<(), Ext2Error> {
    let v = vol(id)?;
    let _batch = ata::FlushBatch::begin();
    write_file_path_at(&v, path, data)
}

pub fn delete_file_path_vol(id: VolId, path: &str) -> Result<(), Ext2Error> {
    let v = vol(id)?;
    let _batch = ata::FlushBatch::begin();
    delete_file_path_at(&v, path)
}

pub fn mkdir_p_vol(id: VolId, path: &str) -> Result<u32, Ext2Error> {
    let v = vol(id)?;
    let _batch = ata::FlushBatch::begin();
    let (parent, last) = resolve_parent_at(&v, path, true)?;
    mkdir_at(&v, parent, &last)
}

/// Участки диска `(LBA, секторов)`, занятые данными файла, в порядке байтов
/// файла; соседние блоки склеены. По ним загрузчик читает файлы (карта
/// загрузки, src/bootmap.rs), когда /system — ext2.
pub fn file_extents_vol(id: VolId, path: &str) -> Result<Vec<(u32, u32)>, Ext2Error> {
    let v = vol(id)?;
    let (dir_ino, name) = resolve_parent_at(&v, path, false)?;
    let (_, _, ino) = find_in_dir(&v, dir_ino, &name)?.ok_or(Ext2Error::FileNotFound)?;
    let inode = read_inode(&v, ino)?;
    let blocks_used = (inode.size as usize).div_ceil(BLOCK_SIZE);
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for block in file_block_list(&v, &inode)?.into_iter().take(blocks_used) {
        let lba = v.start_lba + block * SECTORS_PER_BLOCK;
        let extends = matches!(runs.last(), Some(&(start, sectors)) if start + sectors == lba);
        if extends {
            if let Some(last) = runs.last_mut() {
                last.1 += SECTORS_PER_BLOCK;
            }
        } else {
            runs.push((lba, SECTORS_PER_BLOCK));
        }
    }
    Ok(runs)
}

/// Сводка тома: (всего блоков, свободных блоков, групп). Для `sysfs
/// status` и экрана установщика.
pub fn volume_stats(id: VolId) -> Result<(u32, u32, u32), Ext2Error> {
    let v = vol(id)?;
    let sb = read_block(&v, SB_BLOCK)?;
    let free = u32::from_le_bytes(sb[12..16].try_into().unwrap());
    Ok((v.total_blocks, free, v.groups))
}
