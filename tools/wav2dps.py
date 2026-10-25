#!/usr/bin/env python3
"""wav2dps — конвертер WAV -> DPS1 (DeiX PCM Sound) для UI-звуков.

DPS1 — мини-формат под проигрыватель в src/sound.rs:
    0..4   магия "DPS1"
    4..8   u32 LE: частота дискретизации (Гц)
    8..12  u32 LE: число сэмплов
    12..   данные: unsigned u8 PCM, моно (128 = тишина)

Зачем так: PC speaker не вытягивает 44.1 кГц/16 бит/стерео исходников,
поэтому на этапе сборки всё приводится к 8 кГц 8-бит моно — ядру остаётся
только отдать байты в порт 0x61 (ШИМ). Вся математика — здесь, на хосте,
чистый Python stdlib (wave + struct), без внешних зависимостей.

Использование:
    python3 tools/wav2dps.py <in.wav> <out.dps> [rate=8000] [--voice]

--voice обрезает тишину по краям и нормализует пик до ~90 % шкалы (для речи).
"""

import struct
import sys
import wave


def load_wav(path):
    """Читает WAV (PCM 8/16 бит, 1/2 канала) -> (список int -32768..32767, rate)."""
    with wave.open(path, "rb") as w:
        ch = w.getnchannels()
        width = w.getsampwidth()
        rate = w.getframerate()
        frames = w.readframes(w.getnframes())

    if width == 2:
        # 16-бит signed little-endian
        count = len(frames) // 2
        samples = list(struct.unpack("<%dh" % count, frames[: count * 2]))
    elif width == 1:
        # 8-бит unsigned (128 = тишина) -> приводим к signed 16-бит-шкале
        samples = [(b - 128) << 8 for b in frames]
    else:
        raise SystemExit("wav2dps: поддерживаются только 8/16-бит PCM WAV (у %s sampwidth=%d)" % (path, width))

    if ch == 2:
        # стерео -> моно: среднее L+R
        samples = [(samples[i] + samples[i + 1]) // 2 for i in range(0, len(samples) - 1, 2)]
    elif ch != 1:
        raise SystemExit("wav2dps: поддерживаются только моно/стерео (у %s каналов: %d)" % (path, ch))

    return samples, rate


def resample_linear(samples, src_rate, dst_rate):
    """Ресемплинг: при понижении частоты усредняет окно (без алиасинга), иначе линейная интерполяция."""
    if src_rate == dst_rate:
        return samples
    if not samples:
        return samples
    step = src_rate / float(dst_rate)
    out_len = int(len(samples) / step)
    out = []
    if step > 1.0:
        for i in range(out_len):
            a = int(i * step)
            b = max(a + 1, min(len(samples), int((i + 1) * step)))
            out.append(sum(samples[a:b]) // (b - a))
        return out
    for i in range(out_len):
        pos = i * step
        i0 = int(pos)
        frac = pos - i0
        i1 = min(i0 + 1, len(samples) - 1)
        out.append(int(samples[i0] * (1.0 - frac) + samples[i1] * frac))
    return out


def trim_and_normalize(samples, rate):
    """Обрезает тишину по краям (с запасом 40 мс) и поднимает пик до ~90 % шкалы."""
    peak = max((abs(x) for x in samples), default=0)
    if peak == 0:
        return samples
    gate = peak // 40
    first = next(i for i, x in enumerate(samples) if abs(x) > gate)
    last = len(samples) - 1 - next(i for i, x in enumerate(reversed(samples)) if abs(x) > gate)
    pad = rate * 40 // 1000
    samples = samples[max(0, first - pad) : last + 1 + pad]
    gain = 29490.0 / peak
    return [int(x * gain) for x in samples]


def to_u8(samples):
    """signed 16-бит-шкала -> unsigned u8 (128 = тишина), с лёгким ограничением пиков."""
    out = bytearray()
    for s in samples:
        v = (s >> 8) + 128
        if v < 0:
            v = 0
        elif v > 255:
            v = 255
        out.append(v)
    return bytes(out)


def main():
    if len([a for a in sys.argv[1:] if a != "--voice"]) < 2:
        print("usage: wav2dps.py <in.wav> <out.dps> [rate=8000]", file=sys.stderr)
        raise SystemExit(2)
    voice = "--voice" in sys.argv
    args = [a for a in sys.argv[1:] if a != "--voice"]
    src, dst = args[0], args[1]
    rate = int(args[2]) if len(args) > 2 else 8000

    samples, src_rate = load_wav(src)
    samples = resample_linear(samples, src_rate, rate)
    if voice:
        samples = trim_and_normalize(samples, rate)
    data = to_u8(samples)

    # Лимит на всякий случай: 10 секунд на эффект больше чем достаточно.
    max_len = rate * 10
    if len(data) > max_len:
        data = data[:max_len]

    import os
    os.makedirs(os.path.dirname(os.path.abspath(dst)), exist_ok=True)
    with open(dst, "wb") as f:
        f.write(b"DPS1")
        f.write(struct.pack("<I", rate))
        f.write(struct.pack("<I", len(data)))
        f.write(data)

    dur = len(data) / float(rate)
    print("    %s -> %s: %d сэмплов @ %d Гц (~%.1f с, %d байт)" % (src, dst, len(data), rate, dur, len(data) + 12))


if __name__ == "__main__":
    main()
