#!/bin/bash
# DeiX OS — Full hardware emulation + networking + Intel HDA audio
DISK="$(dirname "$0")/build/deix_disk.img"

echo "=== DeiX OS v0.2 ==="
echo "512 MB RAM | RTL8139 NIC | Intel HDA Audio | 10.0.2.15 IP"
echo "Commands: help, sound, sound hda test, profile ls, ping 10.0.2.2"
echo ""

# Автоопределение аудио-бэкенда хоста (PulseAudio / PipeWire / WSL2 WSLg)
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

exec qemu-system-x86_64 \
  -drive file="$DISK",format=raw \
  -m 512M \
  -net nic,model=rtl8139 \
  -net user \
  -vga std \
  "${AUDIO_ARGS[@]}" \
  -serial stdio \
  -no-reboot "$@"
