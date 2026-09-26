#!/usr/bin/env python3
"""
Скрипт сверки карты разделов DeiX OS между источниками:
  1. src/partition_map.rs (PARTITION_LAYOUT)
  2. tools/make_deix_fs.py (PRIMARY + LOGICALS)

Запускается из build.sh перед сборкой образа диска для предотвращения
рассинхронизации LBA и затирания разделов.
"""

import os
import re
import sys

def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    pm_path = os.path.join(root, "src", "partition_map.rs")
    fs_path = os.path.join(root, "tools", "make_deix_fs.py")

    errors = []

    # 1. Читаем src/partition_map.rs
    with open(pm_path, "r", encoding="utf-8") as f:
        pm_text = f.read()

    pm_map = {}
    pattern_pm = r"name:\s*\"([^\"]+)\",\s*start_lba:\s*(\d+),\s*sectors:\s*(\d+)"
    for m in re.finditer(pattern_pm, pm_text):
        name = m.group(1)
        lba = int(m.group(2))
        secs = int(m.group(3))
        pm_map[name] = (lba, secs)

    if not pm_map:
        errors.append(f"Не удалось распарсить PARTITION_LAYOUT в {pm_path}")

    # 2. Читаем tools/make_deix_fs.py
    with open(fs_path, "r", encoding="utf-8") as f:
        fs_text = f.read()

    fs_map = {}
    pattern_fs = r"\(\s*\d+,\s*0x[0-9a-fA-F]+,\s*(\d+),\s*(\d+),\s*\"([^\"]+)\"\)"
    for m in re.finditer(pattern_fs, fs_text):
        lba = int(m.group(1))
        secs = int(m.group(2))
        name = m.group(3)
        fs_map[name] = (lba, secs)

    if not fs_map:
        errors.append(f"Не удалось распарсить PRIMARY/LOGICALS в {fs_path}")

    # Сверка partition_map.rs <-> make_deix_fs.py
    for name, (lba, secs) in fs_map.items():
        if name not in pm_map:
            errors.append(f"[make_deix_fs] Раздел '{name}' отсутствует в src/partition_map.rs")
        elif pm_map[name] != (lba, secs):
            pm_lba, pm_secs = pm_map[name]
            errors.append(
                f"Несовпадение для '{name}': make_deix_fs=(LBA {lba}, {secs} сект) vs "
                f"partition_map=(LBA {pm_lba}, {pm_secs} сект)"
            )

    for name, (lba, secs) in pm_map.items():
        if name not in fs_map:
            errors.append(f"[partition_map] Раздел '{name}' отсутствует в tools/make_deix_fs.py")

    if errors:
        print("ОШИБКА: Обнаружены расхождения в карте разделов:", file=sys.stderr)
        for e in errors:
            print(f"  * {e}", file=sys.stderr)
        sys.exit(1)

    print("Карта разделов синхронизирована и верна.")

if __name__ == "__main__":
    main()
