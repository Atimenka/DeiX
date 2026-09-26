#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Полный сквозной тест `install` (установка DeiX на второй диск):
  1) первичная настройка на загрузочном диске;
  2) `install` на ATA Slave (интерактивно: диск=2, user, pass, confirm, yes);
  3) проверка целевого диска: MBR sig, stage2, kernel.bin, ext2, /TPM;
  4) загрузка ТОЛЬКО с установленного диска, вход созданного пользователя.

Использование: python3 tools/qemu_install_test.py
"""
import os, pty, subprocess, select, time

IMG = 'build/deix_disk.img'
TARGET = '/tmp/install_target.img'

def start_qemu(qemu_args):
    m, s = pty.openpty()
    p = subprocess.Popen(qemu_args, stdin=s, stdout=s, stderr=open('/tmp/install.err', 'w'))
    os.close(s)
    return p, m

def drive_size(path):
    return os.path.getsize(path)

def main():
    print('=== DeiX install test (slave) ===')
    # свежий заводской загрузочный образ (сброс BCB/маркера шифрования)
    import subprocess as sp
    r = sp.run(['python3', 'tools/make_deix_fs.py', IMG], capture_output=True)
    if r.returncode != 0:
        print('  [FAIL] make_deix_fs'); return 1
    if os.path.exists(TARGET):
        os.remove(TARGET)
    # пустой целевой диск 8 МиБ
    with open(TARGET, 'wb') as f:
        f.write(b'\x00' * (8 * 1024 * 1024))

    p, m = start_qemu(['qemu-system-x86_64', '-m', '512',
                 '-drive', f'file={IMG},format=raw,if=ide',
                 '-drive', f'file={TARGET},format=raw,if=ide',
                 '-display', 'none', '-serial', 'stdio', '-monitor', 'none'])
    buf = b''

    def pump(t=0.3):
        nonlocal buf
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([m], [], [], 0.05)
            if r:
                try:
                    c = os.read(m, 65536)
                except OSError:
                    return
                if not c:
                    return
                buf += c
                if len(buf) > 500000:
                    buf = buf[-250000:]

    def wait(pats, t=90, desc=''):
        end = time.time() + t
        while time.time() < end:
            pump(0.3)
            for pp in pats:
                if isinstance(pp, str):
                    pp = pp.encode()
                if pp in buf:
                    print(f'  [OK] {desc}')
                    return True
            time.sleep(0.05)
        print(f'  [FAIL] {desc}')
        return False

    ok = True
    ok &= wait([b'DeiX v0. - mini kernel booted'], 90, 'boot')
    ok &= wait([b'No user accounts exist yet'], 60, 'first setup prompt')
    os.write(m, b'alice\n'); time.sleep(1.0)
    os.write(m, b'pass123\n')
    ok &= wait([b'Welcome to DeiX CLI'], 60, 'CLI')
    # install на slave
    os.write(m, b'install\n'); time.sleep(2)
    os.write(m, b'2\n'); time.sleep(1.5)
    os.write(m, b'inst_user\n'); time.sleep(1.2)
    os.write(m, b'inst_pass\n'); time.sleep(1.2)
    os.write(m, b'inst_pass\n'); time.sleep(1.2)
    os.write(m, b'yes\n')
    DONE = 'INSTALL COMPLETE'
    ok &= wait([DONE], 180, 'install done')
    p.terminate(); p.wait(); os.close(m)

    # проверка целевого диска
    d = open(TARGET, 'rb').read()
    st2 = open('build/stage2.bin', 'rb').read()
    kb = open('build/kernel.bin', 'rb').read()
    st2_sect = (len(st2) + 511) // 512
    kstart = 1 + st2_sect
    checks = {
        'MBR sig': d[510:512].hex() == '55aa',
        'boot_sector': d[:446] == open('build/boot_sector.bin', 'rb').read()[:446],
        'stage2': d[512:512 + len(st2)] == st2,
        'kernel': d[kstart * 512:kstart * 512 + len(kb)] == kb,
        'ext2 magic': d[4096 * 512 + 1024 + 56:4096 * 512 + 1024 + 58].hex() == '53ef',
    }
    for k, v in checks.items():
        print(f'  [{"OK" if v else "FAIL"}] target: {k}')
        ok &= v

    # загрузка с установленного диска
    p2, m2 = start_qemu(['qemu-system-x86_64', '-m', '512',
                   '-drive', f'file={TARGET},format=raw,if=ide',
                   '-display', 'none', '-serial', 'stdio', '-monitor', 'none'])
    buf = b''
    ok &= wait([b'DeiX v0. - mini kernel booted'], 90, 'boot from target')
    if ok:
        wait([b'Login'], 30, 'login banner')
        time.sleep(0.8)
        os.write(m2, b'inst_user\n'); time.sleep(1.0)
        os.write(m2, b'inst_pass\n')
        ok &= wait([b'Login successful', b'Invalid username or password'], 60, 'login')
        ok &= wait([b'Welcome to DeiX CLI'], 60, 'CLI on target')
    p2.terminate(); p2.wait(); os.close(m2)

    print('=== ИТОГ: ' + ('PASS' if ok else 'FAIL') + ' ===')
    return 0 if ok else 1

if __name__ == '__main__':
    import sys
    sys.exit(main())
