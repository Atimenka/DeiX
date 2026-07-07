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

qemu-system-x86_64 \
    -drive format=raw,file="$IMG" \
    -m 512M \
    "$@"
