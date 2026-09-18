# 🪐 DeiX OS (v0.2-beta · Release)

A modern, highly-secure 64-bit Operating System written from scratch in **Rust** and **Assembly** for the `x86_64` architecture. Developed in a unique collaboration between a Human Architect and an AI Agent.

Операционная система нового поколения, написанная с нуля на **Rust** и **Ассемблере** под `x86_64`. Релиз **v0.2-beta** включает супервизор **Dinit (PID 1, Ring 0)**, эвристический монитор безопасности **Security Monitor**, полноценный звуковой драйвер **Intel High Definition Audio (HDA)** с кольцевым DMA-буфером, сжатый загрузочный логотип **DXLG**, полнодисковое шифрование **TPM 2.0 / LUKS**, графический вход, вытесняющую многозадачность и A/B OTA-обновления.

**Ядро:** ~0.7 МБ (Rust `#![no_std]` + NASM stage2) · **Лимит ядра:** 1 МиБ (2048 секторов) · **Образ диска:** 10 МиБ · **Код:** ~28 000 строк Rust, 102 файла · реальное железо и QEMU.

---

## ⚡ Новое в версии v0.2-beta

### 🛡️ Dinit — Супервизор PID 1 (Ring 0)
Корневой инит-процесс и супервизор (`src/dinit/`), исполняемый в привилегированном режиме ядра:
- **Управление жизненным циклом служб:** состояния `Stopped`, `Starting`, `Running`, `Restarting`, `Failed`, `Terminated`, `Crashed`, `Disabled`.
- **Политики перезапуска с backoff:** политики `Always`, `OnFailure`, `Never`, `UnlessStopped` с экспоненциальной задержкой (`restart_backoff_ms`) и защитой от циклического падения (`max_restarts`).
- **Разделение по кольцам:** раздельный запуск служб ядра (Ring 0) и системных демонов пользователя (Ring 3).
- **Исполнение сценариев `init.deix`:** парсинг декларативного конфига и переключение стадий загрузки (`InitBoot` → `VendorBoot` → `Boot`).
- **Каталог точек монтирования:** контроль и динамический опрос точек монтирования `/kernel`, `/init_boot`, `/system`, `/userdata`, `/dev`, `/proc`.
- **Изоляция пространств имён и Capabilities:** битовые привилегии (`CAP_MOUNT`, `CAP_REBOOT`, `CAP_KILL`, `CAP_AUDIT`, `CAP_SETUID`, `CAP_RAW_IO`, `CAP_NET_ADMIN`, `CAP_SYS_ADMIN`, `CAP_PTRACE`, `CAP_CHROOT`) и лимиты ресурсов `ResourceLimits`.
- **Централизованная матрица авторизации (`authorize.rs`):** изоляция профилей пользователей (`/users/<user>`), аппаратная изоляция enclava `/TPM` (запрещён абсолютно всем, включая UID 0).
- **Кольцевой журнал аудита (`audit.rs`):** буфер на 1024 события с фиксацией операций, нарушений, предупреждений и ликвидаций процессов.
- **Управление через CLI:** команды `dinit status`, `dinit services`, `dinit mounts`, `dinit users`, `dinit audit`, `dinit security`, `dinit stage`, `dinit reload`.

### 🚨 Эвристический монитор безопасности (Security Monitor)
Интегрированный в ядро движок предотвращения вторжений (`src/security_monitor.rs`) с порогом риска 0.85:
- **Ransomware-детектор:** отслеживание лавинообразной записи (`sys_write` с частотой > 150) и высокой энтропии данных (> 0.75) с нелинейным ростом риска.
- **Защита от инъекций кода:** блокировка вызовов `sys_mmap` и `sys_ptrace`, направленных на системные разделы (`/kernel`, `/system`, `/init_boot`), с присвоением максимального риска 1.0.
- **Ликвидация угроз:** мгновенная генерация `KillSignal(SIGKILL)` и принудительное уничтожение процесса через планировщик ядра (`sched::terminate`).
- **Самоконтроль при старте:** этап `boot_selfcheck()` проверяет работу детектора на легитимных и вредоносных событиях во время загрузки.

### 🔊 Intel High Definition Audio (HDA) + PC Speaker
Полноценная аудиоподсистема (`src/hda.rs` и `src/sound.rs`):
- **Драйвер контроллера Intel HDA (PCI):** инициализация колец CORB/RIRB, Immediate Command Interface (ICW), обнаружение аудио-узлов AFG, DAC и Output Pin.
- **DMA-воспроизведение 48 кГц / 16-бит стерео:** кольцевой 4-периодный буфер на 64 КиБ, плавное воспроизведение без прерываний и задержек, потокобезопасный неблокирующий вывод.
- **Линейный ресемплер:** передискретизация DPS-звуков с фиксированной точкой 16.16 в 48 000 Гц.
- **Гибридный вывод:** автоматическое переключение HDA / PC Speaker (ШИМ), команды CLI `sound hda info|play|beep` и `sound mode hda|speaker`.
- **Утилиты конвертации:** `tools/wav2dps.py`, корневые скрипты `wav2dps.py` и `dps2wav.py` для двустороннего преобразования WAV ↔ DPS1.

### 🖼️ Загрузочное лого DXLG и графический вход
- **Компактный формат DXLG:** 128x128 пикселей, 5-цветная оптимизированная палитра, RLE-сжатие в 21 раз (2.3 КиБ вместо 48 КиБ сырого RGB).
- **Чистый сборщик `make_logo.py`:** встроенный декодер PNG (zlib + struct) работает автономно без внешних библиотек (Pillow опционален).
- **Плавный индикатор прогресса:** отображение этапов инициализации ядра (партиции, память, AVB, TPM, Dinit) с масштабированным шрифтом.
- **Логотип в окне авторизации:** рендеринг эмблемы DeiX над графической панелью входа.
- **Интерактивный просмотрщик:** команда CLI `logo show` с возвратом по Esc/Enter/Space.

### 🔐 TPM 2.0 / LUKS полнодисковое шифрование
- Поддержка LUKS-подобных слотов паролей, деривация ключей `PBKDF2-HMAC-SHA512` (100 000 итераций).
- Шифрование диска по умолчанию алгоритмом XTS-AES-256.
- Запрос пароля при старте через PS/2 клавиатуру или COM1 serial.

### 💾 Расширение лимита ядра до 1 МиБ
- Лимит `KERNEL_SECTORS` расширен с 1400 до 2048 секторов (1 048 576 байт), `KERNEL_SIZE_DWORDS` = 262 144.
- Ядро чисто компилируется без ошибок и предупреждений с флагом `#![allow(dead_code)]`.

---

## ⚡ Базовые возможности ядра

- **🦀 Pure Rust `#![no_std]`**, старт в long mode: `boot_sector` (MBR) → `stage2` (32-бит → long mode) → `kernel.bin` @ 0x100000.
- **🛡 Ring 3 + syscall/sysret** — аппаратная изоляция (GDT + TSS).
- **🧵 Вытесняющая многозадачность** — планировщик с переключением по прерыванию IRQ0 PIT (`threads list/test`).
- **🔁 kexec** — запуск нового ядра из `/kernel_a|b` без перезагрузки BIOS (`kexec check/a/b`).
- **📦 A/B-слоты + OTA** — разделы `/kernel_a|b`, `/boot_a|b`, активный слот в BCB; команды `ota check/fetch/apply/rollback`.
- **🧾 Настоящий EROFS** (магия `0xE0F5E1E2`, валидируется `fsck.erofs`) на всех системных разделах.
- **🔒 Верифицированная загрузка (AVB)** — проверка цепочки разделов `/dsm` → `/init_boot` → `/vendor_boot` → `/boot` → `/kernel`.
- **🥽 Режимы BCB**: **DSM** (аварийный COM1-прошивальщик), **fastbootd** (графический прошивальщик), **recovery** (TWRP-подобное меню).
- **🐧 Совместимость с Linux** — загрузчик ELF64 + слой системных вызовов Linux ABI (`src/linux/`).
- **🎨 Графика и UI** — VBE до 1280x1024, мышь, кириллический шрифт (VGA Plane 2), локализация en/ru (`lang`).
- **🌐 Сеть** — RTL8139 + ARP + IPv4 + ICMP + TCP + HTTP (`ifconfig`, `ping`).
- **📦 MEX-приложения** — собственный формат бинарников DeiX (`run`, `pkg`, `mexcc`).

---

## 🗂️ Карта разделов (10 МиБ, 13 разделов)

| Раздел | LBA | Секторов | ФС | Назначение |
|---|---|---|---|---|
| `/system` | 4096 | 8192 | ext2 rw | рабочий том ядра (USERS.DB, AUTOSTART.CFG, профили) |
| `/TPM` | 12288 | 512 | скрытый 0xDA | аппаратно изолированный маркер TPM |
| `/userdata` | 12800 | 512 | ext2/ext4 rw | пользовательские данные и пакеты Ring 3 |
| `/kernel_a` | 13313 | 1279 | EROFS ro | слот A: `kernel.tar.gz` (gzip, inflate в ядре) |
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
ls cat write rm pkg run install bigfile
useradd passwd whoami users encrypt crypt
nvidia hal microcode logo linux profile lock
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
python3 tools/check_partition_map.py    # проверка синхронизации 13 разделов
python3 tools/make_logo.py assets/logo.png build/logo.dxlg 128
python3 tools/wav2dps.py assets/start.wav build/sounds/start.dps 8000
python3 dps2wav.py build/sounds/start.dps build/sounds/test.wav
python3 tools/qemu_mode_test.py os      # сквозной QEMU-тест
```

---

## 📂 Структура проекта

```
DeiX/
├── boot/                # Ассемблер: boot_sector, stage2, long_mode_init, ramboot, линкеры
├── src/                 # Ядро Rust (102 файла, ~28k строк кода)
│   ├── dinit/           # PID 1 Ring 0 супервизор: службы, монтирование, namespaces, аудит, авторизация
│   ├── security_monitor.rs # Эвристический монитор угроз (ransomware / code-injection)
│   ├── hda.rs           # Драйвер Intel High Definition Audio (HDA) с DMA-стримингом
│   ├── sound.rs         # Аудиоподсистема: HDA + PC Speaker (ШИМ), UiSound
│   ├── bootlogo.rs      # Рендерер DXLG-логотипа и статус загрузки
│   ├── loginui.rs       # Графический экран входа (VBE) с эмблемой DeiX
│   ├── init_parser.rs   # Движок разбора сценариев init.deix
│   ├── vault.rs         # KERNEL SECURITY VAULT (защита системных разделов)
│   ├── crypto/          # AES, SHA-256/512, PBKDF2-HMAC, XTS
│   ├── drivers/ + hal/  # Драйверный уровень HAL и NVIDIA nouveau
│   ├── linux/           # Загрузчик ELF64 и эмуляция Linux syscall ABI
│   ├── mm/              # Физическая память (Buddy Allocator), куча 16 МиБ
│   ├── net/             # RTL8139, ARP, IPv4, ICMP, TCP, HTTP
│   └── sched.rs / usermode.rs / kexec.rs / ext2.rs / erofs.rs / ...
├── tools/               # make_deix_fs.py, make_logo.py, wav2dps.py, dps2wav.py, check_partition_map.py
├── assets/              # logo.png, logo.dxlg, UI-звуки (*.wav)
├── docs/                # Техническая документация, спецификации форматов и бут-логи
└── security/            # master_spec — спецификация архитектуры безопасности
```

---

## 🗺️ Roadmap

- [x] Dinit (PID 1, Ring 0 supervisor) с супервизией служб и namespaces
- [x] Эвристический монитор угроз (Heuristic Security Monitor, SIGKILL)
- [x] Intel High Definition Audio (HDA) с кольцевым DMA и ресемплером
- [x] Компактный формат логотипа DXLG + автономный конвертер
- [x] Расширение лимита ядра до 1 МиБ (2048 секторов)
- [x] Настоящий EROFS во всех системных разделах + ext2/ext4 `/userdata`
- [x] LUKS-слоты + PBKDF2-HMAC-SHA512 + TPM 2.0 полнодисковое шифрование
- [x] Графический экран входа, профили пользователей, блокировка экрана
- [x] TCP + HTTP/1.0 (OTA-обновления по воздуху)
- [x] Открытый драйвер NVIDIA (HAL + MMIO)
- [x] USB-загрузка (ramboot / Ventoy)
- [ ] ACPI + мониторинг питания и батареи
- [ ] USB-стек устройств (HID мышь/клавиатура)
- [ ] Многоядерность (SMP / APIC)

---

Developed by **Atimenka** & AI. Released under the **GPL-3.0** License.
