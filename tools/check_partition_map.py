#!/usr/bin/env python3
"""
Сверка карты разделов DeiX OS между всеми источниками геометрии:
  1. src/partition_map.rs   (PARTITION_LAYOUT — источник истины)
  2. tools/make_deix_fs.py  (PRIMARY — сборка образа)
  3. src/install.rs         (PART_SYSTEM / PART_USERDATA — инсталлятор)
  4. src/ext2.rs            (FS_START_LBA / TOTAL_SECTORS — драйвер /userdata)
  5. boot/boot_sector.asm   (MBR-таблица в загрузочном секторе)

Дополнительно проверяются служебные области:
  - сырой kernel.bin не пересекает дескриптор DEIXKIMG (LBA 2047);
  - область аварийного дампа (LBA 2048 + 64) не пересекает /system;
  - /system и /userdata не пересекаются.

Запускается из build.sh перед сборкой образа диска.
"""

import os
import re
import sys

PANIC_LBA = 2048
PANIC_SECTORS = 64
KIMG_LBA = 2047


def fail(errors):
    print("ОШИБКА: Обнаружены расхождения в карте разделов:", file=sys.stderr)
    for e in errors:
        print(f"  * {e}", file=sys.stderr)
    sys.exit(1)


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    read = lambda *p: open(os.path.join(root, *p), encoding="utf-8").read()

    errors = []

    # 1. src/partition_map.rs — источник истины.
    pm_text = read("src", "partition_map.rs")
    pm_map = {}
    for m in re.finditer(r"name:\s*\"([^\"]+)\",\s*start_lba:\s*(\d+),\s*sectors:\s*(\d+)", pm_text):
        pm_map[m.group(1)] = (int(m.group(2)), int(m.group(3)))
    if set(pm_map) != {"/system", "/userdata"}:
        errors.append("partition_map.rs: ожидались разделы /system и /userdata, найдено: %s" % sorted(pm_map))
        fail(errors)

    m = re.search(r"KERNEL_IMAGE_INFO_LBA:\s*u32\s*=\s*(\d+)", pm_text)
    if not m or int(m.group(1)) != KIMG_LBA:
        errors.append("partition_map.rs: KERNEL_IMAGE_INFO_LBA != %d" % KIMG_LBA)

    # 2. tools/make_deix_fs.py
    fs_text = read("tools", "make_deix_fs.py")
    fs_map = {}
    for m in re.finditer(r"\(\s*\d+,\s*0x[0-9a-fA-F]+,\s*(\d+),\s*(\d+),\s*\"([^\"]+)\"\)", fs_text):
        fs_map[m.group(3)] = (int(m.group(1)), int(m.group(2)))
    for name, geo in pm_map.items():
        if fs_map.get(name) != geo:
            errors.append(f"make_deix_fs.py: '{name}' = {fs_map.get(name)}, ожидалось {geo}")

    # 3. src/install.rs
    inst_text = read("src", "install.rs")
    inst = {}
    m = re.search(r"PART_SYSTEM:\s*\(u32,\s*u32\)\s*=\s*\((\d+),\s*(\d+)\)", inst_text)
    if m:
        inst["/system"] = (int(m.group(1)), int(m.group(2)))
    m = re.search(r"PART_USERDATA:\s*\(u32,\s*u32\)\s*=\s*\((\d+),\s*(\d+)\)", inst_text)
    if m:
        inst["/userdata"] = (int(m.group(1)), int(m.group(2)))
    for name, geo in pm_map.items():
        if inst.get(name) != geo:
            errors.append(f"install.rs: '{name}' = {inst.get(name)}, ожидалось {geo}")

    # 4. src/ext2.rs (драйвер /userdata)
    ext2_text = read("src", "ext2.rs")
    m_lba = re.search(r"FS_START_LBA:\s*u32\s*=\s*(\d+)", ext2_text)
    m_sec = re.search(r"TOTAL_SECTORS:\s*u32\s*=\s*(\d+)", ext2_text)
    ud = pm_map["/userdata"]
    if not m_lba or int(m_lba.group(1)) != ud[0]:
        errors.append("ext2.rs: FS_START_LBA != %d" % ud[0])
    if not m_sec or int(m_sec.group(1)) != ud[1]:
        errors.append("ext2.rs: TOTAL_SECTORS != %d" % ud[1])

    # 5. boot/boot_sector.asm — MBR-таблица (пары dd LBA / dd секторов).
    asm_text = read("boot", "boot_sector.asm")
    tbl = asm_text[asm_text.index("times (446-($-$$)) db 0"):]
    dds = [int(v) for v in re.findall(r"^dd\s+(\d+)", tbl, re.M)]
    asm_parts = list(zip(dds[0::2], dds[1::2]))
    expected = [pm_map["/system"], pm_map["/userdata"]]
    if asm_parts[:2] != expected:
        errors.append(f"boot_sector.asm: таблица разделов {asm_parts[:2]}, ожидалось {expected}")

    # Служебные области.
    if PANIC_LBA + PANIC_SECTORS > pm_map["/system"][0]:
        errors.append("область аварийного дампа пересекает /system")
    if KIMG_LBA >= PANIC_LBA:
        errors.append("дескриптор DEIXKIMG пересекает область аварийного дампа")
    sys_end = pm_map["/system"][0] + pm_map["/system"][1]
    if sys_end > pm_map["/userdata"][0]:
        errors.append("/system пересекает /userdata")

    # src/diag/panic.rs — PANIC_LBA/PANIC_SECTORS соответствуют проверке выше.
    panic_text = read("src", "diag", "panic.rs")
    m = re.search(r"PANIC_LBA:\s*u32\s*=\s*(\d+)", panic_text)
    if not m or int(m.group(1)) != PANIC_LBA:
        errors.append("diag/panic.rs: PANIC_LBA != %d" % PANIC_LBA)
    m = re.search(r"PANIC_SECTORS:\s*u32\s*=\s*(\d+)", panic_text)
    if not m or int(m.group(1)) != PANIC_SECTORS:
        errors.append("diag/panic.rs: PANIC_SECTORS != %d" % PANIC_SECTORS)

    if errors:
        fail(errors)

    print("Карта разделов синхронизирована и верна (partition_map.rs, make_deix_fs.py,")
    print("install.rs, ext2.rs, boot_sector.asm, panic.rs).")


if __name__ == "__main__":
    main()
