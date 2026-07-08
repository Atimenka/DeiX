#!/usr/bin/env python3
"""kmod_pack.py — упаковщик .bin в .kmod (DeiX Kernel Module v1.0)

Usage: python3 kmod_pack.py <input.bin> <output.kmod> <module_name> [init_offset]

Формат .kmod (44-байтный заголовок):
  magic (4) + version (2+2) + header_size (4) + init_offset (4) +
  body_size (4) + bss_size (4) + name_len (4) + name (28) + body
"""

import struct
import sys


KMOD_MAGIC = 0x444F4D4B  # "KMOD" LE
HEADER_SIZE = 44
VERSION_MAJOR = 1
VERSION_MINOR = 0


def main():
    if len(sys.argv) < 4:
        print(f"Usage: {sys.argv[0]} <input.bin> <output.kmod> <module_name> [init_offset] [bss_size]")
        sys.exit(1)

    input_path = sys.argv[1]
    output_path = sys.argv[2]
    name = sys.argv[3]
    init_offset = int(sys.argv[4]) if len(sys.argv) > 4 else 0
    bss_size = int(sys.argv[5]) if len(sys.argv) > 5 else 0

    with open(input_path, "rb") as f:
        body = f.read()

    body_size = len(body)
    name_bytes = name.encode("ascii", errors="replace")[:28]
    name_padded = name_bytes + b"\x00" * (28 - len(name_bytes))

    header = struct.pack(
        "<IHHIIIII",
        KMOD_MAGIC,       # magic
        VERSION_MAJOR,    # version_major
        VERSION_MINOR,    # version_minor
        HEADER_SIZE,      # header_size
        init_offset,      # init_offset
        body_size,        # body_size
        bss_size,         # bss_size
        len(name_bytes),  # name_len
    )

    assert len(header) == 32, f"First part: {len(header)} != 32"
    name_field = struct.pack("28s", name_padded)
    assert len(name_field) == 28

    full_header = header + name_field
    assert len(full_header) == HEADER_SIZE, f"Full header: {len(full_header)} != {HEADER_SIZE}"

    with open(output_path, "wb") as f:
        f.write(full_header)
        f.write(body)

    print(f"Packed: {input_path} ({body_size} bytes) -> {output_path} "
          f"(module '{name}', v{VERSION_MAJOR}.{VERSION_MINOR})")


if __name__ == "__main__":
    main()
