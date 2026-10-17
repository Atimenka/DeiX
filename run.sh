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

# PulseAudio-сервер реально отвечает? Без pactl проверить нельзя — тогда
# считаем, что отвечает (прежнее поведение).
pulse_alive() {
  command -v pactl >/dev/null 2>&1 || return 0
  timeout 3 pactl info >/dev/null 2>&1
}

AUDIO_ARGS=()
if [ "${AUDIO_OFF:-0}" = "1" ] || [ "${QEMU_AUDIO_DRV:-}" = "none" ]; then
  AUDIO_ARGS=(-audiodev none,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
elif [ -e "/mnt/wslg/PulseServer" ] && PULSE_SERVER="unix:/mnt/wslg/PulseServer" pulse_alive; then
  export PULSE_SERVER="unix:/mnt/wslg/PulseServer"
  AUDIO_ARGS=(-audiodev pa,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
elif [ -n "${PULSE_SERVER:-}" ] && { [ -f "/run/user/$(id -u)/pulse/pid" ] || [ -f "/run/user/1000/pulse/pid" ] || pulseaudio --check 2>/dev/null; } && pulse_alive; then
  AUDIO_ARGS=(-audiodev pa,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
elif [ -e "/dev/snd" ] && [ "$(id -u)" -ne 0 ]; then
  AUDIO_ARGS=(-audiodev alsa,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
else
  # Безопасный режим (в т.ч. когда PulseAudio не отвечает): эмуляция HDA в гости без зависания от системного PulseAudio root
  AUDIO_ARGS=(-audiodev none,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
fi

# Сеть: user-mode networking (slirp) + RTL8139 — гость получает
# 10.0.2.15/24, шлюз 10.0.2.2, DNS 10.0.2.3; без этих флагов браузер и
# команды dns/ping честно сообщат об отсутствии сетевой карты.
qemu-system-x86_64 \
    -drive format=raw,file="$IMG" \
    -m 512M \
    -netdev user,id=n0 -device rtl8139,netdev=n0 \
    "${AUDIO_ARGS[@]}" \
    "$@"
