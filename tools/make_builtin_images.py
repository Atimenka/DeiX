#!/usr/bin/env python3
"""Готовит встроенные картинки ОС из исходников <имя>.png.

  assets/wallpapers/<имя>.png — обои 640x480, палитра 256 цветов с дизерингом
                                 (ядро декодирует PNG тип 3, src/png.rs);
  assets/pictures/<имя>.png   — картинки в /system/media/pictures, тот же формат.

Использование: python3 tools/make_builtin_images.py <каталог с исходниками>
Нужен Pillow.
"""

import os
import sys

from PIL import Image, ImageOps

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SIZE = (640, 480)
WALLPAPERS = ["savanna", "glacier", "canyon", "reef", "galaxy", "mono", "lavender"]
PICTURES = {"pic-desk": "desk", "pic-boat": "boat", "pic-fox": "fox"}


def convert(src, dst):
    img = Image.open(src).convert("RGB")
    img = ImageOps.fit(img, SIZE, Image.LANCZOS)
    pal = img.quantize(256, method=Image.MEDIANCUT, dither=Image.FLOYDSTEINBERG)
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    pal.save(dst, optimize=True)
    return os.path.getsize(dst)


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        raise SystemExit(2)
    src_dir = sys.argv[1]
    for name in WALLPAPERS:
        size = convert(os.path.join(src_dir, name + ".png"),
                       os.path.join(ROOT, "assets", "wallpapers", name + ".png"))
        print("wallpaper %-9s %7d B" % (name, size))
    for src_name, name in PICTURES.items():
        size = convert(os.path.join(src_dir, src_name + ".png"),
                       os.path.join(ROOT, "assets", "pictures", name + ".png"))
        print("picture   %-9s %7d B" % (name, size))


if __name__ == "__main__":
    main()
