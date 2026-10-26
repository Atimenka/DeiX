#!/usr/bin/env bash
# make-update.sh — собирает пакет обновления DeiX: сжатую нагрузку (.payload)
# и подписанный манифест (.manifest, внутри каноничный JSON).
#
#   ./make-update.sh --notes "что изменилось"
#
# Порядок: ключ подписи (создаётся при первом запуске) -> ./build.sh с этим
# ключом (публичная часть вшивается в ядро) -> упаковка образа с вашими
# параметрами -> проверка пакета. Готовые файлы лежат в build/update/ и
# загружаются в /admin -> «Обновления ОС».
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

usage() {
    cat <<'EOF'
Использование: ./make-update.sh [параметры]

  --notes ТЕКСТ     заметки к выпуску (до 200 символов: A-Za-z0-9 .,:;_/=+()-)
  --channel КАНАЛ   stable (по умолчанию) или beta
  --version ВЕРСИЯ  подпись версии в манифесте (по умолчанию из build.sh)
  --min-build N     самая старая сборка, с которой можно ставить это обновление
  --op ОПЕРАЦИЯ     операция над /userdata (можно несколько раз):
                    mkdir:/userdata/путь | remove:/userdata/путь |
                    write:/userdata/путь=hex
  --schema N        версия схемы /userdata после обновления (по умолчанию 1)
  --key ФАЙЛ        приватный ключ (по умолчанию ~/.deix/update_key.pem)
  --build-id N      идентификатор сборки (по умолчанию время HEAD-коммита)
  --profile P       release (по умолчанию) или fast — профиль ./build.sh
  --out-dir DIR     куда класть пакет (по умолчанию build/update)
  --no-build        не пересобирать: упаковать готовый build/deix_disk.img
                    (или файл из DEIX_IMAGE)
  -h, --help        эта справка

Идентификатор сборки обязан расти от выпуска к выпуску: ОС ставит только
сборку новее установленной. Для несохранённых правок задайте --build-id.
EOF
}

NOTES=""
CHANNEL="stable"
VERSION=""
MIN_BUILD="0"
SCHEMA="1"
KEY="${DEIX_UPDATE_KEY:-$HOME/.deix/update_key.pem}"
BUILD_ID_ARG=""
PROFILE=""
OUT_DIR="$ROOT/build/update"
DO_BUILD=1
OPS=()

need_value() {
    if [ "$#" -lt 2 ] || [ -z "$2" ]; then
        echo "ошибка: у $1 нет значения" >&2
        exit 2
    fi
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --notes)     need_value "$@"; NOTES="$2"; shift 2 ;;
        --channel)   need_value "$@"; CHANNEL="$2"; shift 2 ;;
        --version)   need_value "$@"; VERSION="$2"; shift 2 ;;
        --min-build) need_value "$@"; MIN_BUILD="$2"; shift 2 ;;
        --schema)    need_value "$@"; SCHEMA="$2"; shift 2 ;;
        --op)        need_value "$@"; OPS+=("$2"); shift 2 ;;
        --key)       need_value "$@"; KEY="$2"; shift 2 ;;
        --build-id)  need_value "$@"; BUILD_ID_ARG="$2"; shift 2 ;;
        --profile)   need_value "$@"; PROFILE="$2"; shift 2 ;;
        --out-dir)   need_value "$@"; OUT_DIR="$2"; shift 2 ;;
        --no-build)  DO_BUILD=0; shift ;;
        -h|--help)   usage; exit 0 ;;
        *) echo "ошибка: неизвестный параметр $1" >&2; usage >&2; exit 2 ;;
    esac
done

TOOL="python3 tools/make_update.py"

if [ ! -f "$KEY" ]; then
    echo "==> Ключа подписи нет: создаю $KEY"
    $TOOL keygen --key "$KEY"
    echo
    echo "    СДЕЛАЙТЕ КОПИЮ ЭТОГО ФАЙЛА. Публичная часть вшивается в ядро каждой"
    echo "    сборки: потеряете ключ — уже установленные системы перестанут принимать"
    echo "    ваши обновления. Утечка ключа позволяет любому подписать обновление."
    echo
fi

if [ "$DO_BUILD" -eq 1 ]; then
    echo "==> Сборка ОС (ключ $KEY)"
    export DEIX_UPDATE_KEY="$KEY"
    export DEIX_SKIP_PACK=1
    if [ -n "$PROFILE" ]; then
        export DEIX_PROFILE="$PROFILE"
    fi
    # DEIX_BUILD_ID не экспортируем в глобальную среду — он leaks в
    # следующую сессию и следующая сборка reuse тот же id. Передаём
    # inline в build.sh (option_env! в ядре видит переменную окружения
    # cargo, не shell).
    if [ -n "$BUILD_ID_ARG" ]; then
        DEIX_BUILD_ID="$BUILD_ID_ARG" ./build.sh
    else
        ./build.sh
    fi
fi

IMAGE="${DEIX_IMAGE:-$ROOT/build/deix_disk.img}"
for f in "$IMAGE" "$ROOT/build/build_id" "$ROOT/build/version" "$ROOT/build/update_pubkey.bin"; do
    if [ ! -f "$f" ]; then
        echo "ошибка: нет $f — запустите без --no-build" >&2
        exit 1
    fi
done

TMP_PUB="$(mktemp)"
trap 'rm -f "$TMP_PUB"' EXIT
$TOOL pubkey --key "$KEY" --out "$TMP_PUB"
if ! cmp -s "$TMP_PUB" "$ROOT/build/update_pubkey.bin"; then
    echo "ошибка: образ собран с другим ключом (в ядре вшит чужой или нулевой ключ)." >&2
    echo "        ОС с таким ядром не примет пакет, подписанный $KEY. Запустите без --no-build." >&2
    exit 1
fi

BUILD_ID="$(tr -d '[:space:]' < "$ROOT/build/build_id")"
if [ -n "$BUILD_ID_ARG" ] && [ "$BUILD_ID" != "$BUILD_ID_ARG" ]; then
    echo "ошибка: готовый образ собран с build id $BUILD_ID, а запрошен $BUILD_ID_ARG." >&2
    echo "        Запустите без --no-build." >&2
    exit 1
fi
if [ -z "$VERSION" ]; then
    VERSION="$(tr -d '[:space:]' < "$ROOT/build/version")"
fi

PACK_ARGS=(pack --image "$IMAGE" --key "$KEY" --version "$VERSION" --build "$BUILD_ID"
    --channel "$CHANNEL" --min-build "$MIN_BUILD" --schema "$SCHEMA" --notes "$NOTES"
    --out-dir "$OUT_DIR")
for op in "${OPS[@]+"${OPS[@]}"}"; do
    PACK_ARGS+=(--op "$op")
done

echo "==> Упаковка: версия $VERSION, сборка $BUILD_ID, канал $CHANNEL"
$TOOL "${PACK_ARGS[@]}"

MANIFEST="$OUT_DIR/deix-update-$VERSION.manifest"
PAYLOAD="$OUT_DIR/deix-update-$VERSION.payload"

echo "==> Проверка пакета"
$TOOL verify --manifest "$MANIFEST" --payload "$PAYLOAD" --key "$KEY"
echo
$TOOL inspect --manifest "$MANIFEST" --key "$KEY"
echo
echo "Готово. Загрузите в /admin -> «Обновления ОС»:"
echo "  $MANIFEST"
echo "  $PAYLOAD"
echo "Полный манифест в JSON: $TOOL inspect --manifest $MANIFEST --json"
