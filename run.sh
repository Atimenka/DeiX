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
if [ -n "${PULSE_SERVER:-}" ] || [ -e "/mnt/wslg/PulseServer" ] || [ -e "${XDG_RUNTIME_DIR:-/run/user/1000}/pulse/native" ]; then
  if [ -e "/mnt/wslg/PulseServer" ] && [ -z "${PULSE_SERVER:-}" ]; then
    export PULSE_SERVER="unix:/mnt/wslg/PulseServer"
  fi
  AUDIO_ARGS=(-audiodev pa,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
elif [ -e "/dev/snd" ]; then
  AUDIO_ARGS=(-audiodev alsa,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
else
  AUDIO_ARGS=(-device intel-hda -device hda-duplex)
fi

qemu-system-x86_64 \
    -drive format=raw,file="$IMG" \
    -m 512M \
    "${AUDIO_ARGS[@]}" \
    "$@"
