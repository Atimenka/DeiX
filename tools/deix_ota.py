#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""deix-ota — host-утилита рассылки OTA-обновлений DeiX (Linux-бинарник).

Возможности:
  build  — собрать подписанный OTA-пакет (kernel.tar.gz + контейнер DEIXOTA1)
           из kernel.bin и библиотек;
  push   — прошить OTA-ядро в A/B-слоты ВСЕХ указанных образов устройств
           (kernel_a/kernel_b как EROFS с kernel.tar.gz) + опционально
           переключить активный слот в BCB (LBA 3000);
  serve  — HTTP-сервер раздачи пакетов по сети (для ota fetch с устройств);
  info   — показать содержимое EROFS-разделов и BCB образа.

Форматы соответствуют ядру DeiX (src/ota.rs, src/bootchain.rs,
src/partition_map.rs) — прошитый пакет ядро распакует и загрузится.

Примеры:
  deix-ota build --kernel build/kernel.bin --out TEST.OTA
  deix-ota push  --img disk1.img --img disk2.img --ota TEST.OTA --slot both
  deix-ota push  --img disk.img --kernel build/kernel.bin --set-slot b
  deix-ota serve --dir . --port 8080
  deix-ota info  --img disk.img
"""
import argparse, gzip, hashlib, http.server, io, os, socketserver, struct, sys, tarfile, time

# ---------- константы (совпадают с ядром) ----------
OTA_MAGIC = b"DEIXOTA1"
OTA_SECRET = b"deix_ota_secret_key_v1"
EROFS_MAGIC = 0xE0F5E1E2  # настоящая магия EROFS v1
BCB_LBA = 3000
SECTOR = 512

# Карта слотов (совпадает с src/partition_map.rs PARTITION_LAYOUT)
SLOTS = {
    "/kernel_a": 13313,
    "/kernel_b": 14593,
    "/init_boot": 15873,
    "/vendor_boot": 16129,
    "/boot_a": 16385,
    "/boot_b": 16641,
    "/super": 16897,
    "/dsm": 17153,
    "/recovery": 17409,
}
SLOT_SECS = {  # размеры разделов (секторов)
    "/kernel_a": 1279, "/kernel_b": 1279,
    "/init_boot": 255, "/vendor_boot": 255,
    "/boot_a": 255, "/boot_b": 255,
    "/super": 255, "/dsm": 255, "/recovery": 255,
}

# ---------- OTA-пакет ----------
def make_kernel_targz(kernel_bytes, libs):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        def add(name, data):
            ti = tarfile.TarInfo(name)
            ti.size = len(data)
            ti.mode = 0o100644
            tar.addfile(ti, io.BytesIO(data))
        add("kernel.bin", kernel_bytes)
        for name, data in libs.items():
            add(name, data)
    raw = buf.getvalue()
    return gzip.compress(raw, compresslevel=9)

def build_ota(payload, version):
    sha = hashlib.sha256(payload).digest()
    sig = hashlib.sha256(payload + OTA_SECRET).digest()
    return OTA_MAGIC + struct.pack("<II", version, len(payload)) + sha + sig + payload

# ---------- EROFS с файлом (как tools/make_deix_fs.py) ----------
def build_erofs_with_file(label, name, data, sectors):
    try:
        from make_deix_fs import build_real_erofs
        blob = build_real_erofs({name: data}, label)
        img = bytearray(sectors * SECTOR)
        dlen = min(len(blob), len(img))
        img[:dlen] = blob[:dlen]
        return bytes(img)
    except Exception:
        pass
    img = bytearray(sectors * SECTOR)
    sb = 1024
    img[sb:sb+4] = struct.pack("<I", EROFS_MAGIC)
    img[sb+8:sb+12] = struct.pack("<I", 1)      # feature_compat v2
    img[sb+12] = 12                              # blkszbits
    img[sb+16:sb+24] = struct.pack("<Q", 2)      # inos
    img[sb+36:sb+40] = struct.pack("<I", 1)
    lb = label.encode()[:16]
    img[sb+64:sb+64+len(lb)] = lb
    tbl = sb + 128
    img[tbl:tbl+4] = struct.pack("<I", 1)
    nb = name.encode()[:31]
    img[tbl+4:tbl+4+len(nb)] = nb
    data_off = tbl + 4 + 40
    img[tbl+36:tbl+40] = struct.pack("<I", data_off)          # offset от начала
    img[tbl+40:tbl+44] = struct.pack("<I", len(data))         # size
    dlen = min(len(data), len(img) - data_off)
    img[data_off:data_off+dlen] = data[:dlen]
    return bytes(img)

def erofs_list(img):
    """Читает файлы EROFS-раздела v1: [(name, size, off)]."""
    if len(img) < 1024 + 128:
        return []
    magic = struct.unpack("<I", img[1024:1028])[0]
    if magic != EROFS_MAGIC:
        return []
    root_nid = struct.unpack("<H", img[1024+14:1024+16])[0]
    meta_blkaddr = struct.unpack("<I", img[1024+40:1024+44])[0]
    root_inode_off = meta_blkaddr * 4096 + root_nid * 32
    if len(img) < root_inode_off + 32:
        return []
    dir_size = struct.unpack("<I", img[root_inode_off+8:root_inode_off+12])[0]
    dir_blkaddr = struct.unpack("<I", img[root_inode_off+16:root_inode_off+20])[0]
    dir_data_off = dir_blkaddr * 4096
    if len(img) < dir_data_off + 12:
        return []
    first_nameoff = struct.unpack("<H", img[dir_data_off+8:dir_data_off+10])[0]
    if first_nameoff < 12 or first_nameoff > 4096:
        return []
    count = first_nameoff // 12
    out = []
    for i in range(count):
        e = dir_data_off + i * 12
        if e + 12 > len(img):
            break
        nid, nameoff, ftype = struct.unpack("<QHB", img[e:e+11])
        if ftype == 2:  # EROFS_FT_DIR (. or ..)
            continue
        name_start = dir_data_off + nameoff
        if i + 1 < count:
            next_nameoff = struct.unpack("<H", img[dir_data_off + (i + 1) * 12 + 8:dir_data_off + (i + 1) * 12 + 10])[0]
            name_end = dir_data_off + next_nameoff
        else:
            name_end = dir_data_off + 4096
        raw_name = img[name_start:name_end].rstrip(b"\x00")
        name = raw_name.decode("utf-8", errors="replace")
        ino_off = meta_blkaddr * 4096 + nid * 32
        fsize = 0
        if ino_off + 12 <= len(img):
            fsize = struct.unpack("<I", img[ino_off+8:ino_off+12])[0]
        out.append((name, fsize, ino_off))
    return out

# ---------- BCB (LBA 3000) ----------
def read_bcb(img):
    sec = img[BCB_LBA*SECTOR:(BCB_LBA+1)*SECTOR]
    if sec[:8] != b"DEIXBCB1":
        return {"magic": False, "mode": 0, "slot": 0}
    mode = struct.unpack("<I", sec[8:12])[0]
    slot = struct.unpack("<I", sec[12:16])[0]
    return {"magic": True, "mode": mode, "slot": slot}

def write_bcb(img, mode=None, slot=None):
    sec = bytearray(img[BCB_LBA*SECTOR:(BCB_LBA+1)*SECTOR])
    if sec[:8] != b"DEIXBCB1":
        sec[:8] = b"DEIXBCB1"
        sec[8:12] = (0).to_bytes(4, "little")
        sec[12:16] = (0).to_bytes(4, "little")
    if mode is not None:
        sec[8:12] = (mode & 0xFF).to_bytes(4, "little")
    if slot is not None:
        sec[12:16] = (slot & 1).to_bytes(4, "little")
    img[BCB_LBA*SECTOR:(BCB_LBA+1)*SECTOR] = sec

MODE_NAMES = {0: "normal", 1: "recovery", 2: "fastbootd", 3: "dsm"}

def build_deix_lib_py(lib_name, exports):
    out = bytearray(b"DEIXLIB1")
    out.extend(struct.pack("<II", 1, len(exports)))
    name_buf = lib_name.encode('utf-8')[:31].ljust(32, b'\x00')
    out.extend(name_buf)

    code_offset = 8 + 8 + 32 + (len(exports) * 48)
    code_bytes = bytearray()

    for sym_name, code in exports:
        sym_buf = sym_name.encode('utf-8')[:31].ljust(32, b'\x00')
        out.extend(sym_buf)
        out.extend(struct.pack("<IIQ", code_offset, len(code), 0))
        code_offset += len(code)
        code_bytes.extend(code)

    out.extend(code_bytes)
    return bytes(out)

def get_default_libs():
    return {
        "libdeix_core.so": build_deix_lib_py("libdeix_core.so", [
            ("deix_core_init", b"\xb8\x01\x00\x00\x00\xc3"),
            ("deix_core_version", b"\xb8\x01\x02\x00\x00\xc3"),
            ("deix_core_yield", b"\xcd\x20\xc3"),
        ]),
        "libdeix_net.so": build_deix_lib_py("libdeix_net.so", [
            ("deix_net_init", b"\xb8\x01\x00\x00\x00\xc3"),
            ("deix_net_socket", b"\xb8\x03\x00\x00\x00\xc3"),
            ("deix_net_send", b"\xb8\x00\x00\x00\x00\xc3"),
        ]),
        "libdeix_gfx.so": build_deix_lib_py("libdeix_gfx.so", [
            ("deix_gfx_init", b"\xb8\x01\x00\x00\x00\xc3"),
            ("deix_gfx_draw_rect", b"\xb8\x00\x00\x00\x00\xc3"),
            ("deix_gfx_swap_buffers", b"\xb8\x00\x00\x00\x00\xc3"),
        ]),
    }

# ---------- команды ----------
def cmd_build(args):
    kernel_path = args.kernel or getattr(args, 'img', None)
    if not kernel_path:
        print("[deix-ota] ОШИБКА: укажите --kernel или --img", file=sys.stderr)
        return 2
    kernel = open(kernel_path, "rb").read()
    libs = {}
    for spec in (args.libs or []):
        if "=" in spec:
            arc, path = spec.split("=", 1)
            libs[arc] = open(path, "rb").read()
        else:
            libs[os.path.basename(spec)] = open(spec, "rb").read()
    # библиотеки по умолчанию (модель встроенных компонентов ядра)
    if not libs:
        libs = get_default_libs()
    tgz = make_kernel_targz(kernel, libs)
    pkg = build_ota(tgz, args.version)
    out = args.out
    open(out, "wb").write(pkg)
    # также кладём рядом kernel.tar.gz (для serve)
    open(out.replace(".OTA", ".tar.gz") if out.endswith(".OTA") else out + ".tar.gz", "wb").write(tgz)
    print(f"[deix-ota] OK: {out} ({len(pkg)} Б, v{args.version})")
    print(f"           kernel.tar.gz: {len(tgz)} Б (kernel.bin {len(kernel)} Б + {len(libs)} libs)")
    print(f"           подпись: SHA-256(payload || secret) — валидна для ядра")
    return 0

def cmd_push(args):
    images = args.img
    if not images:
        print("[deix-ota] ОШИБКА: укажите --img (можно несколько)", file=sys.stderr)
        return 2
    # пакет: либо --ota (готовый), либо --kernel (собрать на лету)
    if args.ota:
        tgz = open(args.ota, "rb").read()
        # если это контейнер DEIXOTA1 — извлечь payload
        if tgz[:8] == OTA_MAGIC:
            plen = struct.unpack("<I", tgz[16:20])[0]
            tgz = tgz[80:80+plen]
    else:
        kernel_path = args.kernel or getattr(args, 'img_kernel', None)
        if not kernel_path:
            print("[deix-ota] ОШИБКА: укажите --kernel или --img-kernel", file=sys.stderr)
            return 2
        kernel = open(kernel_path, "rb").read()
        libs = get_default_libs()
        tgz = make_kernel_targz(kernel, libs)
    # слоты для прошивки
    targets = []
    if args.slot in ("a", "both"):
        targets.append("/kernel_a")
    if args.slot in ("b", "both"):
        targets.append("/kernel_b")
    if not targets:
        print("[deix-ota] ОШИБКА: --slot должен быть a|b|both", file=sys.stderr)
        return 2
    for img_path in images:
        img = bytearray(open(img_path, "rb").read())
        for part in targets:
            lba = SLOTS[part]; secs = SLOT_SECS[part]
            erofs = build_erofs_with_file("kernel", "kernel.tar.gz", tgz, secs)
            img[lba*SECTOR:(lba+secs)*SECTOR] = erofs
            print(f"[deix-ota] {img_path}: прошит {part} (LBA {lba}, {secs} сект, kernel.tar.gz {len(tgz)} Б)")
        if args.set_slot is not None:
            s = 0 if args.set_slot == "a" else 1
            write_bcb(img, slot=s)
            print(f"[deix-ota] {img_path}: BCB активный слот -> {args.set_slot}")
        open(img_path, "wb").write(img)
        bcb = read_bcb(img)
        print(f"[deix-ota] {img_path}: OK (BCB: mode={MODE_NAMES.get(bcb['mode'], bcb['mode'])}, slot={'a' if bcb['slot']==0 else 'b'})")

    # АВТО-СЕРВЕР: после рассылки поднимаем HTTP-сервер раздачи пакета,
    # чтобы устройства могли забрать TEST.OTA по сети (ota fetch).
    if getattr(args, 'serve', False):
        import threading
        sdir = args.serve_dir
        os.makedirs(sdir, exist_ok=True)
        open(os.path.join(sdir, "TEST.OTA"), "wb").write(tgz)
        open(os.path.join(sdir, "kernel.tar.gz"), "wb").write(tgz)
        port = args.serve_port
        OtaHandler._dir = sdir
        socketserver.TCPServer.allow_reuse_address = True
        httpd = socketserver.TCPServer(("0.0.0.0", port), OtaHandler)
        th = threading.Thread(target=httpd.serve_forever, daemon=True)
        th.start()
        print(f"[deix-ota] АВТО-СЕРВЕР запущен: http://0.0.0.0:{port}/TEST.OTA (Ctrl+C для остановки)")
        print(f"           Устройства: ota fetch http://<host>:{port}/TEST.OTA")
        try:
            while True:
                time.sleep(1)
        except KeyboardInterrupt:
            httpd.shutdown()
    return 0

def cmd_info(args):
    img = open(args.img, "rb").read()
    bcb = read_bcb(img)
    print(f"BCB: magic={bcb['magic']}, mode={MODE_NAMES.get(bcb['mode'], bcb['mode'])}, slot={'a' if bcb['slot']==0 else 'b'}")
    for part, lba in SLOTS.items():
        secs = SLOT_SECS[part]
        chunk = img[lba*SECTOR:(lba+secs)*SECTOR]
        files = erofs_list(chunk)
        if files:
            desc = ", ".join(f"{n}({s}Б)" for n, s, _ in files)
            print(f"{part:<12} LBA{lba:<6} {desc}")
        else:
            print(f"{part:<12} LBA{lba:<6} (пусто/не EROFS)")
    return 0

class OtaHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.lstrip("/")
        if path not in ("TEST.OTA", "kernel.tar.gz"):
            self.send_response(404); self.end_headers()
            self.wfile.write(b"not found")
            return
        base = getattr(self.__class__, "_dir", ".")
        data = open(os.path.join(base, path), "rb").read()
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *a):
        pass

def cmd_serve(args):
    os.chdir(args.dir)
    print(f"[deix-ota] OTA-сервер: http://0.0.0.0:{args.port}/TEST.OTA")
    print("           устройства: ota fetch <host> (когда реализовано в ядре)")
    socketserver.TCPServer.allow_reuse_address = True
    httpd = socketserver.TCPServer(("0.0.0.0", args.port), OtaHandler)
    httpd.serve_forever()
    return 0

def main():
    p = argparse.ArgumentParser(prog="deix-ota", description="DeiX OTA host utility")
    sub = p.add_subparsers(dest="cmd")

    b = sub.add_parser("build", help="build signed OTA package")
    b.add_argument("--kernel", help="path to kernel.bin")
    b.add_argument("--img", help=".img file used as kernel base (like --kernel)")
    b.add_argument("--out", default="TEST.OTA", help="output package file")
    b.add_argument("--version", type=int, default=6)
    b.add_argument("--libs", nargs="*", help="extra libs: arcname=path or path")
    b.set_defaults(fn=cmd_build)

    ps = sub.add_parser("push", help="push OTA kernel into A/B slots of images")
    ps.add_argument("--img", action="append", required=True, help="disk image (repeatable)")
    ps.add_argument("--ota", help="ready OTA package (TEST.OTA)")
    ps.add_argument("--kernel", help="kernel.bin to build package on the fly")
    ps.add_argument("--slot", default="both", choices=["a", "b", "both"])
    ps.add_argument("--set-slot", choices=["a", "b"], help="switch active slot in BCB")
    ps.add_argument("--img-kernel", help=".img file used as kernel base")
    ps.add_argument("--serve", action="store_true", help="auto-start HTTP server after push")
    ps.add_argument("--serve-dir", default=".", help="dir for auto server")
    ps.add_argument("--serve-port", type=int, default=8080)
    ps.set_defaults(fn=cmd_push)

    sv = sub.add_parser("serve", help="HTTP server to distribute OTA")
    sv.add_argument("--dir", default=".", help="directory with TEST.OTA")
    sv.add_argument("--port", type=int, default=8080)
    sv.set_defaults(fn=cmd_serve)

    inf = sub.add_parser("info", help="show partitions/BCB of an image")
    inf.add_argument("--img", required=True)
    inf.set_defaults(fn=cmd_info)

    args = p.parse_args()
    if not hasattr(args, "fn"):
        p.print_help()
        return 2
    return args.fn(args)

if __name__ == "__main__":
    sys.exit(main())
