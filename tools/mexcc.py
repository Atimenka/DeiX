#!/usr/bin/env python3
"""
mexcc — кросс-компилятор для превращения C/C++/Rust в .mex (DeiX Executable).

ИСПОЛЬЗОВАНИЕ:
  python3 mexcc.py build input.c       — скомпилировать C в .mex
  python3 mexcc.py build input.cpp     — скомпилировать C++ в .mex
  python3 mexcc.py build input.rs      — скомпилировать Rust в .mex
  python3 mexcc.py build input.asm     — ассемблировать .asm в .mex
  python3 mexcc.py cmake               — сгенерировать CMakeLists.txt
  python3 mexcc.py cargo               — сгенерировать Cargo.toml для .mex
  python3 mexcc.py toolchain           — показать toolchain для интеграции

Архитектура:
  C/C++ -> gcc/clang -static -nostdlib -> .bin -> mex_pack -> .mex
  Rust  -> cargo build --target x86_64-unknown-deix -> .bin -> mex_pack -> .mex
  ASM   -> nasm -f bin -> .bin -> mex_pack -> .mex

ТРЕБОВАНИЯ (на хосте):
  - Для C:   x86_64-elf-gcc или clang --target=x86_64-unknown-none
  - Для C++: x86_64-elf-g++ или clang++
  - Для Rust: cargo + rustc с target x86_64-unknown-deix (см. gen-target)
  - Для ASM:  nasm
"""

import os, sys, struct, subprocess, shutil

MEX_MAGIC = 0x314D4558  # "MEX1" LE
HEADER_SIZE = 32
VERSION_MAJOR = 1
VERSION_MINOR = 1

MEX_LOAD_ADDR = 0x600000  # фиксированный адрес загрузки DeiX

def pack_mex(bin_path: str, mex_path: str, entry_offset: int = 0, bss_size: int = 0):
    with open(bin_path, "rb") as f:
        body = f.read()
    header = struct.pack("<IHHIIIIQ",
        MEX_MAGIC, VERSION_MAJOR, VERSION_MINOR,
        HEADER_SIZE, entry_offset, len(body), bss_size, 0)
    with open(mex_path, "wb") as f:
        f.write(header)
        f.write(body)
    print(f"  PACKED: {mex_path} ({len(body)} bytes code)")

def link_flat(obj_path: str, bin_path: str):
    """Линкует объектный файл в плоский бинарник для загрузки по MEX_LOAD_ADDR."""
    ld_script = f"""
SECTIONS {{
    . = {hex(MEX_LOAD_ADDR)};
    .text : {{ *(.text*) }}
    .rodata : {{ *(.rodata*) }}
    .data : {{ *(.data*) }}
    .bss : {{ *(.bss*) }}
}}
"""
    script_path = "/tmp/mexcc_ld.ld"
    with open(script_path, "w") as f:
        f.write(ld_script)
    subprocess.run(["ld", "-T", script_path, "-o", bin_path.replace(".bin", ".elf"), obj_path,
                    "--oformat=binary"], check=True)
    # ld with --oformat=binary outputs directly to the output file
    # Actually we need objcopy:
    subprocess.run(["ld", "-T", script_path, "-o", bin_path.replace(".bin", ".elf"), obj_path], check=True)
    subprocess.run(["objcopy", "-O", "binary", bin_path.replace(".bin", ".elf"), bin_path], check=True)
    print(f"  LINKED: {bin_path}")

def build_c(input_path: str, output_path: str):
    """C -> .o -> flat .bin -> .mex"""
    obj_path = output_path.replace(".mex", ".o")
    bin_path = output_path.replace(".mex", ".bin")

    cc = os.environ.get("MEX_CC", "x86_64-elf-gcc")
    cflags = [
        cc, "-c", input_path, "-o", obj_path,
        "-ffreestanding", "-nostdlib", "-nostdinc",
        "-mno-red-zone", "-mcmodel=large",
        "-fno-pic", "-fno-pie",
        "-O2", "-Wall",
    ]
    print(f"  CC: {' '.join(cflags)}")
    subprocess.run(cflags, check=True)

    link_flat(obj_path, bin_path)
    pack_mex(bin_path, output_path)

def build_cpp(input_path: str, output_path: str):
    """C++ -> .o -> flat .bin -> .mex"""
    obj_path = output_path.replace(".mex", ".o")
    bin_path = output_path.replace(".mex", ".bin")

    cxx = os.environ.get("MEX_CXX", "x86_64-elf-g++")
    cflags = [
        cxx, "-c", input_path, "-o", obj_path,
        "-ffreestanding", "-nostdlib", "-nostdinc",
        "-mno-red-zone", "-mcmodel=large",
        "-fno-pic", "-fno-pie", "-fno-rtti", "-fno-exceptions",
        "-O2", "-Wall",
    ]
    print(f"  CXX: {' '.join(cflags)}")
    subprocess.run(cflags, check=True)

    link_flat(obj_path, bin_path)
    pack_mex(bin_path, output_path)

def build_rust(input_path: str, output_path: str):
    """Rust -> cargo build -> .bin -> .mex"""
    # Requires: rustup target add x86_64-unknown-none
    bin_path = output_path.replace(".mex", ".bin")

    # Build using cargo with our target
    subprocess.run([
        "cargo", "+nightly", "build", "--release",
        "--target", "x86_64-unknown-deix",
    ], check=True, cwd=os.path.dirname(input_path) or ".")

    # Find the output binary
    # ... simplistic path
    pack_mex(bin_path, output_path)

def build_asm(input_path: str, output_path: str):
    """NASM -> flat .bin -> .mex"""
    bin_path = output_path.replace(".mex", ".bin")
    subprocess.run(["nasm", "-f", "bin", input_path, "-o", bin_path], check=True)
    print(f"  NASM: {input_path} -> {bin_path}")
    pack_mex(bin_path, output_path)

def gen_cmake():
    """Генерирует CMakeLists.txt для сборки C/C++ .mex через cmake."""
    cmake_content = """cmake_minimum_required(VERSION 3.16)
project(DeiX-Program LANGUAGES C ASM_NASM)

set(CMAKE_C_COMPILER x86_64-elf-gcc)
set(CMAKE_CXX_COMPILER x86_64-elf-g++)
set(CMAKE_ASM_NASM_COMPILER nasm)

set(MEX_LOAD_ADDR 0x600000)

set(CMAKE_C_FLAGS "-ffreestanding -nostdlib -mno-red-zone -mcmodel=large -fno-pic -O2")
set(CMAKE_CXX_FLAGS "-ffreestanding -nostdlib -mno-red-zone -mcmodel=large -fno-pic -fno-rtti -fno-exceptions -O2")
set(CMAKE_EXE_LINKER_FLAGS "-T ${CMAKE_SOURCE_DIR}/mex_linker.ld -nostdlib")

add_executable(program.mex program.c)

# Post-build: pack into .mex format
add_custom_command(TARGET program.mex POST_BUILD
    COMMAND python3 ${CMAKE_SOURCE_DIR}/tools/mex_pack.py
        $<TARGET_FILE:program.mex> program.mex
    COMMENT "Packing into .mex format (v1.1)"
)
"""
    with open("CMakeLists.txt", "w") as f:
        f.write(cmake_content)
    # Also write linker script
    ld_script = f"""SECTIONS {{
    . = {hex(MEX_LOAD_ADDR)};
    .text : {{ *(.text*) }}
    .rodata : {{ *(.rodata*) }}
    .data : {{ *(.data*) }}
    .bss : {{ *(.bss*) }}
    /DISCARD/ : {{ *(.comment*) *(.eh_frame*) }}
}}"""
    with open("mex_linker.ld", "w") as f:
        f.write(ld_script)
    print("Generated: CMakeLists.txt + mex_linker.ld")
    print("Usage: cmake -B build && cmake --build build")

def gen_cargo():
    """Генерирует Cargo.toml для Rust .mex-проекта."""
    cargo_toml = """[package]
name = "mex-program"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "program"
path = "src/main.rs"

[profile.release]
panic = "abort"
opt-level = "s"
lto = true
"""
    with open("Cargo.toml", "w") as f:
        f.write(cargo_toml)

    os.makedirs("src", exist_ok=True)
    main_rs = """#![no_std]
#![no_main]

#[repr(C)]
pub struct MexApi {
    pub print: extern "C" fn(*const u8, usize),
    pub read_char: extern "C" fn() -> u8,
    pub try_read_char: extern "C" fn() -> i32,
    pub uptime_ms: extern "C" fn() -> u64,
    pub read_file: extern "C" fn(*const u8, usize, *mut u8, usize) -> i64,
    pub write_file: extern "C" fn(*const u8, usize, *const u8, usize) -> i64,
    pub ping: extern "C" fn(*const u8, usize, u64) -> i64,
    pub get_mac: extern "C" fn(*mut u8) -> i64,
    pub get_ip: extern "C" fn(*mut u8) -> i64,
    pub arp_resolve: extern "C" fn(*const u8, usize, *mut u8) -> i64,
}

#[no_mangle]
pub extern "C" fn mex_main(api: *const MexApi) -> i64 {
    let api = unsafe { &*api };
    let msg = b"Hello from Rust .mex! v1.1\\n";
    (api.print)(msg.as_ptr(), msg.len());
    0
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
"""
    with open("src/main.rs", "w") as f:
        f.write(main_rs)

    # Generate target spec
    target_json = """{
  "llvm-target": "x86_64-unknown-none",
  "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128",
  "arch": "x86_64",
  "target-endian": "little",
  "target-pointer-width": "64",
  "target-c-int-width": "32",
  "os": "none",
  "executables": true,
  "linker-flavor": "ld.lld",
  "linker": "rust-lld",
  "panic-strategy": "abort",
  "disable-redzone": true,
  "features": "-mmx,-sse,+soft-float",
  "position-independent-executables": false,
  "relocation-model": "static"
}"""
    os.makedirs(".cargo", exist_ok=True)
    with open("x86_64-unknown-deix.json", "w") as f:
        f.write(target_json)

    print("Generated: Cargo.toml + src/main.rs + x86_64-unknown-deix.json")
    print("Usage:")
    print("  1. rustup target add x86_64-unknown-none  # for core/alloc")
    print("  2. cargo +nightly build --release -Zbuild-std=core,alloc --target x86_64-unknown-deix.json")
    print("  3. python3 tools/mex_pack.py target/x86_64-unknown-deix/release/mex-program program.mex")

def print_help():
    print("mexcc — DeiX MEX Cross-Compiler")
    print()
    print("COMMANDS:")
    print("  build <file>       — compile C/C++/Rust/ASM to .mex")
    print("  cmake              — generate CMakeLists.txt for C/C++")
    print("  cargo              — generate Cargo.toml for Rust")
    print("  toolchain          — show toolchain info")
    print()
    print("EXAMPLES:")
    print("  python3 mexcc.py build hello.c")
    print("  python3 mexcc.py build app.cpp")
    print("  python3 mexcc.py build program.asm")
    print("  python3 mexcc.py cmake   # then: cmake -B build && cmake --build build")

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print_help()
        sys.exit(0)

    cmd = sys.argv[1]

    if cmd == "build":
        if len(sys.argv) < 3:
            print("Usage: mexcc.py build <file>")
            sys.exit(1)
        src = sys.argv[2]
        base = os.path.splitext(os.path.basename(src))[0]
        out = os.path.join(os.path.dirname(src) or ".", base + ".mex")

        if src.endswith(".c"):
            build_c(src, out)
        elif src.endswith(".cpp") or src.endswith(".cxx") or src.endswith(".cc"):
            build_cpp(src, out)
        elif src.endswith(".rs"):
            build_rust(src, out)
        elif src.endswith(".asm"):
            build_asm(src, out)
        else:
            print(f"Unknown source type: {src}")
            sys.exit(1)

        print(f"\nDone! {out}")

    elif cmd == "cmake":
        gen_cmake()
    elif cmd == "cargo":
        gen_cargo()
    elif cmd == "toolchain":
        print("DeiX MEX Cross-Compilation Toolchain")
        print("=====================================")
        print()
        print("Host:   Linux x86_64 / macOS / Windows (WSL)")
        print("Target: DeiX OS x86_64 (flat binary, no OS dependencies)")
        print()
        print("C:      x86_64-elf-gcc -ffreestanding -nostdlib")
        print("C++:    x86_64-elf-g++ -ffreestanding -nostdlib -fno-rtti -fno-exceptions")
        print("ASM:    nasm -f bin")
        print("Rust:   cargo +nightly --target x86_64-unknown-deix.json")
        print()
        print("Linker: ld -T mex_linker.ld --oformat=binary")
        print("Packer: python3 tools/mex_pack.py")
        print()
        print("Installation:")
        print("  apt-get install gcc-x86-64-linux-gnu nasm binutils")
        print("  rustup target add x86_64-unknown-none")
    else:
        print_help()
