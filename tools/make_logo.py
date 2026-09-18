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

Поддерживает Pillow (если установлен) или встроенный чистый декодер PNG (zlib).

Использование:
    python3 tools/make_logo.py logo.png build/logo.dxlg [размер]
"""
import struct
import sys
import zlib

MAGIC = b"DXLG"

# Палитра подобрана под исходное лого: фон, голубой, синий, белый, оранжевый.
PALETTE = [
    (0x00, 0x00, 0x00), # 0: фон (прозрачный)
    (0x00, 0xD0, 0xFF), # 1: циан
    (0x00, 0x60, 0xC0), # 2: синий
    (0xFF, 0xFF, 0xFF), # 3: белый
    (0xFF, 0x7A, 0x1A), # 4: оранжевый
]


def nearest(px):
    r, g, b = px[0], px[1], px[2]
    if r < 18 and g < 18 and b < 18:
        return 0
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


def decode_png_pure(src, target_size):
    """Декодирует PNG без внешних библиотек (через стандартный zlib)."""
    with open(src, "rb") as f:
        sig = f.read(8)
        if sig != b"\x89PNG\r\n\x1a\n":
            raise ValueError("Файл не является PNG")
        idat = bytearray()
        w, h = 0, 0
        bit_depth, color_type = 0, 0
        while True:
            hdr = f.read(8)
            if not hdr or len(hdr) < 8:
                break
            length, chunk_type = struct.unpack(">I4s", hdr)
            chunk_data = f.read(length)
            f.read(4)  # crc
            if chunk_type == b"IHDR":
                w, h, bit_depth, color_type, comp, filt, interlace = struct.unpack(
                    ">IIBBBBB", chunk_data
                )
                if bit_depth != 8 or comp != 0 or filt != 0 or interlace != 0:
                    raise ValueError("Неподдерживаемые параметры PNG")
            elif chunk_type == b"IDAT":
                idat.extend(chunk_data)
            elif chunk_type == b"IEND":
                break

    decompressed = zlib.decompress(bytes(idat))
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(color_type)
    if not channels:
        raise ValueError(f"Неподдерживаемый color_type: {color_type}")

    stride = 1 + w * channels
    prev_row = bytearray(w * channels)
    curr_row = bytearray(w * channels)

    def paeth(a, b, c):
        p = a + b - c
        pa = abs(p - a)
        pb = abs(p - b)
        pc = abs(p - c)
        if pa <= pb and pa <= pc:
            return a
        if pb <= pc:
            return b
        return c

    all_rows = []
    for y in range(h):
        f_type = decompressed[y * stride]
        line = decompressed[y * stride + 1 : (y + 1) * stride]
        for x in range(w * channels):
            filt = line[x]
            a = curr_row[x - channels] if x >= channels else 0
            b = prev_row[x]
            c = prev_row[x - channels] if x >= channels else 0
            if f_type == 0:
                v = filt
            elif f_type == 1:
                v = (filt + a) & 0xFF
            elif f_type == 2:
                v = (filt + b) & 0xFF
            elif f_type == 3:
                v = (filt + ((a + b) >> 1)) & 0xFF
            elif f_type == 4:
                v = (filt + paeth(a, b, c)) & 0xFF
            curr_row[x] = v
        prev_row[:] = curr_row
        all_rows.append(bytes(curr_row))

    # Сэмплируем до target_size x target_size
    idx = []
    for ty in range(target_size):
        sy = int(ty * h / target_size)
        row = all_rows[sy]
        for tx in range(target_size):
            sx = int(tx * w / target_size)
            px_idx = sx * channels
            if channels == 1:
                r = g = b = row[px_idx]
            else:
                r, g, b = row[px_idx], row[px_idx + 1], row[px_idx + 2]
            idx.append(nearest((r, g, b)))
    return idx


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    src, dst = sys.argv[1], sys.argv[2]
    size = int(sys.argv[3]) if len(sys.argv) > 3 else 128

    has_pil = False
    try:
        from PIL import Image

        has_pil = True
    except ImportError:
        pass

    if has_pil:
        im = Image.open(src).convert("RGB")
        im = im.resize((size, size), Image.LANCZOS)
        idx = [nearest(p) for p in list(im.getdata())]
    else:
        idx = decode_png_pure(src, size)

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
    print(
        "%s: %dx%d, %d цв., %d Б (%.1f КиБ)"
        % (dst, size, size, len(PALETTE), len(out), len(out) / 1024)
    )
    print("  сырой RGB был бы %d КиБ — сжатие в %.0f раз" % (raw // 1024, raw / len(out)))


if __name__ == "__main__":
    main()
