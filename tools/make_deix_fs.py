#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""DeiX OS — образ диска на ЗАВОДСКИХ настройках с ПОЛНОЙ MBR-разметкой.

Полная карта разделов DeiX в MBR-таблице (классический MBR = 4 первичных,
поэтому: 3 первичных + расширенный раздел с 6 логическими):

  P1 0x83 bootable  LBA 4096  .. 6143  (2048 сект)  /kernel     (EROFS)
  P2 0x83           LBA 6144  .. 8191  (2048 сект)  /init_boot  (EROFS)
  P3 0x83           LBA 8192  .. 10239 (2048 сект)  /boot       (EROFS)
  E 0x05 (extended) LBA 10240 .. 12799 (2560 сект)  контейнер логических
    L5 0x83  /vendor_boot  10240..11263 (1024)      (EROFS)
    L6 0x83  /super        11264..12287 (1024)      (EROFS, контейнер system/vendor/product)
    L7 0x83  /system       12288..12543 (256)       (EROFS, логический в /super)
    L8 0x83  /recovery     12544..12799 (256)       (EROFS, TWRP/OrangeFox)
    L9 0x83  /userdata     13312..15359 (2048)      (ext4, Ring 3)
    L10 0xDA /TPM          15360..15871 (512)       (скрытый, стереть невозможно)

Итого 10 разделов: /kernel /init_boot /boot /vendor_boot /super /system
/recovery /userdata /TPM (+ расширенный контейнер).

ПРИМЕЧАНИЕ по адресам: первичный /kernel на LBA 4096 (совпадает с
FS_START_LBA ядра = ext2-том в образе build.sh). Логические разделы
размещаются в области расширенного раздела (LBA 10240..), а /userdata и
/TPM — в хвосте диска (LBA 13312..) — там достаточно места в образе 12800
секторов? НЕТ: образ 12800 секторов (до LBA 12799). Поэтому /userdata и
/TPM вынесены ЗА пределы расширенного раздела... но MBR требует, чтобы
логические разделы лежали ВНУТРИ extended. Решение: расширенный раздел
покрывает LBA 10240..12799 (2560 сект), а /userdata и /TPM размещены как
ПЕРВИЧНЫЕ... но первичных уже 3 занято.

ПРАВИЛЬНОЕ РЕШЕНИЕ: образ расширяется до 16384 секторов (8 МиБ), и полная
карта размещается так (все логические ВНУТРИ extended):

  P1 0x83 boot  /kernel       LBA 4096..6143   (2048)   EROFS
  P2 0x83       /init_boot    LBA 6144..8191   (2048)   EROFS
  P3 0x83       /boot         LBA 8192..10239  (2048)   EROFS
  E  0x05       extended      LBA 10240..16383 (6144)   контейнер
    L4  /vendor_boot 10240..11263  (1024) EROFS
    L5  /super       11264..12287  (1024) EROFS
    L6  /system      12288..12543  (256)  EROFS (внутри /super)
    L7  /recovery    12544..12799  (256)  EROFS
    L8  /userdata    12800..14847  (2048) ext4 (Ring 3)
    L9  /TPM         14848..15359  (512)  скрытый (0xDA)
    (остаток extended 15360..16383 — свободен)

Ядро читает ФС по фиксированному LBA 4096 (ext2-том build.sh) — P1
совпадает с ним; остальные разделы — карта для внешних инструментов и
будущей работы с ними (логические тома поверх).

Формат ext2-тома на P1 (пустой — заводская первая настройка) — байт-в-байт
по src/ext2.rs.
"""
import os
import struct
import sys

SECTOR = 512
BLOCK = 1024
FS_START_LBA = 4096          # начало P1 = ext2-том ядра
TOTAL_SECTORS = 8192         # том ядра (P1)
TOTAL_BLOCKS = TOTAL_SECTORS // (BLOCK // SECTOR)  # 4096
SPB = BLOCK // SECTOR        # 2

# --- геометрия ext2 ---
SB_BLOCK = 1
GDT_BLOCK = 2
BLOCK_BITMAP = 3
INODE_BITMAP = 4
INODE_TABLE_START = 5
INODE_SIZE = 128
INODES = ((max(32, TOTAL_BLOCKS * BLOCK // 4096) + 7) // 8) * 8  # 1024
INODE_TABLE_BLOCKS = INODES * INODE_SIZE // BLOCK  # 128
DATA_START = INODE_TABLE_START + INODE_TABLE_BLOCKS  # 133
ROOT_DIR_BLOCK = DATA_START
LOST_FOUND_BLOCK = DATA_START + 1
RESERVED = LOST_FOUND_BLOCK  # 134
VALID_BLOCKS = TOTAL_BLOCKS - 1
FREE_BLOCKS = VALID_BLOCKS - RESERVED
FREE_INODES = INODES - 10 - 1
EXT2_MAGIC = 0xEF53
ROOT_INO = 2
LOST_FOUND_INO = 11

# --- КАРТА РАЗДЕЛОВ DeiX (все разделы, не пересекается с ФС ядра) ---
# P1 (/system) = 4096..12287 — это рабочий ext2-том ядра (FS_START_LBA=4096,
# TOTAL_SECTORS=8192): суперблок, USERS.DB, AUTOSTART.CFG и т.д.
# Остальные разделы — в свободных областях диска (образ 8 МиБ = 16384 сект):
#   * /userdata — полноценный ext2-том (данные Ring 3);
#   * системные /kernel /init_boot /boot /vendor_boot /super /recovery —
#     НАСТОЯЩИЕ EROFS-образы (магия 0xE0F5E1E2, проходят fsck.erofs);
#   * /TPM — скрытый раздел (маркер DEIXTPM, тип 0xDA).
PRIMARY = [
    (1, 0x83, 4096,  8192, "/system"),    # bootable, рабочий том ядра
    (2, 0xDA, 12288, 512,  "/TPM"),       # скрытый
    (3, 0x83, 12800, 512,  "/userdata"),  # ext2, данные Ring 3
    (4, 0x83, 17664, 2816, "/OTA"),       # ext2, скачанные OTA-пакеты (10 МиБ диск)
]
EXT_START = 13312
EXT_SECTORS = 4352  # до LBA 17151 (разделы кратны блоку EROFS 4 КиБ)
LOGICALS = [
    (5, 0x83, 13313, 1279, "/kernel_a"),
    (6, 0x83, 14593, 1279, "/kernel_b"),
    (7, 0x83, 15873, 255, "/init_boot"),
    (8, 0x83, 16129, 255, "/vendor_boot"),
    (9, 0x83, 16385, 255, "/boot_a"),
    (10, 0x83, 16641, 255, "/boot_b"),
    (11, 0x83, 16897, 255, "/super"),
    (12, 0x83, 17153, 255, "/dsm"),
    (13, 0x83, 17409, 255, "/recovery"),
]
ER0FS_MAGIC = 0xE0F5E1E2  # настоящая магия EROFS v1


def block_lba(b):
    return FS_START_LBA + b * SPB


def read_block(img, b):
    lb = block_lba(b) * SECTOR
    return bytearray(img[lb:lb + BLOCK])


def write_block(img, b, data):
    lb = block_lba(b) * SECTOR
    img[lb:lb + BLOCK] = data


def inode_offset(ino):
    idx = ino - 1
    block = INODE_TABLE_START + (idx * INODE_SIZE) // BLOCK
    off = (idx * INODE_SIZE) % BLOCK
    return block, off


def write_inode(img, ino, mode, size, links, ptrs, used_blocks):
    block, off = inode_offset(ino)
    raw = bytearray(INODE_SIZE)
    raw[0:2] = struct.pack("<H", mode)
    raw[4:8] = struct.pack("<I", size)
    raw[26:28] = struct.pack("<H", links)
    raw[28:32] = struct.pack("<I", used_blocks * SPB)
    for i in range(15):
        raw[40 + i * 4:44 + i * 4] = struct.pack("<I", ptrs[i])
    buf = read_block(img, block)
    buf[off:off + INODE_SIZE] = raw
    write_block(img, block, buf)


def format_ext2(img):
    now = 0x60000000
    sb = bytearray(BLOCK)
    sb[0:4] = struct.pack("<I", INODES)
    sb[4:8] = struct.pack("<I", TOTAL_BLOCKS)
    sb[12:16] = struct.pack("<I", FREE_BLOCKS)
    sb[16:20] = struct.pack("<I", FREE_INODES)
    sb[20:24] = struct.pack("<I", SB_BLOCK)
    sb[32:36] = struct.pack("<I", TOTAL_BLOCKS)
    sb[36:40] = struct.pack("<I", TOTAL_BLOCKS)
    sb[40:44] = struct.pack("<I", INODES)
    sb[48:52] = struct.pack("<I", now)
    sb[54:56] = struct.pack("<H", 0xFFFF)
    sb[56:58] = struct.pack("<H", EXT2_MAGIC)
    sb[58:60] = struct.pack("<H", 1)
    sb[64:68] = struct.pack("<I", now)
    write_block(img, SB_BLOCK, sb)

    gdt = bytearray(BLOCK)
    gdt[0:4] = struct.pack("<I", BLOCK_BITMAP)
    gdt[4:8] = struct.pack("<I", INODE_BITMAP)
    gdt[8:12] = struct.pack("<I", INODE_TABLE_START)
    gdt[12:14] = struct.pack("<H", FREE_BLOCKS)
    gdt[14:16] = struct.pack("<H", FREE_INODES)
    gdt[16:18] = struct.pack("<H", 2)
    write_block(img, GDT_BLOCK, gdt)

    bm = bytearray(BLOCK)
    for b in range(RESERVED):
        bm[b // 8] |= 1 << (b % 8)
    for b in range(VALID_BLOCKS, BLOCK * 8):
        bm[b // 8] |= 1 << (b % 8)
    write_block(img, BLOCK_BITMAP, bm)

    ibm = bytearray(BLOCK)
    for i in range(10):
        ibm[i // 8] |= 1 << (i % 8)
    ibm[(LOST_FOUND_INO - 1) // 8] |= 1 << ((LOST_FOUND_INO - 1) % 8)
    for i in range(INODES, BLOCK * 8):
        ibm[i // 8] |= 1 << (i % 8)
    write_block(img, INODE_BITMAP, ibm)

    root = bytearray(BLOCK)
    root[0:4] = struct.pack("<I", ROOT_INO)
    root[4:6] = struct.pack("<H", 12)
    root[6] = 1
    root[8:9] = b"."
    root[12:16] = struct.pack("<I", ROOT_INO)
    root[16:18] = struct.pack("<H", 12)
    root[18] = 2
    root[20:22] = b".."
    root[24:28] = struct.pack("<I", LOST_FOUND_INO)
    root[28:30] = struct.pack("<H", BLOCK - 24)
    root[30] = len("lost+found")
    root[32:32 + len("lost+found")] = b"lost+found"
    write_block(img, ROOT_DIR_BLOCK, root)

    ptrs = [0] * 15
    ptrs[0] = ROOT_DIR_BLOCK
    write_inode(img, ROOT_INO, 0o040755, BLOCK, 3, ptrs, 1)

    lf = bytearray(BLOCK)
    lf[0:4] = struct.pack("<I", LOST_FOUND_INO)
    lf[4:6] = struct.pack("<H", 12)
    lf[6] = 1
    lf[8:9] = b"."
    lf[12:16] = struct.pack("<I", ROOT_INO)
    lf[16:18] = struct.pack("<H", BLOCK - 12)
    lf[18] = 2
    lf[20:22] = b".."
    write_block(img, LOST_FOUND_BLOCK, lf)

    ptrs2 = [0] * 15
    ptrs2[0] = LOST_FOUND_BLOCK
    write_inode(img, LOST_FOUND_INO, 0o040700, BLOCK, 2, ptrs2, 1)


def chs(lba):
    c = lba // (63 * 255)
    h = (lba // 63) % 255
    s = (lba % 63) + 1
    if c > 1023:
        c, h, s = 1023, 254, 63
    return bytes([h, (s & 0x3F) | ((c >> 2) & 0xC0), c & 0xFF])


def fill_mbr(img):
    """Заполняет MBR: PRIMARY (3) + extended; EBR с LOGICALS."""
    for i, (num, typ, start, secs, name) in enumerate(PRIMARY):
        boot = 0x80 if i == 0 else 0x00
        off = 446 + i * 16
        img[off] = boot
        img[off + 1:off + 4] = chs(start)
        img[off + 4] = typ
        img[off + 5:off + 8] = chs(start + secs - 1)
        img[off + 8:off + 12] = struct.pack("<I", start)
        img[off + 12:off + 16] = struct.pack("<I", secs)
    # Extended (4-я запись).
    off = 446 + 3 * 16
    img[off] = 0x00
    img[off + 1:off + 4] = chs(EXT_START)
    img[off + 4] = 0x05
    img[off + 5:off + 8] = chs(EXT_START + EXT_SECTORS - 1)
    img[off + 8:off + 12] = struct.pack("<I", EXT_START)
    img[off + 12:off + 16] = struct.pack("<I", EXT_SECTORS)
    img[510] = 0x55
    img[511] = 0xAA

    # EBR (Extended Boot Record): по одному на каждый логический раздел.
    # Каждый EBR занимает 1 сектор в начале своего логического раздела.
    # Запись 1: сам логический раздел (начинается СРАЗУ ПОСЛЕ EBR =>
    # относительный старт = 1, размер = secs-1).
    # Запись 2: указатель на СЛЕДУЮЩИЙ EBR (относительно начала extended),
    # либо нули для последнего.
    for idx, (num, typ, start, secs, name) in enumerate(LOGICALS):
        ebr_off = start * SECTOR
        # Запись 1: текущий логический раздел.
        e1 = bytearray(16)
        e1[0] = 0x00
        e1[1:4] = chs(start + 1)
        e1[4] = typ
        e1[5:8] = chs(start + secs - 1)
        e1[8:12] = struct.pack("<I", 1)  # relative start (после EBR)
        e1[12:16] = struct.pack("<I", secs - 1)
        img[ebr_off + 446:ebr_off + 462] = e1
        # Запись 2: указатель на следующий EBR.
        if idx + 1 < len(LOGICALS):
            nxt = LOGICALS[idx + 1][2]
            e2 = bytearray(16)
            e2[0] = 0x00
            e2[1:4] = chs(EXT_START + (nxt - EXT_START))
            e2[4] = 0x05
            e2[8:12] = struct.pack("<I", nxt - EXT_START)
            e2[12:16] = struct.pack("<I", LOGICALS[idx + 1][3])
            img[ebr_off + 462:ebr_off + 478] = e2
        img[ebr_off + 510] = 0x55
        img[ebr_off + 511] = 0xAA


def build_real_erofs(files, label=""):
    """Собирает НАСТОЯЩИЙ EROFS-образ (спецификация v1, магия 0xE0F5E1E2).

    Приоритет — системный mkfs.erofs (эталонная утилита erofs-utils).
    Если её нет, собираем образ сами по спецификации: суперблок @1024,
    compact-иноды по 32 байта (nid = off/32), FLAT_PLAIN-данные в блоках
    по 4096. Записи каталога обязаны быть отсортированы лексикографически
    (это проверяет fsck.erofs).
    """
    import shutil, subprocess, tempfile, os
    mkfs = shutil.which("mkfs.erofs")
    if mkfs:
        with tempfile.TemporaryDirectory() as td:
            src = os.path.join(td, "root")
            os.makedirs(src)
            for name, data in files.items():
                with open(os.path.join(src, name), "wb") as f:
                    f.write(data)
            out = os.path.join(td, "out.erofs")
            r = subprocess.run([mkfs, "-b4096", "-T0", "-U",
                                "00000000-0000-0000-0000-000000000000",
                                out, src],
                               capture_output=True)
            if r.returncode == 0 and os.path.exists(out):
                with open(out, "rb") as f:
                    return f.read()
    return _erofs_fallback(files)


def _erofs_fallback(files):
    """Чистая Python-реализация формата EROFS v1 (без внешних утилит)."""
    BS = 4096
    SB_OFF, SB_SIZE, ISLOT = 1024, 128, 32
    FT_REG, FT_DIR = 1, 2
    S_IFREG, S_IFDIR = 0o100000, 0o040000

    inode_area = SB_OFF + SB_SIZE          # 1152
    root_nid = inode_area // ISLOT         # 36
    names = list(files.keys())
    nids = {n: root_nid + 1 + i for i, n in enumerate(names)}
    if inode_area + (1 + len(names)) * ISLOT > BS:
        raise ValueError("слишком много файлов для одноблочной области инодов")

    # Записи каталога — отсортированы лексикографически (требование EROFS).
    dirents = [(".", root_nid, FT_DIR), ("..", root_nid, FT_DIR)]
    dirents += [(n, nids[n], FT_REG) for n in names]
    dirents.sort(key=lambda e: e[0].encode())

    dir_blk = bytearray()
    used = sum(12 for _ in dirents) + sum(len(e[0]) for e in dirents)
    if used > BS:
        raise ValueError("каталог не помещается в один блок")
    blk = bytearray(BS)
    nameoff = len(dirents) * 12
    for i, (nm, nid, ft) in enumerate(dirents):
        e = i * 12
        struct.pack_into("<QHBB", blk, e, nid, nameoff, ft, 0)
        blk[nameoff:nameoff + len(nm)] = nm.encode()
        nameoff += len(nm)
    dir_blk += blk
    dir_size = used

    blocks = bytearray(dir_blk)
    dir_blkaddr = 1
    file_addr = {}
    for n in names:
        file_addr[n] = dir_blkaddr + len(blocks) // BS
        data = files[n]
        blocks += data
        blocks += b"\x00" * ((-len(data)) % BS)

    img = bytearray(BS + len(blocks))

    def wr_inode(off, mode, nlink, size, blkaddr, ino):
        struct.pack_into("<HHHHIIIIHHI", img, off,
                         0,          # i_format: compact + FLAT_PLAIN
                         0,          # i_xattr_icount
                         mode, nlink, size,
                         0,          # i_reserved
                         blkaddr,    # i_u.raw_blkaddr
                         ino, 0, 0, 0)

    struct.pack_into("<I", img, SB_OFF, 0xE0F5E1E2)      # magic
    img[SB_OFF + 12] = 12                                 # blkszbits
    struct.pack_into("<H", img, SB_OFF + 14, root_nid)    # root_nid
    struct.pack_into("<Q", img, SB_OFF + 16, 1 + len(names))
    struct.pack_into("<I", img, SB_OFF + 36, len(img) // BS)
    struct.pack_into("<I", img, SB_OFF + 40, 0)           # meta_blkaddr

    wr_inode(inode_area, S_IFDIR | 0o755, 2, dir_size, dir_blkaddr, 1)
    for i, n in enumerate(names):
        wr_inode(inode_area + (1 + i) * ISLOT, S_IFREG | 0o644, 1,
                 len(files[n]), file_addr[n], i + 2)

    img[BS:BS + len(blocks)] = blocks
    return bytes(img)


def write_erofs_image(img, start_lba, secs, label, files=None):
    """Пишет в раздел НАСТОЯЩИЙ EROFS-образ (магия 0xE0F5E1E2).

    Раньше здесь писалась самодельная «файловая таблица» с выдуманной
    сигнатурой 0xE0F5E0F5 — такие разделы не понимала ни одна настоящая
    утилита EROFS. Теперь формат честный: образ проходит fsck.erofs.
    """
    files = files or {}
    base = start_lba * SECTOR
    capacity = secs * SECTOR
    blob = build_real_erofs(files, label)
    if len(blob) > capacity:
        raise ValueError(
            "EROFS-образ %s (%d Б) больше раздела %s (%d Б)"
            % (label, len(blob), label, capacity))
    img[base:base + len(blob)] = blob


def make_kernel_targz(kernel_bin_path):
    """Собирает НАСТОЯЩИЙ kernel.tar.gz (gzip deflate + tar ustar):
    содержит kernel.bin и важные библиотеки ядра. Возвращает байты."""
    import io, tarfile
    kernel = open(kernel_bin_path, 'rb').read()
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode='w') as tar:
        def add(name, data):
            ti = tarfile.TarInfo(name)
            ti.size = len(data)
            ti.mode = 0o100644
            tar.addfile(ti, io.BytesIO(data))
        add('kernel.bin', kernel)
        # Важные библиотеки ядра (модель: встроенные компоненты ядра).
        add('libdeix_core.so', b'DEIXLIB1\x00core\x00' + b'\x00' * 64)
        add('libdeix_net.so',  b'DEIXLIB1\x00net\x00' + b'\x00' * 64)
        add('libdeix_gfx.so',  b'DEIXLIB1\x00gfx\x00' + b'\x00' * 64)
    raw_tar = buf.getvalue()
    # Сжимаем настоящим gzip (deflate).
    import gzip
    return gzip.compress(raw_tar, compresslevel=9)


def write_tpm_marker(img, start_lba):
    """Маркер скрытого раздела /TPM: 'DEIXTPM' + версия + счётчик."""
    base = start_lba * SECTOR
    # ВАЖНО: b"DEIXTPM" — это 7 байт, а срез [base:base+8] — 8; slice-assignment
    # укорачивал весь образ на 1 байт (размер был 8 МиБ - 1, и ramboot не мог
    # прочитать ровно 20480 секторов). Заводской маркер — ровно 8 байт,
    # как в src/install.rs (b"DEIXTPM\x00"); база USERS.DB появится в /TPM
    # после создания первого аккаунта (tpm.rs пишет "DEIXTPM1").
    img[base:base + 8] = b"DEIXTPM\x00"
    img[base + 8:base + 12] = struct.pack("<I", 1)  # версия
    img[base + 12:base + 16] = struct.pack("<I", 0)  # sealed-slot count


def format_ext2_at(img, start_lba, total_sectors):
    """Форматирует ext2-том начиная с произвольного LBA (для /userdata)."""
    total_blocks = total_sectors // SPB
    inodes = ((max(32, total_blocks * BLOCK // 4096) + 7) // 8) * 8
    itb = (inodes * INODE_SIZE + BLOCK - 1) // BLOCK
    dstart = INODE_TABLE_START + itb
    reserved = dstart + 1
    valid_blocks = total_blocks - 1
    free_blocks = valid_blocks - reserved
    free_inodes = inodes - 10 - 1
    now = 0x60000000

    def blk_lba(b):
        return start_lba + b * SPB

    def rb(b):
        lb = blk_lba(b) * SECTOR
        return bytearray(img[lb:lb + BLOCK])

    def wb(b, data):
        lb = blk_lba(b) * SECTOR
        img[lb:lb + BLOCK] = data

    def wi(ino, mode, size, links, ptrs, used):
        idx = ino - 1
        block = INODE_TABLE_START + (idx * INODE_SIZE) // BLOCK
        off = (idx * INODE_SIZE) % BLOCK
        raw = bytearray(INODE_SIZE)
        raw[0:2] = struct.pack("<H", mode)
        raw[4:8] = struct.pack("<I", size)
        raw[26:28] = struct.pack("<H", links)
        raw[28:32] = struct.pack("<I", used * SPB)
        for i in range(15):
            raw[40 + i * 4:44 + i * 4] = struct.pack("<I", ptrs[i])
        buf = rb(block)
        buf[off:off + INODE_SIZE] = raw
        wb(block, buf)

    # Суперблок.
    sb = bytearray(BLOCK)
    sb[0:4] = struct.pack("<I", inodes)
    sb[4:8] = struct.pack("<I", total_blocks)
    sb[12:16] = struct.pack("<I", free_blocks)
    sb[16:20] = struct.pack("<I", free_inodes)
    sb[20:24] = struct.pack("<I", 1)
    sb[32:36] = struct.pack("<I", total_blocks)
    sb[36:40] = struct.pack("<I", total_blocks)
    sb[40:44] = struct.pack("<I", inodes)
    sb[48:52] = struct.pack("<I", now)
    sb[54:56] = struct.pack("<H", 0xFFFF)
    sb[56:58] = struct.pack("<H", EXT2_MAGIC)
    sb[58:60] = struct.pack("<H", 1)
    sb[64:68] = struct.pack("<I", now)
    wb(SB_BLOCK, sb)

    # GDT.
    gdt = bytearray(BLOCK)
    gdt[0:4] = struct.pack("<I", BLOCK_BITMAP)
    gdt[4:8] = struct.pack("<I", INODE_BITMAP)
    gdt[8:12] = struct.pack("<I", INODE_TABLE_START)
    gdt[12:14] = struct.pack("<H", free_blocks)
    gdt[14:16] = struct.pack("<H", free_inodes)
    gdt[16:18] = struct.pack("<H", 2)
    wb(GDT_BLOCK, gdt)

    # Bitmap блоков.
    bm = bytearray(BLOCK)
    for b in range(reserved):
        bm[b // 8] |= 1 << (b % 8)
    for b in range(valid_blocks, BLOCK * 8):
        bm[b // 8] |= 1 << (b % 8)
    wb(BLOCK_BITMAP, bm)

    # Bitmap инодов.
    ibm = bytearray(BLOCK)
    for i in range(10):
        ibm[i // 8] |= 1 << (i % 8)
    ibm[(11 - 1) // 8] |= 1 << ((11 - 1) % 8)
    for i in range(inodes, BLOCK * 8):
        ibm[i // 8] |= 1 << (i % 8)
    wb(INODE_BITMAP, ibm)

    # Корневой каталог.
    root = bytearray(BLOCK)
    root[0:4] = struct.pack("<I", 2)
    root[4:6] = struct.pack("<H", 12)
    root[6] = 1
    root[8:9] = b"."
    root[12:16] = struct.pack("<I", 2)
    root[16:18] = struct.pack("<H", 12)
    root[18] = 2
    root[20:22] = b".."
    root[24:28] = struct.pack("<I", 11)
    root[28:30] = struct.pack("<H", BLOCK - 24)
    root[30] = 10
    root[32:42] = b"lost+found"
    wb(dstart, root)
    ptrs = [0] * 15
    ptrs[0] = dstart
    wi(2, 0o040755, BLOCK, 3, ptrs, 1)

    lf = bytearray(BLOCK)
    lf[0:4] = struct.pack("<I", 11)
    lf[4:6] = struct.pack("<H", 12)
    lf[6] = 1
    lf[8:9] = b"."
    lf[12:16] = struct.pack("<I", 2)
    lf[16:18] = struct.pack("<H", BLOCK - 12)
    lf[18] = 2
    lf[20:22] = b".."
    wb(dstart + 1, lf)
    ptrs2 = [0] * 15
    ptrs2[0] = dstart + 1
    wi(11, 0o040700, BLOCK, 2, ptrs2, 1)


def init_all_partitions(img, kernel_bin='build/kernel.bin'):
    """Инициализирует ФС во всех разделах: EROFS с РЕАЛЬНЫМИ файлами
    (bootchain: init_boot->bootloader, vendor_boot->vendor, boot->fastbootd/
    recovery, kernel->kernel.tar.gz с kernel.bin и библиотеками), ext2
    (/userdata), маркер (/TPM)."""
    import os as _os
    # Содержимое разделов загрузочной цепочки.
    bootloader_bin = b'DEIXBOOTLDR\x00v2.0\x00\x00' + b'\x00' * 48
    vendor_bin = b'DEIXVENDOR\x00hal\x00' + b'\x00' * 64
    fastbootd_bin = b'DEIXFB01\x00fastbootd\x00' + b'\x00' * 64
    recovery_bin = b'DEIXREC01\x00recovery\x00' + b'\x00' * 64
    system_img = b'DEIXSYS01\x00system\x00' + b'\x00' * 64
    if _os.path.exists(kernel_bin):
        kernel_targz = make_kernel_targz(kernel_bin)
    else:
        kernel_targz = b'DEIXTAR\x00empty\x00' + b'\x00' * 64

    for num, typ, start, secs, name in PRIMARY + LOGICALS:
        if name == "/TPM":
            write_tpm_marker(img, start)
        elif name in ("/userdata", "/OTA"):
            format_ext2_at(img, start, secs)
        elif name == "/init_boot":
            write_erofs_image(img, start, secs, name, {"bootloader.bin": bootloader_bin})
        elif name == "/dsm":
            dsm_bin = b'DEIXDSM01\x00emergency\x00' + b'\x00' * 64
            write_erofs_image(img, start, secs, name, {"dsm.bin": dsm_bin})
        elif name == "/vendor_boot":
            write_erofs_image(img, start, secs, name, {"vendor.bin": vendor_bin})

        elif name in ("/kernel_a", "/kernel_b"):
            write_erofs_image(img, start, secs, name, {"kernel.tar.gz": kernel_targz})
        elif name in ("/boot_a", "/boot_b"):
            write_erofs_image(img, start, secs, name,
                              {"fastbootd.bin": fastbootd_bin, "recovery.bin": recovery_bin})
        elif name == "/super":
            write_erofs_image(img, start, secs, name, {"system.img": system_img})
        elif name == "/recovery":
            write_erofs_image(img, start, secs, name, {"recovery.bin": recovery_bin})
        else:
            write_erofs_image(img, start, secs, name)


def main():
    img_path = sys.argv[1] if len(sys.argv) > 1 else "build/deix_disk.img"
    size = os.path.getsize(img_path)
    # Образ должен быть >= 8 МиБ (16384 секторов), чтобы вместить extended.
    need = 20480 * SECTOR  # образ 10 МиБ (20480 секторов): /OTA расширен до 4352 сект
    if size < need:
        # Дополняем нулями до 10 МиБ.
        with open(img_path, "ab") as f:
            f.write(b"\x00" * (need - size))
        size = need
    with open(img_path, "rb") as f:
        img = bytearray(f.read())

    fill_mbr(img)
    format_ext2(img)            # P1: рабочий ext2-том ядра (пустой — заводской)
    init_all_partitions(img, os.path.join(os.path.dirname(img_path) if os.path.dirname(img_path) else '.', 'kernel.bin'))

    # ЗАВОДСКОЙ СБРОС служебных секторов (иначе остатки от прошлых
    # прошивок/прогонов QEMU ломают первичную настройку):
    #   * LBA 3000 (BCB)  -> DEIXBCB1 + mode=Normal (0) + 0xFF;
    #   * LBA 4095 (маркер шифрования crypto_storage MARKER_LBA) -> нули
    #     (диск НЕ зашифрован; enable_encryption при первой настройке
    #      должен реально выполнить шифрование, а не увидеть чужой маркер).
    img[3000 * SECTOR:3000 * SECTOR + 8] = b"DEIXBCB1"
    img[3000 * SECTOR + 8:3000 * SECTOR + 12] = struct.pack("<I", 0)   # boot_mode = normal
    img[3000 * SECTOR + 12:3000 * SECTOR + 16] = struct.pack("<I", 0)  # current_slot = a (A/B)
    img[3000 * SECTOR + 16:3000 * SECTOR + 20] = struct.pack("<I", 0)  # ota_pending = 0
    img[3000 * SECTOR + 20:3000 * SECTOR + 512] = b"\xff" * 492
    img[4095 * SECTOR:4095 * SECTOR + 512] = b"\x00" * 512

    with open(img_path, "wb") as f:
        f.write(img)

    print(f"OK: DeiX OS — заводской образ с ПОЛНОЙ MBR-разметкой ({img_path})")
    print("  Все разделы DeiX (каждый со своей ФС):")
    for num, typ, start, secs, name in PRIMARY:
        fs = "ext2(рабочий том ядра)" if name == "/system" else ("ext2" if name == "/userdata" else "маркер TPM")
        print(f"    P{num}: 0x{typ:02X} {name:<13} LBA {start:>6}..{start+secs-1:>6} ({secs:>4} сект)  {fs}")
    print(f"    E : 0x05 extended    LBA {EXT_START:>6}..{EXT_START+EXT_SECTORS-1:>6} ({EXT_SECTORS:>4} сект)")
    for num, typ, start, secs, name in LOGICALS:
        print(f"    L{num}: 0x{typ:02X} {name:<13} LBA {start:>6}..{start+secs-1:>6} ({secs:>4} сект)  EROFS")
    print("  /system = рабочий ext2-том ядра (пустой — первая настройка)")
    print("  Шифрование: включится при создании первого аккаунта (ключ = пароль)")


if __name__ == "__main__":
    main()
