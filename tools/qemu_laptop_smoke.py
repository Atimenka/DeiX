#!/usr/bin/env python3
"""USB + AHCI + HDA regression, not a Ventoy or real-hardware test.

Boot a clean image from USB on Q35/Sandy Bridge with an unrelated internal
AHCI disk. Verify boot/storage and ensure that disk is not modified.
"""
import hashlib
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time

image = os.environ.get('DEIX_USB_IMAGE', 'build/deix_disk.img')
log_path = Path('/tmp/qemu_laptop_smoke.log')
with tempfile.TemporaryDirectory() as tmp:
    disk = Path(tmp) / 'internal.img'
    # Distinct MBR and nonzero data, so even accidental writes are observable.
    disk.write_bytes((bytes(range(256)) * 256) * 128)
    original = hashlib.sha256(disk.read_bytes()).digest()
    command = ['qemu-system-x86_64', '-machine', 'q35', '-cpu', 'SandyBridge',
               '-m', '512', '-usb', '-device', 'intel-hda',
               '-drive', f'file={disk},format=raw,if=none,id=internal',
               '-device', 'ide-hd,drive=internal,bus=ide.0',
               '-drive', f'file={image},format=raw,if=none,id=usbimg',
               '-device', 'usb-storage,drive=usbimg,bootindex=1',
               '-display', 'none', '-serial', 'stdio', '-monitor', 'none', '-no-reboot']
    print('Laptop-like smoke: USB + Q35 AHCI + SandyBridge + HDA', flush=True)
    proc = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    data = bytearray()
    deadline = time.monotonic() + 150
    try:
        while time.monotonic() < deadline:
            ready, _, _ = select.select([proc.stdout], [], [], 0.25)
            if ready:
                part = os.read(proc.stdout.fileno(), 65536)
                if not part:
                    break
                data.extend(part)
                if b'KERNEL PANIC' in data or b'RAMBOOT ERROR' in data:
                    break
                if b'[loginui]' in data and b'BOOT COMPLETE' in data:
                    break
            if proc.poll() is not None:
                break
    finally:
        proc.terminate()
        try:
            _, err = proc.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            _, err = proc.communicate()
    log_path.write_bytes(data)
    text = data.decode('utf-8', errors='replace')
    for line in text.splitlines():
        if any(k in line for k in ('[hda]', '[ramdisk]', 'STORAGE', 'BOOT COMPLETE', '[loginui]', 'PANIC', 'RAMBOOT ERROR', 'DX-')):
            print(line)
    if err:
        print('QEMU stderr:', err.decode('utf-8', errors='replace')[-3000:])
    required = [b'[ramdisk] active', b'ATA disk: not used', b'[hda]', b'[ OK ] STORAGE', b'BOOT COMPLETE', b'[loginui]',
                b'boot chain done: 3 launched, 0 failed', b'[vm-test] PASS: private CR3 switched between tasks']
    missing = [x.decode() for x in required if x not in data]
    unchanged = hashlib.sha256(disk.read_bytes()).digest() == original
    if missing or not unchanged or b'KERNEL PANIC' in data or b'DX-DISK-0004' in data:
        print('FAIL: missing markers:', missing, 'internal disk unchanged:', unchanged, flush=True)
        print('Serial tail:', text[-5000:], flush=True)
        sys.exit(1)
    print('PASS: RAM-disk booted, dependency chain launched, HDA probed, internal disk unchanged', flush=True)
