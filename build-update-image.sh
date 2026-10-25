#!/usr/bin/env bash
# build-update-image.sh — собирает НОВУЮ сборку ОС в отдельный файл образа
# (по умолчанию build/123.img) и готовит из неё пакет обновления. Рабочий
# образ build/deix_disk.img, который запускает ./run.sh, НЕ изменяется:
# ОС, установленная из него, сможет принять это обновление.
#
#   ./build-update-image.sh [--notes ТЕКСТ] [--build-id N] [--image ФАЙЛ]
#                           [--out-dir DIR] [--channel stable|beta] [--profile fast]
#
# Порядок: проверка ключа -> ./build.sh с DEIX_DISK_IMG (образ в другой файл,
# новый build id) -> упаковка и проверка пакета через make-update.sh.
#
# Ключ подписи должен быть ТЕМ ЖЕ, что при сборке уже установленной ОС
# (~/.deix/update_key.pem или DEIX_UPDATE_KEY): иначе она отвергнет подпись.
# Здесь ключ никогда не создаётся.
#
# Build id по умолчанию — текущее время (секунды Unix): он обязан быть больше,
# чем у установленной ОС. У образа от ./build.sh это время HEAD-коммита.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

IMG="$ROOT/build/123.img"
OUT_DIR="$ROOT/build/update-mini"
NOTES="mini update"
BUILD_ID="$(date +%s)"
CHANNEL="stable"
PROFILE=""
KEY="${DEIX_UPDATE_KEY:-$HOME/.deix/update_key.pem}"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --notes|--build-id|--image|--out-dir|--channel|--profile|--key)
            if [ "$#" -lt 2 ] || [ -z "$2" ]; then
                echo "ошибка: у $1 нет значения" >&2
                exit 2
            fi
            case "$1" in
                --notes)    NOTES="$2" ;;
                --build-id) BUILD_ID="$2" ;;
                --image)    IMG="$2" ;;
                --out-dir)  OUT_DIR="$2" ;;
                --channel)  CHANNEL="$2" ;;
                --profile)  PROFILE="$2" ;;
                --key)      KEY="$2" ;;
            esac
            shift 2 ;;
        -h|--help)
            sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *) echo "ошибка: неизвестный параметр $1" >&2; exit 2 ;;
    esac
done

case "$BUILD_ID" in
    ''|*[!0-9]*) echo "ошибка: build id должен быть целым числом, получено '$BUILD_ID'" >&2; exit 2 ;;
esac

if [ ! -f "$KEY" ]; then
    echo "ошибка: нет ключа подписи $KEY." >&2
    echo "        Нужен ключ, с которым собрана установленная ОС: новый ключ она не примет." >&2
    echo "        Нет ключа вовсе — обновления в той сборке выключены, см. docs/UPDATE.md." >&2
    exit 1
fi

WORK_IMG="$ROOT/build/deix_disk.img"
mkdir -p "$(dirname "$IMG")"
if [ "$(readlink -m "$IMG")" = "$(readlink -m "$WORK_IMG")" ]; then
    echo "ошибка: --image указывает на рабочий образ $WORK_IMG; нужен другой файл." >&2
    exit 2
fi

# Отпечаток рабочего образа до сборки — чтобы убедиться, что он не тронут.
before=""
if [ -f "$WORK_IMG" ]; then
    before="$(sha256sum "$WORK_IMG" | cut -d' ' -f1)"
fi

echo "==> Сборка нового образа: $IMG (build id $BUILD_ID, ключ $KEY)"
if [ -n "$PROFILE" ]; then
    export DEIX_PROFILE="$PROFILE"
fi
DEIX_DISK_IMG="$IMG" DEIX_BUILD_ID="$BUILD_ID" DEIX_UPDATE_KEY="$KEY" DEIX_SKIP_PACK=1 ./build.sh

echo "==> Пакет обновления"
DEIX_IMAGE="$IMG" ./make-update.sh --no-build --key "$KEY" --build-id "$BUILD_ID" \
    --channel "$CHANNEL" --notes "$NOTES" --out-dir "$OUT_DIR"

if [ -n "$before" ]; then
    after="$(sha256sum "$WORK_IMG" | cut -d' ' -f1)"
    if [ "$before" = "$after" ]; then
        echo "==> Рабочий образ $WORK_IMG не изменён."
    else
        echo "ВНИМАНИЕ: рабочий образ $WORK_IMG изменился во время сборки." >&2
        exit 1
    fi
fi
echo "==> Новый образ (для проверки/запуска отдельно): $IMG"
echo "    Файлы для /admin -> «Обновления ОС» лежат в $OUT_DIR"
