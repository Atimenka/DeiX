#!/usr/bin/env bash
# Сборка DeiX в classic BIOS-загружаемый дисковый образ (без GRUB — только
# nasm + ld + objcopy, поэтому подходит и для Termux на телефоне).
#
# Схема:
#   Сектор 0        — boot_sector.bin (MBR, 512 байт, BIOS грузит его сам)
#   Сектор 1..N     — stage2.bin (32-бит код перехода в long mode
#                      + 64-бит трамплин + ядро на Rust), склеены линкером
#                      в один плоский бинарник с базовым адресом 0x10000
set -euo pipefail

source "$HOME/.cargo/env" 2>/dev/null || true

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

BUILD="$ROOT/build"
mkdir -p "$BUILD"

echo "==> [1/7] Ассемблируем stage2 (32-бит вход + переход в long mode)"
nasm -f elf64 boot/stage2.asm           -o "$BUILD/stage2.o"
nasm -f elf64 boot/long_mode_init.asm   -o "$BUILD/long_mode_init.o"

echo "==> [2/7] Собираем демонстрационные .mex-программы (tools/*.asm)"
# Формат .mex (DeiX EXecutable) — см. docs/MEX_FORMAT.md. Собираются ДО
# ядра, потому что src/pkg.rs подключает их через include_bytes! на
# этапе компиляции Rust-кода.
nasm -f bin tools/hello.asm   -o "$BUILD/hello.bin"
nasm -f bin tools/sysinfo.asm -o "$BUILD/sysinfo.bin"
python3 tools/mex_pack.py "$BUILD/hello.bin"   "$BUILD/hello.mex"
python3 tools/mex_pack.py "$BUILD/sysinfo.bin" "$BUILD/sysinfo.mex"

echo "==> [3/7] Собираем ядро на Rust (nightly, build-std, target: x86_64-unknown-none)"
cargo +nightly build --release

KERNEL_LIB="$ROOT/target/x86_64-unknown-none/release/libdeix_kernel.a"
if [ ! -f "$KERNEL_LIB" ]; then
    echo "Не найден $KERNEL_LIB — сборка Rust-части не удалась"
    exit 1
fi

echo "==> [4/7] Линкуем stage2 + ядро в плоский бинарник (адрес загрузки 0x10000)"
ld -n --gc-sections -T boot/linker2.ld -o "$BUILD/stage2.elf" \
    "$BUILD/stage2.o" \
    "$BUILD/long_mode_init.o" \
    "$KERNEL_LIB"

strip --strip-all "$BUILD/stage2.elf" -o "$BUILD/stage2.stripped.elf"
objcopy -O binary "$BUILD/stage2.stripped.elf" "$BUILD/stage2.bin"

# Сколько 512-байтных секторов занимает stage2 (округляем вверх)
STAGE2_SIZE=$(stat -c%s "$BUILD/stage2.bin")
STAGE2_SECTORS=$(( (STAGE2_SIZE + 511) / 512 ))
echo "    stage2.bin: $STAGE2_SIZE байт => $STAGE2_SECTORS секторов"

echo "==> [5/7] Ассемблируем boot-сектор (MBR, 16-бит реальный режим),"
echo "          передаём точное число секторов stage2 = $STAGE2_SECTORS"
nasm -f bin -D NUM_SECTORS=$STAGE2_SECTORS boot/boot_sector.asm -o "$BUILD/boot_sector.bin"

BOOT_SIZE=$(stat -c%s "$BUILD/boot_sector.bin")
if [ "$BOOT_SIZE" -ne 512 ]; then
    echo "ОШИБКА: boot_sector.bin должен быть ровно 512 байт, а получилось $BOOT_SIZE"
    exit 1
fi

echo "==> [6/7] Склеиваем итоговый образ диска"

DISK_IMG="$BUILD/deix_disk.img"
cat "$BUILD/boot_sector.bin" "$BUILD/stage2.bin" > "$DISK_IMG"

# Дополняем образ нулями до размера, кратного 512, а затем до размера,
# достаточного для FAT16-тома (см. src/fat16.rs: FS_START_LBA=4096,
# TOTAL_SECTORS=8192 -> том занимает секторы [4096, 12288), оставляем
# небольшой запас сверху).
python3 - "$DISK_IMG" <<'PYEOF'
import sys, os
path = sys.argv[1]
size = os.path.getsize(path)
pad = (512 - size % 512) % 512
if pad:
    with open(path, "ab") as f:
        f.write(b"\x00" * pad)

MIN_SECTORS = 12800  # запас над FS_START_LBA(4096) + TOTAL_SECTORS(8192)
min_size = MIN_SECTORS * 512
size = os.path.getsize(path)
if size < min_size:
    with open(path, "ab") as f:
        f.write(b"\x00" * (min_size - size))
PYEOF

echo "==> [7/7] Собираем гибридный .iso (El Torito \"no emulation\" boot)"
# Отдельный маленький загрузчик (boot/boot_sector_iso.asm) — см. подробное
# объяснение прямо в этом файле: реальный SeaBIOS не поддерживает El
# Torito "hard disk emulation" для произвольных образов (нулевая
# геометрия диска), поэтому используется "no emulation" режим, при
# котором BIOS сама копирует весь образ в память по адресу 0x7c00, а
# дальше наш стаб перемещает stage2 на 0x10000 — точно так же, как это
# делают GRUB/isolinux у настоящих Linux-дистрибутивов.
STAGE2_DWORDS=$(( (STAGE2_SIZE + 3) / 4 ))
nasm -f bin -D STAGE2_SIZE_DWORDS=$STAGE2_DWORDS boot/boot_sector_iso.asm -o "$BUILD/boot_sector_iso.bin"

ISO_BOOT_IMAGE="$BUILD/iso_boot_image.bin"
cat "$BUILD/boot_sector_iso.bin" "$BUILD/stage2.bin" > "$ISO_BOOT_IMAGE"

ISO_IMAGE_SIZE=$(stat -c%s "$ISO_BOOT_IMAGE")
ISO_BOOT_SECTORS=$(( (ISO_IMAGE_SIZE + 511) / 512 ))

ISO_ROOT="$BUILD/iso_root"
rm -rf "$ISO_ROOT"
mkdir -p "$ISO_ROOT"
cp "$ISO_BOOT_IMAGE" "$ISO_ROOT/BOOT.BIN"

ISO_OUT="$BUILD/deix.iso"
if command -v genisoimage >/dev/null 2>&1; then
    genisoimage -quiet \
        -o "$ISO_OUT" \
        -b BOOT.BIN \
        -no-emul-boot \
        -boot-load-size "$ISO_BOOT_SECTORS" \
        -V "DEIX" \
        -iso-level 3 \
        "$ISO_ROOT"
    echo "    deix.iso собран через genisoimage ($ISO_BOOT_SECTORS секторов boot-образа)"
elif command -v xorriso >/dev/null 2>&1; then
    # xorriso (пакет libisoburn/libarchive-tools) умеет эмулировать
    # интерфейс mkisofs теми же флагами — используем как запасной
    # вариант там, где нет genisoimage/cdrtools (типичная ситуация в
    # свежем Arch Linux/WSL — xorriso там куда чаще предустановлен).
    xorriso -as mkisofs -quiet \
        -o "$ISO_OUT" \
        -b BOOT.BIN \
        -no-emul-boot \
        -boot-load-size "$ISO_BOOT_SECTORS" \
        -V "DEIX" \
        -iso-level 3 \
        "$ISO_ROOT"
    echo "    deix.iso собран через xorriso ($ISO_BOOT_SECTORS секторов boot-образа)"
else
    echo "    ПРЕДУПРЕЖДЕНИЕ: не найден ни genisoimage, ни xorriso — .iso не собран."
    echo "    Установите один из них, например:"
    echo "      Debian/Ubuntu: sudo apt-get install -y genisoimage"
    echo "      Arch Linux:    sudo pacman -S libisoburn      (даёт xorriso)"
    echo "      Termux:        pkg install cdrkit             (даёт genisoimage)"
fi

echo ""
echo "==> Готово! Образ диска: $DISK_IMG"
if [ -f "$ISO_OUT" ]; then
    echo "==> Готово! ISO-образ:   $ISO_OUT"
fi
echo "==> Запуск (диск):  ./run.sh"
echo "==> Запуск (ISO):   qemu-system-x86_64 -cdrom $ISO_OUT -m 256M"
