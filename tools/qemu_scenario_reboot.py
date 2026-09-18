#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Сценарий: фабрика -> первичная настройка -> вход -> reboot -> вход -> reboot -> вход.

Проверка бага «после двух перезагрузок ОС забывает пароль».

Перезагрузка моделируется перезапуском QEMU-процесса с тем же образом
(raw — записи гостя попадают в файл): полный сброс RAM (NV TPM теряется,
как при ребуте), диск персистентен. Это честнее и надёжнее, чем
system_reset при -serial stdio (вывод после reset пропадал).

Примечание: print! (приглашения "Username:", "Password:", "deix> ") идёт
ТОЛЬКО в VGA; в serial видны только println!/serial_println!. Ввод
отправляется «вслепую» по маркерам println!.
"""
import os, pty, subprocess, select, time, sys, signal

IMG = 'build/deix_disk.img'
LOG_PATH = '/tmp/qemu_scenario.log'
ERR_PATH = '/tmp/qemu_scenario.err'

proc = None
master = None
buf = b''
logf = open(LOG_PATH, 'w', encoding='utf-8', errors='replace')

def start_qemu():
    global proc, master
    master, slave = pty.openpty()
    proc = subprocess.Popen(
        [
            'qemu-system-x86_64', '-m', '512',
            '-drive', f'file={IMG},format=raw,if=ide',
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
            logf.write(chunk.decode('utf-8', 'replace'))
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
    lines = [l for l in txt.splitlines() if '[dbg]' in l or '[bcb]' in l]
    print(f'  -- {title} --')
    for l in lines[-12:]:
        print('   ' + l.strip(), flush=True)

try:
    # ==== Фаза 1: первичная настройка ====
    print('=== Фаза 1: первичная настройка (создание пользователя alice) ===', flush=True)
    start_qemu()
    wait_for([b'DeiX v0. - mini kernel booted successfully!'], desc='первый бут')
    wait_for([b'No user accounts exist yet'], desc='заводской образ: аккаунтов нет', timeout=60)
    snapshot('фаза 1 до создания')
    send('alice\n')
    send('pass123\n')
    r1 = wait_for([b"Account 'alice' created. Logging in..."], desc='аккаунт создан', timeout=60)
    if not r1:
        print('>>> ФАЗА 1 FAIL: аккаунт не создан', flush=True); sys.exit(2)
    wait_for([b'Disk encryption ENABLED'], desc='шифрование включено', timeout=60)
    wait_for([b'Welcome to DeiX CLI'], desc='CLI после первого входа', timeout=60)
    snapshot('фаза 1 после создания (TPM записан)')
    stop_qemu()
    print('  [OK] QEMU остановлен (моделируем перезагрузку)', flush=True)

    # ==== Фаза 2: вход после reboot #1 ====
    print('=== Фаза 2: вход после reboot #1 ===', flush=True)
    buf = b''
    start_qemu()
    wait_for([b'DeiX v0. - mini kernel booted successfully!'], desc='второй бут', timeout=120)
    wait_for([b'Disk is encrypted'], desc='диск зашифрован (DEIXCRYP найден)', timeout=60)
    wait_for([b'Login'], desc='экран входа', timeout=30)
    time.sleep(1.0)
    if b'No user accounts exist yet' in buf:
        snapshot('фаза 2: АККАУНТОВ НЕТ')
        print('>>> ФАЗА 2: NO_ACCOUNTS (баг после 1-го ребута)', flush=True); sys.exit(3)
    snapshot('фаза 2 перед вводом (источник USERS.DB)')
    send('alice\n')
    send('pass123\n')
    r2 = wait_for([b'Login successful', b'Invalid username or password'], desc='результат входа #1', timeout=60)
    if not r2 or b'Invalid' in r2:
        snapshot('фаза 2: вход НЕ удался')
        print('>>> ФАЗА 2: LOGIN_FAIL (пароль «забыт» после 1-го ребута)', flush=True); sys.exit(4)
    print('  Фаза 2: вход после reboot #1 — OK', flush=True)
    wait_for([b'Welcome to DeiX CLI'], desc='CLI #2', timeout=60)
    stop_qemu()
    print('  [OK] QEMU остановлен (моделируем перезагрузку)', flush=True)

    # ==== Фаза 3: вход после reboot #2 ====
    print('=== Фаза 3: вход после reboot #2 ===', flush=True)
    buf = b''
    start_qemu()
    wait_for([b'DeiX v0. - mini kernel booted successfully!'], desc='третий бут', timeout=120)
    wait_for([b'Disk is encrypted'], desc='диск зашифрован (DEIXCRYP найден)', timeout=60)
    wait_for([b'Login'], desc='экран входа', timeout=30)
    time.sleep(1.0)
    if b'No user accounts exist yet' in buf:
        snapshot('фаза 3: АККАУНТОВ НЕТ')
        print('>>> ФАЗА 3: NO_ACCOUNTS — БАГ ПОСЛЕ 2-ГО РЕБУТА (пароль «забыт»)', flush=True); sys.exit(5)
    snapshot('фаза 3 перед вводом (источник USERS.DB)')
    send('alice\n')
    send('pass123\n')
    r3 = wait_for([b'Login successful', b'Invalid username or password'], desc='результат входа #2', timeout=60)
    if not r3 or b'Invalid' in r3:
        snapshot('фаза 3: вход НЕ удался')
        print('>>> ФАЗА 3: LOGIN_FAIL (пароль «забыт» после 2-го ребута)', flush=True); sys.exit(6)
    print('  Фаза 3: вход после reboot #2 — OK', flush=True)

    print('>>> ИТОГ: PASS — пароль пережил две перезагрузки', flush=True)
    sys.exit(0)
finally:
    stop_qemu()
    logf.close()
