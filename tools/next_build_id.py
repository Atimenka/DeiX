#!/usr/bin/env python3
"""Монотонный build id для центра обновлений DeiX OS.

Состояние — файл build/last_build_id (или путь в DEIX_BUILD_ID_STATE).
Без аргументов печатает следующий id: max(время unix, последний + 1) —
каждая сборка строго новее предыдущей, даже если коммит и секунда те же.
С --mark N запоминает max(последний, N): явно заданные id тоже двигают
счётчик, чтобы следующая автосборка не оказалась старее вручную указанной.
"""

import os
import sys
import time


def state_path():
    env = os.environ.get("DEIX_BUILD_ID_STATE")
    if env:
        return env
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(root, "build", "last_build_id")


def read_last(path):
    try:
        with open(path, encoding="utf-8") as f:
            return int(f.read().strip() or 0)
    except (OSError, ValueError):
        return 0


def main():
    path = state_path()
    if len(sys.argv) == 3 and sys.argv[1] == "--mark":
        new = max(read_last(path), int(sys.argv[2]))
    elif len(sys.argv) == 1:
        new = max(int(time.time()), read_last(path) + 1)
    else:
        print(__doc__)
        raise SystemExit(2)
    parent = os.path.dirname(path)
    if parent:
        os.makedirs(parent, exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write("%d\n" % new)
    if len(sys.argv) == 1:
        print(new)


if __name__ == "__main__":
    main()
