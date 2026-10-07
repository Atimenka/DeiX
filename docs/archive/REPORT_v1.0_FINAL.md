# DeiX OS — Итоговый отчёт (v0.5 + расширения)

Дата: 2026-08-11
Статус: **стабильно, все сценарии верифицированы в QEMU**

---

## 1. Архитектура загрузки (двухчастная — лимит размера ядра снят)

```
Сектор 0          boot_sector.bin  (512 Б, MBR — грузит BIOS)
Секторы 1..K      stage2.bin       (374 Б: 32-бит вход → long mode → копирование kernel)
Секторы K+1..M    kernel.bin       (463 КБ, база 0x100000, LBA 2)
```

- `boot_sector` читает stage2 (int13 AH=42) и kernel.bin (int13, KERNEL_SECTORS=1100 —
  запас) в real-память (0x11000), затем stage2 копирует kernel в 0x100000.
- Ядро может расти до ~4090 секторов (до ext2-тома на LBA 4096) — это ~2 МиБ.
- Paging: identity-map 4 ГиБ (1 ГиБ RAM + GPU MMIO 0xFC000000 для framebuffer'а
  оболочек DSM/Fastbootd/Recovery).
- `linker_kernel.ld`: .got/.got.plt в .data, `__image_end` выровнен до 512 байт —
  install копирует kernel побайтово точно.
- `install` пишет kernel.bin, прочитанный с загрузочного диска (чистый .data),
  сбрасывает BCB + маркер шифрования целевого диска.

## 2. Загрузочные режимы (BCB: dsm > fastbootd > recovery > normal)

| Режим | Команда | Что умеет |
|---|---|---|
| **DSM** (аналог EDL) | `reboot dsm` | INFO/READ/FLASH/ERASE/VERIFY (SHA-256)/REBOOT; работает при сломанной ОС; serial: `dsm <cmd>` |
| **Fastbootd** | `reboot fastbootd` | Flash (EROFS-валидация+readback)/Erase/Format/Getvar/OEM unlock-lock/Reboot; serial: `fb <cmd>` |
| **Recovery** | `reboot recovery` | Install OTA (разблокировка тома паролем), Nandroid Backup/Restore, Factory Reset, Wipe, Mount, Sideload, Crash log; serial: `rc <cmd>` |
| **OS** | `reboot` | обычная загрузка |

Все оболочки: английский UI по умолчанию, GUI-диалоги подтверждений
(Enter/Esc, таймаут 15-30 с — без зависаний).

## 3. Отладчик ошибок (как Android)

- `bugreport` — полный диагностический отчёт (система, разделы, TPM, BCB, dmesg,
  crash) → экран + BUGREPORT.TXT на /system.
- `dmesg` — кольцевой журнал ядра (512 строк).
- `crashlog` / `crashlog clear` — tombstone паники; паник-хендлер пишет дамп
  сырыми секторами (LBA 2048, вне ext2/шифрования) — том не повреждается.
- `crash panic` — принудительная паника (тест).

## 4. Хранилище учётных данных

- USERS.DB: TPM NV (не персистентен в QEMU) + скрытый раздел /TPM (LBA 12288,
  маркер DEIXTPM1) + ext2-том.
- Исправлен баг: NV-слот создавался на 8192, seal() даёт 8196 → запись /TPM
  не выполнялась → «пароль забыт после перезагрузки». Фикс: NV_USERS_DB_MAX+4,
  /TPM пишется всегда. Пароль переживает 2+ перезагрузки (подтверждено).

## 5. install (установщик)

- `install` — интерактивный мастер (диск Master/Slave, username, пароль+подтверждение, yes).
- `install --test <user> <pass>` — неинтерактивный (первый доступный диск).
- Пишет: MBR (10 разделов), stage2+kernel (LBA 1..), ext2 /system и /userdata,
  EROFS логические, /TPM (заводской маркер DEIXTPM), USERS.DB на ЦЕЛЕВОЙ диск,
  шифрование (XTS-AES-256, ключ = пароль) для Master.
- Исправлены баги: DRQ-состояние slave (первая запись терялась), «живой» .data
  (Invalid opcode), __image_end < len(kernel.bin) (хвост терялся), старый маркер
  DEIXCRYP (диск «зашифрован» не тем ключом), USERS.DB не на целевой диск,
  /TPM маркер «записанной базы».

## 6. Верификация (QEMU, tools/qemu_mode_test.py, tools/qemu_install_test.py)

```
dsm:       >>> DSM: PASS
fastbootd: >>> FASTBOOTD: PASS
recovery:  >>> RECOVERY: PASS
install:   >>> INSTALL: PASS
os:        >>> OS (bugreport): PASS
crash:     >>> CRASH (debugger): PASS
>>> ИТОГ: PASS — пароль пережил две перезагрузки

tools/qemu_install_test.py (install на slave → проверка диска → загрузка → вход):
  [OK] все 13 проверок → ИТОГ: PASS
install на Master (self-install): установка → reboot → зашифрован → вход → PASS
install --test: установка → зашифрован ключом tpass → вход tuser → PASS
ISO + диск: boot OK
```

## 7. Сборка

```
bash build.sh   # stage2 (1 сект) + kernel.bin (906 сект) + deix_disk.img + deix.iso
python3 tools/make_deix_fs.py build/deix_disk.img   # заводской образ (полная MBR)
```
- Тулчейн: rustup nightly + rust-src + x86_64-unknown-none, nasm, genisoimage.
- `cargo check` — 0 ошибок; код без отладочных маркеров.

## 8. Образы

- `/home/user/deix_disk.img` — заводской образ (8 МиБ, полная MBR-разметка).
- `/home/user/deix.iso` — загрузочный ISO (El Torito no-emulation).
- Запуск: `qemu-system-x86_64 -m 512 -drive file=deix_disk.img,format=raw,if=ide`

## 8.5 Полная цепочка загрузки через ВСЕ разделы

```
MBR(boot_sector) + stage2  — первый загрузчик (real mode -> long mode)
   -> /dsm         dsm.bin          (emergency Download System Manager)
   -> /init_boot   bootloader.bin   (загрузчик 2-го уровня)
   -> /vendor_boot vendor.bin       (прошивка вендора/HAL)
   -> /boot        fastbootd.bin, recovery.bin  (образы режимов)
   -> /kernel      kernel.tar.gz    (НАСТОЯЩИЙ gzip deflate!)
                     ├─ kernel.bin (489 КБ, совпадает с загруженным)
                     ├─ libdeix_core.so / libdeix_net.so / libdeix_gfx.so
```

- EROFS-разделы содержат файловую таблицу (после суперблока): u32 count +
  записи (name[32] + offset + size) + данные. Ядро читает их (bootchain.rs).
- Распаковка kernel.tar.gz: собственный inflate (RFC 1951, src/inflate.rs) —
  stored/fixed/dynamic Huffman. Проверено в QEMU: kernel.bin + 3 библиотеки.
- fastbootd/recovery при старте загружают свои образы из /boot.
- install копирует образы разделов с загрузочного диска на целевой — установленная
  система сохраняет полную цепочку (подтверждено проверкой /tmp/install_target.img).

## 9. GUI оболочек (скриншоты из QEMU, 800x600)

Все три оболочки — на английском, с градиентными заголовками, скруглёнными
панелями и GUI-диалогами подтверждений (Enter=Yes, Esc=No, таймаут 15-30 с —
без зависаний).

| Режим | Скриншот |
|---|---|
| DSM | `docs/screens/dsm_gui.png` |
| Fastbootd | `docs/screens/fastbootd_gui.png` |
| Recovery | `docs/screens/recovery_gui.png` |
| Fastbootd — диалог подтверждения Erase | `docs/screens/fastbootd_confirm_dialog.png` |

Диалог подтверждения подтверждён работой: выбор Erase → Enter показывает окно
«Erase /userdata? (data will be destroyed)», serial-зеркало `[dialog] ...`,
экран не зависает (ждёт Enter/Esc, затем возврат в меню).

## 10. A/B разметка, OTA по воздуху, защита цепочки

- **A/B**: /kernel_a/b, /boot_a/b; активный слот в BCB (offset 12..16);
  `bcb slot <a|b>`, цепочка грузит активный слот.
- **OTA wireless**: check/download/apply/rollback; apply прошивает kernel.tar.gz
  в НЕактивный слот и переключает; откат — rollback. Recovery: «OTA wireless
  update (A/B)» (download -> прошивка -> подтверждение переключения).
- **Защита**: повреждение звена цепочки -> загрузка остановлена (RED-подобно);
  подтверждено: `dsm erase /kernel_a` -> «ЗАГРУЗКА ОСТАНОВЛЕНА».
- **CLI/install** — английский по умолчанию.
- Верификация: все 6 режимов PASS, install PASS (слоты a/b на target),
  OTA A/B PASS (слот a -> apply -> слот b -> загрузка со слота b),
  пароль через 2 перезагрузки PASS.

## 11. Утилита рассылки OTA (Linux-бинарник deix-ota)

`/home/user/deix-ota` — собранный onefile-бинарник (PyInstaller):
- `build` — сборка подписанного OTA-пакета из kernel.bin (kernel.tar.gz + DEIXOTA1);
- `push` — прошивка ядра в A/B-слоты всех образов (+ переключение слота в BCB);
- `serve` — HTTP-сервер раздачи пакетов по сети;
- `info` — диагностика образа (BCB + содержимое разделов).

Проверено: push на 2 образа -> QEMU загрузился со слота b и распаковал
прошитый kernel.tar.gz; serve отдаёт TEST.OTA/kernel.tar.gz идентично (curl).
