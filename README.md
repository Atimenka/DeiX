# 🪐 DeiX OS (v0.2-dev)

A modern, highly-secure 64-bit Operating System written from scratch in **Rust** and **Assembly** for `x86_64` architecture. Developed in a unique collaboration between a Human Architect and an AI Agent.

Операционная система нового поколения, написанная с нуля на **Rust** и **Ассемблере** под архитектуру `x86_64`. Разработана в уникальном тандеме Человека-Архитектора и ИИ-Агента.

---

## ⚡ What's New in v0.2

### 🧠 Ring 3 User Mode + Syscalls
Hardware isolation via GDT/TSS + `syscall`/`sysret`. Programs run in unprivileged ring 3 with 11 system calls (`print`, `read_char`, `uptime_ms`, `ping`, `get_mac`, `get_ip`, `open`, `read`, `write`, `close`).

### 📦 Module System (`.kmod`)
Kernel modules loaded from ext2 disk at boot: `NET.KMOD`, `CRYPTO.KMOD`, `GFX.KMOD`. Module API (KernelApi): print, alloc, files, PCI, ports, timer. No more monolithic binary.

### 🌐 MEX API v1.1 — Network Functions
Network access from `.mex` programs: `ping()`, `get_mac()`, `get_ip()`, `arp_resolve()`. Fully backward-compatible with v1.0.

### 💬 DeiX Script (DS) — Shell Scripting Language
Built-in interpreter with variables, conditionals (`if/else`), loops (`for/while`), commands. Used for `AUTOSTART.CFG`, `DXINIT.CFG`, and `*.dxs` scripts. Interactive REPL via `ds -i`.

### 🎨 DUIL — DeiX UI Language
Declarative XML-like markup for GUI: `<window>`, `<button>`, `<label>`, `<terminal>`, `<vbox>`, `<hbox>`.

### 🚀 dxinit — Boot Manager
Systemd-like init system. Services in stages (`early` → `middle` → `late`). Dependency resolution, auto-restart. Config: `/etc/DXINIT.CFG`.

### 🎮 NVIDIA Open 3D Driver
Based on nouveau documentation (freedesktop.org). Supports NV04–NV40 PGRAPH 3D engine with triangle rendering. Requires Falcon firmware for NV50+ (available at freedesktop.org/nouveau/linux-firmware).

### 🖥️ ELF Loader + glibc Compatibility
Loads static ELF64 binaries (glibc `--static` / musl). New address space per process, user stack, brk heap.

### 🗂️ Memory Manager
Buddy allocator (4K page bitmap), 4-level x86_64 page tables, virtual memory allocation, map/unmap, TLB management.

### 🔌 New Drivers (skeleton)
- **xHCI USB 3.0**: PCI probe, MMIO registers, reset/start, DCBAA, Command Ring
- **HD Audio**: CORB/RIRB, widget graph, PCM setup
- **DisplayPort/HDMI**: AUX channel, DPCD, link training, EDID, DDC/I2C
- **Initrd**: Early filesystem for pre-ATA module loading

---

## ⚡ Core Features (v0.1)

### 🦀 Pure Rust Kernel (`#![no_std]`)
- **Long Mode:** boots directly into 64-bit mode
- **Safe Memory:** heap allocator + page tables under Rust's ownership rules
- **Микроядерная база:** memory management + component isolation

### 🎨 Procedural Desktop
- Live-animated diagonal background lines (procedural)
- Z-Order Window Manager with drag-and-drop
- Dynamic resolution up to 1280x1024 (Bochs VBE)

### 🔐 Ironclad Security
- **Ring 3 isolation:** GDT, TSS, `syscall` hardware switching
- **Salted SHA-256 hashing** for user passwords (`USERS.DB`)
- **AES-256-XTS full-disk encryption** (compatible with Linux `cryptsetup`)

### 🌐 Network & Localization
- RTL8139 NIC driver + IPv4/ARP/ICMP stack
- IEEE 802.11 Wi-Fi protocol + WPA2-PSK handshake
- Hardware Cyrillic font in VGA Plane 2
- Russian/English UI localization (`lang ru`/`lang en`)

---

## 📂 Project Structure

```
DeiX/
├── boot/              # NASM bootloader (MBR + long mode)
├── src/               # Rust kernel source (~17,000 lines)
│   ├── lib.rs         # Entry point (kernel_main)
│   ├── cli.rs         # Command-line interface (~30 commands)
│   ├── mm/            # Memory manager (phys + virt)
│   ├── crypto/        # AES, SHA-256/512, HMAC-SHA1, PBKDF2, XTS
│   ├── net/           # Ethernet, ARP, IPv4, ICMP
│   ├── wifi/          # IEEE 802.11, WPA2-PSK, EAPOL
│   ├── ui/            # Window manager + desktop renderer
│   ├── ds.rs          # DeiX Script interpreter
│   ├── duil.rs        # DUIL UI language parser
│   ├── dxinit.rs      # Boot manager (systemd-like)
│   ├── elf.rs         # ELF64 static loader
│   ├── usermode.rs    # Ring 3 + syscall/sysret
│   ├── module.rs      # .kmod module loader
│   ├── nv3d.rs        # NVIDIA open 3D driver
│   ├── xhci.rs        # USB 3.0 xHCI driver
│   ├── hda.rs         # HD Audio driver
│   ├── dp.rs          # DisplayPort/HDMI driver
│   ├── initrd.rs      # Initial RAM disk
│   └── autostart.rs   # Autostart script runner
├── tools/             # Build tools
│   ├── mexcc.py       # Cross-compiler (C/C++/Rust → .mex)
│   ├── mex_pack.py    # .bin → .mex packer
│   ├── kmod_pack.py   # .bin → .kmod packer
│   ├── hello.asm      # Demo .mex program
│   ├── sysinfo.asm    # System info .mex
│   └── netping.asm    # Network ping .mex
├── docs/              # Documentation
│   └── MEX_FORMAT.md  # .mex format specification (v1.1)
├── build.sh           # Build script (NASM + cargo + ld)
├── run.sh             # QEMU launch script
└── run_iso.sh         # ISO boot script
```

---

## 🏗 Архитектура загрузки (двухчастная, лимит размера ядра снят)

```
Сектор 0          boot_sector.bin (512 Б, MBR — BIOS грузит сам)
Секторы 1..K      stage2.bin (загрузчик: 32-бит вход -> long mode -> копирует kernel)
Секторы K+1..M    kernel.bin (ядро, база 0x100000; грузится через BIOS int13 в 0x11000,
                   затем stage2 копирует в 0x100000 и прыгает)
```

- Ядро больше НЕ ограничено 1 МиБ/real mode: stage2 (маленький, 1 сектор) читает
  kernel.bin через BIOS int13 и копирует в high memory. Область загрузчика — до
  ext2-тома (LBA 4096), т.е. ~4090 секторов (~2 МиБ) под ядро.
- **ПОЛНАЯ ЦЕПОЧКА ЗАГРУЗКИ через ВСЕ разделы** (не пустышки!, порядок по ТЗ):
  `загрузчик(MBR/stage2) → /dsm(dsm.bin) → /init_boot(bootloader.bin) →
  /vendor_boot(vendor.bin) → /boot(fastbootd.bin, recovery.bin) →
  /kernel(kernel.tar.gz)`.
  Каждый EROFS-раздел содержит реальные файлы (таблица файлов после
  суперблока); ядро при старте читает и верифицирует их, а kernel.tar.gz —
  НАСТОЯЩИЙ gzip (deflate): ядро распаковывает его своим inflate-декодером
  (src/inflate.rs) и извлекает kernel.bin + библиотеки (libdeix_*.so).
  Режимы fastbootd/recovery загружают свои образы из /boot; install копирует
  разделы с загрузочного диска на целевой.
- `boot_sector` читает kernel с `LBA = 1 + NUM_SECTORS(stage2)`, `KERNEL_SECTORS=1100`
  (запас — ядро может расти); install пишет ровно туда же.
- `install` на целевой диск пишет kernel.bin, прочитанный с загрузочного диска
  (чистый `.data`), а не из памяти — «живой» .data с указателями на кучу
  ломал бы установленную систему (Invalid opcode).
- `install` сбрасывает служебные секторы целевого диска (BCB, маркер шифрования
  DEIXCRYP): иначе остатки от прошлой настройки оставляли том «зашифрованным»
  старым ключом, и вход с новым паролем давал Invalid password.
- `linker_kernel.ld` включает .got/.got.plt в .data и выравнивает __image_end
  до 512 байт — иначе install копировал kernel на сектор короче (хвост .data
  терялся, установленная система падала Invalid opcode).
- Paging: identity-map первых 4 ГиБ (1 ГиБ обычной памяти + GPU MMIO 0xFC000000
  для framebuffer'а оболочек recovery/fastbootd/DSM).
- GUI-оболочки (DSM/Fastbootd/Recovery): английский UI, подтверждения через
  графические диалоги (Enter/Esc, таймаут) — без зависаний.

## 💻 CLI Commands

```
help     about    echo      clear     uptime    color     cpuid
mem      lang     ifconfig  arp       ping      wifi      gpu
ls       cat      write     rm        pkg       run       install
bigfile  useradd  passwd    whoami    users     encrypt   modules
nv3d     ds       duil      dxinit    crash     reboot    halt
bugreport dmesg   crashlog  dsm       adb       ota       dev
```

### Загрузочные режимы (BCB): DSM > Fastbootd > Recovery > ОС

| Команда | Режим при следующей перезагрузке |
|---------|----------------------------------|
| `reboot dsm` | **DSM** — Download System Manager (аналог EDL): emergency-прошивка по COM1, READ/WRITE/FLASH/ERASE/VERIFY (SHA-256), работает даже при сломанной ОС |
| `reboot fastbootd` | **Fastbootd** — полный прошивальщик: Flash/Erase/Format, Getvar, OEM unlock/lock, перезагрузка в любой режим |
| `reboot recovery` | **Recovery** — TWRP/OrangeFox-style: Install OTA (с разблокировкой зашифрованного тома паролем), Nandroid Backup/Restore, Factory Reset, Wipe, Mount, Sideload, crash log |
| `bcb <mode>` | прямая установка одноразового флажка (`normal`/`recovery`/`fastbootd`/`dsm`) |

### Отладчик ошибок (как в Android)

| Команда | Что делает |
|---------|------------|
| `bugreport` | полный диагностический отчёт (система, разделы, TPM, BCB, dmesg, crash) → экран + BUGREPORT.TXT на /system |
| `dmesg` | кольцевой журнал ядра (последние строки) |
| `crashlog` | дамп последней паники (tombstone): `crashlog clear` — очистить |
| `crash panic` | принудительная паника (тест отладчика) |
| `ota file [payload]` | сгенерировать OTA-пакет и записать TEST.OTA на /system (для recovery Install) |

Паника ядра записывает tombstone сырыми секторами (LBA 2048, вне ext2/шифрования) —
том не повреждается, дамп читается после перезагрузки.

### New in v0.2:
| Command | Description |
|---------|-------------|
| `nv3d info/demo` | NVIDIA 3D driver info & demo |
| `ds <file>/-c/-i` | DeiX Script interpreter |
| `duil <file>` | Render DUIL layout |
| `dxinit boot/status` | Boot manager control |
| `modules` | List loaded kernel modules |

---

## 🚀 Quick Start

```bash
cd Deix/
chmod +x build.sh run.sh
./build.sh          # Build everything
./run.sh            # Launch in QEMU
```

```bash
# Or with network + large memory:
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw \
  -m 512M -net nic,model=rtl8139 -net user
```

---

## 🔧 Cross-Compilation (mexcc)

```bash
# Generate Rust project scaffold:
python3 tools/mexcc.py cargo

# Generate CMake project for C/C++:
python3 tools/mexcc.py cmake

# Compile C to .mex:
python3 tools/mexcc.py build hello.c

# Compile ASM to .mex:
python3 tools/mexcc.py build program.asm
```

---

## 🗺️ Roadmap

- [x] Ring 3 user mode + syscall/sysret
- [x] Module system (.kmod)
- [x] MEX API v1.1 (network functions)
- [x] NVIDIA open 3D driver (NV04–NV40)
- [x] ELF static loader (glibc compat)
- [x] Memory manager (buddy + page tables)
- [x] DeiX Script language (DS)
- [x] DUIL UI language
- [x] dxinit boot manager
- [x] xHCI USB 3.0 / HD Audio / DP-HDMI skeleton
- [ ] TCP/IP stack (currently Ethernet+ARP+IPv4+ICMP only)
- [ ] USB device enumeration + HID driver
- [ ] NVIDIA firmware loader (Falcon) + NV50+ 3D
- [ ] ext4 / FAT32 filesystem support
- [ ] SMP (multi-core)

---

Developed by **Atimenka** & AI. Released under the GPL-3.0 License.

## 🔐 Security Subsystem (интеграция)

В ядро интегрирована подсистема инициализации и безопасности (7 модулей в `src/`):
`init_parser` (парсер `init.deix`, PID 1), `vault` (барьер системных разделов —
erofs/ro, rw только в Fastbootd/EDL/Recovery, иначе `panic!`), `security_monitor`
(эвристический демон Ring 3, порог 0.85, SIGKILL), `kernel_loader` (сэндвич
ядра с EROFS-суперблоком), `arch_pkg_bridge` (Arch PKG → `.pkg.erofs` в `/userdata`),
`recovery_flash_engine` (TWRP/OrangeFox + Fastbootd + EDL), `partition_map`
(карта разделов). Хуки подключены в `kernel_main()` (`src/lib.rs`): стадия
`init_boot` и самоконтроль демона после инициализации Ring 3. Подробности —
в `docs/SECURITY_SUBSYSTEM.md`.


## 🧪 Тестирование (QEMU)

```bash
# все режимы (DSM/Fastbootd/Recovery/Install/OS/Crash)
python3 tools/qemu_mode_test.py dsm
python3 tools/qemu_mode_test.py fastbootd
python3 tools/qemu_mode_test.py recovery
python3 tools/qemu_mode_test.py install
python3 tools/qemu_mode_test.py os
python3 tools/qemu_mode_test.py crash

# пароль переживает 2 перезагрузки
python3 tools/qemu_scenario_reboot.py

# полная установка на второй диск + загрузка с него
python3 tools/qemu_install_test.py
```

Подробный отчёт: `docs/REPORT_v1.0_FINAL.md`.

## 🔄 A/B разметка и OTA по воздуху

- **A/B слоты**: `/kernel_a`/`/kernel_b`, `/boot_a`/`/boot_b`. Активный слот
  хранится в BCB (`bcb slot <a|b>`). Цепочка загрузки читает АКТИВНЫЙ слот.
- **OTA wireless**: `ota check` (проверка), `ota download` (скачать по воздуху —
  формирует новый kernel.tar.gz), `ota apply` (прошивает в НЕактивный слот и
  переключает), `ota rollback` (откат). Recovery: пункт «OTA wireless update (A/B)».
- **Защита цепочки**: если раздел загрузочной цепочки стёрт/повреждён
  (например `dsm erase /kernel_a`), загрузка ОСТАНАВЛИВАЕТСЯ («ЗАГРУЗКА
  ОСТАНОВЛЕНА») — система не стартует без ядра (как Android RED state).
  Восстановление — прошивкой раздела через DSM/fastbootd или `bcb slot b`.
- CLI установщика и оболочек — на английском.

## 📦 Утилита рассылки OTA (Linux-бинарник)

**`deix-ota`** — host-инструмент для генерации и рассылки OTA-обновлений
на все устройства DeiX (бинарник в корне проекта / home).

```bash
deix-ota build --kernel build/kernel.bin --out TEST.OTA --version 7
    # собрать подписанный OTA-пакет (kernel.tar.gz + контейнер DEIXOTA1)

deix-ota push --img dev1.img --img dev2.img --ota TEST.OTA --slot b --set-slot b
    # прошить ядро в /kernel_b ВСЕХ образов и переключить активный слот на b

deix-ota info --img dev1.img
    # показать BCB (mode/slot) и содержимое всех EROFS-разделов

deix-ota serve --dir . --port 8080
    # HTTP-сервер раздачи: http://host:8080/TEST.OTA (для ota fetch с устройств)
```

Прошитый пакет ядро распаковывает (gzip+tar) и грузится со слота b —
подтверждено в QEMU. Подпись SHA-256(payload||secret) валидна для ядра.

## 🛠 mexmake — сборщик DeiX .mex (C/ASM/Rust)

Система сборки .mex-приложений (аналог make для DeiX). Расположение:
- **в DeiX**: `tools/mexmake` (бинарник);
- **отдельно**: `/home/user/mexmake/` (исходники) и `/home/user/mexmake-bin` (бинарник).

Использование (в корне проекта с файлом `mex`):
```bash
mmake init [c|asm|rust]   # создать проект (файл 'mex' + src/)
mmake build               # собрать в .mex
mmake info [file.mex]     # показать заголовок
mmake clean               # очистить build/
```

Файл `mex` (конфигурация, без расширения):
```
[project]
name = demo
base = 0x600000      # адрес загрузки
entry = mex_main     # точка входа (ASM: _start)
src  = src
outdir = build

[mex]
version = 1.1        # формат .mex

[cc]
cmd = cc
include = include    # каталог заголовков

[rust]
mode = file
```

Поддерживает многофайловые проекты (структура папок, include), C/ASM/Rust,
автоподстановка entry_offset/bss из ELF. Формат .mex v1.1 — как у ядра
(MEX_LOAD_ADDR 0x600000, MexApi в RDI, тело после 32-байтного заголовка).
