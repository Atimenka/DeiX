import wave, struct, sys

def wav_to_dps(wav_path, dps_path, target_rate=8000):
    with wave.open(wav_path, 'rb') as w:
        n_channels = w.getnchannels()
        sampwidth = w.getsampwidth()
        framerate = w.getframerate()
        n_frames = w.getnframes()
        raw = w.readframes(n_frames)

    if sampwidth == 1:
        samples = list(raw)
    elif sampwidth == 2:
        samples = list(struct.unpack('<' + 'h'*(len(raw)//2), raw))
        samples = [(s + 32768) * 255 // 65535 for s in samples]
    else:
        raise ValueError("Пока только 8/16 бит")

    if n_channels == 2:
        samples = [(samples[i] + samples[i+1]) // 2 for i in range(0, len(samples), 2)]

    if framerate != target_rate:
        ratio = target_rate / framerate
        new_len = int(len(samples) * ratio)
        resampled = []
        for i in range(new_len):
            pos = i / ratio
            idx = int(pos)
            frac = pos - idx
            if idx + 1 < len(samples):
                val = int(samples[idx] * (1 - frac) + samples[idx+1] * frac)
            else:
                val = samples[idx]
            resampled.append(val)
        samples = resampled

    with open(dps_path, 'wb') as f:
        f.write(b'DPS1')
        f.write(struct.pack('<I', target_rate))
        f.write(struct.pack('<I', len(samples)))
        f.write(bytes(samples))

if __name__ == '__main__':
    if len(sys.argv) < 3:
        print("Использование: python wav2dps.py <input.wav> <output.dps> [частота]")
        sys.exit(1)
    rate = int(sys.argv[3]) if len(sys.argv) > 3 else 8000
    wav_to_dps(sys.argv[1], sys.argv[2], rate)
    print(f"Готово: {sys.argv[2]}")