# 🪐 DeiX OS (v0.2.1-beta · Release)

A modern, fast, and secure 64-bit Operating System written from scratch in **Rust** and **Assembly** for the `x86_64` architecture. Small core, immutable system, writable user space.

Операционная система нового поколения под `x86_64` с модульной графической оболочкой, неизменяемым системным разделом **EROFS** (`/system`) и пользовательским пространством **ext2/4** (`/userdata`).

**Ядро:** ~0.7 МБ (Rust `#![no_std]` + NASM stage2) · **Лимит ядра:** 1 МиБ (2048 секторов) · **Образ диска:** 10 МиБ · **Код:** ~25 000 строк Rust · реальное железо и QEMU.

---

## 🏛️ Архитектура системы

```text
Disk
│
├── boot          MBR + stage2 loader
├── /system       EROFS RO (kernel, modules, libraries)
└── /userdata     EXT2/4 RW (users, configs, packages)
```

```text
                    DeiX OS
                       │
                ┌──────┴──────┐
                │   Kernel    │
                └──────┬──────┘
                       │
       ┌───────────────┼────────────────┐
       │               │                │
     Memory          Scheduler         VFS
       │               │                │
       └───────────────┼────────────────┘
                       │
                  Process Manager
                       │
          ┌────────────┼────────────┐
          │            │            │
        Dinit        Syscalls     Loaders
                                      │
                                 ┌────┴────┐
                               ELF        MEX
```

- **Неизменяемое ядро и подсистемы (`/system`)**: системный раздел монтируется в режиме Read-Only с проверкой целостности.
- **Пользовательские данные (`/userdata`)**: база пользователей (`/userdata/users.db`), пользовательские файлы и конфигурации.
- **Графический композитор DeiX Fluent**: событийно-управляемый рендеринг, кеширование поверхностей, трекинг повреждений и адаптивный UI.
- **Декларативный GUI фреймворк DUIL & DeiX Script**: компактные инструменты описания графического интерфейса и скриптового управления.

### 🗂️ VFS — единая файловая система (`src/vfs.rs`)

Все подсистемы ядра ходят за файлами через VFS, а не в драйверы разделов
напрямую. Это одна точка монтирования, одна точка проверки прав и один набор
кодов ошибок.

```text
VFS
├── /system    → EROFS, только чтение
└── /userdata  → EXT2,  чтение/запись
```

API: `read_file` / `write_file` / `mkdir` / `remove` / `stat` / `readdir` /
`exists`. Запись на `/system` возвращает `ReadOnly`, не доходя до диска. Образ
`/system` читается с диска один раз и кешируется в куче.

### 🧬 Менеджер процессов (`src/process.rs`)

Единая структура для всего, что исполняется:

```text
Process
 ├── PID / UID / parent
 ├── AddressSpace (образ, стек, куча)
 ├── таблица файловых дескрипторов + cwd
 ├── capabilities
 └── слот планировщика
```

`spawn()` создаёт запись процесса, `exec()` загружает образ и создаёт задачу
планировщика, привязанную к PID. Завершение нити возвращается обратно через
`notify_exit`, поэтому мёртвая нить не оставляет процесс в состоянии Running.

**Ограничение формата MEX.** `tools/mex.ld` линкует `.mex` на фиксированный
адрес `0x03000000`, и образ не является позиционно-независимым. Поэтому MEX
загружается строго по адресу линковки, а одновременное исполнение нескольких
MEX-процессов запрещено (`MEX_IN_USE`) — изоляция по адресным пространствам
для этого формата появится только вместе с PIC-сборкой или настоящим
постраничным отображением.

### 📜 Скриптовый язык DeiX Script (`src/ds.rs`)
- Полноценный интерпретатор с ветвлениями `if`/`else`/`fi`, циклами `while`/`done` и функциями `fn`/`end`.
- Подстановка переменных (`$var`), системные переменные (`PATH`, `HOME`, `SHELL`), арифметика (`expr $a + $b`) и логические операторы (`==`, `!=`, `<`, `>`, `<=`, `>=`).
- Команды взаимодействия с файловой системой ext2 (`cat`, `write`, `append`, `mkdir`, `rm`, `ls`, `cd`, `pwd`) и интерактивный REPL (`ds -i`).

### 🛠️ Фристендинг C/C++ SDK MEX v1.2
- В заголовок `tools/include/deix/mex.h` добавлены базовые функции памяти (`memset`, `memcpy`, `memmove`, `memcmp`) с атрибутом `weak` для сборки C/C++ программ без `glibc`.
- Драйвер `mexcc.cpp` поддерживает каскадный поиск ресурсов `locate_resource()` (`mex.ld`, `mex_pack.py`, `include/`).

### 🏛️ Содержимое системного раздела `/system` (EROFS, только чтение)
- `kernel/kernel.bin` — образ ядра.
- `etc/init.deix` — скрипт инициализации.
- `services/dinit.cfg` — конфигурация служб Dinit.
- `media/audio/ui/*.dps` — звуковые ресурсы оболочки.

В образ кладутся только настоящие файлы. Заглушки `libdeix_*.so` и
`kmod/*.kmod` не создаются: `module.rs` ожидает у `.kmod` валидный заголовок
и исполняемое тело, поэтому placeholder-байты загрузились бы только как
повреждённый модуль. Модуля, которого нет, в образе нет.

Модули ядра (`.kmod`) — отдельный механизм: они загружаются с диска через
`module.rs` и видны в `lsmod`. Встроенные подсистемы ядра загруженными
модулями не являются и в этот список не попадают.

### ⚡ Оптимизация скорости загрузки
- `Ramboot`: Размер портативного блока INT 13h увеличен до 127 секторов (сокращение вызовов BIOS и смен режимов в 2 раза, ускорение считывания на ~40%).
- `Bootchain`: Direct-load чтение EROFS-файлов `/system/kernel/kernel.bin` прямо в память без промежуточной распаковки.

### 🧪 Набор сквозных тестов (`tools/test_all_subsystems.py`)
- Автоматизированный скрипт тестирования карты разделов и кросс-компиляции C++ приложений.

---

## ⚡ Базовые возможности ядра

- **🦀 Pure Rust `#![no_std]`**, старт в long mode: `boot_sector` (MBR) → `stage2` (32-бит → long mode) → `kernel.bin` @ 0x100000.
- **🛡 Dinit (PID 1, Ring 0)** — супервизор служб, точек монтирования и аудита.
- **🚨 Security Monitor** — эвристический детектор угроз (ransomware, code-injection) с ликвидацией (SIGKILL).
- **🔊 Intel HDA + PC Speaker** — DMA 48 кГц / 16-бит стерео воспроизведение + ШИМ PC Speaker.
- **🖼️ DXLG & VBE** — VBE 32bpp графика, кириллический шрифт.
- **🛡 Ring 3 + syscall/sysret** — аппаратная изоляция (GDT + TSS).
- **🧵 Вытесняющая многозадачность** — планировщик с переключением по прерыванию IRQ0 PIT (`threads list/test`).
- **🧾 Настоящий EROFS** (магия `0xE0F5E1E2`, валидируется `fsck.erofs`) на системном разделе `/system`.
- **🐧 Совместимость с Linux** — загрузчик ELF64 + слой системных вызовов Linux ABI (`src/linux/`).
- **🌐 Сеть** — RTL8139 + ARP + IPv4 + ICMP + TCP + HTTP (`ifconfig`, `ping`).
- **📦 MEX-приложения** — собственный формат бинарников DeiX (`run`, `pkg`, `mexcc`).

---

## 🗂️ Карта разделов

| Раздел | LBA | Секторов | ФС | Назначение |
|---|---|---|---|---|
| `/system` | 4096 | 8704 | EROFS ro | Системный раздел (ядро `kernel.bin`, библиотеки, модули `.kmod`, службы) |
| `/userdata` | 12800 | 5632 | ext2/ext4 rw | Пользовательские данные, программы, приложения Ring 3 |

---

## 💻 Команды CLI

```
help about echo clear uptime color cpuid mem lang
ifconfig arp ping gpu [info|nvinfo|mode] sound [list|play|beep|mode|hda]
dinit [status|services|mounts|users|audit|security|stage|reload]
duil [run|calc] ds [script.dxs|-i|-c] taskmgr ls cat write mkdir rm pkg run install bigfile
useradd passwd whoami users encrypt crypt nvidia hal microcode logo linux profile lock
threads crash bugreport dmesg crashlog adb dev root reboot poweroff shutdown halt
ps kill kmod [load|list] lsmod
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
python3 tools/check_partition_map.py    # проверка синхронизации карты разделов (2 раздела)
python3 tools/make_logo.py assets/logo.png build/logo.dxlg 128
python3 tools/wav2dps.py assets/start.wav build/sounds/start.dps 8000
python3 tools/make_deix_fs.py build/deix_disk.img   # сборка заводского образа
```

---

Developed by **Atimenka** & AI. Released under the **GPL-3.0** License.
