#!/usr/bin/env python3
"""Собирает assets/logo.dxlg: сглаженное лого 256x256 из assets/logo.png для загрузочного экрана.

В отличие от tools/make_logo.py (5 цветов без сглаживания) палитра здесь
состоит из цветов лого, их смесей с чёрным (по 25/50/75 %) и
попарных смесей, поэтому края не ступенчатые. Нужен Pillow; результат
(assets/logo.dxlg) лежит в репозитории, и build.sh берёт его готовым.

    python3 tools/make_logo_aa.py assets/logo.png assets/logo.dxlg [размер]
"""
import itertools
import struct
import sys

from PIL import Image

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from make_logo import rle  # noqa: E402

# Цвета лого assets/logo.png: циан, синий, оранжевый акцент.
BASE = [(0x18, 0xFC, 0xF4), (0x0A, 0x58, 0xDE), (0xFF, 0x7D, 0x19)]
BLACK = (0, 0, 0)


def mix(a, b, t):
    return tuple(round(a[i] * (1 - t) + b[i] * t) for i in range(3))


def palette():
    cols = [BLACK] + BASE
    for c in BASE:
        cols += [mix(BLACK, c, t) for t in (0.25, 0.5, 0.75)]
    cols += [mix(a, b, 0.5) for a, b in itertools.combinations(BASE, 2)]
    return cols


def crop_square(img):
    """Квадрат по рамке содержимого с полями 8 %, чтобы лого занимало кадр."""
    box = img.point(lambda v: 255 if v > 40 else 0).convert("L").getbbox()
    if box is None:
        return img
    cx, cy = (box[0] + box[2]) / 2, (box[1] + box[3]) / 2
    half = max(box[2] - box[0], box[3] - box[1]) * 1.08 / 2
    return img.crop((round(cx - half), round(cy - half), round(cx + half), round(cy + half)))


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else "assets/logo.png"
    dst = sys.argv[2] if len(sys.argv) > 2 else "assets/logo.dxlg"
    size = int(sys.argv[3]) if len(sys.argv) > 3 else 256
    cols = palette()
    img = Image.open(src).convert("RGB")
    img = crop_square(img).resize((size, size), Image.Resampling.HAMMING)
    cache = {}

    def nearest(p):
        if p not in cache:
            if max(p) < 20:
                cache[p] = 0
            else:
                cache[p] = min(range(len(cols)),
                               key=lambda i: sum((p[k] - cols[i][k]) ** 2 for k in range(3)))
        return cache[p]

    raw = img.tobytes()
    indices = [nearest(tuple(raw[i:i + 3])) for i in range(0, size * size * 3, 3)]
    data = bytearray(b"DXLG" + struct.pack("<HH", size, size) + bytes([len(cols), 0, 0, 0]))
    for c in cols:
        data += bytes(c)
    data += rle(indices)
    with open(dst, "wb") as f:
        f.write(data)
    print("%s: %dx%d, %d colors, %d bytes" % (dst, size, size, len(cols), len(data)))


if __name__ == "__main__":
    main()
