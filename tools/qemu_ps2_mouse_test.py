#!/usr/bin/env python3
"""Exercise relative PS/2 mouse and visible cursor on graphical login."""
import os, pty, select, socket, subprocess, tempfile, time, shutil, hashlib
from pathlib import Path

source = os.environ.get('DEIX_USB_IMAGE', 'build/deix_disk.img')
serial = bytearray()
with tempfile.TemporaryDirectory() as tmp:
    disk = Path(tmp) / 'disk.img'
    shutil.copyfile(source, disk)
    mon = Path(tmp) / 'monitor.sock'
    master, slave = pty.openpty()
    proc = subprocess.Popen(['qemu-system-x86_64', '-machine', 'pc,vmport=off', '-m', '512', '-vga', 'cirrus',
        '-drive', f'file={disk},format=raw,if=ide', '-display', 'none',
        '-serial', 'stdio', '-monitor', f'unix:{mon},server=on,wait=off', '-no-reboot'],
        stdin=slave, stdout=slave, stderr=subprocess.PIPE)
    os.close(slave)
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        end = time.monotonic() + 15
        while True:
            try:
                sock.connect(str(mon)); break
            except (FileNotFoundError, ConnectionRefusedError):
                if time.monotonic() > end: raise TimeoutError('monitor unavailable')
                time.sleep(.1)
        sock.settimeout(5)
        def reply():
            data = b''
            while b'(qemu)' not in data:
                chunk = sock.recv(8192)
                if not chunk: raise RuntimeError('monitor closed')
                data += chunk
        reply()
        def wait_for(needle, timeout=120):
            end = time.monotonic() + timeout
            while time.monotonic() < end:
                if needle in serial: return
                if proc.poll() is not None: break
                ready, _, _ = select.select([master], [], [], .2)
                if ready:
                    try: data = os.read(master, 65536)
                    except OSError: break
                    if not data: break
                    serial.extend(data)
            raise RuntimeError('missing marker: ' + repr(needle))
        wait_for('[loginui] графический вход, создание учётки: true'.encode())
        wait_for(b'[mouse] PS/2 config true defaults true reporting true', 90)
        before = Path(tmp) / 'before.ppm'
        after = Path(tmp) / 'after.ppm'
        sock.sendall(f'screendump {before}\n'.encode()); reply()
        sock.sendall(b'mouse_move 40 30\n'); reply()
        wait_for(b'[mouse] PS/2 movement received', 30)
        time.sleep(1)
        sock.sendall(f'screendump {after}\n'.encode()); reply()
        if before.read_bytes() == after.read_bytes():
            raise RuntimeError('mouse report did not change the visible screen')
        if b'KERNEL PANIC' in serial: raise RuntimeError('kernel panic')
        print('PASS: PS/2 mouse cursor moves on graphical login', flush=True)
    except Exception as exc:
        print('FAIL:', exc, '\nSerial tail:', serial[-8000:].decode('utf-8','replace'), flush=True)
        raise
    finally:
        Path('/tmp/qemu_ps2_mouse.log').write_bytes(serial)
        sock.close(); proc.terminate()
        try: _, err = proc.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill(); _, err = proc.communicate()
        os.close(master)
        if err: print('QEMU stderr:', err.decode('utf-8','replace')[-2000:])

