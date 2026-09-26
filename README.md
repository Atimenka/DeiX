# 🪐 DeiX OS (v0.2.1-beta · Release)

A modern, highly-secure 64-bit Operating System written from scratch in **Rust** and **Assembly** for the `x86_64` architecture. Developed in a unique collaboration between a Human Architect and an AI Agent.

Операционная система нового поколения, написанная с нуля на **Rust** и **Ассемблере** под `x86_64`. Релиз **v0.2.1-beta** приносит единый канонический источник ядра в `/kernel_a` (LBA 13313), исправление персистентности аккаунтов и маркера `DEIXTPM1`, исключение пользовательских данных из VBMETA, декларативный UI-фреймворк **DUIL**, интерпретатор скриптов **DeiX Script (DS)**, кросс-компилятор **MEX v1.2 C/C++**, модульные системные библиотеки (`libdeix_*.so`), оптимизацию скорости загрузки и автоматизированный тестовый набор подсистем.

**Ядро:** ~0.7 МБ (Rust `#![no_std]` + NASM stage2) · **Лимит ядра:** 1 МиБ (2048 секторов) · **Образ диска:** 10 МиБ · **Код:** ~30 000 строк Rust, 105 файлов · реальное железо и QEMU.

---

## ⚡ Новое в версии v0.2.1-beta

### 🎯 Консолидация ядра в `/kernel_a` (LBA 13313)
- Убрано дублирование сырых секторов `kernel.bin` на LBA 3.
- Единым источником правды для загрузчиков `ramboot` и `stage2` стал раздел `/kernel_a` (LBA 13313).

### 🔑 Сохранение аккаунтов и выравнивание маркера `/TPM`
- Выравнен маркер `/TPM`: унифицирована сигнатура `DEIXTPM1` с сохранением полной обратной совместимости с `DEIXTPM`.
- Поддержка многосекторного хранения базы пользователей в разделе `/TPM` (до 16 секторов / 8 КиБ).
- Устранена гонка при первой загрузке: функция `has_any_users()` проверяет состояние шифрования и LUKS-заголовков перед попыткой чтения `USERS.DB`.

### 🛡️ Корректировка AVB VBMETA и пользовательских данных
- Из статического снимка VBMETA исключён динамический файл `USERS.DB`.
- VBMETA отныне проверяет строго неизменяемые системные компоненты (`SYSTEM.IMG`, `DXINIT.CFG`, `init.deix`), а пользовательские данные защищены шифрованием XTS-AES-256 / LUKS / TPM 2.0 без ложных блокировок `RED STATE`.

### 🖼️ Декларативный GUI-фреймворк DUIL (`src/duil.rs`)
- Иерархический парсер разметки DUIL с поддержкой виджетов `Window`, `VBox`, `HBox`, `Grid`, `Button`, `Label`, `Input`, `ProgressBar`, `CheckBox`, `GroupBox`.
- Движок автоматического расчёта геометрии (Layout) и программный 2D-рендерер в буферы поверхностей `Compositor` / `VBE`.
- Интерактивная обработка кликов мыши, хитбоксы и переключение состояний компонентов.
- Графические C++ и DUIL калькуляторы.

### 📜 Скриптовый язык DeiX Script (`src/ds.rs`)
- Полноценный интерпретатор с ветвлениями `if`/`else`/`fi`, циклами `while`/`done` и функциями `fn`/`end`.
- Подстановка переменных (`$var`), системные переменные (`PATH`, `HOME`, `SHELL`), арифметика (`expr $a + $b`) и логические операторы (`==`, `!=`, `<`, `>`, `<=`, `>=`).
- Команды взаимодействия с файловой системой ext2 (`cat`, `write`, `append`, `mkdir`, `rm`, `ls`, `cd`, `pwd`) и интерактивный REPL (`ds -i`).

### 🛠️ Фристендинг C/C++ SDK MEX v1.2
- В заголовок `tools/include/deix/mex.h` добавлены базовые функции памяти (`memset`, `memcpy`, `memmove`, `memcmp`) с атрибутом `weak` для сборки C/C++ программ без `glibc`.
- Драйвер `mexcc.cpp` поддерживает каскадный поиск ресурсов `locate_resource()` (`mex.ld`, `mex_pack.py`, `include/`).

### 🏛️ Модульные системные библиотеки (`kernel.tar.gz` и `/system`)
- Выделение системных библиотек в архиве ядра и EROFS `/system`:
  - `libdeix_core.so` — ядро, многозадачность, память.
  - `libdeix_net.so` — сетевой стек и RTL8139.
  - `libdeix_gfx.so` — 2D-рендерер и VBE.
  - `libdeix_sys.so` — супервизор Dinit и Security Monitor.
  - `libdeix_gui.so` — UI-фреймворк DUIL.
  - `libdeix_ds.so` — интерпретатор DeiX Script.

### ⚡ Оптимизация скорости загрузки
- `Ramboot`: Размер портативного блока INT 13h увеличен до 127 секторов (сокращение вызовов BIOS и смен режимов в 2 раза, ускорение считывания на ~40%).
- `Bootchain`: Zero-copy чтение EROFS-разделов прямо в целевые срезы памяти без промежуточных аллокаций.
- `Inflate`: Пакетное декодирование битпотока DEFLATE для быстрой распаковки `kernel.tar.gz`.

### 🧪 Набор сквозных тестов (`tools/test_all_subsystems.py`)
- Автоматизированный скрипт тестирования карты разделов, флагов BCB, OTA-пакетов и кросс-компиляции C++ приложений.

---

## ⚡ Базовые возможности ядра

- **🦀 Pure Rust `#![no_std]`**, старт в long mode: `boot_sector` (MBR) → `stage2` (32-бит → long mode) → `kernel.bin` @ 0x100000.
- **🛡 Dinit (PID 1, Ring 0)** — супервизор служб, точек монтирования, системных стадий и аудита.
- **🚨 Security Monitor** — эвристический детектор угроз (ransomware, code-injection) с ликвидацией (SIGKILL).
- **🔊 Intel HDA + PC Speaker** — DMA 48 кГц / 16-бит стерео воспроизведение + ШИМ PC Speaker.
- **🖼️ DXLG & VBE** — сжатый 5-цветный логотип, VBE 32bpp графика, кириллический шрифт.
- **🛡 Ring 3 + syscall/sysret** — аппаратная изоляция (GDT + TSS).
- **🧵 Вытесняющая многозадачность** — планировщик с переключением по прерыванию IRQ0 PIT (`threads list/test`).
- **🔁 kexec** — запуск нового ядра из `/kernel_a|b` без перезагрузки BIOS (`kexec check/a/b`).
- **📦 A/B-слоты + OTA** — разделы `/kernel_a|b`, `/boot_a|b`, активный слот в BCB; команды `ota check/fetch/apply/rollback`.
- **🧾 Настоящий EROFS** (магия `0xE0F5E1E2`, валидируется `fsck.erofs`) на всех системных разделах.
- **🔒 Верифицированная загрузка (AVB)** — проверка целостности VBMETA.
- **🥽 Режимы BCB**: **DSM** (аварийный COM1-прошивальщик), **fastbootd** (графический прошивальщик), **recovery** (TWRP-подобное меню).
- **🐧 Совместимость с Linux** — загрузчик ELF64 + слой системных вызовов Linux ABI (`src/linux/`).
- **🌐 Сеть** — RTL8139 + ARP + IPv4 + ICMP + TCP + HTTP (`ifconfig`, `ping`).
- **📦 MEX-приложения** — собственный формат бинарников DeiX (`run`, `pkg`, `mexcc`).

---

## 🗂️ Карта разделов (10 МиБ, 13 разделов)

| Раздел | LBA | Секторов | ФС | Назначение |
|---|---|---|---|---|
| `/system` | 4096 | 8192 | ext2 rw | рабочий том ядра (USERS.DB, AUTOSTART.CFG, профили) |
| `/TPM` | 12288 | 512 | скрытый 0xDA | аппаратно изолированный маркер TPM (`DEIXTPM1`) |
| `/userdata` | 12800 | 512 | ext2/ext4 rw | пользовательские данные и пакеты Ring 3 |
| `/kernel_a` | 13313 | 1279 | EROFS ro | слот A: `kernel.tar.gz` (GZIP + USTAR) |
| `/kernel_b` | 14593 | 1279 | EROFS ro | слот B |
| `/init_boot` | 15873 | 255 | EROFS ro | сценарий `init.deix`, `bootloader.bin` |
| `/vendor_boot` | 16129 | 255 | EROFS ro | `vendor.bin` (HAL, прошивки) |
| `/boot_a` | 16385 | 255 | EROFS ro | слот A ядра ОС |
| `/boot_b` | 16641 | 255 | EROFS ro | слот B ядра ОС |
| `/super` | 16897 | 255 | EROFS ro | системный образ + **UI-звуки (`*.dps`)** |
| `/dsm` | 17153 | 255 | EROFS ro | аварийный модуль DSM |
| `/recovery` | 17409 | 255 | EROFS ro | среда восстановления TWRP |
| `/OTA` | 17664 | 2816 | ext2 | хранилище скачанных OTA-пакетов |

---

## 💻 Команды CLI

```
help about echo clear uptime color cpuid mem lang
ifconfig arp ping gpu [info|nvinfo|mode] sound [list|play|beep|mode|hda]
dinit [status|services|mounts|users|audit|security|stage|reload]
duil [run|calc] ds [script.dxs|-i|-c] avb [status|verify|lock|unlock]
tpm [status|dump|pcr] taskmgr ls cat write rm pkg run install bigfile
useradd passwd whoami users encrypt crypt nvidia hal microcode logo linux profile lock
threads kexec crash bugreport dmesg crashlog
reboot [normal|recovery|fastbootd|dsm] bcb ota adb dev root halt
```

---

## 🚀 Быстрый старт

### Сборка
Требуются: `nasm`, `rustup nightly` с таргетом `x86_64-unknown-none`, `python3`, `qemu-system-x86_64`.

```bash
git clone https://github.com/Atimenka/DeiX.git
cd DeiX
./build.sh
```

### Запуск в QEMU

```bash
# Стандартный запуск с графикой и звуком Intel HDA:
./run_full.sh

# Быстрый запуск (VGA):
./run.sh

# Загрузка с ISO-образа:
./run_iso.sh
```

```bash
# Ручной запуск с HDA-аудио и сетевой картой RTL8139:
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
  -m 512M -serial stdio \
  -netdev user,id=n0 -device rtl8139,netdev=n0 \
  -device intel-hda -device hda-duplex \
  -audiodev pa,id=snd0
```

---

## 🧪 Тестирование

```bash
python3 tools/test_all_subsystems.py    # сквозной тест всех подсистем и тулчейна
python3 tools/check_partition_map.py    # проверка синхронизации 13 разделов
python3 tools/make_logo.py assets/logo.png build/logo.dxlg 128
python3 tools/wav2dps.py assets/start.wav build/sounds/start.dps 8000
python3 tools/qemu_mode_test.py os      # сквозной QEMU-тест
```

---

Developed by **Atimenka** & AI. Released under the **GPL-3.0** License.
