#!/usr/bin/env python3
"""Проверка встроенных обоев: список в src/ui/theme.rs совпадает с файлами в
assets/wallpapers, каждый файл — несжатый BMP 24 бит 640x480, а сжатые обои
оставляют запас в пакете обновления (tools/make_update.py: 6 МиБ)."""

import gzip
import os
import re
import struct
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WALLPAPERS = os.path.join(ROOT, "assets", "wallpapers")
PAYLOAD_LIMIT = 6 * 1024 * 1024
WALLPAPER_BUDGET = 4 * 1024 * 1024


def theme_source():
    with open(os.path.join(ROOT, "src", "ui", "theme.rs"), encoding="utf-8") as f:
        return f.read()


class WallpaperAssets(unittest.TestCase):
    def test_files_match_theme_list(self):
        src = theme_source()
        m = re.search(r"SYSTEM_WALLPAPER_FILES: \[&str; (\d+)\]\s*=\s*\[(.*?)\];", src, re.S)
        self.assertIsNotNone(m)
        names = re.findall(r'"([^"]+\.bmp)"', m.group(2))
        self.assertEqual(len(names), int(m.group(1)))
        self.assertEqual(sorted(names), sorted(os.listdir(WALLPAPERS)))

    def test_style_count_matches(self):
        src = theme_source()
        styles = re.search(r"WALLPAPER_STYLES: \[&str; (\d+)\]\s*=\s*\[(.*?)\];", src, re.S)
        files = re.search(r"SYSTEM_WALLPAPER_FILES: \[&str; (\d+)\]", src)
        first = re.search(r"SYSTEM_WALLPAPER_FIRST: usize = (\d+);", src)
        listed = re.findall(r'"([^"]+)"', styles.group(2))
        self.assertEqual(len(listed), int(styles.group(1)))
        self.assertEqual(int(first.group(1)) + int(files.group(1)), len(listed))
        self.assertTrue(all(len(n) <= 8 for n in listed))

    def test_bmp_format(self):
        for name in os.listdir(WALLPAPERS):
            with open(os.path.join(WALLPAPERS, name), "rb") as f:
                data = f.read()
            self.assertEqual(data[:2], b"BM", name)
            offset, = struct.unpack_from("<I", data, 10)
            width, height = struct.unpack_from("<ii", data, 18)
            bpp, = struct.unpack_from("<H", data, 28)
            compression, = struct.unpack_from("<I", data, 30)
            self.assertEqual((width, abs(height), bpp, compression), (640, 480, 24, 0), name)
            self.assertGreaterEqual(len(data), offset + 640 * 3 * 480, name)

    def test_payload_budget(self):
        total = 0
        for name in os.listdir(WALLPAPERS):
            with open(os.path.join(WALLPAPERS, name), "rb") as f:
                total += len(gzip.compress(f.read(), 6))
        self.assertLess(total, WALLPAPER_BUDGET, "обои съедают пакет обновления")
        self.assertLess(total, PAYLOAD_LIMIT)


if __name__ == "__main__":
    unittest.main()
