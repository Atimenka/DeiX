#!/usr/bin/env python3
"""Input must work when COM1 does not exist and port reads return 0xff."""
import os, re, shutil, socket, subprocess, tempfile, time
from pathlib import Path

source = os.environ.get('DEIX_USB_IMAGE', 'build/deix_disk.img')
usb_mouse = os.environ.get('DEIX_USB_MOUSE') == '1'
with tempfile.TemporaryDirectory() as tmp:
    disk = Path(tmp) / 'disk.img'
    shutil.copyfile(source, disk)
    mon = Path(tmp) / 'monitor.sock'
    cmd = ['qemu-system-x86_64', '-machine', 'pc,vmport=off', '-m', '512',
           '-vga', 'cirrus', '-drive', f'file={disk},format=raw,if=ide',
           '-display', 'none', '-serial', 'none', '-no-reboot',
           '-monitor', f'unix:{mon},server=on,wait=off']
    if usb_mouse:
        cmd += ['-device', 'usb-ehci,id=ehci', '-device', 'usb-mouse,bus=ehci.0']
    proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        deadline = time.monotonic() + 20
        while True:
            try:
                sock.connect(str(mon)); break
            except (FileNotFoundError, ConnectionRefusedError):
                if time.monotonic() > deadline: raise TimeoutError('monitor unavailable')
                time.sleep(.1)
        sock.settimeout(10)
        def reply():
            data = b''
            while b'(qemu)' not in data:
                chunk = sock.recv(8192)
                if not chunk: raise RuntimeError('monitor closed')
                data += chunk
            return data
        def send(command):
            sock.sendall(command.encode() + b'\n')
            return reply()
        reply()
        port = send('i /1bx 0x3fd')
        if b'0xff' not in port.lower():
            raise RuntimeError('COM1 absent-port read is not 0xff: ' + repr(port))
        # No serial exists to signal login. Give the RAM-disk loader and
        # graphical login time to settle before comparing focused regions.
        time.sleep(25)
        def screen(name):
            path = Path(tmp) / name
            result = send(f'screendump {path}')
            if not path.exists(): raise RuntimeError('screendump failed: ' + repr(result))
            data = path.read_bytes()
            match = re.match(rb'P6\s+(\d+)\s+(\d+)\s+255\s', data)
            if not match: raise RuntimeError('unexpected screenshot format')
            width, height = int(match.group(1)), int(match.group(2))
            if (width, height) != (800, 600):
                raise RuntimeError(f'not at graphical login: {width}x{height}')
            pixels = data[match.end():]
            if len(pixels) != width * height * 3: raise RuntimeError('truncated screenshot')
            return pixels
        def region(image, x0, y0, x1, y1):
            return b''.join(image[(y*800+x0)*3:(y*800+x1)*3] for y in range(y0, y1))
        before = screen('before.ppm')
        send('sendkey a')
        time.sleep(2)
        typed = screen('typed.ppm')
        if region(before, 210, 258, 260, 278) == region(typed, 210, 258, 260, 278):
            raise RuntimeError('PS/2 key did not update the username field without COM1')
        send('mouse_move 50 40')
        time.sleep(2)
        moved = screen('moved.ppm')
        if region(typed, 375, 280, 475, 365) == region(moved, 375, 280, 475, 365):
            raise RuntimeError('mouse cursor did not move without COM1')
        print('PASS: absent COM1; PS/2 keyboard and ' + ('EHCI USB' if usb_mouse else 'PS/2') + ' mouse', flush=True)
    finally:
        sock.close()
        proc.terminate()
        try: _, err = proc.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill(); _, err = proc.communicate()
        if err: print('QEMU stderr:', err.decode('utf-8', 'replace')[-2000:])
