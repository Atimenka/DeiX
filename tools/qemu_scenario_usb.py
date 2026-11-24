#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Сценарий: загрузка с USB (без ATA-диска) — эмулирует Ventoy/флешку.

Проверяет фикс бага «загрузка с USB/Ventoy»: ramboot должен заполнить
RAM-диск (весь образ, 40960 секторов из LBA 0 -> 0x2000000), ядро должно
активировать RAM-диск (ATA не отвечает или содержит другой образ) и
читать все разделы из памяти — без DX-DISK-0004 / DX-EXT-0006.

QEMU: образ подключён как USB-накопитель (-device usb-storage), ATA-диска
НЕТ — это имитация флешки/Ventoy, где нет аппаратного ATA-контроллера
для загрузочного носителя.

Поведение live-сессии: записи в /userdata живут только до перезагрузки
(RAM-диск), поэтому после reboot система снова в режиме настройки —
это ожидаемо для загрузки с флешки (как live CD).

Маркеры (serial): «[ramdisk] active: ... sectors at 0x2000000»,
«ATA disk: not used (RAM-диск активен...)», «mini kernel booted
successfully!», «[loginui] графический вход, создание учётки: true/false»,
«Welcome to DeiX, <name>!», «Welcome to DeiX CLI».
"""
import os, pty, subprocess, select, time, sys, signal

IMG = 'build/deix_disk.img'
LOG_PATH = '/tmp/qemu_scenario_usb.log'
ERR_PATH = '/tmp/qemu_scenario_usb.err'

proc = None
master = None
buf = b''
logf = open(LOG_PATH, 'wb')


def start_qemu():
    global proc, master
    master, slave = pty.openpty()
    proc = subprocess.Popen(
        [
            'qemu-system-x86_64', '-m', '512',
            # USB-накопитель вместо if=ide: ATA-диска нет — эмулирует
            # загрузку с флешки/Ventoy.
            # -usb: явно включает USB-поддержку в SeaBIOS. Без него
            # SeaBIOS не сканирует usb-storage как загрузочное устройство
            # (ни MBR, ни ядро не стартуют — serial пуст).
            '-usb',
            '-drive', f'file={IMG},format=raw,if=none,id=disk0',
            '-device', 'usb-storage,drive=disk0',
            '-display', 'none', '-serial', 'stdio', '-monitor', 'none',
        ],
        stdin=slave, stdout=slave, stderr=open(ERR_PATH, 'w'),
    )
    os.close(slave)
    time.sleep(0.3)


def stop_qemu():
    global proc, master
    if proc is not None:
        try:
            proc.terminate()
        except Exception:
            pass
        try:
            proc.wait(timeout=5)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass
        proc = None
    if master is not None:
        try:
            os.close(master)
        except Exception:
            pass
        master = None


def pump(timeout=0.2):
    global buf
    if master is None:
        return
    end = time.time() + timeout
    while time.time() < end:
        r, _, _ = select.select([master], [], [], 0.05)
        if r:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                return
            if not chunk:
                return
            buf += chunk
            logf.write(chunk)
            logf.flush()


def wait_for(patterns, timeout=90, desc=''):
    end = time.time() + timeout
    while time.time() < end:
        pump(0.2)
        for p in patterns:
            if isinstance(p, str):
                p = p.encode('utf-8')
            if p in buf:
                print(f'  [OK] {desc} (t={timeout - (end - time.time()):.1f}s)', flush=True)
                return p
        time.sleep(0.05)
    print(f'  [TIMEOUT] waiting for {desc}', flush=True)
    return None


def send(s, delay=0.6):
    time.sleep(delay)
    if master is not None:
        os.write(master, s.encode('utf-8'))


def snapshot(title):
    txt = buf.decode('utf-8', 'replace')
    lines = [l for l in txt.splitlines()
             if '[dbg]' in l or '[bcb]' in l or '[ramdisk]' in l or 'RAMBOOT:' in l or 'DX-' in l]
    print(f'  -- {title} (filtered) --')
    for l in lines[-15:]:
        print('   ' + l.strip(), flush=True)
    # Полный serial-лог — видно, что печатал загрузчик/ядро до падения.
    if os.path.exists(LOG_PATH):
        full = open(LOG_PATH, encoding='utf-8', errors='replace').read()
        flines = [l for l in full.splitlines() if l.strip()]
        print(f'  -- {title} (full serial tail) --', flush=True)
        for l in flines[-60:]:
            print('   ' + l, flush=True)


try:
    # ==== Фаза 1: загрузка с USB, активация RAM-диска ====
    print('=== Фаза 1: загрузка с USB (без ATA-диска) ===', flush=True)
    start_qemu()
    r = wait_for([b'mini kernel booted successfully!'], desc='первый бут с USB', timeout=120)
    if not r:
        snapshot('фаза 1: бут не состоялся')
        print('>>> ФАЗА 1 FAIL: ядро не загрузилось с USB', flush=True)
        sys.exit(2)
    r = wait_for([b'[ramdisk] active'], desc='RAM-диск активирован', timeout=30)
    if not r:
        snapshot('фаза 1: рамдиск не активировался')
        print('>>> ФАЗА 1 FAIL: RAM-диск не активирован', flush=True)
        sys.exit(3)
    r = wait_for([b'ATA disk: not used'], desc='ATA не используется', timeout=30)
    if not r:
        snapshot('фаза 1: ATA-блок')
        print('>>> ФАЗА 1 FAIL: ATA-инициализация не пропущена', flush=True)
        sys.exit(4)
    # Ключевая проверка: ни одного DX-DISK-0004 (BSY) на пути загрузки.
    txt = buf.decode('utf-8', 'replace')
    if 'DX-DISK-0004' in txt:
        snapshot('фаза 1: DX-DISK-0004')
        print('>>> ФАЗА 1 FAIL: DX-DISK-0004 (ATA BSY) на пути загрузки с USB', flush=True)
        sys.exit(5)
    print('  [OK] без DX-DISK-0004 — разделы читаются из RAM-диска', flush=True)

    # ==== Фаза 2: настройка + вход (live-сессия) ====
    print('=== Фаза 2: настройка + вход (live-сессия с USB) ===', flush=True)
    if not wait_for(['[loginui] графический вход, создание учётки: true'],
                    desc='графический вход, режим настройки', timeout=60):
        snapshot('фаза 2: нет режима настройки')
        sys.exit(11)
    send('alice\n')
    send('pass123\n')
    send('pass123\n')
    r = wait_for(['[loginui] шифрование тома включено (XTS-AES-256)'],
                 desc='шифрование включено', timeout=60)
    if not r:
        snapshot('фаза 2 до создания')
        print('>>> ФАЗА 2 FAIL: шифрование не включено', flush=True)
        sys.exit(6)
    if not wait_for([b'Welcome to DeiX, alice!'], desc='вход после создания', timeout=60):
        snapshot('фаза 2: вход не состоялся')
        sys.exit(12)
    if not wait_for([b'Welcome to DeiX CLI'], desc='CLI после первого входа', timeout=60):
        snapshot('фаза 2: CLI не запустился')
        sys.exit(13)
    print('  [OK] настройка и вход с USB — OK', flush=True)
    stop_qemu()
    print('  [OK] QEMU остановлен (моделируем перезагрузку)', flush=True)

    # ==== Фаза 3: перезагрузка — live-сессия, аккаунты в RAM (ожидаемо) ====
    print('=== Фаза 3: перезагрузка — live-сессия (аккаунты в RAM) ===', flush=True)
    buf = b''
    start_qemu()
    r = wait_for([b'mini kernel booted successfully!'], desc='второй бут с USB', timeout=120)
    if not r:
        snapshot('фаза 3: бут не состоялся')
        print('>>> ФАЗА 3 FAIL: ядро не загрузилось после перезагрузки', flush=True)
        sys.exit(8)
    r = wait_for([b'[ramdisk] active'], desc='RAM-диск активирован (после ребута)', timeout=30)
    if not r:
        snapshot('фаза 3: рамдиск')
        print('>>> ФАЗА 3 FAIL: RAM-диск не активирован после перезагрузки', flush=True)
        sys.exit(9)
    txt = buf.decode('utf-8', 'replace')
    if 'DX-DISK-0004' in txt:
        snapshot('фаза 3: DX-DISK-0004')
        print('>>> ФАЗА 3 FAIL: DX-DISK-0004 после перезагрузки', flush=True)
        sys.exit(10)
    # Live-сессия: после перезагрузки аккаунты в RAM-диске — система снова
    # в режиме настройки. Это ожидаемо для загрузки с флешки.
    if not wait_for(['[loginui] графический вход, создание учётки: true'],
                    desc='режим настройки (live: аккаунты в RAM)', timeout=60):
        snapshot('фаза 3: нет режима настройки')
        sys.exit(14)
    print('  [OK] после перезагрузки — live-режим (настройка), без ATA-ошибок', flush=True)

    print('>>> ИТОГ: PASS — загрузка с USB/Ventoy работает из RAM-диска', flush=True)
    sys.exit(0)
finally:
    stop_qemu()
    logf.close()
