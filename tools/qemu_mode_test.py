#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Тестовый драйвер QEMU для режимов DeiX:
  dsm, fastbootd, recovery, os (bugreport/dmesg/crashlog), crash (panic-тест).

Управление: serial (COM1), перезагрузка = рестарт QEMU-процесса с тем же
образом (полный сброс RAM, диск персистентен). BCB-флажок ставится в образ
до старта (mode dsm=3, fastbootd=2, recovery=1, os/crash=0).
"""
import os, pty, subprocess, select, time, sys, struct

IMG = 'build/deix_disk.img'
LOG_PATH = '/tmp/qemu_mode_test.log'
ERR_PATH = '/tmp/qemu_mode_test.err'

BCB_LBA = 3000
MODES = {'dsm': 3, 'fastbootd': 2, 'recovery': 1, 'os': 0, 'crash': 0}

def set_bcb(mode_code: int):
    d = bytearray(open(IMG, 'rb').read())
    sec = bytearray([0xFF] * 512)
    sec[:8] = b'DEIXBCB1'
    sec[8:12] = struct.pack('<I', mode_code)
    d[BCB_LBA * 512:BCB_LBA * 512 + 512] = sec
    open(IMG, 'wb').write(d)

proc = None
master = None
buf = b''
logf = open(LOG_PATH, 'w', encoding='utf-8', errors='replace')

def start_qemu():
    global proc, master
    master, slave = pty.openpty()
    proc = subprocess.Popen(
        ['qemu-system-x86_64', '-m', '512',
         '-drive', f'file={IMG},format=raw,if=ide',
         '-display', 'none', '-serial', 'stdio', '-monitor', 'none'],
        stdin=slave, stdout=slave, stderr=open(ERR_PATH, 'w'))
    os.close(slave)
    time.sleep(0.3)

def stop_qemu():
    global proc, master
    if proc is not None:
        try: proc.terminate()
        except Exception: pass
        try: proc.wait(timeout=5)
        except Exception:
            try: proc.kill()
            except Exception: pass
        proc = None
    if master is not None:
        try: os.close(master)
        except Exception: pass
        master = None

def pump(t=0.2):
    global buf
    if master is None: return
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([master], [], [], 0.05)
        if r:
            try: chunk = os.read(master, 65536)
            except OSError: return
            if not chunk: return
            buf += chunk
            logf.write(chunk.decode('utf-8', 'replace'))
            logf.flush()

FAILS = {'n': 0}
def expect(patterns, timeout=90, desc='', count_fail=True):
    end = time.time() + timeout
    while time.time() < end:
        pump(0.2)
        for p in patterns:
            if isinstance(p, str): p = p.encode('utf-8')
            if p in buf:
                print(f'  [OK] {desc} (t={timeout-(end-time.time()):.1f}s)', flush=True)
                return True
        time.sleep(0.05)
    print(f'  [{"warn" if not count_fail else "FAIL"}] таймаут: {desc}', flush=True)
    if count_fail:
        FAILS['n'] += 1
    return False

def send(s, delay=0.5):
    time.sleep(delay)
    if master is not None:
        os.write(master, s.encode('utf-8'))

def boot_marker():
    return b'DeiX v0. - mini kernel booted successfully!'

def ensure_cli():
    """Доводим до CLI: если аккаунтов нет — первичная настройка (alice),
    иначе — вход alice/pass123. Приглашения print! не видны в serial,
    поэтому ввод вслепую по маркерам println! (с запасом по таймингу)."""
    if expect([b'No user accounts exist yet'], desc='аккаунтов нет (первичная настройка)', timeout=45, count_fail=False):
        send('alice\n', delay=1.0)
        send('pass123\n', delay=1.2)
        expect([b'Account', b'Logging in'], desc='аккаунт создан', timeout=60)
    else:
        send('alice\n', delay=1.0)
        send('pass123\n', delay=1.2)
        expect([b'Login successful', b'Invalid username or password'], desc='вход', timeout=60)
    expect([b'Welcome to DeiX CLI'], desc='CLI', timeout=60)

def run(mode):
    print(f'===== РЕЖИМ: {mode} =====', flush=True)
    set_bcb(MODES[mode])
    buf_global_clear = globals(); buf_global_clear['buf'] = b''
    start_qemu()
    try:
        if mode == 'dsm':
            expect([b'DeiX DSM'], desc='DSM GUI', timeout=120)
            send('dsm info\n')
            expect([b'DSM protocol: DSM-1.0'], desc='dsm info', timeout=30)
            send('dsm verify /kernel_a\n')
            expect([b'sha256='], desc='dsm verify /boot', timeout=60)
            send('dsm erase /kernel_a\n')
            expect([b'dsm erase /kernel_a: OK (511'], desc='dsm erase /boot', timeout=60)
            send('dsm verify /kernel_a\n')
            expect([b'sha256='], desc='dsm verify после erase', timeout=60)
            send('dsm reboot system\n')
            expect([b'dsm reboot ->'], desc='dsm reboot', timeout=30)
            print(f'  >>> DSM: {"PASS" if FAILS["n"]==0 else f"FAIL({FAILS["n"]})"}', flush=True)

        elif mode == 'fastbootd':
            expect([b'DeiX Fastbootd'], desc='fastbootd GUI', timeout=120)
            send('fb getvar\n')
            expect([b'GETVAR:', b'unlocked: no'], desc='fb getvar (locked)', timeout=30)
            send('fb unlock\n')
            send('yes\n')
            expect([b'OEM UNLOCK'], desc='fb unlock', timeout=30)
            send('fb getvar\n')
            expect([b'unlocked: yes'], desc='fb getvar (unlocked)', timeout=30)
            send('fb flash /kernel_b\n')
            expect([b'FLASH /kernel_b: OK'], desc='fb flash /boot', timeout=90)
            send('fb erase /kernel_b\n')
            send('yes\n')
            expect([b'ERASE /kernel_b: OK'], desc='fb erase /boot', timeout=60)
            send('fb reboot recovery\n')
            expect([b'Reboot recovery (serial)'], desc='fb reboot', timeout=30)
            print(f'  >>> FASTBOOTD: {"PASS" if FAILS["n"]==0 else f"FAIL({FAILS["n"]})"}', flush=True)

        elif mode == 'recovery':
            expect([b'DeiX Recovery'], desc='recovery GUI', timeout=120)
            send('rc mount\n')
            expect([b'Mount:'], desc='rc mount', timeout=30)
            send('rc backup\n')
            expect([b'Backup: OK'], desc='rc backup', timeout=120)
            send('rc restore /kernel_a\n')
            expect([b'Restore /kernel_a: OK'], desc='rc restore /boot', timeout=90)
            send('rc crash\n')
            expect([b'Crash log:'], desc='rc crash', timeout=30)
            send('rc log\n')
            expect([b'RECOVERY.LOG'], desc='rc log', timeout=30)
            send('rc reboot system\n')
            expect([b'Reboot system (serial)'], desc='rc reboot', timeout=30)
            print(f'  >>> RECOVERY: {"PASS" if FAILS["n"]==0 else f"FAIL({FAILS["n"]})"}', flush=True)

        elif mode == 'os':
            expect([boot_marker()], desc='первый бут', timeout=120)
            ensure_cli()
            send('bugreport\n')
            expect([b'FULL DEBUG REPORT'], desc='bugreport собран', timeout=60)
            expect([b'BUGREPORT.TXT'], desc='bugreport сохранён', timeout=30)
            send('dmesg\n')
            expect([b'[dmesg]'], desc='dmesg', timeout=30)
            send('crashlog\n')
            expect([b'[crashlog]'], desc='crashlog', timeout=30)
            print(f'  >>> OS (bugreport): {"PASS" if FAILS["n"]==0 else f"FAIL({FAILS["n"]})"}', flush=True)

        elif mode == 'crash':
            expect([boot_marker()], desc='бут', timeout=120)
            ensure_cli()
            send('crash panic\n')
            expect([b'[PANIC]'], desc='паника + crash-лог', timeout=30)
            stop_qemu()
            print('  -- перезагрузка после паники --', flush=True)
            globals()['buf'] = b''
            start_qemu()
            expect([boot_marker()], desc='бут после паники', timeout=120)
            ensure_cli()
            send('crashlog\n')
            expect([b'[crashlog]', b'CRASHLOG.TXT'], desc='crash-лог на диске', timeout=30)
            send('crashlog clear\n')
            expect([b'crashlog]'], desc='crashlog clear', timeout=30)
            print(f'  >>> CRASH (debugger): {"PASS" if FAILS["n"]==0 else f"FAIL({FAILS["n"]})"}', flush=True)
    finally:
        stop_qemu()
        logf.flush()

def run_install():
    """Двухфазный тест recovery Install:
    фаза A: ОС — первичная настройка, `ota file` (TEST.OTA на /system);
    фаза B: reboot recovery, `rc install` -> выбор 1 -> применение OTA."""
    print('===== РЕЖИМ: install (recovery Install OTA) =====', flush=True)
    set_bcb(0)  # normal
    globals()['buf'] = b''
    start_qemu()
    try:
        expect([boot_marker()], desc='бут ОС', timeout=120)
        ensure_cli()
        send('ota file\n')
        expect([b'TEST.OTA'], desc='ota file создал TEST.OTA', timeout=30)
        stop_qemu()
        print('  -- reboot в recovery --', flush=True)
        set_bcb(1)  # recovery
        globals()['buf'] = b''
        start_qemu()
        expect([b'DeiX Recovery'], desc='recovery GUI', timeout=120)
        send('rc install\n')
        expect(['Password to decrypt /system'], desc='запрос пароля разблокировки', timeout=30)
        send('pass123\n')
        expect(['Packages on /system'], desc='список пакетов', timeout=30)
        # ищем номер TEST.OTA в списке и выбираем его
        import re
        m = re.search(rb'(\d+)\. TEST\.OTA', buf)
        if m:
            send(m.group(1).decode() + '\n')
            print(f'  [OK] выбран TEST.OTA (номер {m.group(1).decode()})', flush=True)
        else:
            print('  [FAIL] TEST.OTA не найден в списке', flush=True)
            FAILS['n'] += 1
        expect([b'Install TEST.OTA'], desc='Install TEST.OTA -> OK', timeout=60)
        send('rc reboot system\n')
        expect([b'Reboot system (serial)'], desc='rc reboot', timeout=30)
        print(f'  >>> INSTALL: {"PASS" if FAILS["n"]==0 else f"FAIL({FAILS["n"]})"}', flush=True)
    finally:
        stop_qemu()
        logf.flush()

if __name__ == '__main__':
    mode = sys.argv[1] if len(sys.argv) > 1 else 'os'
    if mode == 'install':
        run_install()
    elif mode in MODES:
        run(mode)
    else:
        print('режим должен быть dsm|fastbootd|recovery|os|crash|install'); sys.exit(2)
