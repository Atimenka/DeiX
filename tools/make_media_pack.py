#!/usr/bin/env python3
"""Сборка медиапака DeiX: web/public/media/** + manifest.txt.

Медиапак — некритичная тяжёлая среда ОС (обои, картинки, демо-видео,
музыка, голоса). Он НЕ лежит в kernel.bin и в /system (EROFS 16 МиБ):
ОС скачивает его с сайта и устанавливает в /userdata/media
(src/mediapack.rs, `media install` / автоустановка при первом входе).

Состав:
  wallpapers/<стиль>.jpg  — 13 встроенных стилей темы (src/ui/theme.rs:
                            SYSTEM_WALLPAPER_FILES), JPEG baseline
                            960x720 из assets/wallpapers;
  pictures/*.jpg          — иллюстрации из assets/pictures (JPEG q85);
  audio/demo_track.wav    — синтезированная демо-музыка (PCM 16 бит);
  voice/*.wav             — голосовые сообщения из assets/voice;
  video/demo.dxv          — демо-видео в контейнере DXV1 (JPEG-кадры
                            320x180 12 fps + PCM моно 11025 Гц);
  manifest.txt            — «#version N» + строки «path|size|sha256».

Требования к декодерам ОС (важно, не менять без правки src/):
  * JPEG — только baseline (SOF0), без прогрессивности: src/jpeg.rs;
  * DXV1 — формат из src/media.rs (заголовок 20 байт + куски 'V'/'A');
  * WAV  — PCM 16 бит моно/стерео, частота 4000..96000 Гц;
  * файл — не больше 4 МиБ (MAX_FILE_BYTES в src/mediapack.rs).

Запуск:  python3 tools/make_media_pack.py [--version V] [--out DIR]
Версия по умолчанию берётся из существующего manifest.txt, иначе «1».
"""

import argparse
import hashlib
import math
import os
import re
import struct
import sys
import wave

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(ROOT, "assets")
DEFAULT_OUT = os.path.join(ROOT, "web", "public", "media")
MAX_FILE_BYTES = 4 * 1024 * 1024

WALLPAPER_SIZE = (960, 720)
WALLPAPER_QUALITY = 82
PICTURE_MAX_SIDE = 800
PICTURE_QUALITY = 85

VIDEO_W, VIDEO_H = 320, 180
VIDEO_FPS = 12
VIDEO_SECONDS = 12
VIDEO_QUALITY = 70
VIDEO_ARATE = 11025

MUSIC_RATE = 22050
MUSIC_SECONDS = 12


def theme_wallpaper_stems():
    """Имена файлов обоев из src/ui/theme.rs — единственный источник правды."""
    with open(os.path.join(ROOT, "src", "ui", "theme.rs"), encoding="utf-8") as f:
        src = f.read()
    m = re.search(r"SYSTEM_WALLPAPER_FILES: \[&str; (\d+)\]\s*=\s*\[(.*?)\];", src, re.S)
    if not m:
        raise SystemExit("theme.rs: SYSTEM_WALLPAPER_FILES не найден")
    names = re.findall(r'"([^"]+)"', m.group(2))
    if len(names) != int(m.group(1)):
        raise SystemExit("theme.rs: число имён не совпадает с размером массива")
    return names


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def build_wallpapers(out, names):
    from PIL import Image

    files = {}
    for name in names:
        stem, ext = os.path.splitext(name)
        if ext.lower() != ".jpg":
            raise SystemExit(f"theme.rs: обои '{name}' должны быть .jpg (медиапак)")
        src_path = None
        for cand in (stem + ".bmp", stem + ".png"):
            p = os.path.join(ASSETS, "wallpapers", cand)
            if os.path.isfile(p):
                src_path = p
                break
        if src_path is None:
            raise SystemExit(f"assets/wallpapers: нет источника для '{name}'")
        img = Image.open(src_path).convert("RGB")
        img = img.resize(WALLPAPER_SIZE, Image.LANCZOS)
        dest = os.path.join(out, "wallpapers", name)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        # progressive=False — src/jpeg.rs понимает только baseline (SOF0).
        img.save(dest, "JPEG", quality=WALLPAPER_QUALITY, optimize=True, progressive=False)
        files[f"wallpapers/{name}"] = dest
    return files


def build_pictures(out):
    from PIL import Image

    files = {}
    src_dir = os.path.join(ASSETS, "pictures")
    for fname in sorted(os.listdir(src_dir)):
        if not fname.lower().endswith((".png", ".jpg", ".bmp")):
            continue
        img = Image.open(os.path.join(src_dir, fname)).convert("RGB")
        if max(img.size) > PICTURE_MAX_SIDE:
            k = PICTURE_MAX_SIDE / max(img.size)
            img = img.resize((round(img.width * k), round(img.height * k)), Image.LANCZOS)
        dest_name = os.path.splitext(fname)[0] + ".jpg"
        dest = os.path.join(out, "pictures", dest_name)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        img.save(dest, "JPEG", quality=PICTURE_QUALITY, optimize=True, progressive=False)
        files[f"pictures/{dest_name}"] = dest
    return files


def _envelope(i, total, attack, release):
    if i < attack:
        return i / attack
    if i > total - release:
        return max(0.0, (total - i) / release)
    return 1.0


def synth_music(rate, seconds):
    """Демо-трек: аккордовая последовательность + мелодия (синус с
    обертонами). Возвращает list[int] (s16le моно)."""
    total = rate * seconds
    samples = [0.0] * total
    # Am - F - C - G, по 3 секунды на аккорд.
    chords = [
        (220.00, 261.63, 329.63),  # Am
        (174.61, 220.00, 261.63),  # F
        (130.81, 164.81, 196.00),  # C (низкая октава)
        (196.00, 246.94, 293.66),  # G
    ]
    melody = [440.00, 523.25, 659.25, 523.25, 392.00, 349.23, 523.25, 587.33,
              659.25, 783.99, 659.25, 523.25]
    beat = total // len(melody)
    attack = rate // 50
    release = rate // 8
    for i in range(total):
        t = i / rate
        chord = chords[min(len(chords) - 1, (i * len(chords)) // total)]
        s = 0.0
        for f in chord:
            s += 0.16 * math.sin(2 * math.pi * f * t) + 0.05 * math.sin(4 * math.pi * f * t)
        # Мелодия: нота на `beat` сэмплов с огибающей.
        mi = min(len(melody) - 1, i // beat)
        j = i - mi * beat
        env = _envelope(j, beat, attack, release)
        f = melody[mi]
        s += 0.22 * env * (math.sin(2 * math.pi * f * t) + 0.3 * math.sin(4 * math.pi * f * t))
        # Общий фейд-аут последних 0.5 с.
        if i > total - rate // 2:
            s *= (total - i) / (rate // 2)
        samples[i] = s
    peak = max(abs(s) for s in samples) or 1.0
    return [max(-32768, min(32767, int(s / peak * 26000))) for s in samples]


def write_wav(path, rate, samples):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(struct.pack("<" + "h" * len(samples), *samples))


def build_audio(out):
    files = {}
    samples = synth_music(MUSIC_RATE, MUSIC_SECONDS)
    dest = os.path.join(out, "audio", "demo_track.wav")
    write_wav(dest, MUSIC_RATE, samples)
    files["audio/demo_track.wav"] = dest

    voice_dir = os.path.join(ASSETS, "voice")
    if os.path.isdir(voice_dir):
        for fname in sorted(os.listdir(voice_dir)):
            if not fname.lower().endswith(".wav"):
                continue
            dest = os.path.join(out, "voice", fname)
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            with open(os.path.join(voice_dir, fname), "rb") as fh:
                data = fh.read()
            with open(dest, "wb") as fh:
                fh.write(data)
            files[f"voice/{fname}"] = dest
    return files


def build_video(out):
    """Демо-видео DXV1: анимация 320x180 12 fps + тон-сопровождение.
    Куски пишутся во временном порядке: за каждую секунду VIDEO_FPS
    кадров 'V' и один кусок 'A' (секунда PCM моно 11025 Гц)."""
    from PIL import Image, ImageDraw

    import io

    frames_total = VIDEO_FPS * VIDEO_SECONDS
    tone = [440.0, 330.0, 392.0, 294.0] * 3  # по секундам, 12 с
    arate = VIDEO_ARATE

    # Заголовок 20 байт (src/media.rs): magic(4) fps(2) w(2) h(2)
    # fmt-кадра(1) fmt-звука(1) arate(4) число кадров(4).
    header = b"DXV1"
    header += struct.pack("<HHH", VIDEO_FPS, VIDEO_W, VIDEO_H)
    header += bytes([0, 1])  # 0 = JPEG baseline, 1 = PCM s16le моно
    header += struct.pack("<II", arate, frames_total)
    assert len(header) == 20, len(header)

    body = bytearray()
    img = Image.new("RGB", (VIDEO_W, VIDEO_H))
    draw = ImageDraw.Draw(img)
    sec_samples = arate  # 1 секунда аудио на кусок 'A'
    for sec in range(VIDEO_SECONDS):
        f0 = tone[sec % len(tone)]
        # Секунда звука: аккорд из двух синусов с фейдом.
        pcm = bytearray()
        for i in range(sec_samples):
            t = i / arate
            env = min(1.0, i / (arate // 40), (sec_samples - i) / (arate // 8))
            v = 0.5 * math.sin(2 * math.pi * f0 * t) + 0.25 * math.sin(2 * math.pi * f0 * 1.5 * t)
            pcm += struct.pack("<h", int(v * env * 20000))
        body += b"A" + struct.pack("<I", len(pcm)) + pcm
        for f in range(VIDEO_FPS):
            frame_i = sec * VIDEO_FPS + f
            t = frame_i / VIDEO_FPS
            # Фон: вертикальный градиент, медленно меняющий оттенок.
            hue_shift = int(40 * math.sin(t * 0.8))
            for y in range(0, VIDEO_H, 6):
                c1 = (18 + (y * 40) // VIDEO_H, 24 + (y * 60) // VIDEO_H, 60 + hue_shift)
                draw.rectangle([0, y, VIDEO_W, y + 6], fill=tuple(max(0, min(255, c)) for c in c1))
            # Прыгающий круг-«логотип».
            cx = int(VIDEO_W / 2 + (VIDEO_W / 2 - 30) * math.sin(t * 1.7))
            cy = int(VIDEO_H / 2 + (VIDEO_H / 2 - 30) * math.cos(t * 2.3))
            r = 14 + int(4 * math.sin(t * 5))
            draw.ellipse([cx - r, cy - r, cx + r, cy + r], fill=(240, 180, 90), outline=(255, 230, 170))
            # Подпись и счётчик кадров.
            draw.text((8, 6), "DeiX OS - demo video (DXV1)", fill=(230, 230, 240))
            draw.text((8, VIDEO_H - 18), f"frame {frame_i + 1}/{frames_total}  {t:0.2f}s", fill=(190, 190, 210))
            buf = io.BytesIO()
            img.save(buf, "JPEG", quality=VIDEO_QUALITY, optimize=True, progressive=False)
            data = buf.getvalue()
            body += b"V" + struct.pack("<I", len(data)) + data

    dest = os.path.join(out, "video", "demo.dxv")
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    with open(dest, "wb") as fh:
        fh.write(header)
        fh.write(bytes(body))
    return {"video/demo.dxv": dest}


def read_manifest_version(path):
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                if line.startswith("#version "):
                    return line[len("#version "):].strip()
    except OSError:
        pass
    return None


def write_manifest(out, files, version):
    lines = [f"#version {version}"]
    total = 0
    for rel in sorted(files):
        path = files[rel]
        with open(path, "rb") as fh:
            data = fh.read()
        if len(data) > MAX_FILE_BYTES:
            raise SystemExit(f"{rel}: {len(data)} байт > лимита {MAX_FILE_BYTES}")
        if not re.fullmatch(r"[a-z0-9._/-]+", rel) or rel.startswith("/") or "//" in rel:
            raise SystemExit(f"{rel}: путь не проходит valid_rel_path ОС")
        total += len(data)
        lines.append(f"{rel}|{len(data)}|{sha256(data)}")
    dest = os.path.join(out, "manifest.txt")
    with open(dest, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(lines) + "\n")
    return dest, total


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--version", default=None, help="версия пака (по умолчанию — из manifest.txt, иначе 1)")
    ap.add_argument("--out", default=DEFAULT_OUT, help="каталог вывода (web/public/media)")
    args = ap.parse_args()

    out = args.out
    os.makedirs(out, exist_ok=True)
    version = args.version or read_manifest_version(os.path.join(out, "manifest.txt")) or "1"

    names = theme_wallpaper_stems()
    files = {}
    files.update(build_wallpapers(out, names))
    files.update(build_pictures(out))
    files.update(build_audio(out))
    files.update(build_video(out))

    manifest, total = write_manifest(out, files, version)
    print(f"медиапак v{version}: {len(files)} файлов, {total / 1024 / 1024:.2f} МиБ")
    print(f"манифест: {os.path.relpath(manifest, ROOT)}")
    if total > 6 * 1024 * 1024:
        print("ВНИМАНИЕ: пак больше 6 МиБ — /userdata ext2 всего 16 МиБ", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
