#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""DeiX OS — разметка образа диска: MBR + /system (EROFS RO) + /userdata (EXT2 RW).

Карта разделов DeiX в MBR-таблице (источник истины — src/partition_map.rs,
сверяется tools/check_partition_map.py):

  P1 0x83 bootable  LBA 4096  .. 24575 (20480 сект)  /system    (EROFS RO)
  P2 0x83           LBA 24576 .. 40959 (16384 сект)  /userdata  (EXT2 RW)

Служебные области до первого раздела:
  LBA 0           — MBR (boot_sector.bin)
  LBA 1..2046     — свободны (нули)
  LBA 2047        — карта загрузки DEIXKIMG v2 (docs/BOOTMAP.md)
  LBA 2048..2111  — область аварийного дампа (DEIXPNIC, src/diag/panic.rs)

Загрузчик и ядро лежат файлами в /system, карта загрузки указывает на них.

/system содержит структуру каталогов:
  - kernel/kernel.bin
  - boot/stage2.bin, boot/ramboot.bin
  - etc/init.deix
  - services/dinit.cfg
  - media/audio/ui/*.dps

В /system кладутся только настоящие файлы. Заглушки (поддельное ядро,
пустые kmod/*.kmod, lib*.so) не создаются: отсутствие kernel.bin —
ошибка сборки, а не повод положить муляж.
"""
import hashlib
import os
import struct
import sys

SECTOR = 512
BLOCK = 1024
SPB = BLOCK // SECTOR        # секторов на блок ext2

# --- геометрия ext2-тома /userdata (та же, что в src/ext2.rs) ---
SB_BLOCK = 1
GDT_BLOCK = 2
INODE_SIZE = 128
EXT2_MAGIC = 0xEF53

# --- КАРТА РАЗДЕЛОВ DeiX (должна совпадать с src/partition_map.rs) ---
# Два первичных раздела в MBR, логических разделов нет.
PRIMARY = [
    (1, 0x83, 4096,  32768, "/system"),    # bootable, системный EROFS
    (2, 0x83, 36864, 32768, "/userdata"),  # ext2, пользовательские данные
]

# Дескриптор образа ядра: сектор LBA 2047 (см. src/partition_map.rs
# KERNEL_IMAGE_INFO_LBA). Формат — docs/BOOTMAP.md.
KERNEL_IMAGE_INFO_LBA = 2047
KERNEL_IMAGE_INFO_MAGIC = b"DEIXKIMG"

# Конец последнего раздела: минимальный размер образа.
PARTITIONS_END_SECTORS = max(start + secs for _, _, start, secs, _ in PRIMARY)

# RAM-диск (boot/ramboot.asm, boot/boot_sector_iso.asm) копирует в память
# ровно 20 МиБ (окно 0x2000000..0x3400000), поэтому образ дополняется
# до 40960 секторов, даже если разделы заканчиваются раньше.
RAMDISK_WINDOW_SECTORS = 40960


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


BLOCK_4K = 4096
BOOTMAP_VERSION = 2
BOOTMAP_EXTENTS = {"stage2": (64, 8), "ramboot": (128, 8), "kernel": (192, 40)}
SYSTEM_START_LBA = next(start for _, _, start, _, name in PRIMARY if name == "/system")

BOOT_FILES = {
    "stage2": "boot/stage2.bin",
    "ramboot": "boot/ramboot.bin",
    "kernel": "kernel/kernel.bin",
}


def pad_block(data):
    """Дополняет файл нулями до целого числа блоков EROFS (4096 Б).

    Иначе mkfs.erofs кладёт хвост файла inline рядом с inode, и файл
    перестаёт быть непрерывным участком диска, который читает MBR.
    """
    return data + b"\x00" * (-len(data) % BLOCK_4K)


def locate_in_blob(blob, content, name):
    """LBA файла внутри собранного EROFS-образа /system.

    Файл не сжат и выровнен по блоку, поэтому лежит непрерывно: ищем его
    байты в образе и требуем единственное, выровненное по 4096 вхождение.
    """
    if blob.count(content) != 1:
        print("ОШИБКА: %s: в образе /system %d вхождений файла, ожидалось ровно 1"
              % (name, blob.count(content)), file=sys.stderr)
        sys.exit(1)
    off = blob.find(content)
    if off % BLOCK_4K:
        print("ОШИБКА: %s лежит в /system со смещением %d — не выровнен по %d"
              % (name, off, BLOCK_4K), file=sys.stderr)
        sys.exit(1)
    return SYSTEM_START_LBA + off // SECTOR


def write_boot_map(img, boot):
    """Пишет карту загрузки DEIXKIMG v2 в сектор LBA 2047.

    `boot` — {"stage2"|"ramboot"|"kernel": (lba, байты файла с отступом,
    размер без отступа)}. Формат — docs/BOOTMAP.md.
    """
    sec = bytearray(SECTOR)
    kernel_lba, kernel_bytes, _ = boot["kernel"]
    sec[0:8] = KERNEL_IMAGE_INFO_MAGIC
    sec[8:12] = struct.pack("<I", kernel_lba)
    sec[12:16] = struct.pack("<I", len(kernel_bytes))
    sec[16:48] = hashlib.sha256(kernel_bytes).digest()
    sec[48:52] = struct.pack("<I", BOOTMAP_VERSION)
    sec[52:56] = struct.pack("<I", (boot["stage2"][2] + SECTOR - 1) // SECTOR)
    sec[56:60] = struct.pack("<I", (boot["ramboot"][2] + SECTOR - 1) // SECTOR)
    for key, (offset, _max) in BOOTMAP_EXTENTS.items():
        lba, data, _ = boot[key]
        sec[offset:offset + 8] = struct.pack("<II", lba, len(data) // SECTOR)
    base = KERNEL_IMAGE_INFO_LBA * SECTOR
    img[base:base + SECTOR] = sec


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
    return blob


BLOCKS_PER_GROUP = 8192  # как в ядре: src/ext2.rs BLOCKS_PER_GROUP


def format_ext2_at(img, start_lba, total_sectors):
    """Форматирует ext2-том начиная с произвольного LBA (для /userdata).

    Раскладка повторяет драйвер ядра (src/ext2.rs): группа — 8192 блоков,
    в каждой группе копия суперблока, копия GDT, карты блоков и инодов и
    таблица инодов; том может состоять из нескольких групп.
    """
    total_blocks = total_sectors // SPB
    bpg = BLOCKS_PER_GROUP
    groups = (total_blocks - 1 + bpg - 1) // bpg
    gdt_blocks = (groups * 32 + BLOCK - 1) // BLOCK
    inodes = ((max(32, total_blocks * BLOCK // 4096) + 7) // 8) * 8
    ipg = inodes // groups
    assert groups * ipg == inodes and 8 <= ipg <= BLOCK * 8
    itb = (ipg * INODE_SIZE + BLOCK - 1) // BLOCK
    now = 0x60000000

    def gmeta(g):
        base = 1 + g * bpg
        bbm = base + 1 + gdt_blocks
        return base, bbm, bbm + 1, bbm + 2

    def gcount(g):
        base = 1 + g * bpg
        return min(bpg, total_blocks - base)

    def data_start(g):
        return gmeta(g)[3] + itb

    def blk_lba(b):
        return start_lba + b * SPB

    def wb(b, data):
        lb = blk_lba(b) * SECTOR
        img[lb:lb + BLOCK] = data

    zeros = bytes(BLOCK)
    wi_table = {}

    def wi(ino, mode, size, links, ptrs, used):
        g = (ino - 1) // ipg
        idx = (ino - 1) % ipg
        block = gmeta(g)[3] + (idx * INODE_SIZE) // BLOCK
        off = (idx * INODE_SIZE) % BLOCK
        raw = bytearray(INODE_SIZE)
        raw[0:2] = struct.pack("<H", mode)
        raw[4:8] = struct.pack("<I", size)
        raw[26:28] = struct.pack("<H", links)
        raw[28:32] = struct.pack("<I", used * SPB)
        for i in range(15):
            raw[40 + i * 4:44 + i * 4] = struct.pack("<I", ptrs[i])
        buf = bytearray(wi_table.get(block, zeros))
        buf[off:off + INODE_SIZE] = raw
        wi_table[block] = bytes(buf)

    dstart = data_start(0)
    reserved0 = dstart - 1 + 2          # метаданные группы 0 + корень + lost+found
    free_per_group = []
    for g in range(groups):
        overhead = data_start(g) - (1 + g * bpg)
        used = overhead + (2 if g == 0 else 0)
        free_per_group.append(gcount(g) - used)
    free_blocks = sum(free_per_group)
    free_inodes = inodes - 10 - 1

    # Суперблок.
    sb = bytearray(BLOCK)
    sb[0:4] = struct.pack("<I", inodes)
    sb[4:8] = struct.pack("<I", total_blocks)
    sb[12:16] = struct.pack("<I", free_blocks)
    sb[16:20] = struct.pack("<I", free_inodes)
    sb[20:24] = struct.pack("<I", 1)
    sb[32:36] = struct.pack("<I", bpg)
    sb[36:40] = struct.pack("<I", total_blocks)
    sb[40:44] = struct.pack("<I", ipg)
    sb[48:52] = struct.pack("<I", now)
    sb[54:56] = struct.pack("<H", 0xFFFF)
    sb[56:58] = struct.pack("<H", EXT2_MAGIC)
    sb[58:60] = struct.pack("<H", 1)
    sb[64:68] = struct.pack("<I", now)
    wb(SB_BLOCK, sb)

    # GDT: по дескриптору на группу.
    gdt = bytearray(BLOCK)
    for g in range(groups):
        _, bbm, ibm, it = gmeta(g)
        o = g * 32
        gdt[o:o + 4] = struct.pack("<I", bbm)
        gdt[o + 4:o + 8] = struct.pack("<I", ibm)
        gdt[o + 8:o + 12] = struct.pack("<I", it)
        gdt[o + 12:o + 14] = struct.pack("<H", free_per_group[g])
        gdt[o + 14:o + 16] = struct.pack("<H", ipg - (11 if g == 0 else 0))
        gdt[o + 16:o + 18] = struct.pack("<H", 2 if g == 0 else 0)
    for b in range(GDT_BLOCK, GDT_BLOCK + gdt_blocks):
        wb(b, gdt if b == GDT_BLOCK else zeros)

    # Карты блоков и инодов, таблицы инодов — по группам.
    for g in range(groups):
        base, bbm, ibm, it = gmeta(g)
        n = gcount(g)
        bm = bytearray(BLOCK)
        for i in range(data_start(g) - base):
            bm[i // 8] |= 1 << (i % 8)
        if g == 0:
            for blk in (dstart, dstart + 1):
                i = blk - base
                bm[i // 8] |= 1 << (i % 8)
        for i in range(n, BLOCK * 8):
            bm[i // 8] |= 1 << (i % 8)
        wb(bbm, bm)

        ib = bytearray(BLOCK)
        if g == 0:
            for i in range(11):
                ib[i // 8] |= 1 << (i % 8)
        for i in range(ipg, BLOCK * 8):
            ib[i // 8] |= 1 << (i % 8)
        wb(ibm, ib)

        for b in range(it, it + itb):
            wb(b, zeros)

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

    ptrs = [0] * 15
    ptrs[0] = dstart
    wi(2, 0o040755, BLOCK, 3, ptrs, 1)
    ptrs2 = [0] * 15
    ptrs2[0] = dstart + 1
    wi(11, 0o040700, BLOCK, 2, ptrs2, 1)
    for block, data in sorted(wi_table.items()):
        wb(block, data)


def init_all_partitions(img, boot_files):
    """Инициализирует ФС во всех разделах: /system (EROFS RO) + /userdata (EXT2 RW).

    `boot_files` — {"stage2"|"ramboot"|"kernel": байты}. Возвращает
    {имя: (LBA, байты с отступом, размер без отступа)}.
    """
    padded = {k: pad_block(v) for k, v in boot_files.items()}
    located = {}
    for num, typ, start, secs, name in PRIMARY:
        if name == "/userdata":
            format_ext2_at(img, start, secs)
        elif name == "/system":
            system_files = {
                BOOT_FILES[k]: padded[k] for k in BOOT_FILES
            }
            system_files.update({
                "etc/init.deix":       b"# DeiX OS init script\nmount /system\nmount /userdata\n",
                "services/dinit.cfg":  b"# Dinit services config\n",
            })
            # UI-звуки лежат ТОЛЬКО в /system/media/audio/ui/ —
            # этот путь читает src/sound.rs.
            snd_dir = os.path.join("build", "sounds")
            if os.path.isdir(snd_dir):
                for _f in sorted(os.listdir(snd_dir)):
                    if _f.endswith(".dps"):
                        with open(os.path.join(snd_dir, _f), "rb") as _fh:
                            system_files[f"media/audio/ui/{_f}"] = _fh.read()
            # Встроенные обои читает ui::desktop из /system/media/wallpapers/:
            # BMP 24 бит и PNG с палитрой, 640x480; в ядро они не вшиваются.
            wp_dir = os.path.join("assets", "wallpapers")
            if os.path.isdir(wp_dir):
                for _f in sorted(os.listdir(wp_dir)):
                    if _f.endswith(".bmp") or _f.endswith(".png"):
                        with open(os.path.join(wp_dir, _f), "rb") as _fh:
                            system_files[f"media/wallpapers/{_f}"] = _fh.read()
            # Картинки-иллюстрации (Files, /system/media/pictures).
            pic_dir = os.path.join("assets", "pictures")
            if os.path.isdir(pic_dir):
                for _f in sorted(os.listdir(pic_dir)):
                    if _f.endswith(".png"):
                        with open(os.path.join(pic_dir, _f), "rb") as _fh:
                            system_files[f"media/pictures/{_f}"] = _fh.read()
            # Цветные иконки приложений (DXIC, tools/make_icons.py) читает
            # ui::appicons из /system/media/icons/.
            ic_dir = os.path.join("assets", "icons")
            if os.path.isdir(ic_dir):
                for _f in sorted(os.listdir(ic_dir)):
                    if _f.endswith(".dxic"):
                        with open(os.path.join(ic_dir, _f), "rb") as _fh:
                            system_files[f"media/icons/{_f}"] = _fh.read()
            blob = write_erofs_image(img, start, secs, name, system_files)
            for k in BOOT_FILES:
                located[k] = (locate_in_blob(blob, padded[k], BOOT_FILES[k]),
                              padded[k], len(boot_files[k]))
    return located


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    opts = {}
    for a in sys.argv[1:]:
        if a.startswith("--") and "=" in a:
            k, v = a[2:].split("=", 1)
            opts[k] = v

    img_path = args[0] if args else "build/deix_disk.img"
    parent_dir = os.path.dirname(img_path)
    if parent_dir:
        os.makedirs(parent_dir, exist_ok=True)

    boot_files = {}
    for key, fname in (("kernel", "kernel.bin"), ("stage2", "stage2.bin"),
                       ("ramboot", "ramboot.bin")):
        path = os.path.join(parent_dir if parent_dir else ".", fname)
        if not os.path.exists(path):
            print("ОШИБКА: %s не найден — соберите загрузчик и ядро (build.sh) перед разметкой образа"
                  % path, file=sys.stderr)
            sys.exit(1)
        with open(path, "rb") as f:
            boot_files[key] = f.read()
    if len(boot_files["stage2"]) > BLOCK_4K:
        print("ОШИБКА: stage2.bin (%d Б) не помещается в 4096 Б (0x10000..0x11000)"
              % len(boot_files["stage2"]), file=sys.stderr)
        sys.exit(1)
    boot_files["kernel"] = pad_block(boot_files["kernel"])

    need = max(PARTITIONS_END_SECTORS, RAMDISK_WINDOW_SECTORS) * SECTOR
    if not os.path.exists(img_path) or os.path.getsize(img_path) < need:
        with open(img_path, "ab") as f:
            f.truncate(need)

    with open(img_path, "rb") as f:
        img = bytearray(f.read())

    fill_mbr(img)
    zero_start = SECTOR
    zero_end = KERNEL_IMAGE_INFO_LBA * SECTOR
    img[zero_start:zero_end] = bytes(zero_end - zero_start)
    located = init_all_partitions(img, boot_files)
    write_boot_map(img, located)

    with open(img_path, "wb") as f:
        f.write(img)

    print(f"OK: DeiX OS — заводской образ с MBR-разметкой ({img_path})")
    print("  Разделы DeiX:")
    for num, typ, start, secs, name in PRIMARY:
        fs = "erofs" if name == "/system" else "ext2"
        print(f"    P{num}: 0x{typ:02X} {name:<13} LBA {start:>6}..{start+secs-1:>6} ({secs:>4} сект)  {fs}")


if __name__ == "__main__":
    main()
