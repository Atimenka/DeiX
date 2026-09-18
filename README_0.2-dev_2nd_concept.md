# 🪐 DeiX OS — v0.2-dev · ВТОРАЯ КОНЦЕПЦИЯ

> **DeiX OS** — независимая, ультра-легковесная, высокозащищённая 64-битная
> операционная система, написанная с нуля на **Rust** и **Ассемблере** для
> `x86_64`. Вторая концепция (v0.2-dev) добавляет к ядру полную подсистему
> безопасности, собственный пакетный менеджер, графический композитор и
> скриптовый язык.

**Образ ядра: ~542 КБ** (stage2.bin: загрузчик 32/64-бит + ядро Rust).
**Образ диска:** `build/deix_disk.img` (6 553 600 байт = 12 800 секторов × 512).

---

## 🔐 Архитектура и кольца защиты

```
┌─────────────────────────────────────────────────────────────────┐
│  RING 3 (User Space)                                            │
│  security_monitor (эвристика, SIGKILL) · pacman · DUIL (Qt-alt) │
│  DS (sh/bash-alt) · композитор (Wayland/X11-alt) · CLI          │
├─────────────────────────────────────────────────────────────────┤
│  RING 0 (Kernel Space / Vault)                                  │
│  init_parser (PID 1) · vault · tpm (шифрование диска) · sched   │
│  arch_abi (Linux x86_64 syscall ABI / glibc-слой) · ELF-loader  │
│  драйверы: ATA, PS/2, RTL8139, Bochs-GPU, PIT, PIC, VGA, ext2   │
└─────────────────────────────────────────────────────────────────┘
```

**Микрогибридная архитектура** с жёстким разделением обязанностей:
* **Ring 0** — парсер `init.deix`, Vault-барьер разделов, TPM/шифрование,
  планировщик многопоточности, слой совместимости Linux syscalls.
* **Ring 3** — демон эвристического мониторинга, пакетный менеджер,
  UI-платформа, скриптовый язык, дисплейный сервер.

---

## 🗂️ Карта разделов (KERNEL SECURITY VAULT)

| Раздел | ФС | Режим | Кольцо | Назначение |
|---|---|---|---|---|
| `/kernel` | erofs | ro | Ring 0 | сэндвич ядра: kernel.tar.gz → kernel.img (магия 0xE0F5E0F5) |
| `/init_boot` | erofs | ro | Ring 0 | микроядро, структуры PID 1, скрипт `init.deix` |
| `/boot` | erofs | ro | Ring 0 | таблицы параметров ядра, ramdisk |
| `/vendor_boot` | erofs | ro | Ring 0 | HAL и прошивка вендора |
| `/super` | erofs | ro | Ring 0 | контейнер динамических разделов system/vendor/product |
| `/system` | erofs | ro | Ring 0 | логический системный раздел внутри /super |
| `/recovery` | erofs | ro | Ring 0 | изолированная среда восстановления TWRP/OrangeFox |
| `/userdata` | ext4 | rw | Ring 3 | единственный пользовательский раздел данных |
| `/TPM` | — | скрытый | Ring 0 | пароли и ключи — только в TPM NV (стереть невозможно) |

**Правило Vault:** системные разделы монтируются только через `erofs` в режиме
`ro`. `rw` вне прошивочных контекстов (Fastbootd/EDL/Recovery) →
`panic!("SECURITY_VIOLATION: Hard-locked system partition reached with RW flags. Boot halted.")`.
Доступ к `/TPM` из Ring 3 запрещён всегда.

---

## 🔒 TPM и полнодисковое шифрование (включено по умолчанию)

* При загрузке ОС **запрашивает пароль разблокировки диска** (ввод с
  клавиатуры или COM1; 3 попытки, затем остановка).
* Шифрование: **XTS-AES-256** + **PBKDF2-HMAC-SHA1** (100 000 итераций);
  ключ выводится из пароля и сверяется с запечатанным в TPM (NV, PCR 7).
* **Скрытый раздел `/TPM`**: пароли учётной записи и ключ диска хранятся в
  самом TPM (NV 0x01000000 / 0x01000001), после первого программирования
  `writable=false` — **стереть невозможно**.
* Демо: XTS round-trip (сектор 512 байт шифруется/расшифровывается).

**Пароль диска (для QEMU-теста): `deix-disk-unlock-2026`**

---

## 📦 Компоненты второй концепции

### 1. Пакетный менеджер `pacman` (порт Arch pacman) — `src/pacman.rs`
```
pacman -Sy               синхронизация sync-репозитория (зеркало DeiX)
pacman -S <pkg>          установка с РЕКУРСИВНЫМ разрешением зависимостей
pacman -R <pkg>          удаление ·  pacman -Su  обновление
pacman -Q / -Qi / -Si    запросы ·  pacman -U <file.pkg.erofs>  установка из файла
```
Пакеты — контейнеры `.pkg.tar.zst` (хранимый zstd + tar-ustar); файлы
раскладываются строго внутри `/userdata/pkg/`. Встроенный репозиторий:
`deix-hello`, `deix-utils` (зависит от deix-hello), `deix-editor` (зависит от deix-utils).

### 2. Дисплейный сервер-композитор (альтернатива Wayland/X11) — `src/compositor.rs`
* Поверхности-окна, z-порядок, фокус, события (клик → поднятие окна + DS-обработчик).
* Композитинг back-to-front в framebuffer (рамки, заголовки, панель задач).
* Команда `wm` — демонстрация: DUIL App, Terminal, System Monitor.

### 3. UI-платформа DUIL (альтернатива Qt) — `src/duil.rs`
* 13 виджетов: window, vbox, hbox, grid, label, button, textbox, canvas, list,
  progressbar, groupbox, checkbox, spacer.
* Layout-движок (box/grid), стили (bg/fg/border/radius/padding/bold — аналог
  Qt stylesheet), события onclick → DS-скрипты.
* Команды: `duil run <file.duil>`, `duil render <file.duil>`, `duil demo`.

### 4. Скриптовый язык DS (альтернатива sh/bash) — `src/ds.rs`
* Переменные, интерполяция `$var`, массивы `let a=(x y z)`, `$a[i]`;
  арифметика с приоритетами, сравнения, логика, `$((expr))`.
* `if/elif/else`, `for in`, `while`, `until`, `case (паттерны =>)`.
* Функции `func name(args) { return N }`, вызовы, рекурсия (защита глубины).
* Команды: echo, print, sleep, cd, pwd, ls, cat, test, seq, whoami, pacman, duil…
* **Строгий синтаксис**: ошибки с позицией токена; shebang `#!/bin/ds`, REPL (`ds -i`).
* Команды: `ds <file.dxs>`, `ds -c <code>`, `dsdemo`.

### 5. Слой совместимости Linux x86_64 (glibc ABI) — `src/arch_abi.rs`
* Диспетчер Linux-совместимых syscall'ов (~40): write, read, open, close,
  mmap, brk, nanosleep, getpid, uname, clock_gettime, getcwd, chdir и др.
* Структуры timespec/timeval/utsname, errno. Позволяет запускать
  ELF-бинарники, собранные под Linux/glibc, на DeiX.

### 6. Многопоточность — `src/sched.rs`
* Планировщик ядра: TCB, состояния, приоритеты 1–10, кванты (PIT 100 Гц),
  round-robin, sleep/terminate. Демо: 4 потока (heuristic_core, net_daemon,
  pkg_worker, fs_sync).

### 7. Эвристический монитор Ring 3 — `src/security_monitor.rs`
* `SecurityEvent` + `HeuristicAnalysisEngine` (порог 0.85).
* Ransomware (freq>150 ∧ entropy>0.75 → нелинейный рост), код-инъекция
  (sys_mmap/sys_ptrace на системные пути → risk 1.0) → **SIGKILL** через планировщик.

### 8. Запрет su/sudo (политика безопасности)
```
su | sudo | root  →  ОТКАЗАНО: эскалация привилегий (su/sudo)
                      запрещена политикой безопасности DeiX.
```

---

## 🚀 Сборка и запуск

### Требования
* nightly Rust (сборка ядра), nasm, ld, objcopy; для запуска — QEMU.

### Сборка образа
```bash
./build.sh            # полная сборка: nasm → cargo+nightly → ld → objcopy
                      # результат: build/deix_disk.img (6.5 МБ)
```

### Запуск в QEMU
```bash
# графический режим:
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide -m 512M

# headless (лог в stdout, пароль диска по COM1):
(sleep 14; printf 'deix-disk-unlock-2026\n') | timeout 55 \
  qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
  -m 512M -display none -monitor none -serial stdio -no-reboot
```

### Порядок загрузки (виден в логе)
1. Загрузчик (MBR) → long mode → ядро (драйверы: ATA, PS/2, GPU, таймер…)
2. **TPM gate**: пароль разблокировки диска → XTS-AES-256 активен
3. security_monitor (самоконтроль: редактор SAFE, рансомвара SIGKILL)
4. pacman (-Sy → -S deix-editor с зависимостями → -Q → -Qi → -Su → -R)
5. композитор (окна, клик, фокус) → DUIL (Qt-подобный интерфейс)
6. DS (полный скрипт: while, for, функции, if/elif, case, арифметика)
7. arch_abi (syscall-слой: write/getpid/uname/clock_gettime)
8. Экран входа DeiX

---

## 🧱 Разметка диска (12 800 секторов × 512)

| Секторы | Назначение |
|---|---|
| 0 | MBR (boot_sector.bin, сигнатура 0xAA55) |
| 1..1058 | stage2: загрузчик 32/64-бит + ядро DeiX (541 592 байта) |
| 1059..4095 | запас |
| 4096..12287 | том ФС ядра (ext2, 4 МиБ): USERS.DB, AUTOSTART.CFG, DXINIT.CFG, скрипты |
| 12288..12799 | запас |

---

## 📁 Структура исходников

```
src/
├── lib.rs                 # реестр модулей, kernel_main (точка входа)
├── arch_abi.rs            # слой совместимости Linux x86_64 (glibc ABI)
├── arch_pkg_bridge.rs     # Arch PKG → .pkg.erofs (внутри /userdata)
├── compositor.rs          # дисплейный сервер (альтернатива Wayland/X11)
├── duil.rs                # UI-платформа (альтернатива Qt)
├── ds.rs                  # скриптовый язык (альтернатива sh/bash)
├── pacman.rs              # пакетный менеджер (порт Arch pacman)
├── sched.rs               # планировщик многопоточности
├── tpm.rs                 # TPM + полнодисковое шифрование (по умолчанию вкл.)
├── vault.rs               # KERNEL SECURITY VAULT (барьер разделов)
├── init_parser.rs         # парсер init.deix (PID 1)
├── security_monitor.rs    # эвристический демон Ring 3 (порог 0.85)
├── partition_map.rs       # карта разделов (/TPM скрыт)
├── kernel_loader.rs       # сэндвич ядра (EROFS, tar, gzip, zstd)
├── recovery_flash_engine.rs # TWRP/OrangeFox + Fastbootd + EDL
├── elf.rs                 # загрузчик статических ELF64
├── ext2.rs, fat16.rs      # файловые системы
├── ata.rs, serial.rs, vga.rs, keyboard.rs, mouse.rs, timer.rs, ...  # драйверы
├── crypto/                # sha1/sha256/sha512, aes, xts, pbkdf2, hmac
├── net/                   # eth/ipv4/tcp/icmp/http/arp
├── mm/                    # физический и виртуальный аллокаторы
└── ui/                    # графический рабочий стол
```

---

## 📜 История версий

* **v0.1** — базовое ядро: long mode, paging, VGA, ATA, сеть, GPU, файловые системы.
* **v0.2** — Ring 3, syscalls, модули `.kmod`, MEX API, DeiX Script v1, DUIL v1,
  dxinit, браузер, файловый менеджер.
* **v0.2-dev (вторая концепция)** — подсистема безопасности: TPM + шифрование
  диска, Vault-барьер разделов, эвристический монитор, планировщик
  многопоточности, pacman, композитор (Wayland/X11-alt), DUIL v2 (Qt-alt),
  DS v2 (sh/bash-alt), слой совместимости Linux x86_64.

---
*DeiX OS — репозиторий: https://github.com/Atimenka/DeiX (ветка development)*

## v0.5: OTA, Dev-режим, ADB, Verified Boot (AVB/vbmeta)

### OTA-обновления (`src/ota.rs`)
* `ota status` — статус прошивки и OTA-гарантии.
* `ota gen <payload>` / `ota apply <payload>` — формирование/применение OTA-пакета.
* Формат: `DEIXOTA1 | version | payload_len | sha256 | signature | payload`; подпись
  проверяется (SHA-256(payload||secret)); откат версии запрещён.
* Применение: запись SYSTEM.IMG + пересчёт vbmeta + перезагрузка.

### Dev-режим (`src/devmode.rs`)
* `dev on` — разблокировка bootloader + sudo; OTA-гарантия и лицензия
  безопасности ПЕРЕСТАЮТ действовать (обновления — только вручную через
  рекавери/прошивальщик). Verified Boot → ORANGE.
* `dev off` — блокировка bootloader (гарантия НЕ восстанавливается без
  перепрошивки через EDL).
* `dev` / `dev status` — состояние. `su`/`sudo`/`root` работают только в dev-режиме.

### ADB-подобный интерфейс (`src/adb.rs`)
* `adb devices|shell|push|pull|reboot|reboot recovery|reboot fastbootd|install|ota|dev|adb-repl`.

### Рекавери и прошивальщик
* `recovery` — вход в рекавери (TWRP/OrangeFox-стиль).
* `fastbootd` — вход в прошивальщик (flash/erase).
* `adb reboot recovery|fastbootd` — то же через ADB.

### Verified Boot / AVB / vbmeta (`src/avb.rs`)
* Три состояния при загрузке:
  * **GREEN** — bootloader locked, файлы целы: система грузится как обычно.
  * **ORANGE** — bootloader unlocked (dev): предупреждение + ЗАДЕРЖКА 5 сек.
  * **RED** — locked, но системные файлы изменены: загрузка ЗАПРЕЩЕНА,
    знак опасности ⚠ и красная надпись
    "Your device is corrupt. It can't be trusted and will not boot".
* vbmeta: SHA-256-хэши системных файлов, запечатанные при загрузке; сверка
  при каждом старте (`avb::boot_verify()` в kernel_main).
* `avb` — демонстрация переходов green/orange/red.

## v0.6: Recovery/Fastbootd GUI + BCB (одноразовый флажок загрузки)

### Bootloader Control Block (`src/bcb.rs`)
* Одноразовый флажок режима загрузки в BCB-секторе (LBA 3000, "DEIXBCB1").
* Приоритет при загрузке: **FASTBOOTD (выше всех) -> RECOVERY (выше ОС) -> NORMAL**.
* Флажок читается и СРАЗУ сбрасывается — следующая загрузка всегда обычная.
* Команды: `reboot recovery`, `reboot fastbootd`, `bcb <recovery|fastbootd|normal>`.

### Графический Recovery (`src/recovery_ui.rs`)
* Загружается РАНЬШЕ обычной ОС (по флажку), «клон ядра» (тот же бинарник,
  но работает только оболочка обслуживания).
* GUI на программном рендерере: меню Install/Backup/Restore/Factory Reset/
  Wipe Cache/Mount/Reboot; навигация стрелками/цифрами/Enter (и мышь в GUI).
* Проверено в QEMU: `reboot recovery` -> при перезапуске GUI-рекавери.

### Графический Fastbootd (`src/fastbootd_ui.rs`)
* Запускается ЕЩЁ ВЫШЕ recovery (проверяется первым).
* GUI: список разделов (8) + действия Flash/Erase/Reboot; flash валидирует
  EROFS для системных разделов (через FastbootdProtocol).
* Проверено в QEMU: флажок fastbootd -> GUI-прошивальщик.

### Оптимизация размера ядра
* Переход на `opt-level="z"` + LTO=fat + panic=abort: ядро **593 КБ -> 424 КБ**
  (829 секторов) — снова с большим запасом под лимит 640 КБ.
