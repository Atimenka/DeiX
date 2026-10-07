#!/usr/bin/env bash
# Сборка DeiX в classic BIOS-загружаемый дисковый образ (без GRUB — только
# nasm + ld + objcopy, поэтому подходит и для Termux на телефоне).
#
# Схема (двухчастная — ЛИМИТ РАЗМЕРА ЯДРА СНЯТ):
#   Сектор 0           — boot_sector.bin (MBR, 512 байт, BIOS грузит его сам)
#   Секторы 1..K       — stage2.bin (ЗАГРУЗЧИК: 32-бит вход -> long mode ->
#                         читает kernel.bin с диска через ATA PIO)
#   Секторы (1+K)..M   — kernel.bin (ядро: long_mode_init + Rust, база 0x100000)
# Загрузчик в real mode грузит только маленький stage2 (несколько секторов);
# само ядро stage2 дочитывает уже в 64-битном режиме в память 0x100000.
#
# Служебные области и разделы (источник истины — src/partition_map.rs):
#   LBA 0           — MBR (boot_sector.bin)
#   LBA 1..2046     — stage2 + ramboot + сырой kernel.bin
#   LBA 2047        — дескриптор образа ядра DEIXKIMG (LBA/размер/SHA-256)
#   LBA 2048..2111  — область аварийного дампа (DEIXPNIC, src/diag/panic.rs)
#   LBA 4096..12799 — /system   (EROFS RO, 8704 сект)
#   LBA 12800..18431 — /userdata (EXT2 RW, 5632 сект)
set -euo pipefail

source "$HOME/.cargo/env" 2>/dev/null || true

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

BUILD="$ROOT/build"
mkdir -p "$BUILD"

echo "==> [1/8] Ассемблируем stage2 (32-бит вход + long mode + ATA-ридер ядра)"
# Временные значения KERNEL_LBA/KERNEL_SECTORS — нужны только для размера
# stage2 (на адреса/размер не влияют), финальные подставим на шаге [6/8].
nasm -f elf64 -D KERNEL_SIZE_DWORDS=262144 boot/stage2.asm -o "$BUILD/stage2.o"
nasm -f elf64 boot/long_mode_init.asm   -o "$BUILD/long_mode_init.o"

echo "==> [1b/8] Линкуем stage2 (загрузчик, база 0x10000)"
ld -n --gc-sections -T boot/linker_stage2.ld -o "$BUILD/stage2.elf" "$BUILD/stage2.o"
strip --strip-all "$BUILD/stage2.elf" -o "$BUILD/stage2.stripped.elf"
objcopy -O binary "$BUILD/stage2.stripped.elf" "$BUILD/stage2.bin"

STAGE2_SIZE=$(stat -c%s "$BUILD/stage2.bin")
STAGE2_SECTORS=$(( (STAGE2_SIZE + 511) / 512 ))
echo "    stage2.bin (загрузчик): $STAGE2_SIZE байт => $STAGE2_SECTORS секторов"

# RAMBOOT (RAM-диск загрузчик) — 1 сектор; kernel.bin начинается после него.
RAMBOOT_SECTORS=1
KERNEL_LBA=$(( 1 + STAGE2_SECTORS + RAMBOOT_SECTORS ))
echo "    kernel.bin LBA: $KERNEL_LBA"

echo "==> [1c/8] Собираем ramboot (RAM-диск: дочитывает весь образ в 0x2000000)"
nasm -f bin -D RAMDISK_SECTORS=20480 -D RAMDISK_DST=0x2000000 \
    -D KERNEL_SECTORS=2048 -D KERNEL_LBA=$KERNEL_LBA \
    boot/ramboot.asm -o "$BUILD/ramboot.bin"
RAMBOOT_SIZE=$(stat -c%s "$BUILD/ramboot.bin")
if [ "$RAMBOOT_SIZE" -ne 512 ]; then
    echo "ОШИБКА: ramboot.bin должен быть ровно 512 байт, а получилось $RAMBOOT_SIZE"
    exit 1
fi
echo "    ramboot.bin: $RAMBOOT_SIZE байт => $RAMBOOT_SECTORS секторов"

echo "==> [2/8] Собираем MBR-загрузчик (NUM_SECTORS=stage2, KERNEL_SECTORS=2048)"
# KERNEL_SECTORS фиксирован (2048 секторов = 1024 КиБ): install включает
# boot_sector в себя через include_bytes, поэтому размер не может зависеть
# от kernel.bin (собирается позже). Чтение лишних секторов даёт нули.
nasm -f bin -D NUM_SECTORS=$STAGE2_SECTORS -D RAMBOOT_SECTORS=$RAMBOOT_SECTORS -D KERNEL_SECTORS=2048 boot/boot_sector.asm -o "$BUILD/boot_sector.bin"

echo "==> [2b/8] Собираем демонстрационные .mex-программы (tools/*.asm)"
# Формат .mex (DeiX EXecutable) — см. docs/MEX_FORMAT.md. Собираются ДО
# ядра, потому что src/pkg.rs подключает их через include_bytes! на
# этапе компиляции Rust-кода.
nasm -f bin tools/hello.asm   -o "$BUILD/hello.bin"
nasm -f bin tools/sysinfo.asm -o "$BUILD/sysinfo.bin"
nasm -f bin tools/netping.asm -o "$BUILD/netping.bin"
python3 tools/mex_pack.py "$BUILD/hello.bin"   "$BUILD/hello.mex"
python3 tools/mex_pack.py "$BUILD/sysinfo.bin" "$BUILD/sysinfo.mex"
python3 tools/mex_pack.py "$BUILD/netping.bin" "$BUILD/netping.mex"

echo "==> [2c/8] Собираем тестовую программу Linux (статический PIE)"
# Настоящий ELF для Linux x86-64: собирается системным gcc, использует
# только Linux syscall ABI. Его запуск в DeiX доказывает, что прослойка
# совместимости работает (src/linux/).
if command -v gcc >/dev/null 2>&1; then
    # -Os и strip: образ встраивается в kernel.bin через include_bytes!,
    # а его размер ограничен загрузчиком (KERNEL_SECTORS в boot/*.asm).
    gcc -nostdlib -nostartfiles -static-pie -fPIE -Os \
        -Wl,--build-id=none -Wl,-z,noseparate-code \
        -o "$BUILD/linux_hello.elf" tools/linux_hello.c
    strip -s "$BUILD/linux_hello.elf" 2>/dev/null || true
    echo "    linux_hello.elf: $(stat -c%s "$BUILD/linux_hello.elf") байт"
else
    echo "    gcc не найден — кладём заглушку (linux test будет недоступен)"
    : > "$BUILD/linux_hello.elf"
fi

echo "==> [2d/8] Готовим загрузочное лого (assets/logo.png -> DXLG+RLE)"
# Декодера PNG в ядре нет, а сырой RGB не влезает в 572 КиБ лимита
# kernel.bin, поэтому лого конвертируется в палитру + RLE (~2.3 КиБ).
if [ -f assets/logo.png ]; then
    python3 tools/make_logo.py assets/logo.png "$BUILD/logo.dxlg" 128
elif [ -f assets/logo.dxlg ]; then
    cp assets/logo.dxlg "$BUILD/logo.dxlg"
else
    echo "    ОШИБКА: assets/logo.png и assets/logo.dxlg не найдены"
    exit 1
fi

echo "==> [2e/8] Конвертируем UI-звуки (assets/*.wav -> DPS 8 кГц, u8 моно)"
# Звуки НЕ зашиваются в kernel.bin (лимит размера ядра): они кладутся
# в /system/media/audio/ui/ (EROFS, шаг 7b, make_deix_fs.py), а ядро
# читает их с диска в рантайме и играет через PC speaker (src/sound.rs).
SND="$BUILD/sounds"
mkdir -p "$SND"
conv_snd() {  # $1 = входной wav, $2 = имя dps
    if [ -f "$1" ]; then
        python3 tools/wav2dps.py "$1" "$SND/$2" \
            || echo "    warn: не удалось сконвертировать $1 — звука не будет"
    else
        echo "    warn: $1 не найден — эффект '$2' будет недоступен"
    fi
}
conv_snd "assets/start.wav"           start.dps
conv_snd "assets/Error.wav"           error.dps
conv_snd "assets/low battery.wav"     lowbat.dps
conv_snd "assets/full battery.wav"    fullbat.dps
conv_snd "assets/usb connect.wav"     usbcon.dps
conv_snd "assets/usb unconnected.wav" usbdisc.dps

echo "==> [3/8] Собираем ядро на Rust (nightly, build-std, target: x86_64-unknown-none)"
cargo +nightly build --release

KERNEL_LIB="$ROOT/target/x86_64-unknown-none/release/libdeix_kernel.a"
if [ ! -f "$KERNEL_LIB" ]; then
    echo "Не найден $KERNEL_LIB — сборка Rust-части не удалась"
    exit 1
fi

echo "==> [4/8] Линкуем kernel.bin (long_mode_init + ядро, база 0x100000)"
ld -n --gc-sections -T boot/linker_kernel.ld -o "$BUILD/kernel.elf" \
    "$BUILD/long_mode_init.o" \
    "$KERNEL_LIB"

strip --strip-all "$BUILD/kernel.elf" -o "$BUILD/kernel.stripped.elf"
objcopy -O binary "$BUILD/kernel.stripped.elf" "$BUILD/kernel.bin"

KERNEL_SIZE=$(stat -c%s "$BUILD/kernel.bin")
KERNEL_SECTORS=$(( (KERNEL_SIZE + 511) / 512 ))
echo "    kernel.bin: $KERNEL_SIZE байт => $KERNEL_SECTORS секторов (LBA $KERNEL_LBA)"

KERNEL_MAX_SECTORS=2048
if [ "$KERNEL_SECTORS" -gt "$KERNEL_MAX_SECTORS" ]; then
    echo "ОШИБКА: kernel.bin ($KERNEL_SECTORS сект) > лимита ($KERNEL_MAX_SECTORS)."
    echo "Увеличьте KERNEL_SECTORS в boot/boot_sector.asm и boot/ramboot.asm."
    exit 1
fi

# Сырое ядро не должно доставать до дескриптора DEIXKIMG (LBA 2047)
# и области аварийного дампа (LBA 2048..2111, src/diag/panic.rs).
KIMG_INFO_LBA=2047
if [ $(( KERNEL_LBA + KERNEL_SECTORS )) -gt "$KIMG_INFO_LBA" ]; then
    echo "ОШИБКА: kernel.bin (LBA $KERNEL_LBA + $KERNEL_SECTORS сект) пересекает"
    echo "дескриптор ядра (LBA $KIMG_INFO_LBA) / область дампа (LBA 2048)."
    exit 1
fi

echo "==> [5/8] Проверяем MBR-загрузчик (512 байт)"
BOOT_SIZE=$(stat -c%s "$BUILD/boot_sector.bin")
if [ "$BOOT_SIZE" -ne 512 ]; then
    echo "ОШИБКА: boot_sector.bin должен быть ровно 512 байт, а получилось $BOOT_SIZE"
    exit 1
fi

echo "==> [6/8] Пересобираем stage2 с реальным KERNEL_SIZE_DWORDS"
KERNEL_SIZE=$(stat -c%s "$BUILD/kernel.bin")
# stage2 копирует kernel из 0x11000 (буфер 2048 секторов): KERNEL_SIZE_DWORDS
# фиксирован и НЕ зависит от размера kernel.bin — иначе stage2, включённый
# в install (include_bytes!), отличался бы от финального и сравнение ломалось.
nasm -f elf64 -D KERNEL_SIZE_DWORDS=262144 boot/stage2.asm -o "$BUILD/stage2.o"
ld -n --gc-sections -T boot/linker_stage2.ld -o "$BUILD/stage2.elf" "$BUILD/stage2.o"
strip --strip-all "$BUILD/stage2.elf" -o "$BUILD/stage2.stripped.elf"
objcopy -O binary "$BUILD/stage2.stripped.elf" "$BUILD/stage2.bin"
STAGE2_SIZE2=$(stat -c%s "$BUILD/stage2.bin")
STAGE2_SECTORS2=$(( (STAGE2_SIZE2 + 511) / 512 ))
if [ "$STAGE2_SECTORS2" -ne "$STAGE2_SECTORS" ]; then
    echo "ПРЕДУПРЕЖДЕНИЕ: размер stage2 изменился ($STAGE2_SECTORS -> $STAGE2_SECTORS2);"
    echo "пересчитываем и пересобираем ещё раз"
    STAGE2_SECTORS=$STAGE2_SECTORS2
    KERNEL_LBA=$(( 1 + STAGE2_SECTORS + RAMBOOT_SECTORS ))
    nasm -f elf64 -D KERNEL_SIZE_DWORDS=262144 boot/stage2.asm -o "$BUILD/stage2.o"
    ld -n --gc-sections -T boot/linker_stage2.ld -o "$BUILD/stage2.elf" "$BUILD/stage2.o"
    strip --strip-all "$BUILD/stage2.elf" -o "$BUILD/stage2.stripped.elf"
    objcopy -O binary "$BUILD/stage2.stripped.elf" "$BUILD/stage2.bin"
    # KERNEL_LBA изменился — пересобираем ramboot с новым значением.
    nasm -f bin -D RAMDISK_SECTORS=20480 -D RAMDISK_DST=0x2000000 \
        -D KERNEL_SECTORS=2048 -D KERNEL_LBA=$KERNEL_LBA \
        boot/ramboot.asm -o "$BUILD/ramboot.bin"
fi
echo "    stage2.bin (финальный): $(stat -c%s "$BUILD/stage2.bin") байт, kernel LBA=$KERNEL_LBA"

echo "==> [6b/8] Сверяем карту разделов (partition_map, make_deix_fs)"
python3 tools/check_partition_map.py

echo "==> [7/8] Склеиваем итоговый образ диска"

DISK_IMG="$BUILD/deix_disk.img"
# Выравниваем stage2 до целого числа секторов (иначе kernel.bin в склейке
# смещается на невыровненный байт, и stage2 читает его со сдвигом -> мусор).
STAGE2_SIZE=$(stat -c%s "$BUILD/stage2.bin")
STAGE2_PAD_BYTES=$(( (512 - STAGE2_SIZE % 512) % 512 ))
if [ "$STAGE2_PAD_BYTES" -gt 0 ]; then
    python3 -c "
data = open('$BUILD/stage2.bin','rb').read()
pad = (512 - len(data) % 512) % 512
open('$BUILD/stage2.pad.bin','wb').write(data + b'\x00' * pad)
"
else
    cp "$BUILD/stage2.bin" "$BUILD/stage2.pad.bin"
fi
cat "$BUILD/boot_sector.bin" "$BUILD/stage2.pad.bin" "$BUILD/ramboot.bin" "$BUILD/kernel.bin" > "$DISK_IMG"

# Дополняем образ нулями до размера, кратного 512, а затем до большего из:
#   - конца последнего раздела (/userdata: LBA 12800 + 5632 = 18432 сект);
#   - окна RAM-диска (ramboot/boot_sector_iso читают ровно 20480 сект = 10 МиБ).
python3 - "$DISK_IMG" <<'PYEOF'
import sys, os
path = sys.argv[1]
size = os.path.getsize(path)
pad = (512 - size % 512) % 512
if pad:
    with open(path, "ab") as f:
        f.write(b"\x00" * pad)

PARTITIONS_END = 12800 + 5632   # сверяется tools/check_partition_map.py
RAMDISK_WINDOW = 20480          # boot/ramboot.asm RAMDISK_SECTORS
min_size = max(PARTITIONS_END, RAMDISK_WINDOW) * 512
size = os.path.getsize(path)
if size < min_size:
    with open(path, "ab") as f:
        f.write(b"\x00" * (min_size - size))
PYEOF

echo "==> [7b/8] Форматируем разделы: MBR + ext2 + НАСТОЯЩИЙ EROFS"
# make_deix_fs.py пишет реальную MBR-разметку, ext2-том и EROFS-образы
# (магия 0xE0F5E1E2, проходят fsck.erofs).
python3 tools/make_deix_fs.py "$DISK_IMG" --kernel-lba=$KERNEL_LBA

echo "==> [8/8] Компилируем ISO-загрузчик (RAM-диск 10 МиБ)"
# Отдельный маленький загрузчик (boot/boot_sector_iso.asm): BIOS грузит
# весь boot-образ по 0x7c00, стаб копирует stage2 на 0x10000, kernel.bin
# на 0x100000 и ПОЛНЫЙ 10-МиБ образ диска (RAM-диск) на 0x2000000 — ядро
# читает все разделы из памяти и работает с любого носителя (Ventoy/USB).
STAGE2_PAD_SIZE=$(stat -c%s "$BUILD/stage2.pad.bin")
KERNEL_SIZE=$(stat -c%s "$BUILD/kernel.bin")
STAGE2_DWORDS=$(( STAGE2_PAD_SIZE / 4 ))
KERNEL_DWORDS=$(( (KERNEL_SIZE + 3) / 4 ))
nasm -f bin -D STAGE2_SIZE_DWORDS=$STAGE2_DWORDS -D KERNEL_SIZE_DWORDS=$KERNEL_DWORDS \
    -D RAMDISK_SIZE_DWORDS=2621440 \
    boot/boot_sector_iso.asm -o "$BUILD/boot_sector_iso.bin"

echo "==> [8b] Релизные артефакты (DeiX 0.2.1-beta x86_64)"
DEIX_VERSION="0.2.1-beta"
cp "$DISK_IMG" "$BUILD/deix-$DEIX_VERSION-x86_64.img"
if [ -f "$BUILD/deix.iso" ]; then
    cp "$BUILD/deix.iso" "$BUILD/deix-$DEIX_VERSION-x86_64.iso"
fi
(
    cd "$BUILD"
    : > SHA256SUMS
    for f in deix-$DEIX_VERSION-x86_64.img deix-$DEIX_VERSION-x86_64.iso kernel.bin boot_sector.bin stage2.bin ramboot.bin; do
        [ -f "$f" ] && sha256sum "$f" >> SHA256SUMS
    done
    echo "    SHA256SUMS:"
    sed 's/^/      /' SHA256SUMS
)

echo ""
echo "==> Готово! Образ диска: $DISK_IMG"
if [ -f "$BUILD/deix.iso" ]; then
    echo "==> Готово! ISO-образ:   $BUILD/deix.iso"
fi
echo "==> Запуск (диск):  ./run.sh"
echo "==> Запуск (ISO):   qemu-system-x86_64 -cdrom $BUILD/deix.iso -m 256M"
