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
    python3 tools/wav2dps.py <in.wav> <out.dps> [rate=8000]
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
    """Линейный ресемплинг: простой и достаточный для 2-4 секундных сигналов."""
    if src_rate == dst_rate:
        return samples
    if not samples:
        return samples
    step = src_rate / float(dst_rate)
    out_len = int(len(samples) / step)
    out = []
    for i in range(out_len):
        pos = i * step
        i0 = int(pos)
        frac = pos - i0
        i1 = min(i0 + 1, len(samples) - 1)
        out.append(int(samples[i0] * (1.0 - frac) + samples[i1] * frac))
    return out


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
    if len(sys.argv) < 3:
        print("usage: wav2dps.py <in.wav> <out.dps> [rate=8000]", file=sys.stderr)
        raise SystemExit(2)
    src, dst = sys.argv[1], sys.argv[2]
    rate = int(sys.argv[3]) if len(sys.argv) > 3 else 8000

    samples, src_rate = load_wav(src)
    samples = resample_linear(samples, src_rate, rate)
    data = to_u8(samples)

    # Лимит на всякий случай: 10 секунд на эффект больше чем достаточно.
    max_len = rate * 10
    if len(data) > max_len:
        data = data[:max_len]

    with open(dst, "wb") as f:
        f.write(b"DPS1")
        f.write(struct.pack("<I", rate))
        f.write(struct.pack("<I", len(data)))
        f.write(data)

    dur = len(data) / float(rate)
    print("    %s -> %s: %d сэмплов @ %d Гц (~%.1f с, %d байт)" % (src, dst, len(data), rate, dur, len(data) + 12))


if __name__ == "__main__":
    main()
