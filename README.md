# 🪐 DeiX OS (v0.3-dev · development)

A modern, highly-secure 64-bit Operating System written from scratch in **Rust** and **Assembly** for the `x86_64` architecture. Developed in a unique collaboration between a Human Architect and an AI Agent.

Операционная система нового поколения, написанная с нуля на **Rust** и **Ассемблере** под `x86_64`. Ветка `development` — актуальная: сюда попали итоги KISS-рефакторинга (−38 % кода, −12 146 строк), подсистема безопасности с LUKS-слотами, графический вход, открытый драйвер NVIDIA и UI-звуки.

**Ядро:** ~0.5 МБ (Rust `#![no_std]` + NASM stage2) · **Образ диска:** 10 МиБ · **Код:** ~25 000 строк Rust, 92 файла · фиксированное железо, реальное железо и QEMU.

---

## ⚡ Новое в development (после v0.2)

### 🔐 LUKS-криптография со слотами паролей
Несколько паролей на один том — как в настоящем LUKS: мастер-ключ шифруется
каждым паролем-слотом. `PBKDF2-HMAC-SHA512` для вывода ключей. Управление —
`crypt status/addpass/delpass/iter`. Том совместим с Linux `cryptsetup`
(XTS-AES-256).

### 🖥️ Графический вход и аккаунты
GUI-экран входа (VBE): создание первого аккаунта, вход с маскировкой пароля,
**экран блокировки** (`lock`), профили пользователей (`/users/<имя>/files`,
`/users/<имя>/configs` — команда `profile`). Том шифруется **прямо из
графического входа** — раньше USERS.DB читался обычным 7-Zip, это исправлено.
`AUTOSTART.CFG` выполняется только ПОСЛЕ успешной аутентификации.

### 🔊 UI-звуковые эффекты (PC speaker, ШИМ)
Системные звуки как в Android (`/system/media/audio/ui`): `start` при входе,
`error` на неверный пароль, `usbcon` при загрузке с флешки; `lowbat`/`fullbat`/
`usbdisc` — API готово, появятся с ACPI/USB-стеком. WAV конвертируются на
сборке в компактный DPS1 (8 кГц/8-бит/моно) и лежат в EROFS `/super` — ядро
не растёт ни на байт. Команда `sound` — список и ручное воспроизведение.

### 🧩 HAL + открытый драйвер NVIDIA
Прослойка драйверов (`src/hal`, MMIO) и открытый драйвер NVIDIA
(`src/drivers/nvidia.rs`, на базе nouveau): `hal` — самопроверка на RTL8139,
`nvidia` — поиск и опознание карты, дамп регистров в файл для отладки на
реальном железе.

### 📁 ext2 с настоящими подкаталогами
Многоуровневые каталоги, `mkdir_p`, файлы по путям (`read_file_path` и ко) —
менять ФС на ext4/NTFS не понадобилось.

### 🌐 TCP + HTTP/1.0
Минимальный TCP (RFC 793) и HTTP-клиент (GET, статусы, тело до 2 МиБ) поверх
RTL8139. Главный потребитель — `ota fetch`: OTA-пакет скачивается «по воздуху»
с host-сервера и прошивается в неактивный слот (A/B).

### 💾 Загрузка с USB (ramboot)
Boot from флешки/Ventoy: весь образ дочитывается в RAM (0x2000000), дальше
разделы работают из памяти — аппаратный диск может вообще отсутствовать.
При этом играется звук `usbcon`.

### 🧠 Intel microcode
Обновление микрокода CPU из встроенного блоба (`data/mc-06-2a-07.bin`),
команда `microcode`. На реальном железе применение аккуратно отключено
там, где оно вызывало перезагрузку.

### 🐛 Фиксы реального железа
- **Triple fault** при загрузке — пустая IDT и неверный порядок
  инициализации; исправлено, проверено на железе.
- ramboot раньше читал **чужой диск** вместо флешки — исправлено.
- VGA: сброс Sequencer при возврате в текстовый режим, очистка 0xB8000;
  буфер ядра поднят выше VGA-памяти.

### 🧹 KISS-рефакторинг (−12 146 строк, −38 %)
Из ядра вынесены подсистемы без реальных потребителей — подробности и
доказательства в `REFACTOR_REPORT.md`.

| Удалено | Почему |
|---|---|
| Wi-Fi 802.11 + WPA2-PSK | единственная реализация драйвера — `NoWifiHardware` |
| DS-скрипты, DUIL, композитор | самодостаточные интерпретаторы без связи с ядром |
| Браузер, nv3d, DisplayPort, xHCI, HDA | модули-сироты (ноль вызовов) |
| pacman, Arch ABI-мост, ELF-дубль, FAT16 | демо-показы/дубли живых механизмов |
| dxinit, fileman, security_monitor | параллельные механизмы при живых `init_parser`/GUI |

---

## ⚡ База ядра

- **🦀 Pure Rust `#![no_std]`**, загрузка сразу в long mode: `boot_sector` (MBR)
  → `stage2` (32-бит вход → long mode → ATA-ридер ядра) → `kernel.bin` @ 0x100000.
  Лимит размера ядра снят (до ~2 МиБ — ядро читается уже в 64-битном режиме).
- **🛡 Ring 3 + syscall/sysret** — аппаратная изоляция (GDT+TSS), самопроверка при загрузке.
- **🧵 Вытесняющая многозадачность** — планировщик с naked-заглушкой IRQ0
  (`threads list/test`).
- **🔁 kexec** — перезапуск ядра из `/kernel_a|b` без BIOS (`kexec check/a/b`),
  счётчик поколений вне `.bss` (0x1360000).
- **📦 A/B-слоты + OTA** — `/kernel_a|b`, `/boot_a|b`, активный слот в BCB;
  `ota check/fetch/apply/rollback`, откат через `bcb slot`.
- **🧾 Настоящий EROFS** (магия `0xE0F5E1E2`, проходит `fsck.erofs`) во всех
  системных разделах; `kernel.tar.gz` — настоящий gzip, ядро распаковывает
  своим inflate (`src/inflate.rs`).
- **🔒 Цепочка загрузки с верификацией** — `/dsm` → `/init_boot` → `/vendor_boot`
  → `/boot` → `/kernel`; повреждённый раздел = «ЗАГРУЗКА ОСТАНОВЛЕНА»
  (аналог Android RED state), AVB-маркер.
- **🥽 Режимы загрузки по BCB**: **DSM** (emergency-прошивка по COM1,
  READ/WRITE/FLASH/ERASE/VERIFY SHA-256), **fastbootd** (полный прошивальщик,
  oem unlock/lock), **recovery** (TWRP-style: Install OTA, Nandroid backup/restore,
  factory reset, sideload).
- **🔎 Отладчик как в Android** — `bugreport` (полный отчёт → экран + файл),
  `dmesg` (кольцевой журнал), `crashlog` (tombstone паники сырыми секторами —
  переживает перезагрузку).
- **🐧 Linux-совместимость** — ELF-загрузчик + слой Linux syscalls
  (`src/linux/`), `linux run/info`.
- **🎨 Графика и UI** — VBE до 1280x1024, мышь, графические диалоги,
  кириллический шрифт (VGA Plane 2), локализация en/ru (`lang`).
- **🌐 Сеть** — RTL8139 + ARP + IPv4 + ICMP + TCP + HTTP (`ifconfig`, `arp`,
  `ping`).
- **📦 MEX-приложения** — собственный формат программ (`run`, `pkg`),
  собираются `mexcc`/`mexmake` (инструменты в `tools/`).

---

## 🗂️ Карта разделов (10 МиБ, MBR + extended)

| Раздел | LBA | Секторов | ФС | Назначение |
|---|---|---|---|---|
| `/system` | 4096 | 8192 | ext2 rw | рабочий том ядра (USERS.DB, AUTOSTART.CFG, профили) |
| `/TPM` | 12288 | 512 | скрытый 0xDA | маркер TPM (пароли/ключи — не читается из ОС) |
| `/userdata` | 12800 | 512 | ext2 rw | данные Ring 3 |
| `/kernel_a` | 13313 | 1279 | EROFS ro | слот A: `kernel.tar.gz` (gzip, inflate в ядре) |
| `/kernel_b` | 14593 | 1279 | EROFS ro | слот B |
| `/init_boot` | 15873 | 255 | EROFS ro | `bootloader.bin` |
| `/vendor_boot` | 16129 | 255 | EROFS ro | `vendor.bin` (HAL) |
| `/boot_a` | 16385 | 255 | EROFS ro | `fastbootd.bin`, `recovery.bin` |
| `/boot_b` | 16641 | 255 | EROFS ro | слот B |
| `/super` | 16897 | 255 | EROFS ro | `system.img` + **UI-звуки (`*.dps`)** |
| `/dsm` | 17153 | 255 | EROFS ro | `dsm.bin` (emergency) |
| `/recovery` | 17409 | 255 | EROFS ro | `recovery.bin` |
| `/OTA` | 17664 | 2816 | ext2 | скачанные OTA-пакеты (сырое хранилище ядра) |

> ⚠️ Правило синхронизации: карта должна совпадать в трёх местах —
> `src/partition_map.rs`, `tools/make_deix_fs.py` (`PRIMARY`/`LOGICALS`),
> `tools/deix_ota.py` (`SLOTS`). Иначе bootchain не найдёт dsm.bin.

---

## 💻 CLI

```
help about echo clear uptime color cpuid mem lang
ifconfig arp ping gpu [info|nvinfo|mode] sound [list|play|beep]
ls cat write rm pkg run install bigfile
useradd passwd whoami users encrypt crypt
nvidia hal microcode logo linux profile lock
threads kexec crash bugreport dmesg crashlog
reboot [normal|recovery|fastbootd|dsm] bcb ota adb dev root halt
```

Полная справка — внутри системы: `help`.

---

## 🔊 UI-звуки

| Файл | Когда звучит |
|---|---|
| `start.dps` | вход выполнен, система готова |
| `error.dps` | неверный пароль (GUI и текстовый логин) |
| `usbcon.dps` | загрузка с USB-флешки (RAM-диск) |
| `usbdisc.dps` | API готово — событие появится с USB-стеком |
| `lowbat.dps` / `fullbat.dps` | API готово — события появятся с ACPI |

Конвейер: `assets/*.wav` --(build.sh [2e/8], `tools/wav2dps.py`)-->
`build/sounds/*.dps` --(`tools/make_deix_fs.py`)--> EROFS `/super` -->
ШИМ-проигрыватель PC speaker (`src/sound.rs`). Ручная проверка: `sound`,
`sound start`, `sound beep 880 200`. В QEMU звук спикера:
`-audiodev pa,id=snd0 -machine pcspk-audiodev=snd0`.

---

## 🚀 Быстрый старт

```bash
# Зависимости: rustup nightly + x86_64-unknown-none, nasm, qemu-system-x86
./build.sh        # nasm → cargo → ld → образ диска → разделы → ISO
./run.sh          # QEMU, VGA-окно
./run_iso.sh      # QEMU, загрузка с ISO
```

```bash
# Headless (весь вывод в терминал):
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
  -m 256M -display none -serial stdio -no-reboot

# С сетью и звуком PC speaker:
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
  -m 256M -serial stdio -netdev user,id=n0 -device rtl8139,netdev=n0 \
  -audiodev pa,id=snd0 -machine pcspk-audiodev=snd0
```

При первом запуске система попросит создать аккаунт — он же ключ
шифрования диска (XTS-AES-256). Подробности: `BUILD_AND_TEST.md`,
`TERMUX_SETUP.md`, `ARCH_SETUP.md`, `USB_BOOT.md`.

## 🧪 Тестирование (QEMU)

```bash
python3 tools/qemu_mode_test.py dsm|fastbootd|recovery|install|os|crash
python3 tools/qemu_scenario_reboot.py   # пароль переживает 2 перезагрузки
python3 tools/qemu_install_test.py      # установка на второй диск + загрузка
```

Бут-логи прошлых прогонов: `docs/QEMU_BOOT_LOG_*.txt`.

---

## 📂 Структура проекта

```
DeiX/
├── boot/                # NASM: boot_sector, stage2, long_mode_init, ramboot, линкеры
├── src/                 # ядро Rust (~25k строк, 92 файла)
│   ├── crypto/          # AES, SHA-256/512, PBKDF2-HMAC, XTS
│   ├── drivers/ + hal/  # драйвер-плоскость и NVIDIA (nouveau-based)
│   ├── linux/           # ELF-загрузчик + Linux syscall ABI
│   ├── mm/              # физическая память (buddy), куча 16 МиБ
│   ├── net/             # ARP, IPv4, ICMP, TCP, HTTP
│   ├── ui/              # оконная система и отрисовка
│   ├── data/            # встроенные данные (Intel microcode blob)
│   ├── sound.rs         # PC speaker: beep, ШИМ-плеер DPS, UiSound
│   ├── loginui.rs / auth.rs      # графический и текстовый вход
│   ├── bootchain.rs / partition_map.rs / erofs.rs / ext2.rs / crypto_storage.rs
│   ├── ota.rs / ota_store.rs / bcb.rs / avb.rs / recovery_ui.rs / fastbootd_ui.rs / dsm.rs
│   └── kexec.rs / sched.rs / usermode.rs / microcode.rs / ...
├── tools/               # make_deix_fs.py, wav2dps.py, make_logo.py, mexcc/mexmake,
│                        # deix_ota.py, qemu_*_test.py и др.
├── assets/              # logo.png + UI-звуки (*.wav — исходники для DPS)
├── docs/                # отчёты, бут-логи, MEX_FORMAT, скриншоты
├── security/            # master_spec — спецификация подсистемы безопасности
├── build.sh  run.sh  run_full.sh  run_iso.sh
└── BUILD_AND_TEST.md    # сборка, тесты, карта памяти, звуки
```

---

## 🗺️ Roadmap

- [x] Ring 3 + syscall/sysret, вытесняющая многозадачность, kexec
- [x] Настоящий EROFS во всех системных разделах
- [x] LUKS-слоты + PBKDF2-HMAC-SHA512
- [x] Графический вход, экран блокировки, профили
- [x] ext2 с подкаталогами
- [x] TCP + HTTP/1.0 (OTA по воздуху)
- [x] HAL + открытый драйвер NVIDIA
- [x] USB-загрузка (ramboot / Ventoy)
- [x] UI-звуки (PC speaker, ШИМ)
- [ ] ACPI + мониторинг батареи → события для `lowbat`/`fullbat`
- [ ] USB-стек устройств (HID) → событие `usbdisc`
- [ ] HPET-таймер → точная скорость ШИМ-воспроизведения на железе
- [ ] NVIDIA Falcon firmware / NV50+
- [ ] ext4 / SMP (дальнее)

---

Developed by **Atimenka** & AI. Released under the **GPL-3.0** License.
