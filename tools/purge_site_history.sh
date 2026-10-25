#!/usr/bin/env bash
# Полностью стирает web/, supabase/ и tools/make_wallpaper_gallery.py из
# ИСТОРИИ всех веток DeiX (код вынесен в приватный репозиторий владельца).
#
# Запускать НЕ в этом клоне, а в свежем зеркале с правами на force push:
#   git clone --mirror https://github.com/Atimenka/DeiX deix-purge
#   cd deix-purge
#   ../DeiX/tools/purge_site_history.sh   # или скопировать скрипт рядом
# Нужен git-filter-repo: pip install git-filter-repo
set -euo pipefail

if ! git rev-parse --is-bare-git-directory >/dev/null 2>&1;
then
  echo "ОШИБКА: запускайте внутри mirror-клона (git clone --mirror)" >&2
  exit 1
fi

git filter-repo --invert-paths \
  --path web/ --path supabase/ --path tools/make_wallpaper_gallery.py \
  --force

git push --mirror --force

echo "Готово. Старые объекты на GitHub станут недоступны после сборки"
echo "мусора; секреты в этих каталогах не коммитились, ротация не нужна."
