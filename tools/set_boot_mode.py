#!/usr/bin/env python3
"""Ставит флаг режима загрузки (BCB) прямо в образе диска — СНАРУЖИ.

Зачем: если системные разделы стёрты, ядро останавливается на проверке
цепочки загрузки и до CLI дело не доходит — команду `reboot dsm` ввести
негде. Флаг BCB лежит в обычном секторе диска, поэтому его можно
записать хост-утилитой, не запуская DeiX.

Формат сектора BCB (LBA 3000, см. src/bcb.rs):

    offset  размер  содержимое
    0       8       магия "DEIXBCB1"
    8       4       boot_mode  (0=normal, 1=recovery, 2=fastbootd, 3=dsm)
    12      4       активный A/B-слот (0=a, 1=b)
    16      4       ota_pending
    20      492     0xFF

Важно: флаг ОДНОРАЗОВЫЙ. `bcb::boot_flow()` вызывает `clear_boot_mode()`
сразу после чтения, поэтому следующая загрузка снова пойдёт обычным
путём. Это штатное поведение (как у Android BCB), а не потеря настройки.

Примеры:
    python3 tools/set_boot_mode.py build/deix_disk.img dsm
    python3 tools/set_boot_mode.py build/deix_disk.img fastbootd
    python3 tools/set_boot_mode.py build/deix_disk.img show
    python3 tools/set_boot_mode.py build/deix_disk.img slot b
"""

import os
import struct
import sys

SECTOR = 512
BCB_LBA = 3000
BCB_OFFSET = BCB_LBA * SECTOR
MAGIC = b"DEIXBCB1"

MODES = {"normal": 0, "recovery": 1, "fastbootd": 2, "dsm": 3}
MODE_NAMES = {v: k for k, v in MODES.items()}


def read_bcb(path):
    """Возвращает (mode, slot, ota_pending) или None, если BCB не размечен."""
    size = os.path.getsize(path)
    if size < BCB_OFFSET + SECTOR:
        raise SystemExit(
            "ОШИБКА: образ %d байт — короче сектора BCB (нужно минимум %d)"
            % (size, BCB_OFFSET + SECTOR)
        )
    with open(path, "rb") as f:
        f.seek(BCB_OFFSET)
        sec = f.read(SECTOR)
    if sec[:8] != MAGIC:
        return None
    mode, slot, ota = struct.unpack_from("<III", sec, 8)
    return mode, slot, ota


def write_bcb(path, mode=None, slot=None):
    """Пишет режим и/или слот, сохраняя остальные поля."""
    cur = read_bcb(path)
    if cur is None:
        # BCB ещё не размечен — создаём с нуля.
        cur_mode, cur_slot, cur_ota = 0, 0, 0
    else:
        cur_mode, cur_slot, cur_ota = cur

    new_mode = cur_mode if mode is None else mode
    new_slot = cur_slot if slot is None else slot

    sec = bytearray(b"\xff" * SECTOR)
    sec[0:8] = MAGIC
    struct.pack_into("<III", sec, 8, new_mode, new_slot & 1, cur_ota)

    with open(path, "r+b") as f:
        f.seek(BCB_OFFSET)
        f.write(sec)
        f.flush()
        os.fsync(f.fileno())

    return new_mode, new_slot & 1


def show(path):
    cur = read_bcb(path)
    if cur is None:
        print("BCB не размечен (магия отсутствует) -> загрузка пойдёт как normal")
        return
    mode, slot, ota = cur
    print("BCB (LBA %d):" % BCB_LBA)
    print("  режим загрузки : %s (%d)" % (MODE_NAMES.get(mode, "неизвестно"), mode))
    print("  активный слот  : %s (%d)" % ("a" if slot == 0 else "b", slot))
    print("  ota_pending    : %d" % ota)


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        raise SystemExit(1)

    path, cmd = sys.argv[1], sys.argv[2].lower()

    if not os.path.exists(path):
        raise SystemExit("ОШИБКА: файл '%s' не найден" % path)

    if cmd == "show":
        show(path)
        return

    if cmd == "slot":
        if len(sys.argv) < 4 or sys.argv[3] not in ("a", "b"):
            raise SystemExit("использование: ... slot <a|b>")
        _, s = write_bcb(path, slot=0 if sys.argv[3] == "a" else 1)
        print("Активный слот -> %s" % ("a" if s == 0 else "b"))
        return

    if cmd not in MODES:
        raise SystemExit(
            "неизвестный режим '%s'. Доступно: %s, slot, show"
            % (cmd, ", ".join(MODES))
        )

    m, s = write_bcb(path, mode=MODES[cmd])
    print("Режим следующей загрузки -> %s (слот %s)" % (MODE_NAMES[m], "a" if s == 0 else "b"))
    if cmd != "normal":
        print("Флаг одноразовый: ядро сбросит его сразу после входа в режим.")


if __name__ == "__main__":
    main()
