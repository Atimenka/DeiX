# DeiX Security Subsystem — интеграция в ядро (ветка development)

## Что интегрировано

В существующее ядро DeiX OS (репозиторий `Atimenka/DeiX`, ветка `development`,
коммит `ef8132b`) добавлена подсистема инициализации и безопасности.
**Новые файлы (7 модулей в `src/`):**

| Файл | Модуль | Роль |
|---|---|---|
| `src/init_parser.rs` | PID 1 (Ring 0) | парсер `init.deix`: `BootStage`, `MountCmd`, `ServiceCmd.execution_ring`, реестр `BTreeMap<BootStage, Vec<Command>>`, отказоустойчивый построчный разбор, `boot_report()` |
| `src/vault.rs` | KERNEL SECURITY VAULT | `SYSTEM_PARTITIONS` (7 разделов), `evaluate()` — erofs/ro обязательно; rw — только Fastbootd/EDL/Recovery; иначе `panic!` с эталонным текстом; политика `/userdata` |
| `src/security_monitor.rs` | Ring 3 демон | `SecurityEvent`, `HeuristicAnalysisEngine` (порог 0.85), ransomware/код-инъекция, SIGKILL; `boot_selfcheck()` |
| `src/kernel_loader.rs` | сэндвич ядра | `ErofsSuperBlock` (магия `0xE0F5E0F5`), `TarArchive` (ustar), `GzipHeader` (RFC 1952), `load_and_boot_kernel()` |
| `src/arch_pkg_bridge.rs` | Arch мост | PKGBUILD/.BUILDINFO/.PKGINFO, zstd (RFC 8878), `.pkg.tar.zst → .pkg.erofs`, монтирование только внутри `/userdata` |
| `src/recovery_flash_engine.rs` | TWRP/Fastbootd/EDL | nandroid-бэкап (FNV-1a), атомарная прошивка с откатом, raw-flash + unbrick |
| `src/partition_map.rs` | карта разделов | `PARTITION_MAP`, `validate_partition_map_report()` |

**Отредактированный существующий файл:**

* `src/lib.rs` — в реестр модулей добавлены `arch_pkg_bridge`, `init_parser`,
  `kernel_loader`, `partition_map`, `recovery_flash_engine`, `security_monitor`,
  `vault` (в алфавитном порядке, как принято в ветке). В `kernel_main()` вставлены
  хуки:
  1. после `usermode::setup_syscall_table()` — `security_monitor::boot_selfcheck()`
     (самоконтроль демона Ring 3: легитимная запись → SAFE, рансомвара → SIGKILL);
  2. после `module::load_boot_modules()` — `partition_map::validate_partition_map_report()`
     и `init_parser::boot_report(init_parser::INIT_DEIX_SCRIPT)` (стадия init_boot,
     PID 1: проверка карты разделов и разбор `init.deix`; нарушение Vault →
     `panic!`, ядро останавливается).

## Стилистика (no_std)

Все модули написаны в стиле ветки: `#![allow(dead_code)]`, `alloc`-типы
(`BTreeMap`, `String`, `Vec`, `format!`, `vec!`), вывод через
`crate::println!`/`crate::print!` (как `dxinit.rs`, `ds.rs`). Ноль внешних
крейтов — в `Cargo.toml` секция `[dependencies]` по-прежнему отсутствует.

## Верификация

* `cargo +nightly check --release` (target `x86_64-unknown-none`) — **0 ошибок**,
  предупреждений по новым модулям **0** (оставшиеся 24 — по существующим
  модулям ветки `duil.rs`, `fileman.rs`, `browser.rs` и др.).
* Отдельная host-проверка модулей как `#![no_std]`-библиотеки (stable rustc,
  `-D warnings`) — **0 предупреждений**.
* Автотесты полигона (`security/master_spec`, `rustc --test`) — **17/17 passed**.

## Сборка ядра (как в ветке)

```bash
# nightly + nasm + build-std (см. build.sh репозитория)
./build.sh
```

Полный бинарный образ (ISO/диск) собирается по штатной схеме репозитория:
nasm → cargo +nightly (build-std, target x86_64-unknown-none) → ld → objcopy.

## v0.3: многопоточность + TPM + шифрование диска + запрет su

### Многопоточность (`src/sched.rs`)
Планировщик ядра: пул TCB (`ThreadControlBlock`), состояния
Ready/Running/Waiting/Terminated, приоритеты 1..=10, кванты времени (PIT 100 Гц),
round-robin диспетчер, `sleep_current`, `terminate`, статистика переключений.
`demo_multithreading()` порождает 4 потока (heuristic_core, net_daemon,
pkg_worker, fs_sync) и прокручивает 40 тиков — в QEMU показана таблица потоков
и 1 переключение контекста за 400 мс.

### TPM + полнодисковое шифрование (`src/tpm.rs`)
* Модель TPM 2.0: PCR-банк (24), NV-хранилище, seal/unseal, привязанные к PCR 7.
* Шифрование диска ВКЛЮЧЕНО ПО УМОЛЧАНИЮ (XTS-AES-256, ключ — PBKDF2-HMAC-SHA1
  100000 итераций). При загрузке ОС ЗАПРАШИВАЕТ пароль разблокировки
  (ввод с клавиатуры или COM1); 3 попытки, затем остановка.
* Ключ диска запечатан в TPM NV 0x01000000 (writable=false после первого
  программирования — стереть невозможно); верификатор пароля учётной записи —
  NV 0x01000001.
* Скрытый раздел `/TPM`: не монтируется в Ring 3, vault запрещает любой доступ
  (VaultRejection::TpmPartitionDenied), стирание невозможно.
* Демо шифрования сектора: XTS round-trip OK, шифртекст отличается от открытого.

### Запрет su/sudo (`src/cli.rs`)
В диспетчере команд ветка `"su" | "sudo" | "root"` → `ACCESS DENIED ... forbidden
by DeiX security policy` (ОТКАЗАНО). Эскалация привилегий в Ring 3 полностью
запрещена; привилегированные операции выполняет только PID 1 (Ring 0).

### Верификация в QEMU (реальный прогон ОС)
QEMU 10.0.11, nightly 1.99 (сборка: rustc -> nasm -> ld -> objcopy). Команда:
```bash
(sleep 14; printf 'deix-disk-unlock-2026\n') | timeout 70 \
  qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
  -m 512M -display none -monitor none -serial stdio -no-reboot
```
Лог полной загрузки — `docs/QEMU_BOOT_LOG.txt`: ядро, все драйверы (ATA, PS/2,
RTL8139 отсутствует — сеть пропускается, Bochs-GPU), mm, fs, TPM gate (диск
разблокирован, XTS round-trip), security_monitor (SIGKILL рансомвара PID 666),
partition_map (/TPM скрыт), init_parser (21 команда), sched (4 потока),
экран логина DeiX.

## v0.4: pacman + композитор (Wayland/X11 alt) + DUIL (Qt alt) + DS (sh/bash alt)

### Пакетный менеджер (`src/pacman.rs`) — порт Arch pacman
* `pacman -Sy` — синхронизация sync-репозитория («зеркало» DeiX, встроенный каталог).
* `pacman -S <pkg>` — установка с РЕКУРСИВНЫМ разрешением зависимостей (DFS, защита от циклов).
* `pacman -R/-Su/-Q/-Qi/-Si`, `pacman -U <file.pkg.erofs>` — удаление/обновление/запросы/
  установка из файла (реальный разбор .pkg.tar.zst через tar-парсер ядра).
* Пакеты — контейнеры .pkg.tar.zst (хранимый zstd + tar-ustar); файлы раскладываются
  СТРОГО внутри /userdata/pkg (системные EROFS-разделы неизменяемы).
* Встроенный репозиторий: deix-hello, deix-utils (зависит от deix-hello),
  deix-editor (зависит от deix-utils). Проверено в QEMU: -S deix-editor подтянул
  обе зависимости.

### Композитор (`src/compositor.rs`) — альтернатива Wayland/X11
* Дисплейный сервер: поверхности (WindowSurface), z-порядок, фокус, события.
* `create_surface/destroy_surface/set_minimized/mark_damage/set_click_handler`.
* `dispatch_event` — клик -> поднятие окна (click-to-raise) + фокус + DS-обработчик.
* `composite()` — рендер back-to-front в framebuffer (рамки, заголовки, панель задач).
* Демо: 3 окна (DUIL App, Terminal, System Monitor), клик, дамп поверхностей.

### DUIL (`src/duil.rs`) — полноценная альтернатива Qt
* Виджеты: window, vbox, hbox, grid, label, button, textbox, canvas, list,
  progressbar, groupbox, checkbox, spacer.
* Layout-движок: box (spacing/padding), grid (cols/rows), выравнивание.
* Стили (аналог Qt stylesheet/CSS): bg, fg, border, radius, padding, bold.
* События: onclick -> DS-скрипт (через ds::run_string).
* Рендер в framebuffer (реальные прямоугольники/текст/прогресс-бары) +
  интеграция с композитором (`duil run` — окно как поверхность).
* Строгий синтаксис: неизвестный виджет/свойство — ошибка парсинга.
* Проверено в QEMU: парсинг OK, рендер OK, окно в композиторе.

### DS (`src/ds.rs`) — полноценная альтернатива sh/bash
* Переменные, строки с интерполяцией ($var), числа, булевы, массивы (let a=(x y z), $a[i]).
* Арифметика с приоритетами ( * / % > + - ), сравнения (== != < > <= >=),
  логика (&& || !), $((expr)).
* if/elif/else, for in, while, until, case (паттерны => блоки).
* Функции: func name(args) { ... return N }, вызовы, рекурсия (защита глубины).
* Команды: echo print sleep exec exit uptime cd pwd ls cat clear test seq whoami
  pacman duil и др. (делегирование в CLI ядра).
* СТРОГИЙ синтаксис: ошибки с позицией токена (не молчаливый пропуск).
* Поддержка shebang (#!/bin/ds), REPL (ds -i), -c, файлы .dxs.
* Проверено в QEMU: полный скрипт демонстрации выполнен успешно.

### Верификация в QEMU (лог: docs/QEMU_BOOT_LOG_v04.txt)
TPM gate -> SIGKILL рансомвара -> pacman (-Sy -S -Q -Qi -Su -R) -> композитор
(окна/клик) -> DUIL (Qt-подобный интерфейс) -> DS (полный скрипт) -> экран входа.
Образ: build/deix_disk.img (LTO, opt=2, ~535KB stage2).
