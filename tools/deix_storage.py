#!/usr/bin/env python3
"""deix_storage.py — доступ к разделам образа диска DeiX с хост-машины.

Умеет:
  * /userdata (ext2, LBA 12800): список/чтение/запись файлов и каталогов,
    в том числе на ЗАШИФРОВАННОМ томе — по паролю пользователя DeiX.
    Поддержаны обе схемы шифрования ядра (src/crypto_storage.rs):
      - заголовок DEIXLUKS (LBA 4094): PBKDF2-SHA512 -> KEK, слоты
        с запечатанным мастер-ключом, XTS-AES-256 посекторно;
      - старые тома без заголовка: ключ = SHA-512(пароль) напрямую.
    Tweak XTS = АБСОЛЮТНЫЙ LBA сектора (схема plain64, байт-в-байт
    совместимо с cryptsetup --type plain --cipher aes-xts-plain64
    --key-size 512 --hash sha512).
  * /system (EROFS, LBA 4096): список/чтение файлов (несжатые образы,
    форматы инодов compact/extended, раскладки FLAT_PLAIN/FLAT_INLINE).

Для XTS/AES нужна библиотека `cryptography` (pip install cryptography);
незашифрованные тома и EROFS читаются без неё.

Примеры:
  python3 tools/deix_storage.py disk.img info
  python3 tools/deix_storage.py disk.img ls /
  python3 tools/deix_storage.py disk.img put wall.bmp /wallpaper.bmp -p secret
  python3 tools/deix_storage.py disk.img get /config/desktop.cfg cfg.txt -p secret
  python3 tools/deix_storage.py disk.img sys-ls /etc
  python3 tools/deix_storage.py disk.img sys-get /etc/os-release os-release
"""

import argparse
import getpass
import hashlib
import struct
import sys

SECTOR = 512

# Раскладка диска — см. tools/make_deix_fs.py и src/partition_map.rs.
USERDATA_LBA = 12800
EROFS_LBA = 4096

# Шифрование — см. src/crypto_storage.rs и src/luks.rs.
CRYPT_MARKER_LBA = 4095
CRYPT_MARKER_MAGIC = b"DEIXCRYP"
LUKS_HEADER_LBA = 4094
LUKS_MAGIC = b"DEIXLUKS"
LUKS_SLOT_BASE = 64
LUKS_SLOT_SIZE = 56
LUKS_MAX_SLOTS = 8

# ext2 — см. src/ext2.rs (BLOCK_SIZE=1024, INODE_SIZE=128, одна группа).
BLOCK = 1024
SPB = BLOCK // SECTOR
INODE_SIZE = 128
ROOT_INO = 2
EXT2_MAGIC = 0xEF53
MAX_FILE_BLOCKS = 12 + BLOCK // 4  # прямые + один indirect, как в ядре


def die(msg):
    print("error: %s" % msg, file=sys.stderr)
    sys.exit(1)


# ---------------------------------------------------------------- crypto


def _aes_ecb_decrypt(key, data):
    from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

    dec = Cipher(algorithms.AES(key), modes.ECB()).decryptor()
    return dec.update(data) + dec.finalize()


class SectorCrypto:
    """XTS-AES-256 посекторно; tweak = абсолютный LBA (plain64)."""

    def __init__(self, key64):
        self.key = key64  # None => том не зашифрован
        if key64 is not None:
            try:
                from cryptography.hazmat.primitives.ciphers import (  # noqa: F401
                    Cipher,
                    algorithms,
                    modes,
                )
            except ImportError:
                die(
                    "том зашифрован, нужна библиотека cryptography "
                    "(pip install cryptography)"
                )

    def _cipher(self, lba):
        from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

        tweak = struct.pack("<Q", lba) + b"\x00" * 8
        return Cipher(algorithms.AES(self.key), modes.XTS(tweak))

    def decrypt(self, lba, data):
        if self.key is None:
            return data
        d = self._cipher(lba).decryptor()
        return d.update(data) + d.finalize()

    def encrypt(self, lba, data):
        if self.key is None:
            return data
        e = self._cipher(lba).encryptor()
        return e.update(data) + e.finalize()


class Image:
    def __init__(self, path, writable=False):
        self.f = open(path, "r+b" if writable else "rb")

    def read_sector(self, lba, count=1):
        self.f.seek(lba * SECTOR)
        data = self.f.read(count * SECTOR)
        if len(data) != count * SECTOR:
            die("образ короче ожидаемого (LBA %d)" % lba)
        return data

    def write_sector(self, lba, data):
        assert len(data) % SECTOR == 0
        self.f.seek(lba * SECTOR)
        self.f.write(data)


def is_encrypted(img):
    return img.read_sector(CRYPT_MARKER_LBA)[:8] == CRYPT_MARKER_MAGIC


def unlock(img, password):
    """Возвращает SectorCrypto; паролем вскрывает слоты DEIXLUKS или
    пробует старую схему (ключ = SHA-512(пароль))."""
    if not is_encrypted(img):
        return SectorCrypto(None)
    if password is None:
        password = getpass.getpass("DeiX password: ")

    hdr = img.read_sector(LUKS_HEADER_LBA)
    if hdr[:8] == LUKS_MAGIC:
        iterations = struct.unpack_from("<I", hdr, 12)[0]
        mk_check = hdr[16:48]
        for i in range(LUKS_MAX_SLOTS):
            off = LUKS_SLOT_BASE + i * LUKS_SLOT_SIZE
            if hdr[off] != 1:
                continue
            salt = hdr[off + 8 : off + 24]
            sealed = hdr[off + 24 : off + 56]
            kek = hashlib.pbkdf2_hmac(
                "sha512", password.encode(), salt, iterations, 64
            )[:32]
            mk = _aes_ecb_decrypt(kek, sealed)
            if hashlib.sha512(mk).digest()[:32] == mk_check:
                key64 = mk + hashlib.sha512(mk).digest()[:32]
                return SectorCrypto(key64)
        die("неверный пароль (ни один слот DEIXLUKS не открылся)")

    # Старая схема без заголовка: ключ напрямую из пароля.
    crypto = SectorCrypto(hashlib.sha512(password.encode()).digest())
    sb = crypto.decrypt(USERDATA_LBA + SPB, img.read_sector(USERDATA_LBA + SPB))
    sb += crypto.decrypt(
        USERDATA_LBA + SPB + 1, img.read_sector(USERDATA_LBA + SPB + 1)
    )
    if struct.unpack_from("<H", sb, 56)[0] != EXT2_MAGIC:
        die("неверный пароль (суперблок ext2 не расшифровался)")
    return crypto


# ------------------------------------------------------------------ ext2


class Ext2:
    """Минимальный драйвер ext2-тома DeiX (блок 1 КБ, одна группа).

    Зеркалирует семантику src/ext2.rs: нумерация блоков = бит + 1,
    file_type в dirent не используется, файл <= 12 прямых блоков + один
    indirect-блок (268 КБ).
    """

    def __init__(self, img, crypto):
        self.img = img
        self.crypto = crypto
        sb = self.read_block(1)
        if struct.unpack_from("<H", sb, 56)[0] != EXT2_MAGIC:
            die("на /userdata нет ext2 (том пуст или пароль не подошёл)")
        self.blocks_count = struct.unpack_from("<I", sb, 4)[0]
        self.inodes_count = struct.unpack_from("<I", sb, 0)[0]
        gdt = self.read_block(2)
        self.block_bitmap = struct.unpack_from("<I", gdt, 0)[0]
        self.inode_bitmap = struct.unpack_from("<I", gdt, 4)[0]
        self.inode_table = struct.unpack_from("<I", gdt, 8)[0]

    # -- блочный слой (шифрование на границе 512-байтных секторов) --

    def read_block(self, n):
        lba = USERDATA_LBA + n * SPB
        out = b""
        for i in range(SPB):
            out += self.crypto.decrypt(lba + i, self.img.read_sector(lba + i))
        return bytearray(out)

    def write_block(self, n, data):
        assert len(data) == BLOCK
        lba = USERDATA_LBA + n * SPB
        for i in range(SPB):
            chunk = bytes(data[i * SECTOR : (i + 1) * SECTOR])
            self.img.write_sector(lba + i, self.crypto.encrypt(lba + i, chunk))

    # -- inode --

    def _inode_loc(self, ino):
        index = ino - 1
        blk = self.inode_table + (index * INODE_SIZE) // BLOCK
        off = (index * INODE_SIZE) % BLOCK
        return blk, off

    def read_inode(self, ino):
        blk, off = self._inode_loc(ino)
        raw = self.read_block(blk)[off : off + INODE_SIZE]
        mode = struct.unpack_from("<H", raw, 0)[0]
        size = struct.unpack_from("<I", raw, 4)[0]
        links = struct.unpack_from("<H", raw, 26)[0]
        ptrs = list(struct.unpack_from("<15I", raw, 40))
        return {"mode": mode, "size": size, "links": links, "ptrs": ptrs}

    def write_inode(self, ino, mode, size, links, ptrs, used_blocks):
        raw = bytearray(INODE_SIZE)
        struct.pack_into("<H", raw, 0, mode)
        struct.pack_into("<I", raw, 4, size)
        struct.pack_into("<H", raw, 26, links)
        struct.pack_into("<I", raw, 28, used_blocks * SPB)
        for i, p in enumerate(ptrs[:15]):
            struct.pack_into("<I", raw, 40 + i * 4, p)
        blk, off = self._inode_loc(ino)
        buf = self.read_block(blk)
        buf[off : off + INODE_SIZE] = raw
        self.write_block(blk, buf)

    # -- битовые карты и счётчики --

    def _bitmap_find_free(self, bitmap_block, limit):
        buf = self.read_block(bitmap_block)
        for bit in range(limit):
            if not buf[bit // 8] & (1 << (bit % 8)):
                buf[bit // 8] |= 1 << (bit % 8)
                self.write_block(bitmap_block, buf)
                return bit
        die("на томе нет места")

    def _bitmap_clear(self, bitmap_block, bit):
        buf = self.read_block(bitmap_block)
        buf[bit // 8] &= ~(1 << (bit % 8)) & 0xFF
        self.write_block(bitmap_block, buf)

    def _adjust(self, sb_off, gdt_off, delta):
        sb = self.read_block(1)
        v = struct.unpack_from("<I", sb, sb_off)[0] + delta
        struct.pack_into("<I", sb, sb_off, v & 0xFFFFFFFF)
        self.write_block(1, sb)
        gdt = self.read_block(2)
        v = struct.unpack_from("<H", gdt, gdt_off)[0] + delta
        struct.pack_into("<H", gdt, gdt_off, v & 0xFFFF)
        self.write_block(2, gdt)

    def alloc_block(self):
        bit = self._bitmap_find_free(self.block_bitmap, self.blocks_count - 1)
        self._adjust(12, 12, -1)
        return bit + 1  # физический номер = first_data_block(1) + бит

    def free_block(self, n):
        self._bitmap_clear(self.block_bitmap, n - 1)
        self._adjust(12, 12, 1)

    def alloc_inode(self):
        bit = self._bitmap_find_free(self.inode_bitmap, self.inodes_count)
        self._adjust(16, 14, -1)
        return bit + 1

    def free_inode(self, ino):
        self._bitmap_clear(self.inode_bitmap, ino - 1)
        self._adjust(16, 14, 1)

    # -- каталоги --

    def iter_dir(self, ino):
        inode = self.read_inode(ino)
        for blk in inode["ptrs"][:12]:
            if blk == 0:
                continue
            buf = self.read_block(blk)
            off = 0
            while off + 8 <= BLOCK:
                e_ino, rec_len = struct.unpack_from("<IH", buf, off)
                if rec_len == 0:
                    break
                name_len = buf[off + 6]
                name = bytes(buf[off + 8 : off + 8 + name_len]).decode(
                    "utf-8", "replace"
                )
                yield blk, off, e_ino, rec_len, name
                off += rec_len

    def lookup(self, path):
        ino = ROOT_INO
        for part in [p for p in path.split("/") if p]:
            found = None
            for _, _, e_ino, _, name in self.iter_dir(ino):
                if e_ino != 0 and name == part:
                    found = e_ino
                    break
            if found is None:
                return None
            ino = found
        return ino

    def read_file(self, ino):
        inode = self.read_inode(ino)
        data = b""
        ptrs = inode["ptrs"][:12]
        if inode["ptrs"][12]:
            ind = self.read_block(inode["ptrs"][12])
            ptrs += [p for p in struct.unpack_from("<256I", ind, 0)]
        for p in ptrs:
            if len(data) >= inode["size"]:
                break
            if p:
                data += bytes(self.read_block(p))
        return data[: inode["size"]]

    def _free_file(self, ino):
        inode = self.read_inode(ino)
        for p in inode["ptrs"][:12]:
            if p:
                self.free_block(p)
        if inode["ptrs"][12]:
            ind = self.read_block(inode["ptrs"][12])
            for p in struct.unpack_from("<256I", ind, 0):
                if p:
                    self.free_block(p)
            self.free_block(inode["ptrs"][12])
        self.free_inode(ino)

    def _remove_dirent(self, dir_ino, name):
        for blk, off, e_ino, _, e_name in self.iter_dir(dir_ino):
            if e_ino != 0 and e_name == name:
                buf = self.read_block(blk)
                struct.pack_into("<I", buf, off, 0)
                self.write_block(blk, buf)
                return True
        return False

    def _add_dirent(self, dir_ino, name, ino):
        nb = name.encode()
        needed = (8 + len(nb) + 3) // 4 * 4
        for blk, off, e_ino, rec_len, e_name in self.iter_dir(dir_ino):
            actual = 0 if e_ino == 0 else (8 + len(e_name.encode()) + 3) // 4 * 4
            buf = self.read_block(blk)
            if e_ino == 0 and rec_len >= needed:
                self._put_dirent(buf, off, ino, nb, rec_len)
                self.write_block(blk, buf)
                return
            if e_ino != 0 and rec_len - actual >= needed:
                struct.pack_into("<H", buf, off + 4, actual)
                self._put_dirent(buf, off + actual, ino, nb, rec_len - actual)
                self.write_block(blk, buf)
                return
        # Свободного места в существующих блоках каталога нет —
        # добавляем каталогу новый блок (как сделало бы ядро).
        inode = self.read_inode(dir_ino)
        for i in range(12):
            if inode["ptrs"][i] == 0:
                nblk = self.alloc_block()
                buf = bytearray(BLOCK)
                self._put_dirent(buf, 0, ino, nb, BLOCK)
                self.write_block(nblk, buf)
                inode["ptrs"][i] = nblk
                used = sum(1 for p in inode["ptrs"] if p)
                self.write_inode(
                    dir_ino,
                    inode["mode"],
                    inode["size"] + BLOCK,
                    inode["links"],
                    inode["ptrs"],
                    used,
                )
                return
        die("каталог переполнен")

    @staticmethod
    def _put_dirent(buf, off, ino, name_bytes, rec_len):
        struct.pack_into("<IHBB", buf, off, ino, rec_len, len(name_bytes), 0)
        buf[off + 8 : off + 8 + len(name_bytes)] = name_bytes

    # -- операции --

    def write_file(self, dir_ino, name, data):
        blocks_needed = max((len(data) + BLOCK - 1) // BLOCK, 0)
        if blocks_needed > MAX_FILE_BLOCKS:
            die(
                "файл больше предела тома DeiX (%d КБ): %d КБ"
                % (MAX_FILE_BLOCKS * BLOCK // 1024, len(data) // 1024)
            )
        for _, _, e_ino, _, e_name in self.iter_dir(dir_ino):
            if e_ino != 0 and e_name == name:
                self._free_file(e_ino)
                self._remove_dirent(dir_ino, name)
                break

        ptrs = [0] * 15
        used = 0
        direct = min(blocks_needed, 12)
        for i in range(direct):
            b = self.alloc_block()
            chunk = data[i * BLOCK : (i + 1) * BLOCK]
            self.write_block(b, bytearray(chunk.ljust(BLOCK, b"\x00")))
            ptrs[i] = b
            used += 1
        if blocks_needed > 12:
            ind_ptrs = [0] * (BLOCK // 4)
            for i in range(blocks_needed - 12):
                b = self.alloc_block()
                chunk = data[(12 + i) * BLOCK : (13 + i) * BLOCK]
                self.write_block(b, bytearray(chunk.ljust(BLOCK, b"\x00")))
                ind_ptrs[i] = b
                used += 1
            ind = self.alloc_block()
            buf = bytearray(BLOCK)
            for i, p in enumerate(ind_ptrs):
                struct.pack_into("<I", buf, i * 4, p)
            self.write_block(ind, buf)
            ptrs[12] = ind
            used += 1

        ino = self.alloc_inode()
        self.write_inode(ino, 0o100644, len(data), 1, ptrs, used)
        self._add_dirent(dir_ino, name, ino)

    def mkdir(self, parent_ino, name):
        for _, _, e_ino, _, e_name in self.iter_dir(parent_ino):
            if e_ino != 0 and e_name == name:
                inode = self.read_inode(e_ino)
                if inode["mode"] & 0xF000 == 0x4000:
                    return e_ino
                die("имя занято обычным файлом: %s" % name)
        ino = self.alloc_inode()
        blk = self.alloc_block()
        buf = bytearray(BLOCK)
        self._put_dirent(buf, 0, ino, b".", 12)
        self._put_dirent(buf, 12, parent_ino, b"..", BLOCK - 12)
        self.write_block(blk, buf)
        ptrs = [0] * 15
        ptrs[0] = blk
        self.write_inode(ino, 0o040755, BLOCK, 2, ptrs, 1)
        self._add_dirent(parent_ino, name, ino)
        parent = self.read_inode(parent_ino)
        pused = sum(1 for p in parent["ptrs"] if p)
        self.write_inode(
            parent_ino,
            parent["mode"],
            parent["size"],
            parent["links"] + 1,
            parent["ptrs"],
            pused,
        )
        return ino


# ----------------------------------------------------------------- erofs


class Erofs:
    """Чтение несжатого EROFS-раздела /system (инody compact/extended,
    раскладки FLAT_PLAIN и FLAT_INLINE)."""

    MAGIC = 0xE0F5E1E2
    ISLOT = 32

    def __init__(self, img):
        self.img = img
        self.base = EROFS_LBA * SECTOR
        sb = self._read(1024, 128)
        if struct.unpack_from("<I", sb, 0)[0] != self.MAGIC:
            die("на /system нет EROFS")
        self.bs = 1 << sb[12]
        self.root_nid = struct.unpack_from("<H", sb, 14)[0]
        self.meta_blkaddr = struct.unpack_from("<I", sb, 40)[0]

    def _read(self, off, size):
        self.img.f.seek(self.base + off)
        return self.img.f.read(size)

    def inode(self, nid):
        off = self.meta_blkaddr * self.bs + nid * self.ISLOT
        raw = self._read(off, 64)
        fmt = struct.unpack_from("<H", raw, 0)[0]
        extended = fmt & 1
        layout = (fmt >> 1) & 7
        if layout not in (0, 2):
            die("поддерживаются только несжатые EROFS (datalayout=%d)" % layout)
        if extended:
            mode = struct.unpack_from("<H", raw, 4)[0]
            size = struct.unpack_from("<Q", raw, 8)[0]
            blkaddr = struct.unpack_from("<I", raw, 16)[0]
            isize = 64
        else:
            mode = struct.unpack_from("<H", raw, 4)[0]
            size = struct.unpack_from("<I", raw, 8)[0]
            blkaddr = struct.unpack_from("<I", raw, 16)[0]
            isize = 32
        xattr_count = struct.unpack_from("<H", raw, 2)[0]
        xattr_size = 12 + (xattr_count - 1) * 4 if xattr_count else 0
        return {
            "mode": mode,
            "size": size,
            "blkaddr": blkaddr,
            "layout": layout,
            "inline_off": off + isize + xattr_size,
        }

    def data(self, ino):
        size = ino["size"]
        if ino["layout"] == 0:  # FLAT_PLAIN
            return self._read(ino["blkaddr"] * self.bs, size)
        full = (size // self.bs) * self.bs
        out = self._read(ino["blkaddr"] * self.bs, full) if full else b""
        tail = size - full
        if tail:
            out += self._read(ino["inline_off"], tail)
        return out

    def dirents(self, ino):
        data = self.data(ino)
        out = []
        pos = 0
        while pos < len(data):
            block = data[pos : pos + self.bs]
            if len(block) < 12:
                break
            first_nameoff = struct.unpack_from("<H", block, 8)[0]
            count = first_nameoff // 12
            if count == 0:
                break
            ents = []
            for i in range(count):
                nid, nameoff, ftype, _ = struct.unpack_from("<QHBB", block, i * 12)
                ents.append((nid, nameoff, ftype))
            for i, (nid, nameoff, ftype) in enumerate(ents):
                end = ents[i + 1][1] if i + 1 < count else len(block)
                name = block[nameoff:end].rstrip(b"\x00").decode("utf-8", "replace")
                if name:
                    out.append((name, nid, ftype))
            pos += self.bs
        return out

    def lookup(self, path):
        nid = self.root_nid
        for part in [p for p in path.split("/") if p]:
            found = None
            for name, child, _ in self.dirents(self.inode(nid)):
                if name == part:
                    found = child
                    break
            if found is None:
                return None
            nid = found
        return nid


# ------------------------------------------------------------------- CLI


def open_userdata(args, writable=False):
    img = Image(args.image, writable)
    crypto = unlock(img, args.password)
    return Ext2(img, crypto)


def cmd_info(args):
    img = Image(args.image)
    enc = is_encrypted(img)
    print("userdata : ext2 @ LBA %d%s" % (USERDATA_LBA, ", ЗАШИФРОВАН" if enc else ""))
    if enc:
        hdr = img.read_sector(LUKS_HEADER_LBA)
        if hdr[:8] == LUKS_MAGIC:
            used = sum(
                1
                for i in range(LUKS_MAX_SLOTS)
                if hdr[LUKS_SLOT_BASE + i * LUKS_SLOT_SIZE] == 1
            )
            iters = struct.unpack_from("<I", hdr, 12)[0]
            print("           DEIXLUKS: слотов занято %d, PBKDF2 итераций %d" % (used, iters))
        else:
            print("           старая схема: ключ = SHA-512(пароль)")
    try:
        fs = Erofs(img)
        print("system   : EROFS @ LBA %d, блок %d Б" % (EROFS_LBA, fs.bs))
    except SystemExit:
        print("system   : EROFS не найден")


def _fmt_entry(name, is_dir, size):
    return "%s %8s  %s" % ("d" if is_dir else "-", "-" if is_dir else size, name)


def cmd_ls(args):
    fs = open_userdata(args)
    ino = fs.lookup(args.path)
    if ino is None:
        die("нет такого пути: %s" % args.path)
    inode = fs.read_inode(ino)
    if inode["mode"] & 0xF000 != 0x4000:
        print(_fmt_entry(args.path.rstrip("/").split("/")[-1], False, inode["size"]))
        return
    for _, _, e_ino, _, name in fs.iter_dir(ino):
        if e_ino == 0 or name in (".", ".."):
            continue
        child = fs.read_inode(e_ino)
        print(_fmt_entry(name, child["mode"] & 0xF000 == 0x4000, child["size"]))


def cmd_cat(args):
    fs = open_userdata(args)
    ino = fs.lookup(args.path)
    if ino is None:
        die("нет такого файла: %s" % args.path)
    sys.stdout.buffer.write(fs.read_file(ino))


def cmd_get(args):
    fs = open_userdata(args)
    ino = fs.lookup(args.path)
    if ino is None:
        die("нет такого файла: %s" % args.path)
    data = fs.read_file(ino)
    with open(args.local, "wb") as f:
        f.write(data)
    print("%s -> %s (%d Б)" % (args.path, args.local, len(data)))


def cmd_put(args):
    with open(args.local, "rb") as f:
        data = f.read()
    fs = open_userdata(args, writable=True)
    parts = [p for p in args.path.split("/") if p]
    if not parts:
        die("нужен путь к файлу, а не корень")
    dir_ino = ROOT_INO
    for part in parts[:-1]:
        dir_ino = fs.mkdir(dir_ino, part)
    fs.write_file(dir_ino, parts[-1], data)
    print("%s -> %s (%d Б)" % (args.local, args.path, len(data)))


def cmd_mkdir(args):
    fs = open_userdata(args, writable=True)
    dir_ino = ROOT_INO
    for part in [p for p in args.path.split("/") if p]:
        dir_ino = fs.mkdir(dir_ino, part)
    print("каталог готов: %s" % args.path)


def cmd_sys_ls(args):
    fs = Erofs(Image(args.image))
    nid = fs.lookup(args.path)
    if nid is None:
        die("нет такого пути: %s" % args.path)
    ino = fs.inode(nid)
    if ino["mode"] & 0xF000 != 0x4000:
        print(_fmt_entry(args.path.rstrip("/").split("/")[-1], False, ino["size"]))
        return
    for name, child, _ in fs.dirents(ino):
        if name in (".", ".."):
            continue
        ci = fs.inode(child)
        print(_fmt_entry(name, ci["mode"] & 0xF000 == 0x4000, ci["size"]))


def cmd_sys_cat(args):
    fs = Erofs(Image(args.image))
    nid = fs.lookup(args.path)
    if nid is None:
        die("нет такого файла: %s" % args.path)
    sys.stdout.buffer.write(fs.data(fs.inode(nid)))


def cmd_sys_get(args):
    fs = Erofs(Image(args.image))
    nid = fs.lookup(args.path)
    if nid is None:
        die("нет такого файла: %s" % args.path)
    data = fs.data(fs.inode(nid))
    with open(args.local, "wb") as f:
        f.write(data)
    print("%s -> %s (%d Б)" % (args.path, args.local, len(data)))


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("image", help="образ диска DeiX (disk.img)")
    sub = ap.add_subparsers(dest="cmd", required=True)

    sub.add_parser("info", help="сводка: шифрование, разделы")

    def add(name, fn, *fields, password=False, help=None):
        p = sub.add_parser(name, help=help)
        for f in fields:
            p.add_argument(f)
        if password:
            p.add_argument("-p", "--password", help="пароль DeiX (иначе спросит)")
        p.set_defaults(fn=fn)
        return p

    p_ls = add("ls", cmd_ls, password=True, help="список /userdata")
    p_ls.add_argument("path", nargs="?", default="/")
    add("cat", cmd_cat, "path", password=True, help="файл /userdata в stdout")
    add("get", cmd_get, "path", "local", password=True, help="скачать из /userdata")
    add("put", cmd_put, "local", "path", password=True, help="записать в /userdata")
    add("mkdir", cmd_mkdir, "path", password=True, help="создать каталог в /userdata")
    p_sls = add("sys-ls", cmd_sys_ls, help="список /system (EROFS)")
    p_sls.add_argument("path", nargs="?", default="/")
    add("sys-cat", cmd_sys_cat, "path", help="файл /system в stdout")
    add("sys-get", cmd_sys_get, "path", "local", help="скачать из /system")

    args = ap.parse_args()
    if args.cmd == "info":
        cmd_info(args)
    else:
        args.fn(args)


if __name__ == "__main__":
    main()
