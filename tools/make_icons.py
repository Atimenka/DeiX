#!/usr/bin/env python3
"""Конвертер иконок приложений: PNG -> DXIC (RGBA) для /system/media/icons.

Формат DXIC: "DXIC" | u16 LE ширина | u16 LE высота | RGBA, строка за строкой.
Исходник обрезается по границе содержимого (белые поля убираются), приводится
к квадрату и получает скруглённую маску с антиалиасингом.

    python3 tools/make_icons.py assets/icons/src assets/icons [размер]

Нужен Pillow.
"""
import struct
import sys
from pathlib import Path

from PIL import Image, ImageDraw

NAMES = ["terminal", "files", "browser", "taskmgr", "errors",
         "personalize", "display", "install", "updates", "about"]
SS = 4  # суперсэмплинг маски


def content_square(im: Image.Image) -> Image.Image:
    rgba = im.convert("RGBA")
    bg = Image.new("RGBA", rgba.size, (0, 0, 0, 255))
    bg.alpha_composite(rgba)
    rgb = bg.convert("RGB")
    w, h = rgb.size
    px = rgb.load()
    cols, rows = [], []
    for y in range(0, h, 4):
        for x in range(0, w, 4):
            r, g, b = px[x, y]
            if not (r > 235 and g > 235 and b > 235):
                cols.append(x)
                rows.append(y)
    x0, x1, y0, y1 = min(cols), max(cols), min(rows), max(rows)
    side = min(x1 - x0, y1 - y0)
    inset = int(side * 0.035)
    x0 += inset
    y0 += inset
    side -= 2 * inset
    return rgb.crop((x0, y0, x0 + side, y0 + side))


def squircle_mask(size: int, radius: float) -> Image.Image:
    big = size * SS
    m = Image.new("L", (big, big), 0)
    ImageDraw.Draw(m).rounded_rectangle((0, 0, big - 1, big - 1), radius=radius * big, fill=255)
    return m.resize((size, size), Image.LANCZOS)


def convert(src: Path, size: int) -> bytes:
    im = content_square(Image.open(src)).resize((size, size), Image.LANCZOS).convert("RGBA")
    im.putalpha(squircle_mask(size, 0.22))
    return b"DXIC" + struct.pack("<HH", size, size) + im.tobytes()


def main() -> None:
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    src_dir, out_dir = Path(sys.argv[1]), Path(sys.argv[2])
    size = int(sys.argv[3]) if len(sys.argv) > 3 else 48
    out_dir.mkdir(parents=True, exist_ok=True)
    for name in NAMES:
        data = convert(src_dir / f"{name}.png", size)
        (out_dir / f"{name}.dxic").write_bytes(data)
        print(f"{name}.dxic  {len(data)} bytes")


if __name__ == "__main__":
    main()
