# DeiX — сборка и тестирование

Ветка содержит всё, что сделано за сессии: настоящий EROFS, Ring 3,
kexec и вытесняющую многозадачность.

## 1. Зависимости

```bash
# Rust nightly + bare-metal target (нужен nightly: abi_x86_interrupt,
# alloc_error_handler, naked-asm)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
    -y --default-toolchain nightly --profile minimal
source "$HOME/.cargo/env"
rustup target add x86_64-unknown-none

# Ассемблер, эмулятор, утилиты EROFS
sudo pacman -S --needed nasm qemu-system-x86 erofs-utils   # Arch
# sudo apt install -y nasm qemu-system-x86 erofs-utils     # Debian/Ubuntu
```

`erofs-utils` не обязателен для сборки: `tools/make_deix_fs.py` умеет
собирать EROFS сам на чистом Python. Но с `mkfs.erofs` образы получаются
компактнее, а `fsck.erofs` позволяет их проверить.

## 2. Сборка

```bash
bash build.sh
```

Результат: `build/deix_disk.img` (образ диска, 10 МиБ) и `build/deix.iso`.

Шаги: nasm stage2 -> ld -> cargo build --release -> objcopy kernel.bin ->
MBR -> пересборка stage2 с реальным KERNEL_SIZE -> склейка образа ->
**форматирование разделов** (шаг 7b, `tools/make_deix_fs.py`) -> ISO.

Только ядро, без образа: `cargo build --release`.

## 3. Запуск

```bash
# Headless, весь вывод в терминал (так снимались все логи ниже)
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
    -m 256M -display none -serial stdio -no-reboot

# С окном VGA и сетью (RTL8139)
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
    -m 256M -serial stdio -netdev user,id=n0 -device rtl8139,netdev=n0
```

При первом запуске система попросит создать аккаунт (он же ключ
шифрования диска XTS-AES-256). Дальше — CLI, `help` для списка команд.

## 4. Что проверять

Самопроверки идут автоматически при загрузке:

```
[sched]  запуск 4 задач: 3 счётчика без yield + 1 спящая
[sched]  переключений контекста: 45
[sched]  САМОПРОВЕРКА ПРОЙДЕНА: все задачи выполнялись одновременно
[ring3]  Hello from Ring 3: syscall write works!
[ring3]  CS=0x23 -> CPL=3
[ring3]  САМОПРОВЕРКА ПРОЙДЕНА: вернулись в Ring 0 (CPL=0)
```

Команды в CLI:

| Команда | Что делает |
|---|---|
| `threads list` | задачи планировщика и счётчик переключений |
| `threads test` | перезапустить самопроверку многозадачности |
| `kexec check` | прочитать и проверить ядро из `/kernel_a`, не запуская |
| `kexec` | **перезапустить ядро из раздела** (загрузка пойдёт заново) |
| `kexec b` | то же из слота B |
| `ifconfig` | состояние сети (нужен `-device rtl8139`) |
| `gpu info` | детект видеокарты |

### Проверка EROFS сторонними утилитами

Разделы содержат настоящий EROFS (магия `0xE0F5E1E2`), а не самоделку:

```bash
# /kernel_a: LBA 13313, 1023 секторов
dd if=build/deix_disk.img of=/tmp/kernel_a.img bs=512 skip=13313 count=1023
fsck.erofs --extract=/tmp/out /tmp/kernel_a.img && ls -l /tmp/out
# -> kernel.tar.gz, 258568 байт

dump.erofs /tmp/kernel_a.img | head -5
# -> Filesystem magic number: 0xE0F5E1E2
```

Карта разделов (`src/partition_map.rs`), EROFS-разделы:

| Раздел | LBA | Секторов |
|---|---|---|
| `/kernel_a` | 13313 | 1023 |
| `/kernel_b` | 14337 | 1023 |
| `/init_boot` | 15361 | 255 |
| `/vendor_boot` | 15617 | 255 |
| `/boot_a` | 15873 | 255 |
| `/boot_b` | 16129 | 255 |
| `/super` | 16385 | 255 |
| `/dsm` | 16641 | 255 |
| `/recovery` | 16897 | 255 |

### Проверка kexec

```
kexec
```
В логе ядро стартует **дважды** (`=== DeiX boot: serial debug active ===`
встретится 2 раза), во втором старте появится
`[kexec] ЯДРО ЗАПУЩЕНО ИЗ РАЗДЕЛА /kernel_* (поколение 1)`.

Счётчик поколений лежит по адресу `0x1360000` — вне `.bss`, поэтому
переживает обнуление секции новым ядром.

## 5. Карта памяти (важно при правках)

```
0x00100000  код+данные ядра        <- сюда kexec копирует новый образ
0x00200000  .bss ядра (16 МиБ кучи), до ~0x121C000
0x01300000  трамплин kexec
0x01350000  стек трамплина
0x01360000  маркер поколения загрузки
0x01400000  страница Ring 3 (код + стек)
```

Не размещайте ничего в диапазоне `.bss` — на этом я уже обжёгся: первая
версия Ring 3 писала свой код на 0x200000, прямо поверх кучи ядра.

## 6. Ключевые файлы

| Файл | Содержимое |
|---|---|
| `src/erofs.rs` | EROFS v1: чтение (FLAT_PLAIN/FLAT_INLINE) и запись |
| `src/sched.rs` | вытесняющая многозадачность, naked-заглушка IRQ0 |
| `src/usermode.rs` | Ring 3: GDT+TSS, syscall/sysret, страницы USER |
| `src/kexec.rs` | загрузка ядра из раздела, трамплин |
| `src/bootchain.rs` | цепочка загрузки по разделам |
| `tools/make_deix_fs.py` | разметка MBR, ext2 и EROFS-образы |
| `REFACTOR_REPORT.md` | что и почему менялось, с доказательствами |

## 7. UI-звуки (PC speaker)

Системные звуковые эффекты лежат в `assets/*.wav` и **не зашиваются в
kernel.bin** (лимит размера ядра). Конвейер:

1. **build.sh [2e/8]** — `tools/wav2dps.py` гоняет WAV (44.1 кГц/16бит/стерео)
   в DPS1 (8 кГц, u8, моно): `build/sounds/{start,error,lowbat,fullbat,usbcon,usbdisc}.dps`.
2. **make_deix_fs.py** — пакует `*.dps` в EROFS-раздел `/super` (вместе с
   `system.img`), как `/system/media/audio/ui` в Android.
3. **Ядро (src/sound.rs)** — в рантайме читает нужный DPS из /super и играет
   ШИМ-ом на PC speaker (порт 0x61, скважность ~ амплитуде сэмпла).

Где звучит:

| Звук | Событие |
|---|---|
| `start.dps` | после успешного входа, система готова (перед CLI) |
| `error.dps` | неверный пароль / ошибка входа (GUI и текстовый логин) |
| `usbcon.dps` | система загружена с USB-флешки (активен RAM-диск) |
| `usbdisc.dps` | API готово; события нет — появится с USB-стеком |
| `lowbat.dps` / `fullbat.dps` | API готово; события нет — появятся с ACPI |

Ручная проверка из CLI:

```
sound            # список эффектов, [ok]/[--] — есть ли файл в образе
sound start      # проиграть (псевдонимы: start error lowbat fullbat usbcon usbdisc)
sound beep 880 200   # классический бипер (частота Гц, длительность мс)
```

Нюансы реализации: ШИМ блокирующий (эффекты ограничены 10 с), на реальном
железе запись в порт медленнее (~1 мкс), поэтому воспроизведение звучит
примерно вдвое ниже/длиннее, чем в QEMU — для коротких сигналов приемлемо.

**Замечание про /OTA:** сырым OTA-хранилищем ядро работает по
`ota_store::OTA_PART_LBA` — значение должно совпадать с картой разделов
(17664+2816; было исправлено с устаревшего 16128, которое затирало
EROFS-разделы /vendor_boot../recovery).
