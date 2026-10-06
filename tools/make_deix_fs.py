#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""DeiX OS — образ диска на ЗАВОДСКИХ настройках с чистой MBR-разметкой.

Карта разделов DeiX в MBR-таблице (2-partition scheme):

  P1 0x83 bootable  LBA 4096  .. 12799 (8704 сект)  /system    (EROFS RO)
  P2 0x83           LBA 12800 .. 18431 (5632 сект)  /userdata  (EXT2 RW)

/system содержит структуру каталогов:
  - kernel/kernel.bin
  - etc/init.deix
  - services/dinit.cfg
  - media/audio/ui/*.dps

В /system кладутся только настоящие файлы. Заглушки вида
libdeix_*.so и kmod/*.kmod не создаются: module.rs ожидает у KMOD
валидный заголовок и исполняемое тело, и placeholder-байты там
воспринимаются как повреждённый модуль.

/userdata содержит файловую систему EXT2 для пользовательских данных и приложений.
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

# --- КАРТА РАЗДЕЛОВ DeiX ---
# Два первичных раздела в MBR, логических разделов нет.
PRIMARY = [
    (1, 0x83, 4096,  8704, "/system"),    # bootable, системный EROFS
    (2, 0x83, 12800, 5632, "/userdata"),  # ext2, пользовательские данные
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
    """Заполняет MBR: PRIMARY разделы."""
    for i, (num, typ, start, secs, name) in enumerate(PRIMARY):
        boot = 0x80 if i == 0 else 0x00
        off = 446 + i * 16
        img[off] = boot
        img[off + 1:off + 4] = chs(start)
        img[off + 4] = typ
        img[off + 5:off + 8] = chs(start + secs - 1)
        img[off + 8:off + 12] = struct.pack("<I", start)
        img[off + 12:off + 16] = struct.pack("<I", secs)
    img[510] = 0x55
    img[511] = 0xAA


def build_real_erofs(files, label=""):
    """Собирает НАСТОЯЩИЙ EROFS-образ (спецификация v1, магия 0xE0F5E1E2)."""
    import shutil, subprocess, tempfile, os
    mkfs = shutil.which("mkfs.erofs")
    if mkfs:
        with tempfile.TemporaryDirectory() as td:
            src = os.path.join(td, "root")
            os.makedirs(src)
            for name, data in files.items():
                file_path = os.path.join(src, name.lstrip("/"))
                os.makedirs(os.path.dirname(file_path), exist_ok=True)
                with open(file_path, "wb") as f:
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
    """Чистая Python-реализация формата EROFS v1 с поддержкой подкаталогов."""
    BS = 4096
    SB_OFF, SB_SIZE, ISLOT = 1024, 128, 32
    FT_REG, FT_DIR = 1, 2
    S_IFREG, S_IFDIR = 0o100000, 0o040000

    inode_area = SB_OFF + SB_SIZE          # 1152
    root_nid = inode_area // ISLOT         # 36

    dirs = {""}
    for p in files.keys():
        parts = p.strip("/").split("/")
        for i in range(1, len(parts)):
            dirs.add("/".join(parts[:i]))

    sorted_dirs = sorted(list(dirs))
    dir_nids = {d: root_nid + i for i, d in enumerate(sorted_dirs)}

    file_nids = {}
    next_nid = root_nid + len(sorted_dirs)
    for p in sorted(files.keys()):
        file_nids[p] = next_nid
        next_nid += 1

    dir_contents = {d: [] for d in sorted_dirs}
    for d in sorted_dirs:
        parent = "/".join(d.split("/")[:-1]) if d else ""
        parent_nid = dir_nids[parent]
        dir_contents[d].append((".", dir_nids[d], FT_DIR))
        dir_contents[d].append(("..", parent_nid, FT_DIR))

    for d in sorted_dirs:
        if not d:
            continue
        parent = "/".join(d.split("/")[:-1])
        base_name = d.split("/")[-1]
        dir_contents[parent].append((base_name, dir_nids[d], FT_DIR))

    for p in sorted(files.keys()):
        parts = p.strip("/").split("/")
        parent = "/".join(parts[:-1])
        base_name = parts[-1]
        dir_contents[parent].append((base_name, file_nids[p], FT_REG))

    dir_blocks = {}
    dir_sizes = {}
    for d, entries in dir_contents.items():
        entries.sort(key=lambda e: e[0].encode())
        blk = bytearray(BS)
        nameoff = len(entries) * 12
        for i, (nm, nid, ft) in enumerate(entries):
            e = i * 12
            struct.pack_into("<QHBB", blk, e, nid, nameoff, ft, 0)
            blk[nameoff:nameoff + len(nm)] = nm.encode()
            nameoff += len(nm)
        dir_blocks[d] = blk
        dir_sizes[d] = nameoff

    blocks = bytearray()
    dir_blkaddr = {}
    for d in sorted_dirs:
        dir_blkaddr[d] = 1 + len(blocks) // BS
        blocks += dir_blocks[d]

    file_addr = {}
    for p in sorted(files.keys()):
        file_addr[p] = 1 + len(blocks) // BS
        data = files[p]
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

    total_inos = len(sorted_dirs) + len(files)
    struct.pack_into("<I", img, SB_OFF, 0xE0F5E1E2)      # magic
    img[SB_OFF + 12] = 12                                 # blkszbits
    struct.pack_into("<H", img, SB_OFF + 14, root_nid)    # root_nid
    struct.pack_into("<Q", img, SB_OFF + 16, total_inos)
    struct.pack_into("<I", img, SB_OFF + 36, len(img) // BS)
    struct.pack_into("<I", img, SB_OFF + 40, 0)           # meta_blkaddr

    for d in sorted_dirs:
        nid = dir_nids[d]
        off = SB_OFF + SB_SIZE + (nid - root_nid) * ISLOT
        wr_inode(off, S_IFDIR | 0o755, 2, dir_sizes[d], dir_blkaddr[d], nid - root_nid + 1)

    for p in sorted(files.keys()):
        nid = file_nids[p]
        off = SB_OFF + SB_SIZE + (nid - root_nid) * ISLOT
        wr_inode(off, S_IFREG | 0o644, 1, len(files[p]), file_addr[p], nid - root_nid + 1)

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
    """Инициализирует ФС во всех разделах: /system (EROFS RO) + /userdata (EXT2 RW)."""
    import os as _os
    if _os.path.exists(kernel_bin):
        kernel_bytes = open(kernel_bin, 'rb').read()
    else:
        kernel_bytes = b'DEIXKERN1\x00kernel.bin\x00' + b'\x00' * 64

    for num, typ, start, secs, name in PRIMARY:
        if name == "/userdata":
            format_ext2_at(img, start, secs)
        elif name == "/system":
            system_files = {
                "kernel/kernel.bin":   kernel_bytes,
                "etc/init.deix":       b"# DeiX OS init script\nmount /system\nmount /userdata\n",
                "services/dinit.cfg":  b"# Dinit services config\n",
            }
            snd_dir = _os.path.join("build", "sounds")
            if _os.path.isdir(snd_dir):
                for _f in sorted(_os.listdir(snd_dir)):
                    if _f.endswith(".dps"):
                        with open(_os.path.join(snd_dir, _f), "rb") as _fh:
                            data = _fh.read()
                            system_files[f"media/audio/ui/{_f}"] = data
                            system_files[f"media/{_f}"] = data
            write_erofs_image(img, start, secs, name, system_files)


def main():
    img_path = sys.argv[1] if len(sys.argv) > 1 else "build/deix_disk.img"
    parent_dir = os.path.dirname(img_path)
    if parent_dir:
        os.makedirs(parent_dir, exist_ok=True)
    need = 20480 * SECTOR
    if not os.path.exists(img_path) or os.path.getsize(img_path) < need:
        with open(img_path, "wb") as f:
            f.write(b"\x00" * need)

    with open(img_path, "rb") as f:
        img = bytearray(f.read())

    fill_mbr(img)
    init_all_partitions(img, os.path.join(parent_dir if parent_dir else '.', 'kernel.bin'))

    img[4095 * SECTOR:4095 * SECTOR + 512] = b"\x00" * 512

    with open(img_path, "wb") as f:
        f.write(img)

    print(f"OK: DeiX OS — заводской образ с MBR-разметкой ({img_path})")
    print("  Все разделы DeiX (каждый со своей ФС):")
    for num, typ, start, secs, name in PRIMARY:
        fs = "erofs" if name == "/system" else "ext2"
        print(f"    P{num}: 0x{typ:02X} {name:<13} LBA {start:>6}..{start+secs-1:>6} ({secs:>4} сект)  {fs}")


if __name__ == "__main__":
    main()
