#!/usr/bin/env python3
"""Генерирует docs/ERRORS.md из реестра кодов src/diag/code.rs.

Таблицы собираются из `TABLE`, а колонка «Где выдаётся» — поиском
`Subsystem::<X>, <N>` по остальным исходникам: если код нигде не создаётся,
документ честно помечает его как зарезервированный.

    python3 tools/gen_error_docs.py            # перезаписать docs/ERRORS.md
    python3 tools/gen_error_docs.py --check    # код 1, если документ устарел
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CODE_RS = ROOT / "src" / "diag" / "code.rs"
OUT = ROOT / "docs" / "ERRORS.md"

STR = r'"((?:[^"\\]|\\.)*)"'
ENTRY = re.compile(
    r"CodeInfo\s*\{\s*code:\s*ErrorCode::new\(Subsystem::(\w+),\s*(\d+)\),\s*"
    r"summary:\s*" + STR + r",\s*detail:\s*" + STR + r"\s*,\s*action:\s*" + STR,
    re.S,
)


def unescape(s: str) -> str:
    return s.replace('\\"', '"').replace("\\\\", "\\")


def esc(s: str) -> str:
    return unescape(s).replace("|", "\\|").replace("\n", " ")


def subsystems(text: str):
    order = re.search(r"pub enum Subsystem \{(.*?)\n\}", text, re.S).group(1)
    names = re.findall(r"(\w+)\s*=\s*(\d+),", order)
    tag_block = re.search(r"pub const fn tag.*?\n    \}", text, re.S).group(0)
    name_block = re.search(r"pub const fn name.*?\n    \}", text, re.S).group(0)
    tags = dict(re.findall(r'Subsystem::(\w+) => "([^"]+)"', tag_block))
    ru = dict(re.findall(r'Subsystem::(\w+) => "([^"]+)"', name_block))
    return [(n, int(i), tags[n], ru[n]) for n, i in names]


def usages():
    found = {}
    pat = re.compile(r"Subsystem::(\w+),\s*(\d+)\)")
    for path in sorted((ROOT / "src").rglob("*.rs")):
        if path == CODE_RS:
            continue
        rel = path.relative_to(ROOT).as_posix()
        for sub, num in pat.findall(path.read_text(encoding="utf-8")):
            found.setdefault((sub, int(num)), set()).add(rel)
    return found


HEAD = """# Ошибки DeiX OS и их расшифровка

Документ создан `tools/gen_error_docs.py` из реестра `src/diag/code.rs`: правьте реестр
и перегенерируйте (`python3 tools/gen_error_docs.py`), а не этот файл.

## Как читать код

Код имеет вид `DX-<ПОДСИСТЕМА>-<НОМЕР>`, например `DX-NET-0007`: подсистема (тег из
таблицы ниже) и четырёхзначный номер внутри неё. `DX-LOG-0000` — запись журнала без
кода (обычное событие).

Карточка любого кода в ОС: `error show DX-NET-0007` — сбой, причина, действие и события
этого кода за текущую сессию. Окно **Errors** (Центр ошибок) показывает то же в GUI.

## Команды диагностики

| Команда | Что делает |
|---|---|
| `error list` | ошибки текущей сессии |
| `error show <код>` | карточка кода из реестра |
| `log [errors\\|warnings\\|critical\\|kernel]` | последние события с фильтром |
| `log boot\\|dinit\\|kmod` | журнал фазы загрузки, служб, модулей с диска |
| `log list` | файлы журналов в `/userdata` и их размер |
| `log flush` / `log clear` | записать буфер на диск / очистить буфер в памяти |
| `panic last` / `panic clear` | отчёт об отказе ядра (сохраняется между загрузками) |
| `diagnostics [summary\\|mode on\\|mode off]` | сводка; TRACE/DEBUG только в диагностическом режиме |

## Уровни критичности

| Уровень | Метка | Значение |
|---|---|---|
| Trace | TRACE | пошаговая трассировка, только в диагностическом режиме |
| Debug | DEBUG | отладочная информация, только в диагностическом режиме |
| Info | INFO | обычное событие жизненного цикла |
| Notice | NOTICE | требует внимания, но не ошибка |
| Warning | WARN | потенциальная проблема, работа продолжается |
| Error | ERROR | ошибка операции, подсистема продолжает работу |
| Critical | CRIT | серьёзный сбой: подсистема деградировала или отключена |
| Panic | PANIC | отказ ядра: система останавливается и сохраняет отчёт |
| Fatal | FATAL | неисправимый отказ: аппаратный сброс |

## Реакции системы (действие в записи)

| Метка | Что происходит |
|---|---|
| `none` | действий не требуется |
| `continue` | работа продолжается |
| `retry` | операция будет повторена |
| `terminate-process` | процесс будет остановлен |
| `quarantine-module` | модуль помещён в карантин |
| `degrade-subsystem` | подсистема отключена |
| `save-report-halt` | отчёт сохранён, система остановится |
| `hardware-reset` | аппаратный сброс |

## Сообщения загрузчика (до ядра, кодов DX нет)

| Сообщение | Источник | Причина |
|---|---|---|
| `Disk error` | MBR (`boot/boot_sector.asm`) | BIOS не прочитал карту загрузки, stage2 или ramboot (int 13h) |
| `Bad boot map` | MBR | сектор LBA 2047 без подписи `DEIXKIMG`, версия не 2 или участки файлов короче заявленного размера; карту пишут сборка, установщик и `sysfs convert` (`docs/BOOTMAP.md`) |
| `RAMBOOT: disk read error` | `boot/ramboot.asm` | int 13h не прочитал ядро; на другой диск ramboot не переключается |
| `RAMBOOT: bad boot map` | `boot/ramboot.asm` | размер ядра в карте нулевой или больше 2 МиБ либо участки ядра кончились раньше времени |
| буква `S` в COM1 | MBR | карта и stage2 прочитаны, дальше читается ramboot (не ошибка) |

## Сообщения файловой системы в интерфейсе (Files, CLI)

| Текст | Значение |
|---|---|
| `не смонтировано: <путь>` | путь не начинается с `/system` или `/userdata` |
| `не найдено: <путь>` | нет такого файла или каталога |
| `только чтение: <путь>` | `/system` — EROFS либо ext2 без dev-режима |
| `файлы загрузчика и ядра защищены (карта загрузки)` | запись, замена или удаление в `/system/boot` и `/system/kernel` запрещены и в dev-режиме: загрузчик читает эти файлы по участкам диска |
| `не каталог: <путь>` | в пути компонент-файл, либо листинг файла |
| `ошибка диска` | ext2 не смог прочитать/записать том (`DX-EXT-0006`, обычно вслед за `DX-DISK-*`) |
| `раздел не отформатирован` | на томе нет ext2 (`DX-EXT-0001`) |
| `Delete failed: folder is not empty` | Files удаляет только пустые каталоги |

"""


def main() -> int:
    text = CODE_RS.read_text(encoding="utf-8")
    subs = subsystems(text)
    table = text[text.index("pub static TABLE"):]
    entries = ENTRY.findall(table)
    used = usages()
    by_sub = {}
    for sub, num, summary, detail, action in entries:
        by_sub.setdefault(sub, []).append((int(num), summary, detail, action))

    out = [HEAD]
    out.append("## Подсистемы\n")
    out.append("| Тег | Подсистема | Кодов в реестре |")
    out.append("|---|---|---|")
    for name, _, tag, ru in subs:
        out.append(f"| `{tag}` | {ru} | {len(by_sub.get(name, []))} |")
    out.append("")
    total = len(entries)
    raised = sum(1 for s, n, *_ in entries if (s, int(n)) in used)
    out.append(
        f"Всего кодов: **{total}**; создаются в коде сейчас: **{raised}**, "
        f"остальные зарезервированы.\n"
    )

    for name, _, tag, ru in subs:
        rows = sorted(by_sub.get(name, []))
        if not rows:
            continue
        out.append(f"## {ru} (`{tag}`)\n")
        out.append("| Код | Сбой | Причина | Что делать | Где выдаётся |")
        out.append("|---|---|---|---|---|")
        for num, summary, detail, action in rows:
            code = f"DX-{tag}-{num:04d}"
            where = used.get((name, num))
            place = ", ".join(f"`{p}`" for p in sorted(where)) if where else "зарезервирован"
            out.append(f"| `{code}` | {esc(summary)} | {esc(detail)} | {esc(action)} | {place} |")
        out.append("")

    doc = "\n".join(out).rstrip() + "\n"
    if "--check" in sys.argv:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != doc:
            print("docs/ERRORS.md устарел: python3 tools/gen_error_docs.py", file=sys.stderr)
            return 1
        return 0
    OUT.write_text(doc, encoding="utf-8")
    print(f"{OUT.relative_to(ROOT)}: {total} кодов, выдаётся из кода: {raised}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
