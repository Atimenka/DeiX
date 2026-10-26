#!/usr/bin/env python3
"""Проверка медиа-ассетов после переноса тяжёлой среды в медиапак.

Обои и картинки БОЛЬШЕ не кладутся в /system (EROFS 16 МиБ) и не вшиваются
в пакет обновления: assets/wallpapers и assets/pictures — только ИСХОЧНИКИ
для tools/make_media_pack.py, который собирает web/public/media (его
форматы проверяет tools/test_media_pack.py). Здесь проверяется, что:
  * имена обоев в src/ui/theme.rs (.jpg) совпадают с исходниками
    assets/wallpapers (по основе имени, .bmp/.png);
  * исходники картинок и голосов на месте;
  * tools/make_deix_fs.py больше не копирует wallpapers/pictures в /system.
"""

import os
import re
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WALLPAPERS = os.path.join(ROOT, "assets", "wallpapers")
PICTURES = os.path.join(ROOT, "assets", "pictures")
VOICE = os.path.join(ROOT, "assets", "voice")


def theme_source():
    with open(os.path.join(ROOT, "src", "ui", "theme.rs"), encoding="utf-8") as f:
        return f.read()


def wallpaper_names():
    m = re.search(r"SYSTEM_WALLPAPER_FILES: \[&str; (\d+)\]\s*=\s*\[(.*?)\];", theme_source(), re.S)
    assert m, "theme.rs: SYSTEM_WALLPAPER_FILES не найден"
    names = re.findall(r'"([^"]+)"', m.group(2))
    assert len(names) == int(m.group(1)), "theme.rs: размер массива не сходится"
    return names


class WallpaperSources(unittest.TestCase):
    def test_names_are_jpeg_media_pack(self):
        for name in wallpaper_names():
            self.assertTrue(name.endswith(".jpg"), f"{name}: обои медиапака — .jpg")

    def test_sources_match_theme_list(self):
        stems_theme = {os.path.splitext(n)[0] for n in wallpaper_names()}
        stems_assets = {
            os.path.splitext(f)[0]
            for f in os.listdir(WALLPAPERS)
            if f.endswith((".bmp", ".png"))
        }
        self.assertEqual(stems_theme, stems_assets)

    def test_sources_are_images(self):
        for f in sorted(os.listdir(WALLPAPERS)):
            p = os.path.join(WALLPAPERS, f)
            with open(p, "rb") as fh:
                head = fh.read(8)
            if f.endswith(".bmp"):
                self.assertEqual(head[:2], b"BM", f)
            elif f.endswith(".png"):
                self.assertEqual(head, b"\x89PNG\r\n\x1a\n", f)
            else:
                self.fail(f"assets/wallpapers/{f}: не BMP и не PNG")

    def test_style_count_matches(self):
        src = theme_source()
        styles = re.search(r"WALLPAPER_STYLES: \[&str; (\d+)\]\s*=\s*\[(.*?)\];", src, re.S)
        first = re.search(r"SYSTEM_WALLPAPER_FIRST: usize = (\d+);", src)
        listed = re.findall(r'"([^"]+)"', styles.group(2))
        self.assertEqual(len(listed), int(styles.group(1)))
        self.assertEqual(int(first.group(1)) + len(wallpaper_names()), len(listed))


class OtherAssets(unittest.TestCase):
    def test_pictures_present(self):
        files = [f for f in os.listdir(PICTURES) if f.endswith((".png", ".jpg"))]
        self.assertGreaterEqual(len(files), 1)

    def test_voice_present(self):
        files = [f for f in os.listdir(VOICE) if f.endswith(".wav")]
        self.assertGreaterEqual(len(files), 1)


class SystemImageSlim(unittest.TestCase):
    def test_make_deix_fs_has_no_wallpapers(self):
        """/system несёт только критичное: загрузчик, etc/, UI-звуки, иконки."""
        with open(os.path.join(ROOT, "tools", "make_deix_fs.py"), encoding="utf-8") as f:
            src = f.read()
        self.assertNotIn("media/wallpapers", src)
        self.assertNotIn("media/pictures", src)
        self.assertIn("media/audio/ui", src)
        self.assertIn("media/icons", src)


if __name__ == "__main__":
    unittest.main(verbosity=2)
