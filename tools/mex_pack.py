#!/usr/bin/env python3
"""mex_pack.py — упаковщик плоского .bin в .mex (DeiX EXecutable v1.1)

Usage: python3 mex_pack.py <input.bin> <output.mex> [entry_offset] [bss_size]
"""

import struct
import sys
import os

MEX_MAGIC = 0x3158454D  # "MEX1" LE
HEADER_SIZE = 32
VERSION_MAJOR = 1
VERSION_MINOR = 1


def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <input.bin> <output.mex> [entry_offset] [bss_size]")
        sys.exit(1)

    input_path = sys.argv[1]
    output_path = sys.argv[2]
    entry_offset = int(sys.argv[3]) if len(sys.argv) > 3 else 0
    bss_size = int(sys.argv[4]) if len(sys.argv) > 4 else 0

    with open(input_path, "rb") as f:
        body = f.read()

    body_size = len(body)

    header = struct.pack(
        "<IHHIIIIQ",
        MEX_MAGIC,          # magic
        VERSION_MAJOR,      # version_major
        VERSION_MINOR,      # version_minor
        HEADER_SIZE,        # header_size
        entry_offset,       # entry_offset
        body_size,          # body_size
        bss_size,           # bss_size
        0,                  # reserved
    )

    assert len(header) == HEADER_SIZE, f"Header size mismatch: {len(header)} != {HEADER_SIZE}"

    with open(output_path, "wb") as f:
        f.write(header)
        f.write(body)

    print(f"Packed: {input_path} ({body_size} bytes) -> {output_path} (v{VERSION_MAJOR}.{VERSION_MINOR})")


if __name__ == "__main__":
    main()
