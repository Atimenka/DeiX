# Запуск DeiX в Termux на телефоне (iQOO Z9 Turbo)

Загрузчик классический (MBR, BIOS, реальный режим → защищённый → long mode),
GRUB не нужен — собирается через `nasm` + `binutils` (ld/objcopy/strip) +
Rust (nightly).

**Важно про Rust в Termux:** обычный `rustup` (с sh.rustup.rs) в голом
Termux не работает — Termux использует Android/bionic-окружение, а
официальные сборки rustup рассчитаны на glibc. Поэтому ставим Rust не
напрямую в Termux, а внутри полноценного Debian, который поднимаем через
`proot-distro`. Это самый надёжный вариант, и именно в таком окружении я
уже проверил, что вся сборка работает от начала до конца.

## 0. Установи Termux

Ставь **из F-Droid**, не из Google Play (там версия старая и неактуальная):
https://f-droid.org/packages/com.termux/

## 1. Дай Termux доступ к памяти телефона

```bash
termux-setup-storage
```

Появится системный запрос — разреши. После этого `/sdcard` будет доступен
как `~/storage/shared`.

## 2. Поставь proot-distro и разверни Debian

```bash
pkg update -y && pkg upgrade -y
pkg install -y proot-distro
proot-distro install debian
```

Это скачает и распакует полноценный Debian (aarch64, под архитектуру
твоего телефона — никакая эмуляция тут не нужна, работает нативно).

Заходим внутрь:

```bash
proot-distro login debian
```

После этого у тебя обычный `bash` внутри Debian — все команды ниже (до
раздела про VNC) выполняются **уже внутри этого Debian**, а не в исходном
Termux.

## 3. Внутри Debian: ставим инструменты сборки

```bash
apt update && apt upgrade -y
apt install -y nasm binutils qemu-system-x86 curl build-essential python3
```

## 4. Внутри Debian: ставим Rust через rustup

Здесь это обычный Linux/glibc, поэтому rustup работает штатно:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly --profile minimal
source $HOME/.cargo/env
rustup component add rust-src --toolchain nightly
```

Проверка:

```bash
rustc --version   # должно показать nightly
cargo --version
nasm -v
ld --version
objcopy --version
qemu-system-x86_64 --version
```

## 5. Перенеси проект на телефон и внутрь Debian

Ты уже скачал архив `deix.tar.gz` (обычно лежит в `Download`). Файловая
система Termux/Android доступна из proot-distro Debian по тому же пути
`/sdcard`, только надо смотреть, куда именно смонтировано хранилище:

```bash
ls /sdcard 2>/dev/null || ls /storage/emulated/0 2>/dev/null
```

Обычно один из этих путей рабочий. Дальше:

```bash
cd ~
cp /sdcard/Download/deix.tar.gz .   # путь поправь под реальное место файла
tar xzf deix.tar.gz
cd deix
```

Если прямого доступа к `/sdcard` из Debian нет — самый простой обходной
путь: скопировать архив ещё в Termux (`~/storage/shared/Download/`), а
затем зайти в Debian и взять файл оттуда через `/root` bind-mount, либо
просто загрузить архив прямо из Debian через `curl`/`wget`, если он у тебя
где-то выложен (например, временно на файлообменник).

## 6. Собери проект

```bash
chmod +x build.sh run.sh
./build.sh
```

Скрипт делает по порядку:
1. Ассемблирует `stage2.asm` (32-бит вход → long mode) + `long_mode_init.asm`
2. Собирает ядро на Rust (`cargo -Z build-std`, target `x86_64-unknown-none`)
3. Линкует stage2+ядро в один плоский бинарник по адресу `0x10000`
4. Ассемблирует `boot_sector.asm` (MBR, 512 байт) — сам узнаёт, сколько
   секторов надо прочитать с диска, чтобы забрать stage2 целиком
5. Склеивает всё в один файл `build/deix_disk.img` — это "виртуальный
   жёсткий диск" целиком

Если всё ок, увидишь в конце:

```
==> Готово! Образ диска: /root/deix/build/deix_disk.img
```

## 7. Запусти в QEMU

Так как телефон — ARM, а наша ОС собрана под x86_64, QEMU здесь работает
в режиме программной эмуляции (TCG, без аппаратного ускорения) — для такого
крошечного ядра это не проблема, грузится за секунды.

Графического окна в proot-distro/Termux нет, поэтому смотрим экран либо
через VNC, либо снимками.

### Вариант A — через VNC (удобнее, видно живьём)

```bash
qemu-system-x86_64 -drive format=raw,file=build/deix_disk.img -m 512M -vnc :0
```

Поставь любой VNC-клиент из Google Play (например "VNC Viewer" от RealVNC)
и подключись к `127.0.0.1:5900`. Увидишь загрузку DeiX вживую, включая
надписи от BIOS/SeaBIOS перед стартом самой ОС.

### Вариант B — снимок экрана без VNC-клиента

```bash
apt install -y socat imagemagick
qemu-system-x86_64 -drive format=raw,file=build/deix_disk.img -m 512M \
    -display none -monitor unix:/tmp/qmon,server,nowait &
sleep 3
echo "screendump /root/deix/build/screen.ppm" | socat - unix-connect:/tmp/qmon
convert /root/deix/build/screen.ppm /root/deix/build/screen.png
```

Дальше скопируй `screen.png` обратно на `/sdcard`, чтобы посмотреть в
галерее/просмотрщике:

```bash
cp /root/deix/build/screen.png /sdcard/Download/ 2>/dev/null || \
cp /root/deix/build/screen.png /storage/emulated/0/Download/
```

## Что должно получиться

Сначала пара строк от SeaBIOS/iPXE (это нормально, так BIOS всегда шумит
перед стартом), потом:

```
DeiX bootloader: loading stage2...
```

и следом чёрный экран с зелёным текстом:

```
DeiX v0.1 ... Rust kernel booted successfully!
Long mode: OK
Paging: OK
VGA text driver: OK

Sleduyushiy shag: preryvaniya, klaviatura, GDT/IDT...
```

## Частые проблемы

- **"Disk read error!"** сразу после "loading stage2..." — значит образ
  диска собран не до конца или повредился при копировании. Пересобери:
  `rm -rf build && ./build.sh`.

- **`proot-distro install debian` падает с ошибкой ptrace** — на некоторых
  телефонах (особенно с новыми ядрами/жёсткими SELinux-политиками
  производителя) proot не может перехватывать системные вызовы. Если так
  случится — напиши мне, попробуем альтернативный путь через нативный
  Termux-пакет `rustc-nightly` из репозитория `tur-repo` (`pkg install
  tur-repo && pkg install rustc-nightly`), это работает без proot, но
  требует пары дополнительных телодвижений с путями и taргетом сборки.

- **Нет доступа к `/sdcard` внутри Debian** — попробуй путь
  `/data/data/com.termux/files/home/storage/shared/Download` в самом
  Termux (не в Debian) и оттуда уже вручную перекладывай файл, либо
  скачай архив прямо внутри Debian через `curl`/`wget`.

Если на каком-то шаге команда упадёт с ошибкой — скинь мне текст ошибки,
поправим прямо под твоё окружение.
