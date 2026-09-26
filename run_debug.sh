#!/usr/bin/env bash
# run_debug.sh — Скрипт запуска QEMU с абсолютным, подробным логированием прерываний,
# инструкций CPU, ошибок и вывода COM1-порта в файл.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMG="$ROOT/build/deix_disk.img"

if [ ! -f "$IMG" ]; then
    echo "Образ $IMG не найден. Сначала запустите: ./build.sh"
    exit 1
fi

LOG_FILE="$ROOT/qemu_debug.log"
SERIAL_LOG="$ROOT/serial.log"

echo "=========================================================="
echo "  DeiX OS — QEMU Full Debug Logging Runner"
echo "=========================================================="
echo "  Файл образа диска: $IMG"
echo "  Лог прерываний/CPU: $LOG_FILE"
echo "  Лог COM1-порта:     $SERIAL_LOG"
echo "=========================================================="

qemu-system-x86_64 \
    -drive file="$IMG",format=raw,if=ide \
    -m 512M \
    -no-reboot \
    -no-shutdown \
    -d guest_errors,int,cpu_reset \
    -D "$LOG_FILE" \
    -serial file:"$SERIAL_LOG" \
    -vga std \
    "$@"

echo ""
echo "=========================================================="
echo "  Логирование завершено. Результаты записаны в:"
echo "    1. $LOG_FILE (трассировка CPU/прерываний)"
echo "    2. $SERIAL_LOG (отладочный вывод ядра в COM1)"
echo "=========================================================="
