#!/usr/bin/env python3
"""Exercise HD BIOS VBE selection against QEMU std VGA using a test-only loader."""
import os, pty, select, socket, subprocess, tempfile, time, shutil, struct, re
from pathlib import Path

source = os.environ.get('DEIX_USB_IMAGE', 'build/deix_disk.img')
serial = bytearray()
with tempfile.TemporaryDirectory() as tmp:
    disk = Path(tmp) / 'disk.img'
    shutil.copyfile(source, disk)
    # Production loader skips Bochs in favour of DISPI. Rebuild only the
    # test copy with that guard disabled, to exercise VBE's HD mode list.
    ramboot = Path(tmp) / 'ramboot.bin'
    subprocess.run(['nasm', '-DFORCE_BIOS_VBE=1', '-f', 'bin', 'boot/ramboot.asm', '-o', str(ramboot)], check=True)
    loader = ramboot.read_bytes()
    with disk.open('r+b') as f:
        f.seek(2047 * 512)
        bootmap = f.read(512)
        if bootmap[:8] != b'DEIXKIMG': raise RuntimeError('bad boot map')
        sectors = struct.unpack_from('<I', bootmap, 56)[0]
        lba, extent_sectors = struct.unpack_from('<II', bootmap, 128)
        if len(loader) != sectors * 512 or sectors > extent_sectors:
            raise RuntimeError('test loader does not fit original ramboot extent')
        f.seek(lba * 512); f.write(loader)
    mon = Path(tmp) / 'monitor.sock'
    master, slave = pty.openpty()
    proc = subprocess.Popen(['qemu-system-x86_64', '-m', '512', '-vga', 'std',
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
        wait_for(b'[renderer] BIOS VBE framebuffer 1280x720 active (16 bpp)')
        wait_for('[loginui] графический вход, создание учётки: true'.encode())
        for text in (b'alice\n', b'pass123\n', b'pass123\n'):
            os.write(master, text); time.sleep(.8)
        wait_for(b'[desktop] starting BIOS framebuffer desktop', 90)
        wait_for(b'[desktop] first frame presented', 90)
        sock.sendall(b'sendkey esc\n'); reply()
        wait_for(b'[desktop] BIOS framebuffer: ignoring exit to text mode', 30)
        time.sleep(3)
        shot = Path(tmp) / 'desktop.ppm'
        sock.sendall(f'screendump {shot}\n'.encode()); reply()
        ppm = shot.read_bytes()
        if not re.match(rb'P6\s+1280\s+720\s+255\s', ppm):
            raise RuntimeError('screenshot is not 1280x720')
        if len(ppm) < 1280*720*3 or len(set(ppm[-1280*720*3:])) < 8:
            raise RuntimeError('HD desktop screenshot is empty')
        if b'KERNEL PANIC' in serial: raise RuntimeError('kernel panic')
        print('PASS: BIOS VBE selects 1280x720, graphical login and desktop screenshot', flush=True)
    except Exception as exc:
        print('FAIL:', exc, '\nSerial tail:', serial[-8000:].decode('utf-8','replace'), flush=True)
        raise
    finally:
        Path('/tmp/qemu_bios_hd.log').write_bytes(serial)
        sock.close(); proc.terminate()
        try: _, err = proc.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill(); _, err = proc.communicate()
        os.close(master)
        if err: print('QEMU stderr:', err.decode('utf-8','replace')[-2000:])
