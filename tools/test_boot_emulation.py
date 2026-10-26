#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Проверка цепочки MBR → ramboot → stage2 на эмуляторе CPU (unicorn).

Собирает загрузчик из boot/*.asm, строит образ через tools/make_deix_fs.py и
исполняет реальный код MBR и ramboot в x86 (BIOS int 13h подменён чтением из
образа). Тест проходит, если ядро из /system оказалось в 0x100000, а stage2 в
0x10000 и управление дошло до его первой инструкции. Проверяются также
раздробленные на участки файлы и отказ при испорченной карте.

Нужны nasm, ld, objcopy, strip и пакет unicorn (`pip install unicorn`);
без них тест пропускается.
"""
import os
import shutil
import struct
import subprocess
import tempfile
import unittest

try:
    from unicorn import Uc, UC_ARCH_X86, UC_MODE_16, UC_HOOK_INTR, UC_HOOK_INSN, UC_HOOK_CODE, UcError
    from unicorn.x86_const import (UC_X86_REG_AH, UC_X86_REG_AL, UC_X86_REG_DL, UC_X86_REG_DS,
                                   UC_X86_REG_SI, UC_X86_REG_SP, UC_X86_REG_EFLAGS,
                                   UC_X86_REG_EIP, UC_X86_INS_IN, UC_X86_INS_OUT)
    HAVE_UNICORN = True
except ImportError:
    HAVE_UNICORN = False

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STAGE2_ADDR = 0x10000
BOOTMAP_LBA = 2047


def have_tools():
    return all(shutil.which(t) for t in ("nasm", "ld", "objcopy", "strip"))


def run(cmd, **kw):
    subprocess.run(cmd, cwd=ROOT, check=True, capture_output=True, **kw)


def run_boot(img):
    """Исполняет MBR; возвращает (EIP, текст на экране, чтения диска, память)."""
    mu = Uc(UC_ARCH_X86, UC_MODE_16)
    mu.mem_map(0, 0x4000000)
    mu.mem_write(0x7C00, img[:512])
    mu.reg_write(UC_X86_REG_DL, 0x80)
    mu.reg_write(UC_X86_REG_SP, 0x7C00)
    out, reads = [], []

    def on_int(uc, no, _):
        if no == 0x13:
            if uc.reg_read(UC_X86_REG_AH) != 0x42:
                raise RuntimeError("int 13h: только AH=42h")
            ds = uc.reg_read(UC_X86_REG_DS)
            si = uc.reg_read(UC_X86_REG_SI)
            _, _, count, off, seg, lba = struct.unpack("<BBHHHQ", bytes(uc.mem_read(ds * 16 + si, 16)))
            if count > 127:
                raise RuntimeError("int 13h: больше 127 секторов за вызов")
            uc.mem_write(seg * 16 + off, img[lba * 512:(lba + count) * 512])
            reads.append((lba, count))
            uc.reg_write(UC_X86_REG_EFLAGS, uc.reg_read(UC_X86_REG_EFLAGS) & ~1)
        elif no == 0x10:
            out.append(chr(uc.reg_read(UC_X86_REG_AL)))
        else:
            raise RuntimeError("неожиданное прерывание %#x" % no)

    mu.hook_add(UC_HOOK_INTR, on_int)
    mu.hook_add(UC_HOOK_INSN, lambda uc, port, size, ud: 0x20 if port == 0x3FD else 0,
                None, 1, 0, UC_X86_INS_IN)
    mu.hook_add(UC_HOOK_INSN, lambda uc, port, size, val, ud: None, None, 1, 0, UC_X86_INS_OUT)
    mu.hook_add(UC_HOOK_CODE, lambda uc, addr, size, ud: uc.emu_stop(), None, STAGE2_ADDR, STAGE2_ADDR)
    try:
        mu.emu_start(0x7C00, 0, timeout=60_000_000, count=20_000_000)
    except UcError:
        pass
    return mu.reg_read(UC_X86_REG_EIP), "".join(out), reads, mu


@unittest.skipUnless(HAVE_UNICORN and have_tools(), "нужны nasm/ld/objcopy/strip и unicorn")
class BootChainTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp()
        t = cls.tmp
        run(["nasm", "-f", "elf64", "-D", "KERNEL_SIZE_DWORDS=262144", "boot/stage2.asm", "-o", t + "/stage2.o"])
        run(["ld", "-n", "--gc-sections", "-T", "boot/linker_stage2.ld", "-o", t + "/stage2.elf", t + "/stage2.o"])
        run(["strip", "--strip-all", t + "/stage2.elf", "-o", t + "/s.elf"])
        run(["objcopy", "-O", "binary", t + "/s.elf", t + "/stage2.bin"])
        run(["nasm", "-f", "bin", "boot/ramboot.asm", "-o", t + "/ramboot.bin"])
        run(["nasm", "-f", "bin", "boot/boot_sector.asm", "-o", t + "/boot_sector.bin"])
        cls.kernel = os.urandom(913_000)
        with open(t + "/kernel.bin", "wb") as f:
            f.write(cls.kernel)
        shutil.copy(t + "/boot_sector.bin", t + "/disk.img")
        run(["python3", "tools/make_deix_fs.py", t + "/disk.img"])
        with open(t + "/disk.img", "rb") as f:
            cls.img = f.read()
        with open(t + "/stage2.bin", "rb") as f:
            cls.stage2 = f.read()

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def test_factory_image_boots_to_stage2(self):
        eip, out, reads, mu = run_boot(self.img)
        self.assertEqual(eip, STAGE2_ADDR, out)
        self.assertEqual(bytes(mu.mem_read(0x100000, len(self.kernel))), self.kernel)
        self.assertEqual(bytes(mu.mem_read(STAGE2_ADDR, len(self.stage2))), self.stage2)
        self.assertTrue(all(lba >= 4096 or lba == BOOTMAP_LBA for lba, _ in reads))

    def test_fragmented_files(self):
        img = bytearray(self.img)
        old = struct.unpack("<I", img[BOOTMAP_LBA * 512 + 8:BOOTMAP_LBA * 512 + 12])[0]
        for lba in range(old, old + 1800):
            img[lba * 512:(lba + 1) * 512] = b"\xEE" * 512
        kernel = self.kernel + b"\0" * (-len(self.kernel) % 512)
        sectors = len(kernel) // 512
        extents, pos, lba = [], 0, 30000
        for size in (300, 1, 127, 500, sectors):
            n = min(size, sectors - pos)
            if n <= 0:
                break
            img[lba * 512:(lba + n) * 512] = kernel[pos * 512:(pos + n) * 512]
            extents.append((lba, n))
            pos += n
            lba += n + 7
        m = bytearray(img[BOOTMAP_LBA * 512:(BOOTMAP_LBA + 1) * 512])
        m[12:16] = struct.pack("<I", len(self.kernel))
        m[192:512] = bytes(320)
        for i, (l, n) in enumerate(extents):
            m[192 + i * 8:200 + i * 8] = struct.pack("<II", l, n)
        img[BOOTMAP_LBA * 512:(BOOTMAP_LBA + 1) * 512] = m
        eip, out, _, mu = run_boot(bytes(img))
        self.assertEqual(eip, STAGE2_ADDR, out)
        self.assertEqual(bytes(mu.mem_read(0x100000, len(self.kernel))), self.kernel)

    def test_missing_map_stops_in_mbr(self):
        img = bytearray(self.img)
        img[BOOTMAP_LBA * 512] = 0
        eip, out, _, _ = run_boot(bytes(img))
        self.assertNotEqual(eip, STAGE2_ADDR)
        self.assertIn("Bad boot map", out)

    def test_short_kernel_extents_stop_in_ramboot(self):
        img = bytearray(self.img)
        base = BOOTMAP_LBA * 512 + 192
        img[base + 4:base + 8] = struct.pack("<I", 100)
        img[base + 8:base + 16] = bytes(8)
        eip, out, _, _ = run_boot(bytes(img))
        self.assertNotEqual(eip, STAGE2_ADDR)
        self.assertIn("RAMBOOT: bad boot map", out)


if __name__ == "__main__":
    unittest.main()
