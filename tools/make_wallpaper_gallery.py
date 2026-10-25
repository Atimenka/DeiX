#!/usr/bin/env python3
"""Галерея обоев сайта: картинки и страницы /wallpapers и /en/wallpapers.

Для каждой исходной картинки (PNG/JPEG любого размера, пропорции 4:3)
делает два файла в web/public/gallery/:
  <имя>.jpg      — превью для страницы, 640x480;
  os/<имя>.png   — картинка для команды `wallpaper <url>` в DeiX OS: размер
                   подобран под лимит файла ext2 в ОС (BMP 24 бита до
                   270000 байт), поэтому ОС ставит её без уменьшения.
Затем пишет обе HTML-страницы.

Нужен Pillow (только для этого скрипта, в зависимости проекта не входит):
    python3 tools/make_wallpaper_gallery.py <каталог с исходниками>
Имена исходных файлов — <slug>.png из списка ITEMS ниже.
"""

import html
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PUBLIC = os.path.join(ROOT, "web", "public")
OS_SIZE = (344, 258)
PREVIEW_SIZE = (640, 480)
BASE_URL = "https://deix-sepia.vercel.app"
BMP_BUDGET = 270_000

# slug, русское название, английское название, русское описание, английское описание
ITEMS = [
    ("aurora-lake", "Сияние над озером", "Aurora over the lake",
     "Зелёное северное сияние над горным озером", "Green aurora over a mountain lake"),
    ("deep-ocean", "Глубины океана", "Deep ocean",
     "Светящиеся медузы в тёмной воде", "Glowing jellyfish in dark water"),
    ("ringed-planet", "Планета с кольцами", "Ringed planet",
     "Газовый гигант над горизонтом спутника", "A gas giant above a moon's horizon"),
    ("sakura-mist", "Сакура в тумане", "Sakura in the mist",
     "Цветущая ветка и горы в утреннем тумане", "Blossoming branch and mountains in morning mist"),
    ("alpine-dawn", "Альпийский рассвет", "Alpine dawn",
     "Заснеженные вершины в первых лучах солнца", "Snowy peaks in the first sunlight"),
    ("liquid-glass", "Жидкое стекло", "Liquid glass",
     "Переливающиеся ленты из стекла", "Shimmering ribbons of glass"),
    ("lagoon", "Лагуна", "Lagoon",
     "Остров с бирюзовой лагуной сверху", "An island with a turquoise lagoon from above"),
    ("rainy-street", "Дождливая улица", "Rainy street",
     "Ночная улица с отражениями фонарей в лужах", "A night street with lamps reflected in puddles"),
    ("autumn-path", "Осенняя тропа", "Autumn path",
     "Лесная тропа в золотых лучах", "A forest path in golden sunbeams"),
    ("low-poly", "Полигональные горы", "Low-poly mountains",
     "Слоистые горы в графичном стиле", "Layered mountains in a graphic style"),
    ("desert-night", "Пустыня ночью", "Desert at night",
     "Дюны под Млечным Путём", "Dunes under the Milky Way"),
    ("volcano-glow", "Огненный вулкан", "Glowing volcano",
     "Извержение вулкана в сумерках", "A volcano erupting at dusk"),
    ("cyber-city", "Неоновый город", "Neon city",
     "Футуристичный город в неоновых огнях", "A futuristic city in neon lights"),
    ("tulip-field", "Поле тюльпанов", "Tulip field",
     "Тюльпаны до горизонта и мельница на закате", "Tulips to the horizon and a windmill at sunset"),
    ("nebula-pillars", "Столпы туманности", "Nebula pillars",
     "Колонны космического газа и звёзды", "Pillars of cosmic gas and stars"),
    ("ice-cave", "Ледяная пещера", "Ice cave",
     "Голубой лёд и свет у выхода из пещеры", "Blue ice and light at the cave mouth"),
    ("bamboo-fog", "Бамбук в тумане", "Bamboo in fog",
     "Каменная тропа в бамбуковом лесу", "A stone path in a bamboo forest"),
    ("abstract-waves", "Абстрактные волны", "Abstract waves",
     "Атласные волны от синего к розовому", "Satin waves from blue to pink"),
    ("sunset-sea", "Закат над морем", "Sunset over the sea",
     "Золотая дорожка и парусник на горизонте", "A golden path and a sailboat on the horizon"),
    ("space-station", "Орбитальная станция", "Space station",
     "Станция над Землёй на рассвете", "A station above the Earth at sunrise"),
]

TEXT = {
    "ru": {
        "lang": "ru",
        "title": "Обои — DeiX OS",
        "h1": 'Обои <span class="grad">DeiX</span>',
        "lead": "Двадцать картинок, созданных нейросетью. Скачайте PNG для своего рабочего стола или поставьте обои прямо в DeiX OS одной командой.",
        "download": "Скачать PNG",
        "command": "Команда для DeiX OS",
        "how": "Как поставить в DeiX OS",
        "how_hint": "Нажмите «Команда для DeiX OS» и вставьте её в терминал системы. ОС скачает картинку по HTTPS, сохранит её в /userdata/wallpaper.bmp и включит стиль обоев Custom. Файлы подготовлены под лимит файла в ОС (268 КиБ), поэтому ОС их не уменьшает. Нужна работающая сеть: в QEMU это флаги -netdev user и -device rtl8139.",
        "feature": "Картинки созданы нейросетью и свободны для использования в DeiX OS.",
        "preload": '\n  <link rel="preload" href="/vendor/fonts/inter-cyrillic.woff2" as="font" type="font/woff2" crossorigin>',
        "canon": "/wallpapers",
    },
    "en": {
        "lang": "en",
        "title": "Wallpapers — DeiX OS",
        "h1": 'DeiX <span class="grad">wallpapers</span>',
        "lead": "Twenty AI-generated pictures. Download a PNG for your own desktop, or set one in DeiX OS with a single command.",
        "download": "Download PNG",
        "command": "Command for DeiX OS",
        "how": "How to set one in DeiX OS",
        "how_hint": "Press “Command for DeiX OS” and paste it into the system terminal. The OS downloads the picture over HTTPS, saves it to /userdata/wallpaper.bmp and switches the wallpaper style to Custom. The files are sized for the OS file limit (268 KiB), so the OS does not shrink them. A working network is required: in QEMU that is the -netdev user and -device rtl8139 flags.",
        "feature": "The pictures are AI-generated and free to use in DeiX OS.",
        "preload": "",
        "canon": "/en/wallpapers",
    },
}


def bmp24_size(w, h):
    return 54 + h * ((w * 3 + 3) // 4 * 4)


def build_images(src_dir):
    from PIL import Image, ImageOps

    out = os.path.join(PUBLIC, "gallery")
    os.makedirs(os.path.join(out, "os"), exist_ok=True)
    assert bmp24_size(*OS_SIZE) <= BMP_BUDGET
    for slug, *_ in ITEMS:
        path = os.path.join(src_dir, slug + ".png")
        img = Image.open(path).convert("RGB")
        ImageOps.fit(img, PREVIEW_SIZE, Image.LANCZOS).save(
            os.path.join(out, slug + ".jpg"), quality=82, optimize=True, progressive=True)
        ImageOps.fit(img, OS_SIZE, Image.LANCZOS).save(
            os.path.join(out, "os", slug + ".png"), optimize=True)


def card(lang, slug, ru_t, en_t, ru_d, en_d):
    t = TEXT[lang]
    title, desc = (ru_t, ru_d) if lang == "ru" else (en_t, en_d)
    command = "wallpaper %s/gallery/os/%s.png" % (BASE_URL, slug)
    return (
        '      <figure class="panel wp-card">\n'
        '        <a class="wp-shot" href="/gallery/%(slug)s.jpg"><img src="/gallery/%(slug)s.jpg" alt="%(alt)s" width="640" height="480" loading="lazy"></a>\n'
        '        <figcaption>\n'
        '          <h3>%(title)s</h3>\n'
        '          <p class="hint">%(alt)s</p>\n'
        '          <div class="row-gap">\n'
        '            <a class="btn small primary" href="/gallery/os/%(slug)s.png" download>%(download)s</a>\n'
        '            <button class="btn small" type="button" data-copy="%(command)s">%(cmd)s</button>\n'
        '          </div>\n'
        '        </figcaption>\n'
        '      </figure>\n'
    ) % {
        "slug": slug, "alt": html.escape(desc), "title": html.escape(title),
        "download": t["download"], "command": html.escape(command), "cmd": t["command"],
    }


def page(lang):
    t = TEXT[lang]
    cards = "".join(card(lang, *item) for item in ITEMS)
    return """<!doctype html>
<html lang="%(lang)s" data-theme="dark">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>%(title)s</title>
  <meta name="theme-color" content="#0b0b0c">
  <link rel="alternate" hreflang="ru" href="/wallpapers">
  <link rel="alternate" hreflang="en" href="/en/wallpapers">
  <link rel="icon" href="/favicon.svg" type="image/svg+xml">
  <script src="/theme.js"></script>
  <link rel="preload" href="/vendor/fonts/inter-latin.woff2" as="font" type="font/woff2" crossorigin>%(preload)s
  <link rel="stylesheet" href="/style.css">
</head>
<body data-page="wallpapers">
  <main class="page wrap">
    <div class="page-head">
      <h1>%(h1)s</h1>
      <p>%(lead)s</p>
    </div>

    <div class="wp-grid" id="gallery">
%(cards)s    </div>

    <div class="panel">
      <h3>%(how)s</h3>
      <p class="hint">%(how_hint)s</p>
      <p class="hint">%(feature)s</p>
    </div>
  </main>
  <script src="/i18n.js"></script>
  <script src="/site.js"></script>
  <script src="/motion.js" defer></script>
  <script src="/wallpapers.js"></script>
</body>
</html>
""" % dict(t, cards=cards)


def main():
    if len(sys.argv) > 2:
        sys.exit(__doc__)
    if len(sys.argv) == 2:
        build_images(sys.argv[1])
    for lang, rel in (("ru", "wallpapers.html"), ("en", os.path.join("en", "wallpapers.html"))):
        with open(os.path.join(PUBLIC, rel), "w", encoding="utf-8") as f:
            f.write(page(lang))
    print("ok")


if __name__ == "__main__":
    main()
