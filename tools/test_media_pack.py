#!/usr/bin/env python3
"""Проверка медиапака (web/public/media) на совместимость с декодерами ОС.

Повторяет ограничения клиентской части — src/mediapack.rs (манифест),
src/jpeg.rs (baseline JPEG, 8 бит, ≤ 1 Мпикс), src/media.rs (DXV1, WAV) —
чтобы собранный tools/make_media_pack.py пак ОС могла установить и
воспроизвести. Запуск: python3 tools/test_media_pack.py
"""

import hashlib
import os
import re
import struct
import unittest
import wave

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MEDIA = os.path.join(ROOT, "web", "public", "media")
MAX_FILE_BYTES = 4 * 1024 * 1024        # src/mediapack.rs
MAX_JPEG_PIXELS = 1024 * 1024           # src/jpeg.rs
REL_PATH_RE = re.compile(r"^[a-z0-9._/-]+$")


def theme_wallpaper_files():
    with open(os.path.join(ROOT, "src", "ui", "theme.rs"), encoding="utf-8") as f:
        src = f.read()
    m = re.search(r"SYSTEM_WALLPAPER_FILES: \[&str; (\d+)\]\s*=\s*\[(.*?)\];", src, re.S)
    assert m, "theme.rs: SYSTEM_WALLPAPER_FILES не найден"
    names = re.findall(r'"([^"]+)"', m.group(2))
    assert len(names) == int(m.group(1)), "theme.rs: размер массива не сходится"
    return names


def parse_manifest():
    """Парсер строго как parse_manifest в src/mediapack.rs."""
    path = os.path.join(MEDIA, "manifest.txt")
    version = None
    entries = []
    with open(path, encoding="utf-8") as f:
        for raw in f:
            line = raw.strip()
            if not line:
                continue
            if line.startswith("#version "):
                version = line[len("#version "):].strip()
                continue
            if line.startswith("#"):
                continue
            parts = line.split("|")
            if len(parts) != 3:
                raise AssertionError(f"строка манифеста не path|size|sha256: {line!r}")
            rel, size, sha = parts
            size = int(size)
            sha = sha.lower()
            assert REL_PATH_RE.match(rel) and not rel.startswith("/") and "//" not in rel, rel
            assert re.fullmatch(r"[0-9a-f]{64}", sha), rel
            assert 0 < size <= MAX_FILE_BYTES, f"{rel}: размер {size}"
            entries.append((rel, size, sha))
    assert version, "в манифесте нет '#version'"
    assert entries, "манифест пуст"
    return version, entries


class MediaPack(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.version, cls.entries = parse_manifest()

    def test_manifest_matches_disk(self):
        """Манифест == фактические файлы: размеры и SHA-256 совпадают,
        лишних файлов на диске нет (иначе ОС их никогда не удалит)."""
        for rel, size, sha in self.entries:
            p = os.path.join(MEDIA, rel)
            self.assertTrue(os.path.isfile(p), f"{rel}: файла нет")
            with open(p, "rb") as fh:
                data = fh.read()
            self.assertEqual(len(data), size, rel)
            self.assertEqual(hashlib.sha256(data).hexdigest(), sha, rel)
        on_disk = set()
        for base, _dirs, files in os.walk(MEDIA):
            for f in files:
                if f == "manifest.txt":
                    continue
                rel = os.path.relpath(os.path.join(base, f), MEDIA).replace(os.sep, "/")
                on_disk.add(rel)
        self.assertEqual(on_disk, {rel for rel, _s, _h in self.entries})

    def test_wallpapers_match_theme(self):
        """Каждые обои из theme.rs есть в паке и это JPEG."""
        names = theme_wallpaper_files()
        packed = {rel for rel, _s, _h in self.entries}
        for name in names:
            self.assertIn(f"wallpapers/{name}", packed)
            self.assertTrue(name.endswith(".jpg"), name)

    def test_jpeg_files_are_baseline(self):
        """src/jpeg.rs декодирует только baseline (SOF0), 8 бит,
        1 или 3 компонента, факторы дискретизации 1..4, ≤ 1 Мпикс."""
        for rel, _size, _sha in self.entries:
            if not rel.endswith(".jpg"):
                continue
            with open(os.path.join(MEDIA, rel), "rb") as fh:
                d = fh.read()
            self.assertEqual(d[:2], b"\xff\xd8", rel)
            i, sof = 2, None
            while i < len(d) - 1:
                if d[i] != 0xFF:
                    i += 1
                    continue
                m = d[i + 1]
                if m in (0xD8, 0x01) or 0xD0 <= m <= 0xD7:
                    i += 2
                    continue
                if m == 0xDA:
                    break
                ln = struct.unpack(">H", d[i + 2:i + 4])[0]
                if 0xC0 <= m <= 0xCF and m not in (0xC4, 0xC8, 0xCC):
                    sof = (m, d[i + 4], struct.unpack(">HH", d[i + 5:i + 9]), d[i + 9], d[i + 10:i + 4 + ln])
                    break
                i += 2 + ln
            self.assertIsNotNone(sof, f"{rel}: SOF не найден")
            marker, prec, (h, w), nf, comps = sof
            self.assertEqual(marker, 0xC0, f"{rel}: не baseline (SOF{marker - 0xC0})")
            self.assertEqual(prec, 8, f"{rel}: точность {prec} != 8")
            self.assertIn(nf, (1, 3), f"{rel}: {nf} компонентов")
            self.assertLessEqual(w * h, MAX_JPEG_PIXELS, f"{rel}: {w}x{h}")
            for c in range(nf):
                hv = comps[3 * c + 1]
                self.assertTrue(1 <= (hv >> 4) <= 4 and 1 <= (hv & 0xF) <= 4, rel)

    def test_dxv_container(self):
        """DXV1-файлы: заголовок 20 байт и куски 'V'/'A' до конца файла,
        число кадров совпадает с заявленным (src/media.rs play_dxv)."""
        for rel, _size, _sha in self.entries:
            if not rel.endswith(".dxv"):
                continue
            d = open(os.path.join(MEDIA, rel), "rb").read()
            self.assertEqual(d[:4], b"DXV1", rel)
            fps = struct.unpack("<H", d[4:6])[0]
            fmt, afmt = d[10], d[11]
            arate, frames = struct.unpack("<II", d[12:20])
            self.assertGreater(fps, 0, rel)
            self.assertIn(fmt, (0, 1), f"{rel}: формат кадра {fmt}")
            self.assertIn(afmt, (0, 1, 2), f"{rel}: формат звука {afmt}")
            if afmt:
                self.assertTrue(4000 <= arate <= 96000, f"{rel}: arate {arate}")
            pos, nv, na = 20, 0, 0
            while pos + 5 <= len(d):
                tag = d[pos]
                ln = struct.unpack("<I", d[pos + 1:pos + 5])[0]
                self.assertIn(tag, (ord("V"), ord("A")), f"{rel}: кусок {tag:#x}")
                pos += 5 + ln
                if tag == ord("V"):
                    nv += 1
                else:
                    na += 1
            self.assertEqual(nv, frames, f"{rel}: кадров {nv}, заявлено {frames}")
            if afmt:
                self.assertGreater(na, 0, f"{rel}: звук объявлен, кусков 'A' нет")
            # Видеокадры — baseline JPEG (формат 0).
            if fmt == 0 and nv:
                pos = 20
                seen = 0
                while pos + 5 <= len(d) and seen == 0:
                    tag = d[pos]
                    ln = struct.unpack("<I", d[pos + 1:pos + 5])[0]
                    if tag == ord("V"):
                        self.assertEqual(d[pos + 5:pos + 7], b"\xff\xd8", f"{rel}: кадр не JPEG")
                        seen = 1
                    pos += 5 + ln

    def test_wav_files(self):
        """WAV: PCM 16 бит, моно/стерео, 4000..96000 Гц — иначе
        ресемплер src/media.rs файл отвергнет."""
        for rel, _size, _sha in self.entries:
            if not rel.endswith(".wav"):
                continue
            with wave.open(os.path.join(MEDIA, rel), "rb") as wf:
                ch, width, rate = wf.getnchannels(), wf.getsampwidth(), wf.getframerate()
            self.assertEqual(width, 2, f"{rel}: {width * 8} бит != 16")
            self.assertIn(ch, (1, 2), f"{rel}: каналов {ch}")
            self.assertTrue(4000 <= rate <= 96000, f"{rel}: rate {rate}")

    def test_pack_budget(self):
        """/userdata ext2 — 16 МиБ; пак обязан оставлять место данным."""
        total = sum(size for _r, size, _s in self.entries)
        self.assertLessEqual(total, 6 * 1024 * 1024, f"пак {total} байт > 6 МиБ")


if __name__ == "__main__":
    unittest.main(verbosity=2)
