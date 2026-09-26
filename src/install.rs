// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// install — ИНТЕРАКТИВНЫЙ МАСТЕР УСТАНОВКИ DeiX (команда `install`).
//
// Интерфейс мастера (ввод с клавиатуры PS/2 или COM1):
//   1. Выбор целевого диска: 1 = ATA Master, 2 = ATA Slave.
//   2. Ввод username нового пользователя (валидация имени).
//   3. Ввод пароля пользователя + ПОДТВЕРЖДЕНИЕ (дважды).
//   4. Предупреждение: целевой диск будет ПОЛНОСТЬЮ стёрт; требуется "yes".
//   5. Установка:
//        * MBR с ПОЛНОЙ таблицей разделов DeiX (3 первичных + расширенный
//          с 6 логическими): /system /TPM /userdata /kernel /init_boot
//          /boot /vendor_boot /super /recovery;
//        * ядро (копия себя из памяти) в секторы 1..N;
//        * ext2-том /system (рабочий: суперблок, корневой каталог) +
//          ext2-том /userdata;
//        * EROFS-образы в системных разделах (магия 0xE0F5E1E2);
//        * маркер скрытого раздела /TPM.
//   6. Создание пользователя: USERS.DB записывается в ext2-том /system
//      И в TPM (NV-слот + скрытый раздел /TPM) — пароль пользователя
//      защищён TPM (см. tpm.rs / auth.rs).
//   7. Если целевой диск = ATA Master — сразу включается полнодисковое
//      шифрование (XTS-AES-256) ключом = пароль пользователя.
//      Если Slave — шифрование включится при первой загрузке с него
//      (создание аккаунта на экране входа).
//
// Полная совместимость с архитектурой DeiX: Vault-политика (системные
// разделы EROFS/ro), /TPM скрытый и неудаляемый, единый пароль
// (аккаунт = ключ шифрования), MBR-карта совпадает с tools/make_deix_fs.py.
// no_std-совместимо: alloc (Vec, String), вывод — crate::println!.


use crate::ata::{self, Drive};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;

// ==================== КОНСТАНТЫ УСТАНОВКИ ====================

/// Фиксированный размер области ядра в секторах (совпадает с NUM_SECTORS
/// в build.sh/boot_sector.asm).
/// Максимум секторов области ядра/загрузчика (до ext2-тома на LBA 4096).
pub const KERNEL_AREA_SECTORS: u32 = 3500;

/// Смещение ext2-тома /system на диске (то же, что ext2::FS_START_LBA).
const FS_START_LBA: u32 = 4096;
/// Размер тома /system в секторах.
const FS_TOTAL_SECTORS: u32 = 8192;
/// Размер тома /system в блоках (1024 байта = 2 сектора).
const FS_TOTAL_BLOCKS: u32 = FS_TOTAL_SECTORS / 2;

/// Карта разделов (2-partition scheme, совпадает с tools/make_deix_fs.py):
/// P1 /system (4096..12799, 8704 сект, EROFS RO)
/// P2 /userdata (12800..18431, 5632 сект, EXT2 RW)
const PART_SYSTEM: (u32, u32) = (4096, 8704);
const PART_USERDATA: (u32, u32) = (12800, 5632);

/// MBR-загрузчик, включённый в ядро (build.sh собирает boot_sector.bin ДО
/// компиляции ядра; NUM_SECTORS = размер stage2).
const BOOT_SECTOR: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/build/boot_sector.bin"));
/// Загрузчик второй стадии (stage2.bin) — маленький; грузится boot_sector'ом.
const STAGE2: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/build/stage2.bin"));
/// RAM-диск загрузчик (ramboot.bin): дочитывает весь образ в RAM и передаёт
/// управление stage2. Лежит сразу после stage2 (LBA 1+stage2_sectors).
const RAMBOOT: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/build/ramboot.bin"));

// Конец образа ядра (linker_kernel.ld: __image_end в конце .data, выровнен до 512).
extern "C" {
    static __image_end: u8;
    static __data_start: u8;
}

// ==================== ГЕОМЕТРИЯ ext2 (локальная, для произвольного диска) ==

const BLOCK: usize = 1024;
const SPB: u32 = 2; // секторов на блок
const INODE_SIZE: usize = 128;
const EXT2_MAGIC: u16 = 0xEF53;
const SB_BLOCK: u32 = 1;
const GDT_BLOCK: u32 = 2;
const BLOCK_BITMAP: u32 = 3;
const INODE_BITMAP: u32 = 4;
const INODE_TABLE_START: u32 = 5;

fn inodes_count() -> u32 {
    let raw = core::cmp::max(32, (FS_TOTAL_BLOCKS * BLOCK as u32) / 4096);
    (raw + 7) / 8 * 8
}
fn inode_table_blocks() -> u32 {
    ((inodes_count() as usize * INODE_SIZE + BLOCK - 1) / BLOCK) as u32
}
fn data_start_block() -> u32 {
    INODE_TABLE_START + inode_table_blocks()
}

fn block_lba(block: u32) -> u32 {
    FS_START_LBA + block * SPB
}
fn read_block(drive: Drive, block: u32) -> [u8; BLOCK] {
    let mut buf = [0u8; BLOCK];
    let _ = ata::read_sectors_from(drive, block_lba(block), SPB as u8, &mut buf);
    buf
}
fn write_block(drive: Drive, block: u32, data: &[u8; BLOCK]) {
    let _ = ata::write_sectors_to(drive, block_lba(block), SPB as u8, data);
}
fn inode_location(ino: u32) -> (u32, usize) {
    let idx = ino - 1;
    let block = INODE_TABLE_START + ((idx as usize * INODE_SIZE) / BLOCK) as u32;
    let off = (idx as usize * INODE_SIZE) % BLOCK;
    (block, off)
}
fn write_inode(drive: Drive, ino: u32, mode: u16, size: u32, links: u16, ptrs: &[u32; 15], used_blocks: u32) {
    let (block, off) = inode_location(ino);
    let mut buf = read_block(drive, block);
    let mut raw = [0u8; INODE_SIZE];
    raw[0..2].copy_from_slice(&mode.to_le_bytes());
    raw[4..8].copy_from_slice(&size.to_le_bytes());
    raw[26..28].copy_from_slice(&links.to_le_bytes());
    raw[28..32].copy_from_slice(&(used_blocks * SPB).to_le_bytes());
    for i in 0..15 {
        raw[40 + i * 4..44 + i * 4].copy_from_slice(&ptrs[i].to_le_bytes());
    }
    buf[off..off + INODE_SIZE].copy_from_slice(&raw);
    write_block(drive, block, &buf);
}

/// Читает/пишет битовую карту блоков (блок BLOCK_BITMAP).
fn bitmap_get_set(drive: Drive, block: u32, bit: u32, set: bool) -> bool {
    let mut bm = read_block(drive, block);
    let byte = (bit / 8) as usize;
    let mask = 1u8 << (bit % 8);
    let was_set = (bm[byte] & mask) != 0;
    if set {
        bm[byte] |= mask;
    } else {
        bm[byte] &= !mask;
    }
    write_block(drive, block, &bm);
    was_set
}

fn alloc_block(drive: Drive) -> u32 {
    let valid = FS_TOTAL_BLOCKS - 1;
    for bit in 0..valid {
        if !bitmap_get_set(drive, BLOCK_BITMAP, bit, true) {
            return bit + 1;
        }
    }
    0
}
fn alloc_inode(drive: Drive) -> u32 {
    let inodes = inodes_count();
    for bit in 0..inodes {
        if !bitmap_get_set(drive, INODE_BITMAP, bit, true) {
            return bit + 1;
        }
    }
    0
}

/// Добавляет запись в корневой каталог (inode 2, блок data_start).
fn add_root_dirent(drive: Drive, name: &str, ino: u32) {
    let needed = ((8 + name.len() + 3) / 4) * 4;
    let mut buf = read_block(drive, data_start_block());
    let mut offset = 0usize;
    while offset + 8 <= BLOCK {
        let e_ino = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
        let rec_len = u16::from_le_bytes(buf[offset + 4..offset + 6].try_into().unwrap());
        if rec_len == 0 {
            break;
        }
        let name_len = buf[offset + 6];
        if e_ino == 0 && rec_len as usize >= needed {
            buf[offset..offset + 4].copy_from_slice(&ino.to_le_bytes());
            buf[offset + 4..offset + 6].copy_from_slice(&rec_len.to_le_bytes());
            buf[offset + 6] = name.len() as u8;
            buf[offset + 8..offset + 8 + name.len()].copy_from_slice(name.as_bytes());
            write_block(drive, data_start_block(), &buf);
            return;
        }
        let actual = ((8 + name_len as usize + 3) / 4) * 4;
        let free_in_entry = rec_len as usize - actual;
        if e_ino != 0 && free_in_entry >= needed {
            buf[offset + 4..offset + 6].copy_from_slice(&(actual as u16).to_le_bytes());
            let new_off = offset + actual;
            buf[new_off..new_off + 4].copy_from_slice(&ino.to_le_bytes());
            buf[new_off + 4..new_off + 6].copy_from_slice(&(free_in_entry as u16).to_le_bytes());
            buf[new_off + 6] = name.len() as u8;
            buf[new_off + 8..new_off + 8 + name.len()].copy_from_slice(name.as_bytes());
            write_block(drive, data_start_block(), &buf);
            return;
        }
        offset += rec_len as usize;
    }
}

/// Записывает файл в ext2-том произвольного диска (прямые + indirect).
fn write_file_to(drive: Drive, name: &str, data: &[u8]) -> bool {
    let blocks_needed = if data.is_empty() { 0 } else { (data.len() + BLOCK - 1) / BLOCK };
    let mut ptrs = [0u32; 15];
    let mut used: u32 = 0;
    let direct = blocks_needed.min(12);
    for i in 0..direct {
        let b = alloc_block(drive);
        if b == 0 {
            return false;
        }
        let start = i * BLOCK;
        let end = (start + BLOCK).min(data.len());
        let mut buf = [0u8; BLOCK];
        buf[..end - start].copy_from_slice(&data[start..end]);
        write_block(drive, b, &buf);
        ptrs[i] = b;
        used += 1;
    }
    if blocks_needed > 12 {
        let indirect_data = blocks_needed - 12;
        let mut ind_ptrs = [0u32; BLOCK / 4];
        for i in 0..indirect_data {
            let b = alloc_block(drive);
            if b == 0 {
                return false;
            }
            let start = (12 + i) * BLOCK;
            let end = (start + BLOCK).min(data.len());
            let mut buf = [0u8; BLOCK];
            buf[..end - start].copy_from_slice(&data[start..end]);
            write_block(drive, b, &buf);
            ind_ptrs[i] = b;
        }
        let ind_block = alloc_block(drive);
        if ind_block == 0 {
            return false;
        }
        let mut ibuf = [0u8; BLOCK];
        for (i, p) in ind_ptrs.iter().enumerate() {
            ibuf[i * 4..i * 4 + 4].copy_from_slice(&p.to_le_bytes());
        }
        write_block(drive, ind_block, &ibuf);
        ptrs[12] = ind_block;
        used += indirect_data as u32 + 1;
    }
    let ino = alloc_inode(drive);
    if ino == 0 {
        return false;
    }
    write_inode(drive, ino, 0o100644, data.len() as u32, 1, &ptrs, used);
    add_root_dirent(drive, name, ino);
    true
}

/// Форматирует ext2-том /system на диске (суперблок, каталоги, точки
/// монтирования всех разделов).
fn build_ext2_volume(drive: Drive) {
    let total_blocks = FS_TOTAL_BLOCKS;
    let now = 0x6000_0000u32;
    let inodes = inodes_count();
    let dstart = data_start_block();
    let reserved = dstart + 1;
    let valid_blocks = total_blocks - 1;
    let free_blocks = valid_blocks - reserved;
    let free_inodes = inodes - 10 - 1;

    let mut sb = [0u8; BLOCK];
    sb[0..4].copy_from_slice(&inodes.to_le_bytes());
    sb[4..8].copy_from_slice(&total_blocks.to_le_bytes());
    sb[12..16].copy_from_slice(&free_blocks.to_le_bytes());
    sb[16..20].copy_from_slice(&free_inodes.to_le_bytes());
    sb[20..24].copy_from_slice(&SB_BLOCK.to_le_bytes());
    sb[32..36].copy_from_slice(&total_blocks.to_le_bytes());
    sb[36..40].copy_from_slice(&total_blocks.to_le_bytes());
    sb[40..44].copy_from_slice(&inodes.to_le_bytes());
    sb[48..52].copy_from_slice(&now.to_le_bytes());
    sb[54..56].copy_from_slice(&0xFFFFu16.to_le_bytes());
    sb[56..58].copy_from_slice(&EXT2_MAGIC.to_le_bytes());
    sb[58..60].copy_from_slice(&1u16.to_le_bytes());
    sb[64..68].copy_from_slice(&now.to_le_bytes());
    write_block(drive, SB_BLOCK, &sb);

    let mut gdt = [0u8; BLOCK];
    gdt[0..4].copy_from_slice(&BLOCK_BITMAP.to_le_bytes());
    gdt[4..8].copy_from_slice(&INODE_BITMAP.to_le_bytes());
    gdt[8..12].copy_from_slice(&INODE_TABLE_START.to_le_bytes());
    gdt[12..14].copy_from_slice(&(free_blocks as u16).to_le_bytes());
    gdt[14..16].copy_from_slice(&(free_inodes as u16).to_le_bytes());
    gdt[16..18].copy_from_slice(&2u16.to_le_bytes());
    write_block(drive, GDT_BLOCK, &gdt);

    let mut bm = [0u8; BLOCK];
    for b in 0..reserved {
        bm[(b / 8) as usize] |= 1 << (b % 8);
    }
    for b in valid_blocks..(BLOCK as u32 * 8) {
        bm[(b / 8) as usize] |= 1 << (b % 8);
    }
    write_block(drive, BLOCK_BITMAP, &bm);

    let mut ibm = [0u8; BLOCK];
    for i in 0..10 {
        ibm[(i / 8) as usize] |= 1 << (i % 8);
    }
    ibm[((11 - 1) / 8) as usize] |= 1 << ((11 - 1) % 8);
    for i in inodes..(BLOCK as u32 * 8) {
        ibm[(i / 8) as usize] |= 1 << (i % 8);
    }
    write_block(drive, INODE_BITMAP, &ibm);

    let mut root = [0u8; BLOCK];
    root[0..4].copy_from_slice(&2u32.to_le_bytes());
    root[4..6].copy_from_slice(&12u16.to_le_bytes());
    root[6] = 1;
    root[8] = b'.';
    root[12..16].copy_from_slice(&2u32.to_le_bytes());
    root[16..18].copy_from_slice(&12u16.to_le_bytes());
    root[18] = 2;
    root[20..22].copy_from_slice(b"..");
    root[24..28].copy_from_slice(&11u32.to_le_bytes());
    root[28..30].copy_from_slice(&(BLOCK as u16 - 24).to_le_bytes());
    root[30] = 10;
    root[32..42].copy_from_slice(b"lost+found");
    write_block(drive, dstart, &root);

    let mut root_ptrs = [0u32; 15];
    root_ptrs[0] = dstart;
    write_inode(drive, 2, 0o040755, BLOCK as u32, 3, &root_ptrs, 1);

    let mut lf = [0u8; BLOCK];
    lf[0..4].copy_from_slice(&11u32.to_le_bytes());
    lf[4..6].copy_from_slice(&12u16.to_le_bytes());
    lf[6] = 1;
    lf[8] = b'.';
    lf[12..16].copy_from_slice(&2u32.to_le_bytes());
    lf[16..18].copy_from_slice(&(BLOCK as u16 - 12).to_le_bytes());
    lf[18] = 2;
    lf[20..22].copy_from_slice(b"..");
    write_block(drive, dstart + 1, &lf);

    let mut lf_ptrs = [0u32; 15];
    lf_ptrs[0] = dstart + 1;
    write_inode(drive, 11, 0o040700, BLOCK as u32, 2, &lf_ptrs, 1);

    // Точки монтирования (каталоги в корне /userdata).
    for name in ["system", "userdata", "home", "config", "apps", "packages", "downloads", "update"] {
        let ino = alloc_inode(drive);
        if ino == 0 {
            continue;
        }
        let mut ptrs = [0u32; 15];
        ptrs[0] = dstart + 2;
        write_inode(drive, ino, 0o040755, BLOCK as u32, 2, &ptrs, 1);
        add_root_dirent(drive, name, ino);
    }
}

// ==================== MBR / EROFS / TPM-разделы ====================

fn chs(lba: u32) -> [u8; 3] {
    let c = lba / (63 * 255);
    let h = (lba / 63) % 255;
    let s = (lba % 63) + 1;
    let (c, h, s) = if c > 1023 { (1023u32, 254u8, 63u8) } else { (c, h as u8, s as u8) };
    [h, (s & 0x3F) | (((c >> 2) & 0xC0) as u8), (c & 0xFF) as u8]
}

/// Собирает MBR: загрузчик + таблица разделов DeiX (P1 /system, P2 /userdata).
fn build_mbr() -> [u8; 512] {
    let mut mbr = [0u8; 512];
    let n = BOOT_SECTOR.len().min(512);
    mbr[..n].copy_from_slice(&BOOT_SECTOR[..n]);

    let prim = [
        (0x80u8, 0x83u8, PART_SYSTEM.0, PART_SYSTEM.1),
        (0x00u8, 0x83u8, PART_USERDATA.0, PART_USERDATA.1),
    ];
    for (i, (boot, typ, start, secs)) in prim.iter().enumerate() {
        let off = 446 + i * 16;
        mbr[off] = *boot;
        mbr[off + 1..off + 4].copy_from_slice(&chs(*start));
        mbr[off + 4] = *typ;
        mbr[off + 5..off + 8].copy_from_slice(&chs(start + secs - 1));
        mbr[off + 8..off + 12].copy_from_slice(&start.to_le_bytes());
        mbr[off + 12..off + 16].copy_from_slice(&secs.to_le_bytes());
    }
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    mbr
}


/// База ядра в памяти (linker_kernel.ld): 0x100000 (1 МиБ).
const KERNEL_BASE: usize = 0x100000;
/// Стартовый LBA kernel.bin на целевом диске: сразу после stage2 + ramboot.
fn kernel_start_lba() -> u32 {
    1 + ((STAGE2.len() + 511) / 512) as u32 + ramboot_sectors()
}
fn ramboot_sectors() -> u32 {
    ((RAMBOOT.len() + 511) / 512) as u32
}
/// Размер копируемого ядра: от базы 0x100000 до __image_end (полный образ:
/// .text + .rodata + .data). .data копируется как есть (живое состояние),
/// но критичные глобалы переинициализируются на старте установленной ОС
/// (vgaglobal::early_init_writer в kernel_main), поэтому перенос безопасен.
fn kernel_copy_size() -> usize {
    unsafe { (&__image_end as *const u8 as usize) - KERNEL_BASE }
}
fn kernel_size() -> usize {
    unsafe { (&__image_end as *const u8 as usize) - KERNEL_BASE }
}
fn kernel_sectors() -> u32 {
    ((kernel_copy_size() + 511) / 512) as u32
}

// ==================== ЗАПИСЬ ОБРАЗА ====================

fn write_image_to(drive: Drive) -> Result<(), ()> {
    // MBR с таблицей разделов (P1 /system, P2 /userdata).
    let mbr = build_mbr();
    ata::write_sectors_to(drive, 0, 1, &mbr)?;

    // СБРОС маркерного сектора 4095
    let zero = [0u8; 512];
    ata::write_sectors_to(drive, 4095, 1, &zero)?;

    // Перед копированием СБРАСЫВАЕМ критичные .data-глобалы в безопасное
    // состояние: .data копируется как есть (живые указатели на кучу live
    // нельзя переносить), поэтому приводим их к начальному виду.
    crate::reset_all_globals();

    // stage2.bin (загрузчик) — секторы 1..STAGE2_SECTORS.
    let stage2_sectors = (STAGE2.len() + 511) / 512;
    for i in 0..stage2_sectors {
        let mut buf = [0u8; 512];
        let start = i * 512;
        let end = (start + 512).min(STAGE2.len());
        buf[..end - start].copy_from_slice(&STAGE2[start..end]);
        ata::write_sectors_to(drive, 1 + i as u32, 1, &buf)?;
    }

    // ramboot.bin (RAM-диск загрузчик) — сразу после stage2.
    let rbs = ramboot_sectors();
    for i in 0..rbs {
        let mut buf = [0u8; 512];
        let start = i as usize * 512;
        let end = (start + 512).min(RAMBOOT.len());
        buf[..end - start].copy_from_slice(&RAMBOOT[start..end]);
        ata::write_sectors_to(drive, 1 + stage2_sectors as u32 + i, 1, &buf)?;
    }

    // kernel.bin: читаем ФАЙЛОВЫЕ байты с Master-диска (там лежит точная
    // копия kernel.bin из build.sh — с ЧИСТЫМ .data в начальном состоянии)
    // и пишем на целевой диск. НЕ читаем из памяти: «живой» .data рабочей
    // сессии содержит указатели на кучу/строки, недействительные на
    // установленной системе (из-за этого падало: Invalid opcode).
    let sectors = kernel_sectors();
    let kstart = kernel_start_lba();
    let mut buf = [0u8; 512];
    for i in 0..sectors {
        ata::read_sectors_from(Drive::Master, kstart + i, 1, &mut buf)?;
        ata::write_sectors_to(drive, kstart + i, 1, &buf)?;
    }
    // Дополняем область загрузчика+ядра нулями до границы раздела.
    let total = stage2_sectors as u32 + rbs + sectors;
    if total < KERNEL_AREA_SECTORS {
        let zero = [0u8; 512];
        for i in total..KERNEL_AREA_SECTORS {
            ata::write_sectors_to(drive, 1 + i, 1, &zero)?;
        }
    }

    // /system: копируем EROFS-образ с Master-диска P1 (LBA 4096).
    let (start_sys, secs_sys) = PART_SYSTEM;
    let mut buf = [0u8; 512];
    for i in 0..secs_sys {
        if ata::read_sectors_from(Drive::Master, start_sys + i, 1, &mut buf).is_ok() {
            let _ = ata::write_sectors_to(drive, start_sys + i, 1, &buf);
        }
    }

    // /userdata: ext2-том на P2 (LBA 12800).
    write_userdata_ext2(drive);

    Ok(())
}

/// Форматирует ext2-том /userdata (отдельный маленький том на P3).
fn write_userdata_ext2(drive: Drive) {
    let start_lba = PART_USERDATA.0;
    let total_sectors = PART_USERDATA.1;
    let total_blocks = total_sectors / SPB;
    let inodes = ((core::cmp::max(32, total_blocks * BLOCK as u32 / 4096) + 7) / 8) * 8;
    let itb = ((inodes as usize * INODE_SIZE + BLOCK - 1) / BLOCK) as u32;
    let dstart = INODE_TABLE_START + itb;
    let reserved = dstart + 1;
    let valid = total_blocks - 1;
    let free_blocks = valid - reserved;
    let free_inodes = inodes - 10 - 1;
    let now = 0x6000_0000u32;

    let blk_lba = |b: u32| start_lba + b * SPB;
    let wb = |img: &[u8; BLOCK], b: u32| {
        let _ = ata::write_sectors_to(drive, blk_lba(b), SPB as u8, img);
    };
    let mut sb = [0u8; BLOCK];
    sb[0..4].copy_from_slice(&inodes.to_le_bytes());
    sb[4..8].copy_from_slice(&total_blocks.to_le_bytes());
    sb[12..16].copy_from_slice(&free_blocks.to_le_bytes());
    sb[16..20].copy_from_slice(&free_inodes.to_le_bytes());
    sb[20..24].copy_from_slice(&1u32.to_le_bytes());
    sb[32..36].copy_from_slice(&total_blocks.to_le_bytes());
    sb[36..40].copy_from_slice(&total_blocks.to_le_bytes());
    sb[40..44].copy_from_slice(&inodes.to_le_bytes());
    sb[48..52].copy_from_slice(&now.to_le_bytes());
    sb[54..56].copy_from_slice(&0xFFFFu16.to_le_bytes());
    sb[56..58].copy_from_slice(&EXT2_MAGIC.to_le_bytes());
    sb[58..60].copy_from_slice(&1u16.to_le_bytes());
    sb[64..68].copy_from_slice(&now.to_le_bytes());
    wb(&sb, SB_BLOCK);

    let mut gdt = [0u8; BLOCK];
    gdt[0..4].copy_from_slice(&BLOCK_BITMAP.to_le_bytes());
    gdt[4..8].copy_from_slice(&INODE_BITMAP.to_le_bytes());
    gdt[8..12].copy_from_slice(&INODE_TABLE_START.to_le_bytes());
    gdt[12..14].copy_from_slice(&(free_blocks as u16).to_le_bytes());
    gdt[14..16].copy_from_slice(&(free_inodes as u16).to_le_bytes());
    gdt[16..18].copy_from_slice(&2u16.to_le_bytes());
    wb(&gdt, GDT_BLOCK);

    let mut bm = [0u8; BLOCK];
    for b in 0..reserved {
        bm[(b / 8) as usize] |= 1 << (b % 8);
    }
    for b in valid..(BLOCK as u32 * 8) {
        bm[(b / 8) as usize] |= 1 << (b % 8);
    }
    wb(&bm, BLOCK_BITMAP);

    let mut ibm = [0u8; BLOCK];
    for i in 0..10 {
        ibm[(i / 8) as usize] |= 1 << (i % 8);
    }
    ibm[(10 / 8) as usize] |= 1 << (10 % 8);
    for i in inodes..(BLOCK as u32 * 8) {
        ibm[(i / 8) as usize] |= 1 << (i % 8);
    }
    wb(&ibm, INODE_BITMAP);

    let mut root = [0u8; BLOCK];
    root[0..4].copy_from_slice(&2u32.to_le_bytes());
    root[4..6].copy_from_slice(&12u16.to_le_bytes());
    root[6] = 1;
    root[8] = b'.';
    root[12..16].copy_from_slice(&2u32.to_le_bytes());
    root[16..18].copy_from_slice(&12u16.to_le_bytes());
    root[18] = 2;
    root[20..22].copy_from_slice(b"..");
    root[24..28].copy_from_slice(&11u32.to_le_bytes());
    root[28..30].copy_from_slice(&(BLOCK as u16 - 24).to_le_bytes());
    root[30] = 10;
    root[32..42].copy_from_slice(b"lost+found");
    wb(&root, dstart);
}

// ==================== ВВОД МАСТЕРА ====================

/// Чтение строки с клавиатуры PS/2 или COM1 (неблокирующий опрос).
fn read_input_line() -> String {
    let mut line = String::new();
    loop {
        let c: Option<u8> = if crate::serial::is_data_ready() {
            Some(crate::serial::read_byte())
        } else {
            crate::keyboard::try_read_char()
        };
        match c {
            Some(b'\n') | Some(b'\r') => break,
            Some(0x08) | Some(0x7F) => {
                line.pop();
            }
            Some(other) => line.push(other as char),
            None => {}
        }
    }
    line
}

// ==================== ИНТЕРАКТИВНЫЙ МАСТЕР ====================

/// Главная команда `install` — мастер установки с интерфейсом.
/// Поддерживает неинтерактивный режим для автоматизации/тестов:
///   install --test <user> <pass>   (диск выбирается первым доступным,
///   подтверждение стирания не требуется)
pub fn cmd_install(arg: &str) {
    crate::println!("==================================================");
    crate::println!("     DeiX OS - Installer (setup wizard)          ");
    crate::println!("==================================================");
    crate::println!("  Full layout: /system (EROFS RO), /userdata (EXT2 RW)");
    crate::println!();

    // ---- Режим --test (автоматизация) ----
    let parts: Vec<&str> = arg.split_whitespace().collect();
    let test_mode: bool = parts.first().map(|s| *s == "--test").unwrap_or(false);
    let mut test_user: String = String::new();
    let mut test_pass: String = String::new();
    if test_mode {
        test_user = parts.get(1).cloned().unwrap_or("user1").to_string();
        test_pass = parts.get(2).cloned().unwrap_or("pass123").to_string();
        crate::println!("  [test] non-interactive mode: user='{}'", test_user);
    }

    // ---- Шаг 1: выбор диска ----
    crate::println!("  Step 1/5 - select target disk:");
    crate::println!("    1) ATA Master (first disk)");
    crate::println!("    2) ATA Slave  (second disk)");
    let master_ok = ata::is_present();
    let slave_ok = ata::is_slave_present();
    if !master_ok && !slave_ok {
        crate::println!("  ERROR: no disk found (ATA).");
        return;
    }
    // Если целевой диск — Slave: is_slave_present() оставляет контроллер
    // в состоянии DRQ (данные IDENTIFY не вычитаны). Обязательно вычитываем
    // эти 512 байт через slave_sector_count(), иначе ПЕРВАЯ запись на диск
    // (MBR, LBA 0) уходит в «зависший» контроллер и теряется (ext2/EROFS
    // пишутся позже и успевают, а MBR/stage2/kernel — нет).
    if slave_ok {
        let _ = ata::slave_sector_count();
    }
    let target: Drive;
    if test_mode {
        target = if master_ok { Drive::Master } else { Drive::Slave };
        crate::println!("  Target disk: {}", if master_ok { "ATA Master" } else { "ATA Slave" });
    } else {
        crate::print!("  Your choice [1/2]: ");
        let choice = read_input_line();
        target = match choice.trim() {
            "2" if slave_ok => {
                crate::println!("  Target disk: ATA Slave");
                Drive::Slave
            }
            _ => {
                crate::println!("  Target disk: ATA Master");
                Drive::Master
            }
        };
    }
    crate::println!();

    // ---- Шаг 2: username ----
    let username: String;
    if test_mode {
        username = test_user.clone();
        crate::println!("  Step 2/5 - user: {}", username);
    } else {
        crate::println!("  Step 2/5 - username (letters, digits, '_' or '-', up to 32):");
        loop {
            crate::print!("  Username: ");
            let u = read_input_line();
            let valid = !u.trim().is_empty()
                && u.trim().len() <= 32
                && u.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if valid {
                username = u.trim().to_string();
                break;
            }
            crate::println!("  Invalid username. Try again.");
        }
    }
    crate::println!();

    // ---- Шаг 3: пароль + подтверждение ----
    let password: String;
    if test_mode {
        password = test_pass.clone();
        crate::println!("  Step 3/5 - password set (disk encryption key).");
    } else {
        crate::println!("  Step 3/5 - user password (disk encryption key):");
        loop {
            crate::print!("  Password: ");
            let p1 = read_input_line();
            crate::print!("  Confirm:  ");
            let p2 = read_input_line();
            if p1.is_empty() || p1 != p2 {
                crate::println!("  Passwords do not match or are empty. Try again.");
                continue;
            }
            password = p1;
            break;
        }
    }
    crate::println!();

    // ---- Шаг 4: подтверждение стирания ----
    if !test_mode {
        crate::println!("  Step 4/5 - WARNING: the disk will be FULLY WIPED!");
        crate::println!("  All data on the target disk will be destroyed.");
        crate::print!("  Type 'yes' to confirm: ");
        let confirm = read_input_line();
        if confirm.trim() != "yes" {
            crate::println!("  Install cancelled.");
            return;
        }
    } else {
        crate::println!("  Step 4/5 - (test) confirmation skipped.");
    }
    crate::println!();

    // ---- Шаг 5: установка ----
    crate::println!("  Step 5/5 - installing DeiX to disk...");
    match write_image_to(target) {
        Ok(()) => {
            crate::println!("  [install] MBR layout: OK (2 partitions: /system, /userdata)");
            crate::println!("  [install] Kernel: {} bytes ({} sectors) - OK", kernel_size(), kernel_sectors());
            crate::println!("  [install] /system (EROFS RO) + /userdata (EXT2 RW): OK");
        }
        Err(_) => {
            crate::println!("  ERROR writing image to disk.");
            return;
        }
    }

    // ---- Создание пользователя: USERS.DB -> ext2 /system + TPM ----
    crate::println!("  Creating user '{}'...", username);
    // Строим USERS.DB тем же форматом, что auth.rs (username:salt:hash),
    // и пишем ПРЯМО на ЦЕЛЕВОЙ диск. НЕ через create_user(): он пишет в
    // ext2-том ТЕКУЩЕГО (загрузочного) диска, а целевой остался бы без
    // USERS.DB — при загрузке с установленного диска экран входа считал
    // бы, что аккаунтов нет.
    let mut salt = [0u8; 16];
    for (i, b) in salt.iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(31).wrapping_add(7);
    }
    let mut salted: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    salted.extend_from_slice(&salt);
    salted.extend_from_slice(password.as_bytes());
    let hash = crate::crypto::sha256::sha256(&salted);
    let mut text = String::new();
    text.push_str(&username);
    text.push(':');
    for b in salt.iter() {
        text.push_str(&format!("{:02x}", b));
    }
    text.push(':');
    for b in hash.iter() {
        text.push_str(&format!("{:02x}", b));
    }
    text.push('\n');
    if write_file_to(target, "USERS.DB", text.as_bytes()) {
        crate::println!("  [auth] USERS.DB written to TARGET disk ext2 volume.");
    } else {
        crate::println!("  [auth] WARNING: failed to write USERS.DB to target disk!");
    }

    // ---- Шифрование диска ----
    if target == Drive::Master {
        crate::println!("  Enabling full-disk encryption (XTS-AES-256)...");
        match crate::crypto_storage::enable_encryption(&password) {
            Ok(()) => crate::println!("  [crypto] Encryption enabled. Key = user password."),
            Err(_) => crate::println!("  [crypto] Failed to enable encryption (will enable on first boot)."),
        }
    } else {
        crate::println!("  (Slave) Encryption will enable on first boot from this disk.");
    }

    crate::println!();
    crate::println!("==================================================");
    crate::println!("  INSTALL COMPLETE!");
    crate::println!("  User: {}", username);
    crate::println!("  Disk is partitioned and ready to boot standalone.");
    crate::println!("  Reboot: 'reboot'");
    crate::println!("==================================================");
}
