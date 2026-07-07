#!/usr/bin/env bash
# Запуск DeiX с гибридного .iso-образа: BIOS грузит его через El Torito
# "no emulation" boot (тот же принцип, которым пользуются GRUB/isolinux
# у настоящих Linux LiveCD) — см. подробности в boot/boot_sector_iso.asm.
#
# По умолчанию виртуальный жёсткий диск НЕ подключается (аналог "Live"
# режима — файловая система ext2 будет недоступна, CLI покажет
# "ATA disk: not detected", это ожидаемо и не является ошибкой).
#
# Чтобы протестировать запись файлов (ext2) при загрузке с ISO, добавьте
# отдельный файл-образ жёсткого диска через переменную окружения DISK_IMG,
# например:
#   qemu-img create -f raw my_hdd.img 8M
#   DISK_IMG=my_hdd.img ./run_iso.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ISO="$ROOT/build/deix.iso"

if [ ! -f "$ISO" ]; then
    echo "Сначала собери проект: ./build.sh"
    exit 1
fi

EXTRA_ARGS=()
if [ -n "${DISK_IMG:-}" ]; then
    EXTRA_ARGS+=(-drive "file=$DISK_IMG,format=raw,if=ide")
fi

qemu-system-x86_64 \
    -cdrom "$ISO" \
    "${EXTRA_ARGS[@]}" \
    -m 512M \
    "$@"
