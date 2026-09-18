#!/usr/bin/env python3
"""Конвертирует PNG-лого в компактный формат для загрузочного экрана DeiX.

Зачем свой формат: декодера PNG в ядре нет, а сырой RGB не помещается —
kernel.bin физически ограничен 572 КиБ (буфер загрузчика упирается в
видеопамять VGA, см. boot/stage2.asm).

Формат deixlogo (little-endian):

    0   4   магия "DXLG"
    4   2   ширина
    6   2   высота
    8   1   число цветов палитры N
    9   3   резерв
    12  N*3 палитра RGB
    ..      RLE-поток: пары [count 1..255][индекс цвета]

Плоская картинка из 5 цветов ужимается в ~4 КиБ на 256x256.

Использование:
    python3 tools/make_logo.py logo.png build/logo.dxlg [размер]
"""
import struct
import sys

try:
    from PIL import Image
except ImportError:
    sys.exit("нужен Pillow: pip install pillow")

MAGIC = b"DXLG"

# Палитра подобрана под исходное лого: фон, голубой, синий, белый, оранжевый.
PALETTE = [
    (0x00, 0x00, 0x00),
    (0x00, 0xD0, 0xFF),
    (0x00, 0x60, 0xC0),
    (0xFF, 0xFF, 0xFF),
    (0xFF, 0x7A, 0x1A),
]


def nearest(px):
    r, g, b = px[:3]
    best, bi = 1 << 30, 0
    for i, (pr, pg, pb) in enumerate(PALETTE):
        d = (r - pr) ** 2 + (g - pg) ** 2 + (b - pb) ** 2
        if d < best:
            best, bi = d, i
    return bi


def rle(indices):
    out = bytearray()
    i, n = 0, len(indices)
    while i < n:
        v = indices[i]
        run = 1
        while i + run < n and indices[i + run] == v and run < 255:
            run += 1
        out += bytes((run, v))
        i += run
    return bytes(out)


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    src, dst = sys.argv[1], sys.argv[2]
    size = int(sys.argv[3]) if len(sys.argv) > 3 else 256

    im = Image.open(src).convert("RGB")
    # LANCZOS сглаживает края, но следом мы всё равно сводим к палитре,
    # поэтому полутонов не остаётся — зато форма получается ровнее,
    # чем при NEAREST.
    im = im.resize((size, size), Image.LANCZOS)

    idx = [nearest(p) for p in list(im.getdata())]
    body = rle(idx)

    out = bytearray()
    out += MAGIC
    out += struct.pack("<HH", size, size)
    out += bytes((len(PALETTE),))
    out += b"\x00\x00\x00"
    for r, g, b in PALETTE:
        out += bytes((r, g, b))
    out += body

    with open(dst, "wb") as f:
        f.write(out)

    raw = size * size * 3
    print("%s: %dx%d, %d цв., %d Б (%.1f КиБ)" %
          (dst, size, size, len(PALETTE), len(out), len(out) / 1024))
    print("  сырой RGB был бы %d КиБ — сжатие в %.0f раз" %
          (raw // 1024, raw / len(out)))


if __name__ == "__main__":
    main()
