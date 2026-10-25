#!/usr/bin/env python3
"""Пакет обновления DeiX: подписанный манифест и сжатая нагрузка.

Подкоманды:
  keygen  создать ключ подписи ECDSA P-256 (PEM) и показать публичный ключ
  pubkey  записать публичный ключ в 65 байт (build/update_pubkey.bin)
  pack    упаковать образ диска в манифест и нагрузку
  verify  проверить подпись и нагрузку готового пакета
  inspect показать манифест: сводка или полный JSON (--json)

Внешняя зависимость одна — утилита openssl. Формат описан в docs/UPDATE.md.
"""

import argparse
import base64
import gzip
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile

SECTOR = 512
MBR_CODE_BYTES = 446
BOOT_LBA = 1
BOOT_SECTORS = 2047
SYSTEM_LBA = 4096
SYSTEM_SECTORS = 20480
CHUNK_SECTORS = 1024
MAX_PAYLOAD_BYTES = 6 * 1024 * 1024
MAX_OPS = 32
MAX_OP_DATA = 1024

DOMAIN = b"DEIX-UPDATE-1\0"
MANIFEST_MAGIC = "DEIXMANIFEST1"
PUBKEY_LEN = 65
SPKI_PREFIX = bytes.fromhex("3059301306072a8648ce3d020106082a8648ce3d030107034200")

SAFE_TEXT = re.compile(r"^[A-Za-z0-9 .,:;_/=+()-]*$")
VERSION_RE = re.compile(r"^[0-9A-Za-z][0-9A-Za-z._-]{0,31}$")
PATH_RE = re.compile(r"^/userdata/[A-Za-z0-9._/-]{1,200}$")
CHANNELS = ("stable", "beta")


class UpdateError(Exception):
    pass


def run_openssl(args, data=None):
    try:
        proc = subprocess.run(["openssl"] + args, input=data, capture_output=True, check=False)
    except FileNotFoundError:
        raise UpdateError("утилита openssl не найдена")
    if proc.returncode != 0:
        raise UpdateError("openssl %s: %s" % (args[0], proc.stderr.decode(errors="replace").strip()))
    return proc.stdout


def der_to_raw(der):
    if len(der) < 8 or der[0] != 0x30:
        raise UpdateError("подпись не в формате DER")
    pos = 2
    if der[1] & 0x80:
        pos = 2 + (der[1] & 0x7F)
    parts = []
    for _ in range(2):
        if der[pos] != 0x02:
            raise UpdateError("подпись не в формате DER")
        n = der[pos + 1]
        value = der[pos + 2:pos + 2 + n]
        pos += 2 + n
        value = value.lstrip(b"\0")
        if len(value) > 32:
            raise UpdateError("компонента подписи длиннее 32 байт")
        parts.append(value.rjust(32, b"\0"))
    return parts[0] + parts[1]


def raw_to_der(raw):
    if len(raw) != 64:
        raise UpdateError("подпись должна быть 64 байта")

    def integer(b):
        b = b.lstrip(b"\0") or b"\0"
        if b[0] & 0x80:
            b = b"\0" + b
        return bytes([0x02, len(b)]) + b

    body = integer(raw[:32]) + integer(raw[32:])
    return bytes([0x30, len(body)]) + body


def public_key_raw(key_path):
    der = run_openssl(["ec", "-in", key_path, "-pubout", "-outform", "DER"])
    if len(der) != len(SPKI_PREFIX) + PUBKEY_LEN or not der.startswith(SPKI_PREFIX):
        raise UpdateError("ключ не ECDSA P-256")
    return der[len(SPKI_PREFIX):]


def sign(key_path, manifest_bytes):
    der = run_openssl(["dgst", "-sha256", "-sign", key_path], DOMAIN + manifest_bytes)
    return der_to_raw(der)


def verify_signature(pub65, manifest_bytes, signature):
    if len(pub65) != PUBKEY_LEN or pub65[0] != 4:
        raise UpdateError("публичный ключ должен быть 65 байт (0x04 || X || Y)")
    pem = (
        b"-----BEGIN PUBLIC KEY-----\n"
        + base64.encodebytes(SPKI_PREFIX + pub65)
        + b"-----END PUBLIC KEY-----\n"
    )
    with tempfile.TemporaryDirectory() as tmp:
        pub_path = os.path.join(tmp, "pub.pem")
        sig_path = os.path.join(tmp, "sig.der")
        with open(pub_path, "wb") as f:
            f.write(pem)
        with open(sig_path, "wb") as f:
            f.write(raw_to_der(signature))
        try:
            run_openssl(["dgst", "-sha256", "-verify", pub_path, "-signature", sig_path], DOMAIN + manifest_bytes)
        except UpdateError:
            return False
    return True


def canonical(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("ascii")


def check_text(name, value):
    if not SAFE_TEXT.match(value):
        raise UpdateError("%s: допустимы латинские буквы, цифры, пробел и . , : ; _ / = + ( ) -" % name)


def parse_op(spec):
    kind, _, rest = spec.partition(":")
    if kind in ("mkdir", "remove"):
        path = rest
        op = {"op": kind, "path": path}
    elif kind == "write":
        path, _, data = rest.partition("=")
        if not re.fullmatch(r"([0-9a-f]{2}){0,%d}" % MAX_OP_DATA, data):
            raise UpdateError("write: данные — hex, не больше %d байт" % MAX_OP_DATA)
        op = {"op": kind, "path": path, "hex": data}
    else:
        raise UpdateError("операция /userdata: mkdir:ПУТЬ, remove:ПУТЬ или write:ПУТЬ=HEX")
    if not PATH_RE.match(op["path"]) or ".." in op["path"].split("/"):
        raise UpdateError("путь операции должен лежать в /userdata/ и состоять из [A-Za-z0-9._/-]")
    return op


def segment_list():
    return [
        {"lba": 0, "mode": "mbr", "sectors": 1},
        {"lba": BOOT_LBA, "mode": "raw", "sectors": BOOT_SECTORS},
        {"lba": SYSTEM_LBA, "mode": "raw", "sectors": SYSTEM_SECTORS},
    ]


def read_image_segments(image_path):
    needed = (SYSTEM_LBA + SYSTEM_SECTORS) * SECTOR
    if os.path.getsize(image_path) < needed:
        raise UpdateError("образ меньше %d байт: нет раздела /system" % needed)
    parts = []
    with open(image_path, "rb") as f:
        for seg in segment_list():
            f.seek(seg["lba"] * SECTOR)
            data = f.read(seg["sectors"] * SECTOR)
            if len(data) != seg["sectors"] * SECTOR:
                raise UpdateError("образ оборван в сегменте LBA %d" % seg["lba"])
            parts.append(data)
    return b"".join(parts)


def pack(args):
    if not VERSION_RE.match(args.version):
        raise UpdateError("версия: до 32 символов [A-Za-z0-9._-]")
    if args.channel not in CHANNELS:
        raise UpdateError("канал: stable или beta")
    check_text("notes", args.notes)
    if len(args.notes) > 200:
        raise UpdateError("notes: не длиннее 200 символов")
    if args.build < 1 or args.build >= 1 << 53:
        raise UpdateError("build должен быть положительным целым")
    if args.min_build < 0 or args.min_build > args.build:
        raise UpdateError("min-build не может превышать build")
    if len(args.op) > MAX_OPS:
        raise UpdateError("операций /userdata не больше %d" % MAX_OPS)
    ops = [parse_op(s) for s in args.op]

    raw = read_image_segments(args.image)
    chunk_bytes = CHUNK_SECTORS * SECTOR
    payload = bytearray()
    chunks = []
    for off in range(0, len(raw), chunk_bytes):
        piece = raw[off:off + chunk_bytes]
        member = gzip.compress(piece, compresslevel=9, mtime=0)
        payload += len(member).to_bytes(4, "little") + member
        chunks.append({"c": len(member), "h": hashlib.sha256(piece).hexdigest()})
    if len(payload) > MAX_PAYLOAD_BYTES:
        raise UpdateError(
            "нагрузка %d Б больше предела %d Б (куча ядра 16 МиБ, пакет читается целиком)"
            % (len(payload), MAX_PAYLOAD_BYTES)
        )

    manifest = {
        "build": args.build,
        "channel": args.channel,
        "chunk_sectors": CHUNK_SECTORS,
        "chunks": chunks,
        "min_build": args.min_build,
        "notes": args.notes,
        "payload_sha256": hashlib.sha256(payload).hexdigest(),
        "payload_size": len(payload),
        "product": "deix",
        "schema": args.schema,
        "segments": segment_list(),
        "userdata_ops": ops,
        "version": args.version,
    }
    manifest_bytes = canonical(manifest)
    signature = sign(args.key, manifest_bytes)

    os.makedirs(args.out_dir, exist_ok=True)
    stem = os.path.join(args.out_dir, "deix-update-%s" % args.version)
    manifest_path = stem + ".manifest"
    payload_path = stem + ".payload"
    with open(manifest_path, "wb") as f:
        f.write(MANIFEST_MAGIC.encode() + b"\n" + signature.hex().encode() + b"\n" + manifest_bytes + b"\n")
    with open(payload_path, "wb") as f:
        f.write(payload)
    print("манифест: %s" % manifest_path)
    print("нагрузка: %s (%d Б, сырых данных %d Б)" % (payload_path, len(payload), len(raw)))
    return manifest_path, payload_path


def load_manifest(path):
    with open(path, "rb") as f:
        lines = f.read().split(b"\n")
    if len(lines) < 3 or lines[0] != MANIFEST_MAGIC.encode():
        raise UpdateError("файл манифеста: нет сигнатуры %s" % MANIFEST_MAGIC)
    signature = bytes.fromhex(lines[1].decode("ascii"))
    return signature, lines[2]


def iter_chunks(payload, manifest):
    pos = 0
    for entry in manifest["chunks"]:
        if pos + 4 > len(payload):
            raise UpdateError("нагрузка оборвана")
        n = int.from_bytes(payload[pos:pos + 4], "little")
        if n != entry["c"]:
            raise UpdateError("размер куска в нагрузке и в манифесте различается")
        member = payload[pos + 4:pos + 4 + n]
        if len(member) != n:
            raise UpdateError("нагрузка оборвана")
        yield entry, gzip.decompress(member)
        pos += 4 + n
    if pos != len(payload):
        raise UpdateError("в нагрузке есть лишние байты")


def verify(args):
    if args.pub:
        with open(args.pub, "rb") as f:
            pub = f.read()
    else:
        pub = public_key_raw(args.key)
    signature, manifest_bytes = load_manifest(args.manifest)
    if not verify_signature(pub, manifest_bytes, signature):
        raise UpdateError("подпись манифеста не сходится")
    manifest = json.loads(manifest_bytes)
    if canonical(manifest) != manifest_bytes:
        raise UpdateError("манифест не в каноническом виде")
    with open(args.payload, "rb") as f:
        payload = f.read()
    if len(payload) != manifest["payload_size"]:
        raise UpdateError("размер нагрузки не совпадает с манифестом")
    if hashlib.sha256(payload).hexdigest() != manifest["payload_sha256"]:
        raise UpdateError("sha256 нагрузки не совпадает с манифестом")
    total = 0
    for entry, piece in iter_chunks(payload, manifest):
        if hashlib.sha256(piece).hexdigest() != entry["h"]:
            raise UpdateError("sha256 куска не совпадает с манифестом")
        total += len(piece)
    want = sum(s["sectors"] for s in manifest["segments"]) * SECTOR
    if total != want:
        raise UpdateError("распакованный размер %d Б, по сегментам нужно %d Б" % (total, want))
    print("пакет %s (сборка %d, канал %s): подпись и нагрузка верны" % (manifest["version"], manifest["build"], manifest["channel"]))


def inspect(args):
    signature, body = load_manifest(args.manifest)
    doc = json.loads(body)
    if canonical(doc) != body:
        raise UpdateError("манифест не в каноническом виде")
    state = "не проверялась (нужен --key или --pub)"
    if args.pub or args.key:
        if args.pub:
            with open(args.pub, "rb") as f:
                pub = f.read()
        else:
            pub = public_key_raw(args.key)
        if not verify_signature(pub, body, signature):
            raise UpdateError("подпись манифеста не сходится")
        state = "верна"
    if args.json:
        print(json.dumps({"signature": signature.hex(), "manifest": doc}, ensure_ascii=False, indent=2, sort_keys=True))
        return
    print("версия:        %s" % doc["version"])
    print("сборка:        %d (минимальная для установки: %d)" % (doc["build"], doc["min_build"]))
    print("канал:         %s" % doc["channel"])
    print("схема данных:  %d" % doc["schema"])
    print("заметки:       %s" % (doc["notes"] or "-"))
    print("нагрузка:      %d Б, sha256 %s" % (doc["payload_size"], doc["payload_sha256"]))
    print("куски:         %d по %d секторов" % (len(doc["chunks"]), doc["chunk_sectors"]))
    for seg in doc["segments"]:
        print("сегмент:       LBA %d, секторов %d, режим %s" % (seg["lba"], seg["sectors"], seg.get("mode", "raw")))
    print("операции /userdata: %d" % len(doc["userdata_ops"]))
    for op in doc["userdata_ops"]:
        print("  %s %s" % (op["op"], op["path"]))
    print("подпись:       %s" % state)


def keygen(args):
    if os.path.exists(args.key) and not args.force:
        raise UpdateError("%s уже существует (--force перезапишет)" % args.key)
    os.makedirs(os.path.dirname(os.path.abspath(args.key)), exist_ok=True)
    run_openssl(["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", args.key])
    os.chmod(args.key, 0o600)
    print("ключ: %s (храните в секрете: им подписываются обновления)" % args.key)
    print("публичный ключ: %s" % public_key_raw(args.key).hex())


def pubkey(args):
    data = b"\0" * PUBKEY_LEN if args.zero else public_key_raw(args.key)
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "wb") as f:
        f.write(data)


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("keygen")
    p.add_argument("--key", required=True)
    p.add_argument("--force", action="store_true")
    p.set_defaults(fn=keygen)

    p = sub.add_parser("pubkey")
    p.add_argument("--key")
    p.add_argument("--zero", action="store_true", help="нулевой ключ: обновления в сборке выключены")
    p.add_argument("--out", required=True)
    p.set_defaults(fn=pubkey)

    p = sub.add_parser("pack")
    p.add_argument("--image", required=True)
    p.add_argument("--key", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--build", required=True, type=int)
    p.add_argument("--channel", default="stable")
    p.add_argument("--min-build", dest="min_build", type=int, default=0)
    p.add_argument("--schema", type=int, default=1)
    p.add_argument("--notes", default="")
    p.add_argument("--op", action="append", default=[])
    p.add_argument("--out-dir", dest="out_dir", required=True)
    p.set_defaults(fn=lambda a: pack(a))

    p = sub.add_parser("verify")
    p.add_argument("--manifest", required=True)
    p.add_argument("--payload", required=True)
    p.add_argument("--pub")
    p.add_argument("--key")
    p.set_defaults(fn=verify)

    p = sub.add_parser("inspect", help="показать манифест: сводка или полный JSON (--json)")
    p.add_argument("--manifest", required=True)
    p.add_argument("--pub")
    p.add_argument("--key")
    p.add_argument("--json", action="store_true")
    p.set_defaults(fn=inspect)

    args = ap.parse_args(argv)
    if args.cmd == "pubkey" and not args.zero and not args.key:
        ap.error("pubkey: нужен --key или --zero")
    if args.cmd == "verify" and not (args.pub or args.key):
        ap.error("verify: нужен --pub или --key")
    try:
        args.fn(args)
    except UpdateError as e:
        print("ошибка: %s" % e, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
