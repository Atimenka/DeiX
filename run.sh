#!/usr/bin/env bash
# Запуск DeiX в QEMU: грузим собранный дисковый образ как обычный "жёсткий
# диск" — BIOS (SeaBIOS внутри QEMU) сам прочитает MBR из сектора 0.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMG="$ROOT/build/deix_disk.img"

if [ ! -f "$IMG" ]; then
    echo "Сначала собери проект: ./build.sh"
    exit 1
fi

AUDIO_ARGS=()
if [ "${AUDIO_OFF:-0}" = "1" ] || [ "${QEMU_AUDIO_DRV:-}" = "none" ]; then
  AUDIO_ARGS=(-audiodev none,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
elif [ -e "/mnt/wslg/PulseServer" ]; then
  export PULSE_SERVER="unix:/mnt/wslg/PulseServer"
  AUDIO_ARGS=(-audiodev pa,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
elif [ -n "${PULSE_SERVER:-}" ] && { [ -f "/run/user/$(id -u)/pulse/pid" ] || [ -f "/run/user/1000/pulse/pid" ] || pulseaudio --check 2>/dev/null; }; then
  AUDIO_ARGS=(-audiodev pa,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
elif [ -e "/dev/snd" ] && [ "$(id -u)" -ne 0 ]; then
  AUDIO_ARGS=(-audiodev alsa,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
else
  # Безопасный режим: эмуляция HDA в гости без зависания от системного PulseAudio root
  AUDIO_ARGS=(-audiodev none,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
fi

qemu-system-x86_64 \
    -drive format=raw,file="$IMG" \
    -m 512M \
    "${AUDIO_ARGS[@]}" \
    "$@"
