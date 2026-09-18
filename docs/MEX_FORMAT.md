# Формат исполняемых файлов DeiX — `.mex` (DeiX EXecutable)

Версия спецификации: 1.1 (соответствует DeiX v0.2-dev)

## Что нового в v1.1

- Добавлены 4 сетевые функции в MexApi:
  - `ping(ip_ptr, ip_len, timeout_ms) -> i64`
  - `get_mac(mac_out) -> i64`
  - `get_ip(ip_out) -> i64`
  - `arp_resolve(ip_ptr, ip_len, mac_out) -> i64`
- Обратная совместимость с v1.0: старые программы продолжают работать
- Версионирование в заголовке: `version_minor = 1` (было 0)

## Зачем свой формат, а не ELF/PE

Мы сознательно не используем ELF или PE:
- В DeiX **нет пользовательского режима** — всё ядро и все программы
  выполняются в кольце защиты 0 (ring 0), в одном общем адресном
  пространстве.
- `.mex` — это честное отражение реальной архитектуры DeiX: плоский
  бинарный код без релокаций, загружаемый по фиксированному адресу,
  вызываемый как обычная функция с указателем на таблицу системных
  функций ядра.

## Модель исполнения

- Программа — это машинный код x86-64, скомпилированный/собранный без
  позиционно-независимого кода (не PIC), рассчитанный на загрузку по
  фиксированному адресу `MEX_LOAD_ADDR = 0x600000`.
- Точка входа — обычная функция с сигнатурой (System V AMD64 ABI):
  ```c
  int64_t mex_main(const MexApi* api);
  ```
- Программа использует стек ядра (ring 0, нет изоляции).

## Формат файла (бинарный, little-endian)

```
Смещение   Размер   Поле              Описание
0x00       4        magic             Байты 'M','E','X','1' (0x3158454D LE)
0x04       2        version_major     Сейчас 1
0x06       2        version_minor     1 (v1.1), 0 (v1.0)
0x08       4        header_size       Размер заголовка в байтах (32)
0x0C       4        entry_offset      Смещение точки входа от начала тела
0x10       4        body_size         Размер тела программы в байтах
0x14       4        bss_size          Доп. память, обнуляемая после тела
0x18       8        reserved          Зарезервировано (0)
0x20       ...      body              Машинный код + данные (body_size байт)
```

## Системный API (`MexApi`) — v1.1

```rust
#[repr(C)]
pub struct MexApi {
    // --- v1.0 (offset 0x00–0x28) ---
    pub print:        fn(ptr: *const u8, len: usize),
    pub read_char:    fn() -> u8,
    pub try_read_char: fn() -> i32,
    pub uptime_ms:    fn() -> u64,
    pub read_file:    fn(name_ptr, name_len, out_ptr, out_cap) -> i64,
    pub write_file:   fn(name_ptr, name_len, data_ptr, data_len) -> i64,

    // --- v1.1 (offset 0x30–0x48) ---
    pub ping:         fn(ip_ptr: *const u8, ip_len: usize, timeout_ms: u64) -> i64,
    pub get_mac:      fn(mac_out: *mut u8) -> i64,
    pub get_ip:       fn(ip_out: *mut u8) -> i64,
    pub arp_resolve:  fn(ip_ptr: *const u8, ip_len: usize, mac_out: *mut u8) -> i64,
}
```

### Новые функции v1.1

| Функция | Описание |
|---|---|
| `ping(ip, 4, timeout_ms)` | Отправляет ICMP echo. Возвращает RTT в мс или -1 (ошибка/таймаут). |
| `get_mac(buf)` | Записывает 6 байт MAC-адреса в `buf`. Возвращает 0 или -1. |
| `get_ip(buf)` | Записывает 4 байта IPv4 в `buf`. Возвращает 0 или -1. |
| `arp_resolve(ip, 4, mac)` | Ищет MAC по IP в ARP-кэше. Возвращает 0 (найден) или -1. |

## Архитектура модулей DeiX

Начиная с v0.2, DeiX отказывается от монолитного бинарника в пользу
файловой модульной архитектуры:

### Core kernel (stage2.bin)
Вкомпилировано намертво:
- Память (allocator), прерывания (IDT/PIC/PIT), VGA-текст
- Клавиатура/мышь (PS/2), ext2, ATA, серийный порт
- Загрузчик модулей (`src/module.rs`)

### Загружаемые модули (.kmod)
Файлы на ext2-диске, загружаются при старте:
- `NET.KMOD` — сетевой стек (RTL8139, IPv4, ARP, ICMP)
- `CRYPTO.KMOD` — криптография (AES, SHA, шифрование диска)
- `GFX.KMOD` — графическая подсистема (VBE, рендерер, UI)

Формат `.kmod`:
```
Смещение  Размер  Поле
0x00      4       magic "KMOD" (0x444F4D4B)
0x04      2       version_major
0x06      2       version_minor
0x08      4       header_size (44)
0x0C      4       init_offset
0x10      4       body_size
0x14      4       bss_size
0x18      4       name_len
0x1C      28      name (ASCII, добито нулями)
0x38      ...     body (плоский машинный код)
```

Точка входа модуля:
```c
int64_t kmod_init(const KernelApi* api);
```

Модуль регистрирует свои сервисы через KernelApi и возвращает 0 при успехе.

## Ограничения (честно)

- **Нет защиты памяти.** Программа выполняется в ring 0.
- **Нет позиционно-независимого кода.** Один экземпляр программы одновременно.
- Собираются через `nasm -f bin` + `python3 tools/mex_pack.py`.
