#!/bin/bash
# DeiX OS — Full hardware emulation + networking + Intel HDA audio
DISK="$(dirname "$0")/build/deix_disk.img"

echo "=== DeiX OS v0.2.1 ==="
echo "512 MB RAM | RTL8139 NIC | Intel HDA Audio | 10.0.2.15 IP"
echo "Commands: help, sound, sound hda test, profile ls, ping 10.0.2.2"
echo ""

# Автоопределение аудио-бэкенда хоста (PulseAudio / PipeWire / WSL2 WSLg)
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
  # Безопасный режим: эмуляция HDA в госте без зависимости от PulseAudio root
  AUDIO_ARGS=(-audiodev none,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0)
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
