# Сборка и запуск DeiX на Arch Linux (в т.ч. WSL2)

Arch — полноценный Linux с glibc, поэтому весь путь намного проще, чем в
Termux: `rustup` ставится штатно, без proot и обходных путей.

## 1. Системные пакеты

```bash
sudo pacman -Syu --needed nasm binutils qemu-system-x86 python libisoburn
```

`libisoburn` даёт `xorriso`, который build.sh использует для сборки
гибридного `deix.iso` (загрузочный образ для `-cdrom`/записи на флешку).
Если предпочитаете классический `genisoimage` — он тоже подойдёт (build.sh
сначала ищет `genisoimage`, и только если его нет — переходит на
`xorriso`), но в основных репозиториях Arch его обычно нет, только через
AUR (`yay -S cdrkit`), поэтому `libisoburn`/`xorriso` — самый простой путь.

Если `qemu-system-x86` не найдётся под таким именем — проверь:

```bash
pacman -Ss qemu-system
```

и возьми пакет, который даёт команду `qemu-system-x86_64`.

## 2. Rust (через rustup — обычным способом)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly --profile minimal
source "$HOME/.cargo/env"
rustup component add rust-src llvm-tools-preview --toolchain nightly
```

## 3. Собери проект

```bash
tar xzf deix.tar.gz
cd deix
chmod +x build.sh run.sh run_iso.sh
./build.sh
```

В конце сборки должно появиться сообщение вида:
```
==> Готово! Образ диска: /путь/до/deix/build/deix_disk.img
==> Готово! ISO-образ:   /путь/до/deix/build/deix.iso
```

Если вместо второй строки видно предупреждение "не найден ни genisoimage,
ни xorriso" — значит, ISO не собрался (сам .img всё равно будет рабочим),
доустанови пакет из шага 1 и запусти `./build.sh` ещё раз.

## 4. Запусти

Обычный образ диска (`build/deix_disk.img`):

```bash
qemu-system-x86_64 -drive format=raw,file=build/deix_disk.img -m 512M \
    -net nic,model=rtl8139 -net user
```

или через готовый скрипт:

```bash
./run.sh
```

ISO-образ (`build/deix.iso`) — загрузка "как с CD":

```bash
qemu-system-x86_64 -cdrom build/deix.iso -m 512M \
    -net nic,model=rtl8139 -net user
```

или через готовый скрипт (без диска — Live-режим, ext2 недоступен, это
ожидаемо):

```bash
./run_iso.sh
```

Чтобы при загрузке с ISO был доступен диск с ext2 (можно было
записывать/читать файлы) — создай отдельный образ и подключи его:

```bash
qemu-img create -f raw my_hdd.img 8M
DISK_IMG=my_hdd.img ./run_iso.sh
```

Если окно не открывается (нет WSLg) — через VNC (работает для обоих
вариантов, добавь `-vnc :0` к любой из команд выше):

```bash
qemu-system-x86_64 -drive format=raw,file=build/deix_disk.img -m 512M \
    -net nic,model=rtl8139 -net user -vnc :0
```


## Что нового: графика (программный рендер + GPU-обнаружение + UI)

**Честная позиция про NVIDIA/AMD** (та же логика, что и с Wi-Fi): настоящий
закрытый чип NVIDIA нельзя нормально поддержать без официальной
документации/прошивок производителя. Даже открытый nouveau — результат
15+ лет реверс-инжиниринга большой командой; современные карты (Turing и
новее) требуют подписанную закрытую прошивку GSP от самой NVIDIA, без
которой модесеттинг невозможен в принципе. К тому же QEMU физически не
эмулирует никакой NVIDIA/AMD GPU. Поэтому здесь:

- **`gpu.rs`** — честное определение GPU через PCI (класс 0x03, Display
  controller): распознаёт NVIDIA/AMD/Intel/QEMU-Bochs/VirtIO/VMware по
  vendor ID и печатает понятное объяснение, что поддерживается, а что нет
- **`vbe.rs`** — **настоящий рабочий** графический драйвер через открытый,
  задокументированный интерфейс Bochs VBE Display Interface (DISPI),
  который эмулирует QEMU по умолчанию (`-vga std`, PCI ID 1234:1111).
  Даёт линейный framebuffer с произвольным разрешением/глубиной цвета.
  Проверено визуально — реально работает, не заглушка
- **`renderer.rs`** — программный 2D-рендерер поверх framebuffer: точки,
  линии (Брезенхэм), прямоугольники, текст через полную битмап-таблицу
  шрифта (`font_full.rs`, сгенерирована `tools/gen_font_full.py`)
- **`mouse.rs`** — драйвер PS/2-мыши (IRQ12), декодирование пакетов,
  курсор
- **`ui/mod.rs`** — оконный менеджер: рабочий стол, перетаскиваемые окна
  с заголовком/кнопкой закрытия, панель задач снизу — упрощённый аналог
  того, что видно на Windows/KDE Plasma

Загрузчик (`boot/stage2.asm`) расширен: identity-mapping теперь покрывает
первые **4 GiB** физической памяти (было 1 GiB) — нужно, чтобы дотянуться
до framebuffer видеокарты, который обычно лежит в районе 0xFC000000.

Новые команды CLI:
- `gpu info` — что за видеокарта найдена и что с ней можно сделать
- `gpu mode [WIDTHxHEIGHT]` — переключиться в графический режим (по
  умолчанию 800x600, 32bpp)
- `gpu demo` — нарисовать демо-рабочий стол с окнами (статичный кадр —
  полноценный интерактивный цикл с обработкой мыши в реальном времени
  не подключён к текстовому CLI, это следующий шаг развития)

Попробуй:
```
deix> gpu info
deix> gpu mode 1024x768
deix> gpu demo
```

## Что было раньше: Wi-Fi стек (протокольный уровень)

Полностью написанный и unit-тестированный (14/14 тестов проходят)
протокольный стек WPA2-PSK: SHA-1/HMAC/PBKDF2/PRF, кадры IEEE 802.11,
EAPOL-Key, 4-way handshake state machine. Команды: `wifi scan/connect/status`.

## Что было раньше: проводной сетевой стек

PCI, драйвер RTL8139, Ethernet/ARP/IPv4/ICMP — настоящий `ping` в обе
стороны. Команды: `ifconfig`, `arp`, `ping <ip>`.

## Пересборка шрифтов (если захочешь что-то поменять)

```bash
sudo pacman -S --needed python-pillow ttf-dejavu
python3 tools/gen_font.py        # кириллица для текстового VGA-режима
python3 tools/gen_font_full.py   # полная таблица для графического рендерера
./build.sh
```
