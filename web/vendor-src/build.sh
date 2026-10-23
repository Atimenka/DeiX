#!/usr/bin/env bash
# Пересобирает public/vendor/three-hero.js: сцена hero3d.src.js + только
# нужные части three.js (tree-shaking), одним минифицированным файлом.
# Нужен интернет (npm). Результат коммитится: у сайта нет шага сборки.
set -euo pipefail
cd "$(dirname "$0")"
tmp="$(mktemp -d)"
cp hero3d.src.js "$tmp/"
(cd "$tmp" && npm init -y >/dev/null && npm i three@0.186.1 esbuild >/dev/null 2>&1 \
  && npx esbuild hero3d.src.js --bundle --minify --format=iife --global-name=DeixHero \
       --target=es2020 --legal-comments=none --outfile=out.js)
cp "$tmp/out.js" ../public/vendor/three-hero.js
rm -rf "$tmp"
ls -la ../public/vendor/three-hero.js
