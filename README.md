# 🪐 DeiX OS (v0.2-dev)

A modern, highly-secure 64-bit Operating System written from scratch in **Rust** and **Assembly** for `x86_64` architecture. Developed in a unique collaboration between a Human Architect and an AI Agent.

Операционная система нового поколения, написанная с нуля на **Rust** и **Ассемблере** под архитектуру `x86_64`. Разработана в уникальном тандеме Человека-Архитектора и ИИ-Агента.

---

## ⚡ What's New in v0.2

### 🧠 Ring 3 User Mode + Syscalls
Hardware isolation via GDT/TSS + `syscall`/`sysret`. Programs run in unprivileged ring 3 with 11 system calls (`print`, `read_char`, `uptime_ms`, `ping`, `get_mac`, `get_ip`, `open`, `read`, `write`, `close`).

### 📦 Module System (`.kmod`)
Kernel modules loaded from ext2 disk at boot: `NET.KMOD`, `CRYPTO.KMOD`, `GFX.KMOD`. Module API (KernelApi): print, alloc, files, PCI, ports, timer. No more monolithic binary.

### 🌐 MEX API v1.1 — Network Functions
Network access from `.mex` programs: `ping()`, `get_mac()`, `get_ip()`, `arp_resolve()`. Fully backward-compatible with v1.0.

### 💬 DeiX Script (DS) — Shell Scripting Language
Built-in interpreter with variables, conditionals (`if/else`), loops (`for/while`), commands. Used for `AUTOSTART.CFG`, `DXINIT.CFG`, and `*.dxs` scripts. Interactive REPL via `ds -i`.

### 🎨 DUIL — DeiX UI Language
Declarative XML-like markup for GUI: `<window>`, `<button>`, `<label>`, `<terminal>`, `<vbox>`, `<hbox>`.

### 🚀 dxinit — Boot Manager
Systemd-like init system. Services in stages (`early` → `middle` → `late`). Dependency resolution, auto-restart. Config: `/etc/DXINIT.CFG`.

### 🎮 NVIDIA Open 3D Driver
Based on nouveau documentation (freedesktop.org). Supports NV04–NV40 PGRAPH 3D engine with triangle rendering. Requires Falcon firmware for NV50+ (available at freedesktop.org/nouveau/linux-firmware).

### 🖥️ ELF Loader + glibc Compatibility
Loads static ELF64 binaries (glibc `--static` / musl). New address space per process, user stack, brk heap.

### 🗂️ Memory Manager
Buddy allocator (4K page bitmap), 4-level x86_64 page tables, virtual memory allocation, map/unmap, TLB management.

### 🔌 New Drivers (skeleton)
- **xHCI USB 3.0**: PCI probe, MMIO registers, reset/start, DCBAA, Command Ring
- **HD Audio**: CORB/RIRB, widget graph, PCM setup
- **DisplayPort/HDMI**: AUX channel, DPCD, link training, EDID, DDC/I2C
- **Initrd**: Early filesystem for pre-ATA module loading

---

## ⚡ Core Features (v0.1)

### 🦀 Pure Rust Kernel (`#![no_std]`)
- **Long Mode:** boots directly into 64-bit mode
- **Safe Memory:** heap allocator + page tables under Rust's ownership rules
- **Микроядерная база:** memory management + component isolation

### 🎨 Procedural Desktop
- Live-animated diagonal background lines (procedural)
- Z-Order Window Manager with drag-and-drop
- Dynamic resolution up to 1280x1024 (Bochs VBE)

### 🔐 Ironclad Security
- **Ring 3 isolation:** GDT, TSS, `syscall` hardware switching
- **Salted SHA-256 hashing** for user passwords (`USERS.DB`)
- **AES-256-XTS full-disk encryption** (compatible with Linux `cryptsetup`)

### 🌐 Network & Localization
- RTL8139 NIC driver + IPv4/ARP/ICMP stack
- IEEE 802.11 Wi-Fi protocol + WPA2-PSK handshake
- Hardware Cyrillic font in VGA Plane 2
- Russian/English UI localization (`lang ru`/`lang en`)

---

## 📂 Project Structure

```
DeiX/
├── boot/              # NASM bootloader (MBR + long mode)
├── src/               # Rust kernel source (~17,000 lines)
│   ├── lib.rs         # Entry point (kernel_main)
│   ├── cli.rs         # Command-line interface (~30 commands)
│   ├── mm/            # Memory manager (phys + virt)
│   ├── crypto/        # AES, SHA-256/512, HMAC-SHA1, PBKDF2, XTS
│   ├── net/           # Ethernet, ARP, IPv4, ICMP
│   ├── wifi/          # IEEE 802.11, WPA2-PSK, EAPOL
│   ├── ui/            # Window manager + desktop renderer
│   ├── ds.rs          # DeiX Script interpreter
│   ├── duil.rs        # DUIL UI language parser
│   ├── dxinit.rs      # Boot manager (systemd-like)
│   ├── elf.rs         # ELF64 static loader
│   ├── usermode.rs    # Ring 3 + syscall/sysret
│   ├── module.rs      # .kmod module loader
│   ├── nv3d.rs        # NVIDIA open 3D driver
│   ├── xhci.rs        # USB 3.0 xHCI driver
│   ├── hda.rs         # HD Audio driver
│   ├── dp.rs          # DisplayPort/HDMI driver
│   ├── initrd.rs      # Initial RAM disk
│   └── autostart.rs   # Autostart script runner
├── tools/             # Build tools
│   ├── mexcc.py       # Cross-compiler (C/C++/Rust → .mex)
│   ├── mex_pack.py    # .bin → .mex packer
│   ├── kmod_pack.py   # .bin → .kmod packer
│   ├── hello.asm      # Demo .mex program
│   ├── sysinfo.asm    # System info .mex
│   └── netping.asm    # Network ping .mex
├── docs/              # Documentation
│   └── MEX_FORMAT.md  # .mex format specification (v1.1)
├── build.sh           # Build script (NASM + cargo + ld)
├── run.sh             # QEMU launch script
└── run_iso.sh         # ISO boot script
```

---

## 💻 CLI Commands

```
help     about    echo      clear     uptime    color     cpuid
mem      lang     ifconfig  arp       ping      wifi      gpu
ls       cat      write     rm        pkg       run       install
bigfile  useradd  passwd    whoami    users     encrypt   modules
nv3d     ds       duil      dxinit    crash     reboot    halt
```

### New in v0.2:
| Command | Description |
|---------|-------------|
| `nv3d info/demo` | NVIDIA 3D driver info & demo |
| `ds <file>/-c/-i` | DeiX Script interpreter |
| `duil <file>` | Render DUIL layout |
| `dxinit boot/status` | Boot manager control |
| `modules` | List loaded kernel modules |

---

## 🚀 Quick Start

```bash
cd Deix/
chmod +x build.sh run.sh
./build.sh          # Build everything
./run.sh            # Launch in QEMU
```

```bash
# Or with network + large memory:
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw \
  -m 512M -net nic,model=rtl8139 -net user
```

---

## 🔧 Cross-Compilation (mexcc)

```bash
# Generate Rust project scaffold:
python3 tools/mexcc.py cargo

# Generate CMake project for C/C++:
python3 tools/mexcc.py cmake

# Compile C to .mex:
python3 tools/mexcc.py build hello.c

# Compile ASM to .mex:
python3 tools/mexcc.py build program.asm
```

---

## 🗺️ Roadmap

- [x] Ring 3 user mode + syscall/sysret
- [x] Module system (.kmod)
- [x] MEX API v1.1 (network functions)
- [x] NVIDIA open 3D driver (NV04–NV40)
- [x] ELF static loader (glibc compat)
- [x] Memory manager (buddy + page tables)
- [x] DeiX Script language (DS)
- [x] DUIL UI language
- [x] dxinit boot manager
- [x] xHCI USB 3.0 / HD Audio / DP-HDMI skeleton
- [ ] TCP/IP stack (currently Ethernet+ARP+IPv4+ICMP only)
- [ ] USB device enumeration + HID driver
- [ ] NVIDIA firmware loader (Falcon) + NV50+ 3D
- [ ] ext4 / FAT32 filesystem support
- [ ] SMP (multi-core)

---

Developed by **Atimenka** & AI. Released under the GPL-3.0 License.
