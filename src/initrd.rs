#![allow(dead_code)]
//! Initrd (Initial RAM Disk) — ранняя файловая система.
//!
//! До того как ATA-драйвер загружен и ext2-диск доступен, DeiX использует
//! initrd: вкомпилированный в ядро tar-подобный архив с необходимыми модулями
//! и конфигурацией, который распаковывается в память при старте.
//!
//! ## Формат
//!
//! Используется упрощённый tar-подобный формат (без сжатия):
//!
//! Каждая запись:
//! ```
//! struct InitrdEntry {
//!     name: [u8; 64],     // имя файла (null-terminated)
//!     size: u64,          // размер в байтах
//!     offset: u64,        // смещение данных относительно начала архива
//! }
//! ```
//!
//! Завершается записью с name[0] == 0.
//! После заголовков идут данные файлов подряд.
//!
//! ## Преимущества перед монолитным include_bytes!
//!
//! - Модули лежат отдельными файлами в архиве, а не вкомпилированы
//! - Можно обновлять модули без пересборки ядра (замена initrd)
//! - Чёткое разделение: ядро = загрузчик, initrd = драйверы/конфиг

use crate::mm;
use alloc::string::{String, ToString};
use alloc::vec::Vec;


// ==================== Формат записи ====================

const ENTRY_NAME_LEN: usize = 64;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct InitrdEntry {
    name: [u8; ENTRY_NAME_LEN],
    size: u64,
    offset: u64,
}

pub struct InitrdFile {
    pub name: String,
    pub data: &'static [u8],
}

/// Парсит initrd-архив и возвращает список файлов.
pub fn parse_initrd(archive: &'static [u8]) -> Vec<InitrdFile> {
    let mut files = Vec::new();
    let mut pos = 0usize;
    let entry_size = core::mem::size_of::<InitrdEntry>();

    loop {
        if pos + entry_size > archive.len() {
            break;
        }

        let entry = unsafe {
            &*(archive.as_ptr().add(pos) as *const InitrdEntry)
        };

        // Пустое имя = конец архива.
        if entry.name[0] == 0 {
            break;
        }

        let name_len = entry.name.iter().position(|&b| b == 0)
            .unwrap_or(ENTRY_NAME_LEN);
        let name = core::str::from_utf8(&entry.name[..name_len])
            .unwrap_or("???")
            .to_string();

        let size = entry.size as usize;
        let offset = entry.offset as usize;

        let file_data = if offset + size <= archive.len() {
            &archive[offset..offset + size]
        } else {
            &[]
        };

        files.push(InitrdFile {
            name,
            data: file_data,
        });

        pos += entry_size;
    }

    files
}

/// Загружает указанный файл из initrd в память через физический аллокатор.
/// Возвращает (физический адрес, размер) или None.
pub fn load_to_memory(name: &str, archive: &'static [u8]) -> Option<(usize, usize)> {
    let files = parse_initrd(archive);
    for file in &files {
        if file.name.eq_ignore_ascii_case(name) && !file.data.is_empty() {
            let bytes = file.data;
            let pages = (bytes.len() + mm::PAGE_SIZE - 1) / mm::PAGE_SIZE;
            let phys = mm::phys::alloc_pages(pages)?;

            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    phys as *mut u8,
                    bytes.len(),
                );
            }

            return Some((phys, bytes.len()));
        }
    }
    None
}

/// Распечатывает содержимое initrd.
pub fn list(archive: &'static [u8]) {
    let files = parse_initrd(archive);
    crate::println!("Initrd contents ({} files):", files.len());
    for f in &files {
        crate::println!("  {:.<32} {} bytes", f.name, f.data.len());
    }
}

// ==================== Встроенный initrd ====================

/// Пустой initrd-заглушка (настоящий initrd прилинковывается на этапе сборки).
static INITRD_DATA: &[u8] = &[];

/// Инициализирует initrd: парсит и грузит модули, скрипты, конфиги.
pub fn init() {
    if INITRD_DATA.is_empty() {
        crate::println!("  [initrd] No initrd embedded (use build_initrd.sh to create one).");
        return;
    }

    let files = parse_initrd(INITRD_DATA);
    crate::println!("  [initrd] Loaded: {} files.", files.len());
}
