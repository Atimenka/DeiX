# DeiX 0.2.1-beta — сборка и тестирование

Документ описывает текущее состояние ветки. Всё, что здесь перечислено,
соответствует коду; известные ограничения собраны в конце.

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

`erofs-utils` не обязателен: `tools/make_deix_fs.py` умеет собирать EROFS
сам на чистом Python. С `mkfs.erofs` образ компактнее, а `fsck.erofs`
позволяет проверить его сторонней утилитой.

## 2. Сборка

```bash
bash build.sh
```

Результат: `build/deix_disk.img` (10 МиБ) и релизные артефакты
`build/deix-0.2.1-beta-x86_64.img` + `build/SHA256SUMS`.

Шаги: сверка карты разделов (`tools/check_partition_map.py`) -> nasm
stage2/ramboot -> cargo build --release -> objcopy kernel.bin -> MBR ->
пересборка stage2/ramboot с реальным размером и LBA ядра -> склейка
образа -> форматирование разделов (`tools/make_deix_fs.py`: EROFS
`/system`, ext2 `/userdata`, дескриптор ядра `DEIXKIMG`).

Только ядро, без образа: `cargo build --release`.

## 3. Схема диска

| Область | LBA | Секторов | Содержимое |
|---|---|---|---|
| MBR | 0 | 1 | загрузочный сектор + таблица разделов |
| boot | 1..2046 | ≤2046 | stage2 + ramboot + сырое ядро kernel.bin |
| `DEIXKIMG` | 2047 | 1 | дескриптор ядра: LBA, размер, SHA-256 |
| panic dump | 2048 | 64 | аварийный дамп (`DEIXPNIC`), вне ФС |
| `/system` | 4096 | 8704 | EROFS, только чтение |
| `/userdata` | 12800 | 5632 | ext2, чтение/запись |

Загрузка идёт по сырому ядру (MBR -> stage2 -> kernel.bin с LBA 1..2046);
`/system/kernel/kernel.bin` — копия того же файла внутри EROFS. После
старта ядро сверяет обе копии по размеру и SHA-256 через дескриптор
`DEIXKIMG`; расхождение регистрируется как `DX-KRN-0013`.

Единственный источник истины геометрии — `src/partition_map.rs`.
`tools/check_partition_map.py` сверяет с ним `tools/make_deix_fs.py`,
`src/install.rs`, `src/ext2.rs`, `boot/boot_sector.asm` и
`src/diag/panic.rs`; запускается в `build.sh` и CI.

## 4. Запуск

```bash
# Headless, весь вывод в терминал
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
    -m 256M -display none -serial stdio -no-reboot

# С окном VGA и сетью (RTL8139)
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
    -m 256M -serial stdio -netdev user,id=n0 -device rtl8139,netdev=n0
```

При первом запуске система попросит создать аккаунт (он же ключ
шифрования диска XTS-AES-256). Дальше — CLI, `help` для списка команд.

Маркер успешной загрузки в serial-логе:
`=== DeiX boot: serial debug active ===`, затем
`DeiX 0.2.1-beta x86_64 - mini kernel booted successfully!`.

## 5. Тесты

```bash
python3 tools/check_partition_map.py    # сверка карты разделов по всем источникам
python3 tools/test_all_subsystems.py    # карта разделов + тулчейн MEX (C/C++)
python3 tools/qemu_scenario_reboot.py   # фабрика -> вход -> reboot x2 (персистентность)
python3 tools/qemu_install_test.py      # установка на второй диск + загрузка с него
```

Проверка EROFS сторонней утилитой:

```bash
dd if=build/deix_disk.img of=/tmp/system.img bs=512 skip=4096 count=8704
dump.erofs /tmp/system.img | head -5     # -> Filesystem magic number: 0xE0F5E1E2
fsck.erofs --extract=/tmp/out /tmp/system.img && ls -R /tmp/out
```

Команды в CLI для ручной проверки:

| Команда | Что делает |
|---|---|
| `threads list` | задачи планировщика и счётчик переключений |
| `dinit status` | службы: встроенные (Ring 0) и процессы |
| `ifconfig` / `ping` | сеть (нужен `-device rtl8139`) |
| `gpu info` | детект видеокарты |
| `error list` / `panic last` | журнал диагностики и последний аварийный дамп |
| `install` | установка системы на второй диск (ATA Slave) |

## 6. Карта памяти (важно при правках)

```
0x00100000  код+данные ядра
0x00200000  .bss ядра (16 МиБ кучи)
0x03000000  область MEX-программы (Ring 3, фиксированный адрес линковки)
```

Не размещайте ничего в диапазоне `.bss` — ранняя версия Ring 3 писала
код на 0x200000 поверх кучи ядра.

## 7. UI-звуки (PC speaker)

Эффекты лежат в `assets/*.wav` и не зашиваются в kernel.bin:

1. **build.sh [2e/8]** — `tools/wav2dps.py` гоняет WAV в DPS1 (8 кГц, u8, моно).
2. **make_deix_fs.py** — кладёт `*.dps` в EROFS как `/system/media/audio/ui/`.
3. **Ядро (src/sound.rs)** — читает DPS через VFS (образ `/system`
   кешируется) и играет ШИМ-ом на PC speaker.

```
sound            # список эффектов, [ok]/[--] — есть ли файл в образе
sound start      # проиграть (start error lowbat fullbat usbcon usbdisc)
sound beep 880 200
```

ШИМ блокирующий (эффекты ограничены 10 с); на реальном железе запись в
порт медленнее, звук примерно вдвое ниже/длиннее, чем в QEMU.

## 8. Известные ограничения

- Один CPU, диски только ATA PIO (~1.4 мс/КиБ в QEMU) — больших чтений
  стоит избегать в интерактивных путях.
- Ring 3 — только MEX; один MEX-процесс одновременно (фиксированный
  адрес линковки `0x03000000`).
- Запуск ELF-процессов отключён (`DX-ELF-0009`): загрузчик и подмножество
  Linux-syscalls есть, но исполнение шло бы в Ring 0.
- `AddressSpace` — учётная структура, без отдельных страничных таблиц
  на процесс (кроме области MEX).
- Аллокатор физических страниц — битовая карта, не buddy.
- `getrandom` — не криптографический ГСЧ.
- Шифрование диска — собственный формат «по мотивам LUKS»
  (XTS-AES-256), несовместим с утилитами LUKS.
- Нет USB, SMP, AHCI/NVMe, ACPI-управления питанием.

Ключевые файлы: `src/partition_map.rs` (геометрия), `src/bootchain.rs`
(сверка образов ядра), `src/vfs.rs` (единая ФС), `src/diag/` (ошибки и
аварийные дампы), `src/dinit/` (PID 1), `tools/make_deix_fs.py` (образ).
