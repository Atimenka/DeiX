import wave, struct, sys

def dps_to_wav(dps_path, wav_path):
    with open(dps_path, 'rb') as f:
        data = f.read()
    if data[:4] != b'DPS1':
        raise ValueError("Не DPS1 файл")
    rate = struct.unpack('<I', data[4:8])[0]
    n_samples = struct.unpack('<I', data[8:12])[0]
    samples = data[12:12+n_samples]

    with wave.open(wav_path, 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(1)
        w.setframerate(rate)
        w.writeframes(samples)

if __name__ == '__main__':
    if len(sys.argv) < 3:
        print("Использование: python dps2wav.py <input.dps> <output.wav>")
        sys.exit(1)
    dps_to_wav(sys.argv[1], sys.argv[2])
    print(f"Готово: {sys.argv[2]}")