//! Чтение раздела /system (EROFS) прямо с диска, без образа в куче.
//!
//! Читатель EROFS просит байты по смещению; здесь они приходят из
//! двух путей:
//!
//! * малые запросы (суперблок, иноды, каталоги — до `DIRECT_MIN`)
//!   идут через блочный кеш: блок 4 КиБ читается с диска один раз и
//!   держится в LRU-кеше не более `CACHE_BLOCKS` блоков (2 МиБ);
//! * крупные запросы (содержимое файлов) читаются с диска одним
//!   проходом мимо кеша: файл всё равно возвращается вызывающему целиком,
//!   а кеш не должен вытесняться чтением одного ядра.
//!
//! Раздел только для чтения, поэтому записи и сброса «грязных» блоков
//! нет. Единственное, что сбрасывает кеш, — `invalidate()` при
//! перемонтировании /system (`sysfs convert`).
//!
//! Целостность: этот слой данные не проверяет. Проверка на загрузке —
//! sha256 `kernel.bin` в `bootchain`; остальные файлы читаются без
//! хеширования.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::ata;
use crate::erofs::{ErofsError, Source};
use crate::spinlock::SpinLock;

const SECTOR: usize = 512;
const BLOCK: usize = 4096;
const SECTORS_PER_BLOCK: u32 = (BLOCK / SECTOR) as u32;
/// Потолок кеша: 512 блоков по 4 КиБ = 2 МиБ.
const CACHE_BLOCKS: usize = 512;
/// Запросы от этого размера читаются мимо кеша.
const DIRECT_MIN: usize = 64 * 1024;
/// Секторов за одну ATA-команду при прямом чтении.
const BATCH_SECTORS: u32 = 128;

struct Slot {
    block: u32,
    stamp: u64,
    data: Box<[u8]>,
}

struct Cache {
    slots: Vec<Slot>,
    clock: u64,
    hits: u64,
    misses: u64,
}

static CACHE: SpinLock<Cache> = SpinLock::new(Cache { slots: Vec::new(), clock: 0, hits: 0, misses: 0 });

/// Раздел EROFS на диске: начальный LBA и размер в секторах.
#[derive(Clone, Copy)]
pub struct DiskImage {
    start_lba: u32,
    sectors: u32,
}

impl DiskImage {
    pub fn new(start_lba: u32, sectors: u32) -> Self {
        DiskImage { start_lba, sectors }
    }

    fn total_bytes(&self) -> usize {
        self.sectors as usize * SECTOR
    }

    /// Отдаёт содержимое блока `blk` функции `f`; при промахе читает блок
    /// с диска (вне блокировки кеша) и вытесняет самый давний.
    fn with_block<R>(&self, blk: u32, f: impl FnOnce(&[u8]) -> R) -> Result<R, ErofsError> {
        {
            let mut guard = CACHE.lock();
            let cache = &mut *guard;
            cache.clock += 1;
            let now = cache.clock;
            if let Some(slot) = cache.slots.iter_mut().find(|s| s.block == blk) {
                slot.stamp = now;
                cache.hits += 1;
                return Ok(f(&slot.data[..]));
            }
            cache.misses += 1;
        }

        let first = blk * SECTORS_PER_BLOCK;
        let count = self.sectors.saturating_sub(first).min(SECTORS_PER_BLOCK);
        if count == 0 {
            return Err(ErofsError::Io);
        }
        let mut data = vec![0u8; BLOCK].into_boxed_slice();
        ata::read_sectors(self.start_lba + first, count as u8, &mut data[..count as usize * SECTOR])
            .map_err(|_| ErofsError::Io)?;
        let result = f(&data[..]);

        let mut guard = CACHE.lock();
        let cache = &mut *guard;
        cache.clock += 1;
        let now = cache.clock;
        if !cache.slots.iter().any(|s| s.block == blk) {
            let slot = Slot { block: blk, stamp: now, data };
            if cache.slots.len() < CACHE_BLOCKS {
                cache.slots.push(slot);
            } else if let Some(victim) = cache.slots.iter_mut().min_by_key(|s| s.stamp) {
                *victim = slot;
            }
        }
        Ok(result)
    }

    /// Читает диапазон `[off, end)` одним проходом по секторам, мимо кеша.
    fn read_direct(&self, off: usize, end: usize) -> Result<Vec<u8>, ErofsError> {
        let first = (off / SECTOR) as u32;
        let last = end.div_ceil(SECTOR) as u32;
        let mut buf = vec![0u8; (last - first) as usize * SECTOR];
        let mut done = 0u32;
        while first + done < last {
            let batch = (last - first - done).min(BATCH_SECTORS);
            let from = done as usize * SECTOR;
            let to = from + batch as usize * SECTOR;
            ata::read_sectors(self.start_lba + first + done, batch as u8, &mut buf[from..to])
                .map_err(|_| ErofsError::Io)?;
            done += batch;
        }
        let skip = off - first as usize * SECTOR;
        let len = end - off;
        buf.copy_within(skip..skip + len, 0);
        buf.truncate(len);
        Ok(buf)
    }
}

impl Source for DiskImage {
    fn read_at(&self, off: usize, len: usize) -> Result<Vec<u8>, ErofsError> {
        let total = self.total_bytes();
        if off >= total || len == 0 {
            return Ok(Vec::new());
        }
        let end = off.saturating_add(len).min(total);
        if end - off >= DIRECT_MIN {
            return self.read_direct(off, end);
        }

        let mut out = Vec::with_capacity(end - off);
        let mut pos = off;
        while pos < end {
            let in_block = pos % BLOCK;
            let take = (BLOCK - in_block).min(end - pos);
            self.with_block((pos / BLOCK) as u32, |data| {
                out.extend_from_slice(&data[in_block..in_block + take]);
            })?;
            pos += take;
        }
        Ok(out)
    }
}

/// Сбрасывает кеш блоков (перемонтирование /system).
pub fn invalidate() {
    let mut guard = CACHE.lock();
    guard.slots = Vec::new();
}

/// Состояние кеша для `describe_mounts`; не блокируется, если кеш занят.
pub fn describe() -> String {
    match CACHE.try_lock() {
        Some(cache) => format!(
            "{} КиБ в кеше блоков, {} попаданий, {} чтений с диска",
            cache.slots.len() * BLOCK / 1024,
            cache.hits,
            cache.misses
        ),
        None => String::from("кеш блоков занят"),
    }
}
