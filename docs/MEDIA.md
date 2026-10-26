# Медиа в DeiX: плееры, форматы и медиапак

Тяжёлая некритичная среда (обои, картинки, демо-видео, музыка, голоса)
больше не живёт в kernel.bin и в /system (EROFS 16 МиБ). Она собрана в
**медиапак** на сайте ОС и автоустанавливается в зашифрованный
`/userdata/media`. Критичное остаётся в /system: загрузчик, `etc/`,
UI-звуки (`media/audio/ui/*.dps`) и иконки приложений (`media/icons/*.dxic`).

## Приложения

| Окно            | Пункт меню/Dock | Что делает |
|-----------------|-----------------|------------|
| Photos          | Photos          | библиотека изображений (`/userdata/media/pictures`, `wallpapers`, `/userdata/downloads`) + просмотр одного файла из Files |
| Video           | Video           | список видео (`/userdata/media/video`, downloads) или воспроизведение файла |
| Music           | Music           | плейлист (`/userdata/media/audio`, `voice`, downloads) + транспорт |

Клавиши в плеерах: `Space` — пауза/повтор, `S` — стоп, `←`/`→` —
предыдущий/следующий трек (Music), `↑`/`↓` — листать библиотеку (Photos).

Двойной клик по файлу в Files открывает его своим приложением по
расширению: изображения (`.png .bmp .jpg .jpeg`) — Photos, видео
(`.dxv .avi`) — Video, аудио (`.wav .dps`) — Music.

## Движок (`src/media.rs`)

Одна фоновая задача `media` (планировщик, `sched::spawn`) владеет
воспроизведением; состояние — атомики (`state()`: IDLE/LOADING/PLAYING/
PAUSED/ERROR/DONE/STOPPED), текущий кадр видео доступен через
`with_frame()`. Звук — HDA (`play_pcm_stereo_48k`), любые частоты
ресемплируются в 48 кГц стерео, громкость 0..100%.

Форматы:

* **DXV1** — собственный контейнер DeiX: заголовок 20 байт
  (`magic, fps u16, w u16, h u16, fmt кадра u8 (0=JPEG,1=PNG),
  fmt звука u8 (0=нет,1=моно,2=стерео), arate u32, кадры u32`),
  далее куски `u8 tag ('V'|'A') + u32 len + данные` во временном порядке;
* **AVI** — Motion JPEG + PCM (списки `movi`);
* **WAV** — PCM 16 бит моно/стерео, 4000..96000 Гц;
* **DPS1** — `magic + u32 rate + u32 samples` и далее u8-отсчёты моно
  (тот же формат, что UI-звуки в /system; медиадвижок, в отличие от
  sound.rs, не ограничивает длительность 30 секундами);
* изображения — PNG (8 бит), **baseline JPEG** (`src/jpeg.rs`,
  без прогрессивности, ≤ 1 Мпикс — ограничение кучи ядра 16 МиБ),
  BMP 24/32 BI_RGB.

## Медиапак (`src/mediapack.rs`)

* источник: `https://deix-sepia.vercel.app/media/**` (статика Vercel,
  API-функции и лимиты Supabase не расходуются);
* манифест `manifest.txt`: `#version N` + строки `path|size|sha256`;
* установка: при первом входе в рабочий стол фоновая задача `media-pack`
  ждёт сеть (до 120 с) и скачивает файлы; каждый проверяется по размеру
  и SHA-256; существующие верные файлы пропускаются (повторный
  `media install` докачивает только недостающее);
* ошибки честные: частичная установка возвращается как ошибка со списком
  файлов и причин; реестр `pack.txt` = `version|files|bytes`;
* обои: `load_system_wallpaper` читает `/userdata/media/wallpapers`,
  запасной путь — `/system/media/wallpapers` (старые образы); без пака
  рисуются процедурные стили.

CLI:

```
media status           — версия пака, файлы по каталогам
media install          — скачать/докачать пак (проверка SHA-256)
media remove           — удалить установленные файлы пака
media play <файл>      — воспроизвести файл движком
media pause | stop     — управление воспроизведением
media vol up | down    — громкость ±10%
```

## Сборка пака

```
python3 tools/make_media_pack.py [--version N]
```

Скрипт кладёт результат в `web/public/media/`: конвертирует
`assets/wallpapers/*.{bmp,png}` в JPEG baseline 960x720 (q82),
`assets/pictures` — в JPEG (q85, ≤800 px), копирует `assets/voice/*.wav`,
синтезирует демо-музыку (`audio/demo_track.wav`, PCM 22050 Гц моно) и
демо-видео (`video/demo.dxv`, 320x180 12 fps, JPEG-кадры + PCM 11025 Гц
моно, 12 с), затем пишет `manifest.txt` с SHA-256 каждого файла.

Версию (`--version`) повышайте при любом изменении содержимого: итог
установки и `media status` показывают именно её.

Проверки: `python3 tools/test_media_pack.py` (форматы против декодеров
ОС: baseline JPEG, DXV1, WAV, манифест, бюджет ≤ 6 МиБ) и
`python3 tools/test_assets.py` (исходники обоев соответствуют списку в
`src/ui/theme.rs`; make_deix_fs.py не кладёт wallpapers/pictures в /system).

Кеширование (web/vercel.json): `manifest.txt` — `no-store` (ОС обязана
видеть свежий манифест), файлы `/media/**` — `max-age=0, must-revalidate`
(CDN проверяет ETag: после обновления пака ОС не может получить устаревшие
байты и упасть на SHA-256).
