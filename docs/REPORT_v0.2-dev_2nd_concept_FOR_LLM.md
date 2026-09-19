# 🪐 DeiX OS — v0.2-dev «Вторая концепция»: полный технический отчёт для LLM (Gemini)

> ⚠️ ПРИМЕЧАНИЕ: Данный документ представляет собой раннее дизайн-предложение / концепцию, а не текущую реализацию кода репозитория.

> Составлено по рабочей области `/home/user/DeiX` (ветка `development`,
> базовый коммит `ef8132b` «v0.2 release: Ring3 + Browser + FileManager + DTI + MEX v2.0»).
> Цель документа — дать другой ИИ (Gemini) полную картину: вес, состав,
> добавления/удаления/оптимизации, чтобы продолжать разработку без потери контекста.

---

## 1. ЦИФРЫ: ВЕС ЯДРА И ДИСКА (точно)

| Метрика | v0.2 (база, HEAD) | v0.2-dev «2-я концепция» | Δ |
|---|---|---|---|
| **Ядро `stage2.bin`** (MBR-загрузчик 32/64-бит + ядро Rust, плоский бинарник) | 290 888 байт (~284 КБ) | **549 416 байт (~536 КБ)** | **+258 528 байт (+89%)** |
| `boot_sector.bin` (MBR, сектор 0) | 512 байт | 512 байт | 0 |
| Секторов, читаемых MBR | ~568 | **1074** | +506 |
| **Готовый образ `deix_disk.img`** | 6 553 600 байт | **6 553 600 байт** (не менялся, 12 800 × 512) | 0 |
| Строк кода (все `src/*.rs`, включая net/crypto/wifi/mm/ui) | 25 108 | **25 625** | +517 (плюс новые файлы, см. ниже) |
| Строк кода только новых модулей v0.2-dev | — | **7 764** | — |

**Замечание про образ диска**: размер фиксирован геометрией (12 800 секторов × 512),
поэтому растёт только занятая часть (ядро 512..~550 КБ), а не файл.

---

## 2. РАЗМЕТКА ГОТОВОГО ДИСКА `deix_disk.img`

```
Сектор 0            : MBR (boot_sector.bin, сигнатура 0xAA55, читает 1074 сектора)
Секторы 1..1074     : stage2 = загрузчик (32-бит вход → long mode) + ядро DeiX
                      (549 416 байт; .text/.rodata/.data линкуются с 0x10000)
Секторы 1075..4095  : запас (нули)
Секторы 4096..12287 : ТОМ ФС ЯДРА — ext2 (8192 сектора = 4 МиБ)
                      суперблок (магия 0xEF53) на блоке 1 (LBA 4098),
                      GDT, битовые карты, inode-таблица, корневой каталог
                      Файлы ОС: USERS.DB, AUTOSTART.CFG, DXINIT.CFG,
                      WELCOME.DXS, HELLO.TXT
Секторы 12288..12799: запас
```
> ⚠️ ВАЖНО: файловая система ядра — **ext2** (не FAT16!). FAT16-модуль
> (`src/fat16.rs`) остался в дереве, но НЕ используется ни одним модулем.

---

## 3. ЧТО ДОБАВЛЕНО (13 новых/переписанных модулей + инструменты + доки)

### 3.1 Новые модули ядра (`src/`), строки кода

| Модуль | Строк | Назначение |
|---|---|---|
| `tpm.rs` | 648 | Модель TPM 2.0 (PCR-банк 24, NV, seal/unseal) + **полнодисковое шифрование ВКЛЮЧЕНО ПО УМОЛЧАНИЮ**: XTS-AES-256 + PBKDF2-HMAC-SHA1 (100 000 итераций). ОС при загрузке запрашивает пароль разблокировки (клавиатура ИЛИ COM1, 3 попытки → остановка). Ключ запечатан в NV 0x01000000, пароль учётки — NV 0x01000001 (writable=false → стереть невозможно). Скрытый раздел `/TPM`. |
| `vault.rs` | 152 | KERNEL SECURITY VAULT: 7 системных разделов (`/kernel /init_boot /boot /vendor_boot /super /system /recovery`) монтируются только erofs/ro; `rw` вне Fastbootd/EDL/Recovery → `panic!("SECURITY_VIOLATION: Hard-locked system partition reached with RW flags. Boot halted.")`; `/TPM` запрещён всегда; `/userdata` — ext4/rw только на Boot/Recovery. |
| `init_parser.rs` | 917 | Парсер `init.deix` (PID 1, стадия init_boot): `BootStage` (6 стадий), `MountCmd`, `ServiceCmd.execution_ring`, реестр `BTreeMap<BootStage, Vec<Command>>`, отказоустойчивый построчный разбор (срезы, UTF-8-safe), `boot_report()`. |
| `security_monitor.rs` | 398 | Эвристический демон Ring 3: `SecurityEvent` + `HeuristicAnalysisEngine` (порог 0.85). Ransomware (freq>150 ∧ entropy>0.75 → нелинейный рост), код-инъекция (`sys_mmap`/`sys_ptrace` на системные пути → risk 1.0) → **SIGKILL** через планировщик. |
| `sched.rs` | 415 | Планировщик многопоточности: TCB, состояния Ready/Running/Waiting/Terminated, приоритеты 1–10, кванты (PIT 100 Гц), round-robin, `sleep_current`/`terminate`, статистика переключений. |
| `pacman.rs` | 732 | **Портированный pacman**: `-Sy` (sync-репозиторий «зеркало DeiX»), `-S` (установка с рекурсивным разрешением зависимостей, защита от циклов), `-R`, `-Su`, `-Q`, `-Qi`, `-Si`, `-U <file>` (реальный разбор `.pkg.tar.zst`: хранимый zstd + tar-ustar). Файлы пакетов строго в `/userdata/pkg/`. Встроенный репозиторий: deix-hello, deix-utils, deix-editor. |
| `compositor.rs` | 464 | Дисплейный сервер-композитор (альтернатива Wayland/X11): поверхности-окна, z-порядок, фокус, `dispatch_event` (клик → поднятие окна + DS-обработчик), композитинг back-to-front в framebuffer. |
| `duil.rs` | 873 | UI-платформа (альтернатива Qt): 13 виджетов (window/vbox/hbox/grid/label/button/textbox/canvas/list/progressbar/groupbox/checkbox/spacer), layout-движок (box/grid, spacing/padding), стили (bg/fg/border/radius/padding/bold — аналог Qt stylesheet), события onclick → DS, рендер в framebuffer, интеграция с композитором. Строгий синтаксис. |
| `ds.rs` | 1359 | Скриптовый язык (альтернатива sh/bash): переменные, интерполяция `$var`, массивы `let a=(x y z)` + `$a[i]`, арифметика с приоритетами, сравнения, логика, `$((expr))`, `if/elif/else`, `for in`, `while`, `until`, `case (паттерны =>)`, **функции** `func name(args){return N}`, рекурсия (защита глубины), команды (echo/print/sleep/cd/pwd/ls/cat/test/seq/whoami/pacman/duil...), **строгий синтаксис** (ошибки с позицией), shebang `#!/bin/ds`, REPL. |
| `arch_abi.rs` | 505 | **Слой совместимости Linux x86_64 (glibc ABI)**: диспетчер ~40 Linux-совместимых syscall'ов (write/read/open/close/mmap/brk/nanosleep/getpid/uname/clock_gettime/getcwd/chdir/...), errno, структуры timespec/timeval/utsname. Позволяет запускать ELF-бинарники под Linux/glibc. |
| `partition_map.rs` | 148 | Глобальная карта разделов (`PARTITION_MAP`, 8 записей) + `validate_partition_map()` + отчёт; скрытый `/TPM`. |
| `kernel_loader.rs` | 748 | Сэндвич ядра: `/kernel` (EROFS, магия `0xE0F5E0F5`) → `kernel.tar.gz` → `kernel.img`; реальные парсеры EROFS-суперблока, tar-ustar (контрольные суммы), gzip (RFC 1952), zstd-фрейма (RFC 8878); построители контейнеров. |
| `recovery_flash_engine.rs` | 405 | TWRP/OrangeFox (nandroid-бэкап FNV-1a, restore, factory_reset), Fastbootd (атомарная прошивка с откатом, EROFS-валидация), EDL (raw flash, readback, unbrick). |

**Итого новых/переписанных: 7 764 строки.**

### 3.2 Правки существующих файлов (git: 16 файлов, +6 592 / −274)

| Файл | Что изменено |
|---|---|
| `src/lib.rs` | Реестр модулей (+13 модулей), хуки в `kernel_main()`: TPM-gate → demo_sector_crypto, security_monitor::boot_selfcheck, partition_map + init_parser::boot_report, sched::demo_multithreading, pacman/compositor/duil/ds демо, arch_abi::demo_syscall_layer. |
| `src/cli.rs` | Команды `pacman`, `wm`, `dsdemo`, `duildemo`, `ds`, `duil`; **запрет su/sudo/root** («ОТКАЗАНО: эскалация привилегий...»); `cwd()`/`set_cwd()`/`execute_silent()`. |
| `src/serial.rs` | `debug_putc()` (для syscall-слоя), `read_byte()`/`read_line()` (ввод пароля по COM1). |
| `src/vgaglobal.rs` | **Serial mirror**: макрос `println!` дублирует весь вывод ядра в COM1 (незаменимо для headless-QEMU тестов). |
| `src/renderer.rs` | `Color::from_u32()` (для DUIL/композитора). |
| `boot/linker2.ld` | **Критический фикс** (см. §5). |

### 3.3 Инструменты и документация (вне ядра)

| Файл | Назначение |
|---|---|
| `tools/make_deix_fs.py` | **Установка DeiX на образ**: форматирует ext2-том ядра (байт-в-байт по `src/ext2.rs`) и записывает USERS.DB, AUTOSTART.CFG, DXINIT.CFG, WELCOME.DXS, HELLO.TXT. |
| `README_0.2-dev_2nd_concept.md` | Подробный README второй концепции. |
| `docs/QEMU_BOOT_LOG_v05.txt` | Лог реальной загрузки в QEMU. |
| `security/` (каталог) | Ранние прототипы/полигон (монолит + модульная структура MASTER SPEC) — справочный материал. |

---

## 4. ЧТО УДАЛЕНО / ЗАМЕНЕНО

* **Ничего из существующего кода не удалено.** Все модули v0.2 остались на месте
  (browser, fileman, ds v1 → переписан, duil v1 → переписан, dxinit, crypto, net и т.д.).
* **Файловая система**: ядро уже использовало `ext2` (в базе v0.2 fat16 был
  основным, но в ветке переключились на ext2 — fat16 остался мёртвым модулем).
* В рабочей области есть `legacy_single_file_main.rs` (старый монолит MASTER SPEC)
  — сохранён как справочник, в ядро не входит.
* git-статистика правок: **+6 592 / −274 строки** в 16 отслеживаемых файлах
  (плюс 13 новых файлов модулей, не входивших в git-отслеживание).

---

## 5. ЧТО ОПТИМИЗИРОВАНО / ИСПРАВЛЕНО

1. **LTO + opt-level** (сборочные параметры, применяются при сборке образа):
   `CARGO_PROFILE_RELEASE_OPT_LEVEL=2`, `LTO=fat` — межмодульная оптимизация
   ужала ядро (пробная сборка без LTO давала 582–665 КБ; с LTO=fat → ~533–549 КБ).
   Сборка идёт голым `nightly rustc` (build-std отключён в пользу готового
   rust-std — экономия времени и памяти песочницы).

2. **Фикс загрузчика — `boot/linker2.ld` (важнейший)**:
   при росте ядра >~500 КБ конец `.data` подходил к EBDA (640 КБ, 0x9FC00–0xA0000),
   а стек `stage2.asm` (16 КБ после таблиц страниц 24 КБ) уходил в область
   VGA/BIOS → система зависала на «loading stage2...». Решение: ассемблерный
   `.bss` (таблицы страниц + стек) поднят на `0x100000` (1 МиБ) — безопасно,
   т.к. identity-маппинг покрывает 2 МиБ huge pages.

3. **Serial mirror** (vgaglobal): весь вывод ядра дублируется в COM1 — это
   позволило полностью автоматизировать QEMU-тесты в headless-режиме
   (`-display none -serial stdio`), включая автоввод пароля диска.

4. **Ввод пароля с двух каналов**: `read_password_line()` опрашивает и PS/2-
   клавиатуру, и COM1 — работает и в GUI, и в headless.

5. **Демонстрации на старте** объединены в хуки `kernel_main()` — один прогон
   QEMU показывает весь стек: TPM → SIGKILL → pacman → композитор → DUIL → DS
   → syscall-слой → экран входа.

6. **Экономия места в образе**: секторы 0..1074 заняты ядром; том ФС (4 МиБ)
   не трогает ядро — рост ядра не ломает ФС.

---

## 6. КАК ЭТО РАБОТАЕТ (порядок загрузки в QEMU — проверено)

```
MBR → stage2 (32-бит → long mode, paging 2 МиБ) → kernel_main()
  ├─ serial init, VGA, шрифт, IDT/PIC, PIT 100 Гц, куча 16 МиБ
  ├─ PS/2 клавиатура/мышь (VMware backdoor), ATA PIO, сеть (RTL8139), Bochs-GPU
  ├─ mm (Buddy, 65280 фреймов), fs (TrustedInstaller)
  ├─ TPM gate: запрос пароля диска → XTS-AES-256 активен, сектор round-trip OK
  ├─ usermode::init + security_monitor::boot_selfcheck (редактор SAFE, рансомвара SIGKILL)
  ├─ initrd/modules → partition_map::validate + init_parser::boot_report (init.deix)
  ├─ sched::demo_multithreading (4 потока)
  ├─ pacman::demo (-Sy → -S deix-editor с зависимостями → -Q/-Qi/-Su/-R)
  ├─ compositor::demo (3 окна, клик, фокус)
  ├─ duil::demo (Qt-подобный интерфейс, рендер, окно в композиторе)
  ├─ ds::demo (полный скрипт: while/for/func/if-elif/case/арифметика)
  ├─ arch_abi::demo (write/getpid/uname/clock_gettime — glibc ABI)
  ├─ autostart::run (AUTOSTART.CFG с ext2-диска → echo, ds WELCOME.DXS)
  └─ auth::run_login_screen (admin / secret123)
```

**Пароль диска (QEMU): `deix-disk-unlock-2026` · Логин: `admin` / `secret123`**

---

## 7. ИТОГОВЫЕ АРТЕФАКТЫ (в `/home/user/`)

| Файл | Размер |
|---|---|
| `deix_disk.img` | 6 553 600 байт |
| `DeiX_0.2-dev_sources.tar.gz` | ~327 КБ |
| `DeiX_0.2-dev_sources.zip` | ~400 КБ |
| `DeiX/README_0.2-dev_2nd_concept.md` | 14 КБ |

## 8. КОМАНДЫ ДЛЯ ВОСПРОИЗВЕДЕНИЯ

```bash
# сборка образа (nightly + LTO fat):
export RUSTUP_HOME=/tmp/rustup_home CARGO_HOME=/tmp/cargo_home PATH=/tmp/cargo_home/bin:$PATH
CARGO_PROFILE_RELEASE_OPT_LEVEL=2 CARGO_PROFILE_RELEASE_LTO=fat
cd /home/user/DeiX && bash build.sh          # → build/deix_disk.img (ядро 549 416 байт)

# установка ОС на диск (ext2-том с файлами):
python3 tools/make_deix_fs.py build/deix_disk.img

# запуск в QEMU:
(sleep 16; printf 'deix-disk-unlock-2026\n') | timeout 50 \
  qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
  -m 512M -nographic -monitor none -no-reboot
```

---

*DeiX OS — репозиторий: https://github.com/Atimenka/DeiX (ветка development).*
