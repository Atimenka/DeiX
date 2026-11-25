#!/usr/bin/env python3
"""Exercise USB keyboard input without an i8042 controller."""
import os
from pathlib import Path
import select
import shutil
import socket
import subprocess
import sys
import tempfile
import time

source = os.environ.get('DEIX_USB_IMAGE', 'build/deix_disk.img')
log = bytearray()
with tempfile.TemporaryDirectory() as tmp:
    image = Path(tmp) / 'disk.img'
    shutil.copyfile(source, image)
    monitor = Path(tmp) / 'monitor.sock'
    command = ['qemu-system-x86_64', '-machine', 'pc,i8042=off', '-m', '512', '-vga', 'none',
               '-device', 'usb-ehci,id=ehci', '-device', 'usb-kbd,bus=ehci.0',
               '-drive', f'file={image},format=raw,if=ide',
               '-display', 'none', '-serial', 'stdio',
               '-monitor', f'unix:{monitor},server=on,wait=off', '-no-reboot']
    proc = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        deadline = time.monotonic() + 15
        while True:
            try:
                sock.connect(str(monitor))
                break
            except (FileNotFoundError, ConnectionRefusedError):
                if time.monotonic() > deadline: raise TimeoutError('monitor unavailable')
                time.sleep(0.1)
        sock.settimeout(5)
        def monitor_reply():
            result = b''
            while b'(qemu)' not in result:
                chunk = sock.recv(4096)
                if not chunk: raise RuntimeError('monitor closed')
                result += chunk
            return result
        monitor_reply()
        def wait_for(needle, seconds=120):
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                if needle in log: return
                if proc.poll() is not None: break
                readable, _, _ = select.select([proc.stdout], [], [], 0.2)
                if readable:
                    part = os.read(proc.stdout.fileno(), 65536)
                    if not part: break
                    log.extend(part)
            raise RuntimeError('missing marker: ' + repr(needle))
        def type_keys(text):
            for c in text:
                name = 'ret' if c == '\n' else ('spc' if c == ' ' else c)
                sock.sendall(f'sendkey {name}\n'.encode())
                monitor_reply()
                time.sleep(0.12)
        wait_for(b'[bootlogo] VGA text artwork active')
        wait_for(b'No user accounts exist yet.')
        wait_for(b'[usb] boot keyboard', 90)
        type_keys('alice\n')
        time.sleep(0.5)
        type_keys('pass123\n')
        wait_for(b'[usb] keyboard report received')
        wait_for(b"Account 'alice' created.")
        wait_for(b'Welcome to DeiX CLI')
        type_keys('gpu nvinfo\n')
        wait_for(b'No NVIDIA display controller found on PCI bus.')
        if b'KERNEL PANIC' in log:
            raise RuntimeError('kernel panic')
        print('PASS: EHCI USB keyboard login and command with i8042 disabled', flush=True)
    except Exception as exc:
        print('FAIL:', exc, flush=True)
        print('Serial tail:', log[-7000:].decode('utf-8', 'replace'), flush=True)
        sys.exit(1)
    finally:
        Path('/tmp/qemu_ehci_keyboard.log').write_bytes(log)
        sock.close()
        proc.terminate()
        try:
            _, err = proc.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            _, err = proc.communicate()
        if err:
            print('QEMU stderr:', err.decode('utf-8', 'replace')[-2000:])
