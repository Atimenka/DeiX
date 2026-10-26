#!/usr/bin/env bash
# Сборка DeiX в classic BIOS-загружаемый дисковый образ (без GRUB — только
# nasm + ld + objcopy, поэтому подходит и для Termux на телефоне).
#
# Схема: MBR + файлы загрузчика и ядра внутри /system (EROFS), а не сырая
# область диска. Загрузчик находит файлы по карте загрузки (docs/BOOTMAP.md):
#   MBR (boot_sector.bin) читает карту, затем stage2 (база 0x10000) и ramboot;
#   ramboot читает kernel.bin (база 0x100000) и передаёт управление stage2,
#   который включает long mode.
#
# Служебные области и разделы (источник истины — src/partition_map.rs):
#   LBA 0           — MBR (boot_sector.bin)
#   LBA 1..2046     — свободны (нули)
#   LBA 2047        — карта загрузки DEIXKIMG v2 (участки stage2/ramboot/ядра)
#   LBA 2048..2111  — область аварийного дампа (DEIXPNIC, src/diag/panic.rs)
#   LBA 4000        — управляющий сектор обновления (DEIXUPDC, src/update.rs)
#   LBA 4096..36863 — /system   (EROFS RO, 32768 сект): boot/, kernel/, etc/, ...
#   LBA 36864..69631 — /userdata (EXT2 RW, 32768 сект)
set -euo pipefail

source "$HOME/.cargo/env" 2>/dev/null || true

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

BUILD="$ROOT/build"
mkdir -p "$BUILD"

DEIX_VERSION="0.3.0-beta"

# Идентификатор сборки для центра обновлений: монотонный счётчик
# tools/next_build_id.py — каждая сборка строго новее предыдущей, даже если
# коммит и секунда те же. Явный DEIX_BUILD_ID тоже двигает счётчик (--mark),
# чтобы следующая автосборка не оказалась старее заданной вручную.
if [ -n "${DEIX_BUILD_ID:-}" ]; then
    python3 tools/next_build_id.py --mark "$DEIX_BUILD_ID"
else
    DEIX_BUILD_ID="$(python3 tools/next_build_id.py)"
fi
export DEIX_BUILD_ID
printf '%s\n' "$DEIX_BUILD_ID" > "$BUILD/build_id"
printf '%s\n' "$DEIX_VERSION" > "$BUILD/version"

echo "==> [1/8] Ассемблируем stage2 (32-бит вход + long mode + ATA-ридер ядра)"
# stage2 не зависит от размера и расположения ядра: ядро уже лежит в 0x100000
# (его грузит ramboot), stage2 включает long mode и прыгает туда.
nasm -f elf64 -D KERNEL_SIZE_DWORDS=262144 boot/stage2.asm -o "$BUILD/stage2.o"
nasm -f elf64 boot/long_mode_init.asm   -o "$BUILD/long_mode_init.o"

echo "==> [1b/8] Линкуем stage2 (загрузчик, база 0x10000)"
ld -n --gc-sections -T boot/linker_stage2.ld -o "$BUILD/stage2.elf" "$BUILD/stage2.o"
strip --strip-all "$BUILD/stage2.elf" -o "$BUILD/stage2.stripped.elf"
objcopy -O binary "$BUILD/stage2.stripped.elf" "$BUILD/stage2.bin"

STAGE2_SIZE=$(stat -c%s "$BUILD/stage2.bin")
STAGE2_SECTORS=$(( (STAGE2_SIZE + 511) / 512 ))
echo "    stage2.bin (загрузчик): $STAGE2_SIZE байт => $STAGE2_SECTORS секторов"

echo "==> [1c/8] Собираем ramboot (читает ядро из /system по карте загрузки в 0x100000)"
nasm -f bin boot/ramboot.asm -o "$BUILD/ramboot.bin"
RAMBOOT_SIZE=$(stat -c%s "$BUILD/ramboot.bin")
RAMBOOT_SECTORS=$(( RAMBOOT_SIZE / 512 ))
if [ $(( RAMBOOT_SIZE % 512 )) -ne 0 ] || [ "$RAMBOOT_SECTORS" -gt 8 ]; then
    echo "ОШИБКА: ramboot.bin должен занимать целое число секторов (не больше 8), а получилось $RAMBOOT_SIZE байт"
    exit 1
fi
echo "    ramboot.bin: $RAMBOOT_SIZE байт => $RAMBOOT_SECTORS секторов"

echo "==> [2/8] Собираем MBR-загрузчик (читает карту загрузки, stage2 и ramboot)"
nasm -f bin boot/boot_sector.asm -o "$BUILD/boot_sector.bin"

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

echo "==> [2d/8] Готовим загрузочное лого (assets/logo.dxlg или assets/logo.png -> DXLG+RLE)"
# Декодера PNG в ядре нет, а сырой RGB не влезает в лимит kernel.bin,
# поэтому лого хранится как палитра + RLE. Готовый сглаженный assets/logo.dxlg
# собирается tools/make_logo_aa.py (нужен Pillow); без него PNG
# конвертируется грубо, в 5 цветов (tools/make_logo.py).
if [ -f assets/logo.dxlg ]; then
    cp assets/logo.dxlg "$BUILD/logo.dxlg"
elif [ -f assets/logo.png ]; then
    python3 tools/make_logo.py assets/logo.png "$BUILD/logo.dxlg" 128
else
    echo "    ОШИБКА: assets/logo.dxlg и assets/logo.png не найдены"
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
conv_voice() {  # $1 = входной wav, $2 = имя dps
    if [ -f "$1" ]; then
        python3 tools/wav2dps.py "$1" "$SND/$2" 8000 --voice \
            || echo "    warn: не удалось сконвертировать $1 — голоса не будет"
    else
        echo "    warn: $1 не найден — голос '$2' будет недоступен"
    fi
}
conv_voice "assets/voice/welcome.wav" welcome.dps
conv_voice "assets/voice/update.wav"  update.dps
conv_voice "assets/voice/bye.wav"     bye.dps

# Публичный ключ обновлений вшивается в ядро. Без приватного ключа
# (tools/make_update.py keygen) вшивается нулевой — обновления выключены.
UPDATE_KEY="${DEIX_UPDATE_KEY:-$HOME/.deix/update_key.pem}"
if [ -f "$UPDATE_KEY" ]; then
    python3 tools/make_update.py pubkey --key "$UPDATE_KEY" --out "$BUILD/update_pubkey.bin"
    echo "    центр обновлений: ключ $UPDATE_KEY, сборка $DEIX_BUILD_ID"
else
    python3 tools/make_update.py pubkey --zero --out "$BUILD/update_pubkey.bin"
    echo "    центр обновлений выключен: нет ключа $UPDATE_KEY (см. docs/UPDATE.md)"
fi

echo "==> [3/8] Собираем ядро на Rust (nightly, build-std, target: x86_64-unknown-none)"
# DEIX_PROFILE=fast ./build.sh — быстрая сборка для отладки: без LTO, 16 единиц
# кодогенерации, инкрементальная (повторная сборка после правки одного файла
# в разы быстрее). Бинарник больше релизного; если не влезет в лимиты ниже,
# сборка остановится с ошибкой — тогда соберите обычным ./build.sh.
PROFILE="${DEIX_PROFILE:-release}"
case "$PROFILE" in
    release|fast) ;;
    *) echo "DEIX_PROFILE: допустимо release или fast, получено '$PROFILE'"; exit 1 ;;
esac
echo "    профиль cargo: $PROFILE"
cargo +nightly build --profile "$PROFILE"

KERNEL_LIB="$ROOT/target/x86_64-unknown-none/$PROFILE/libdeix_kernel.a"
if [ ! -f "$KERNEL_LIB" ]; then
    echo "Не найден $KERNEL_LIB — сборка Rust-части не удалась"
    exit 1
fi

echo "==> [4/8] Линкуем kernel.bin (long_mode_init + ядро, база 0x100000)"
# Права сегментов (text R+X, rodata R, data/bss R+W) заданы в PHDRS
# линкер-скрипта — RWX-сегментов нет, предупреждать ld не о чем.
ld -n --gc-sections -T boot/linker_kernel.ld -o "$BUILD/kernel.elf" \
    "$BUILD/long_mode_init.o" \
    "$KERNEL_LIB"

strip --strip-all "$BUILD/kernel.elf" -o "$BUILD/kernel.stripped.elf"
objcopy -O binary "$BUILD/kernel.stripped.elf" "$BUILD/kernel.bin"

KERNEL_SIZE=$(stat -c%s "$BUILD/kernel.bin")
KERNEL_SECTORS=$(( (KERNEL_SIZE + 511) / 512 ))
echo "    kernel.bin: $KERNEL_SIZE байт => $KERNEL_SECTORS секторов"

KERNEL_MAX_SECTORS=2048
if [ "$KERNEL_SECTORS" -gt "$KERNEL_MAX_SECTORS" ]; then
    echo "ОШИБКА: kernel.bin ($KERNEL_SECTORS сект) > лимита ($KERNEL_MAX_SECTORS)."
    echo "Ядро грузится в 0x100000..0x200000 (ниже кучи на 0x300000): лимит 1 МиБ."
    exit 1
fi

# .bss ядра (куча 16 МиБ и статические буферы) обязана кончаться ниже
# Ring 3 (0x1400000, src/usermode.rs): иначе ядро затирает собственную
# память и загрузка не доходит до оболочки.
BSS_END_HEX=$(nm "$BUILD/kernel.elf" | awk '$3=="__kernel_bss_end"{print $1}')
BSS_END=$(( 16#${BSS_END_HEX:-0} ))
BSS_LIMIT=$(( 0x1400000 ))
printf '    .bss ядра заканчивается на 0x%X (лимит 0x%X, запас %d КиБ)\n' \
    "$BSS_END" "$BSS_LIMIT" $(( (BSS_LIMIT - BSS_END) / 1024 ))
if [ "$BSS_END" -eq 0 ] || [ "$BSS_END" -gt "$BSS_LIMIT" ]; then
    echo "ОШИБКА: .bss ядра не помещается ниже 0x1400000 (или __kernel_bss_end не найден)."
    exit 1
fi

echo "==> [5/8] Проверяем MBR-загрузчик (512 байт)"
BOOT_SIZE=$(stat -c%s "$BUILD/boot_sector.bin")
if [ "$BOOT_SIZE" -ne 512 ]; then
    echo "ОШИБКА: boot_sector.bin должен быть ровно 512 байт, а получилось $BOOT_SIZE"
    exit 1
fi

echo "==> [6/8] Сверяем карту разделов (partition_map, make_deix_fs)"
python3 tools/check_partition_map.py

echo "==> [7/8] Склеиваем итоговый образ диска"

# DEIX_DISK_IMG — собрать образ диска в ДРУГОЙ файл, не трогая build/deix_disk.img
# (так build-update-image.sh готовит образ для обновления).
DISK_IMG="${DEIX_DISK_IMG:-$BUILD/deix_disk.img}"
# Образ начинается с MBR; stage2, ramboot и ядро кладёт в /system
# make_deix_fs.py на шаге 7b (с картой загрузки в LBA 2047). Для ISO-стаба
# (шаг 8) stage2 нужен, выровненный до сектора.
python3 -c "
data = open('$BUILD/stage2.bin','rb').read()
pad = (512 - len(data) % 512) % 512
open('$BUILD/stage2.pad.bin','wb').write(data + b'\\x00' * pad)
"
cp "$BUILD/boot_sector.bin" "$DISK_IMG"

# Дополняем образ нулями до размера, кратного 512, а затем до большего из:
#   - конца последнего раздела (/userdata: LBA 36864 + 32768 = 69632 сект);
#   - окна RAM-диска ISO-стаба (boot_sector_iso читает 40960 сект = 20 МиБ).
python3 - "$DISK_IMG" <<'PYEOF'
import sys, os
path = sys.argv[1]
size = os.path.getsize(path)
pad = (512 - size % 512) % 512
if pad:
    with open(path, "ab") as f:
        f.write(b"\x00" * pad)

PARTITIONS_END = 36864 + 32768  # сверяется tools/check_partition_map.py
RAMDISK_WINDOW = 40960          # окно RAM-диска boot_sector_iso.asm
min_size = max(PARTITIONS_END, RAMDISK_WINDOW) * 512
size = os.path.getsize(path)
if size < min_size:
    with open(path, "ab") as f:
        f.write(b"\x00" * (min_size - size))
PYEOF

echo "==> [7b/8] Форматируем разделы: MBR + ext2 + НАСТОЯЩИЙ EROFS"
# make_deix_fs.py пишет реальную MBR-разметку, ext2-том и EROFS-образы
# (магия 0xE0F5E1E2, проходят fsck.erofs).
python3 tools/make_deix_fs.py "$DISK_IMG"

if [ -f "$UPDATE_KEY" ] && [ -z "${DEIX_SKIP_PACK:-}" ]; then
    echo "==> [7c/8] Пакет обновления (подписанный манифест + нагрузка)"
    python3 tools/make_update.py pack --image "$DISK_IMG" --key "$UPDATE_KEY" \
        --version "$DEIX_VERSION" --build "$DEIX_BUILD_ID" --out-dir "$BUILD/update"
fi

echo "==> [8/8] Компилируем ISO-загрузчик (RAM-диск 20 МиБ)"
# Отдельный маленький загрузчик (boot/boot_sector_iso.asm): BIOS грузит
# весь boot-образ по 0x7c00, стаб копирует stage2 на 0x10000, kernel.bin
# на 0x100000 и ПОЛНЫЙ 20-МиБ образ диска (RAM-диск) на 0x2000000 — ядро
# читает все разделы из памяти и работает с любого носителя (Ventoy/USB).
STAGE2_PAD_SIZE=$(stat -c%s "$BUILD/stage2.pad.bin")
KERNEL_SIZE=$(stat -c%s "$BUILD/kernel.bin")
STAGE2_DWORDS=$(( STAGE2_PAD_SIZE / 4 ))
KERNEL_DWORDS=$(( (KERNEL_SIZE + 3) / 4 ))
nasm -f bin -D STAGE2_SIZE_DWORDS=$STAGE2_DWORDS -D KERNEL_SIZE_DWORDS=$KERNEL_DWORDS \
    -D RAMDISK_SIZE_DWORDS=5242880 \
    boot/boot_sector_iso.asm -o "$BUILD/boot_sector_iso.bin"

echo "==> [8b] Релизные артефакты (DeiX $DEIX_VERSION x86_64)"
if [ -z "${DEIX_DISK_IMG:-}" ]; then
    cp "$DISK_IMG" "$BUILD/deix-$DEIX_VERSION-x86_64.img"
fi
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
