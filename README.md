# 🪐 DeiX OS (0.3.0-beta · x86_64)

A modern, fast, and secure 64-bit operating system written from scratch in **Rust** and **Assembly** for the `x86_64` architecture. Small core, immutable system partition, writable user space.

A next-generation OS for `x86_64` with a modular graphical shell, an immutable **EROFS** system partition (`/system`), and an **ext2/4** user space (`/userdata`).

**Kernel:** ~0.9 MB (Rust `#![no_std]` + NASM stage2) · **Kernel limit:** 1 MiB · **Disk image:** 20 MiB · **Code:** ~32,000 lines of Rust · runs on real hardware and QEMU.

---

## 🏛️ System architecture

```text
Disk
│
├── boot          MBR + stage2 loader
├── /system       EROFS RO (kernel, modules, libraries)
└── /userdata     EXT2/4 RW (users, configs, packages)
```

```text
                    DeiX OS
                       │
                ┌──────┴──────┐
                │   Kernel    │
                └──────┬──────┘
                       │
       ┌───────────────┼────────────────┐
       │               │                │
     Memory          Scheduler         VFS
       │               │                │
       └───────────────┼────────────────┘
                       │
                  Process Manager
                       │
          ┌────────────┼────────────┐
          │            │            │
        Dinit        Syscalls     Loaders
                                      │
                                 ┌────┴────┐
                               ELF        MEX
```

- **Immutable kernel and subsystems (`/system`)**: the system partition is mounted read-only with integrity verification.
- **User data (`/userdata`)**: user database (`/userdata/users.db`), user files and configuration.
- **DeiX Fluent graphical compositor**: event-driven rendering, surface caching, damage tracking, and an adaptive UI.
- **DUIL declarative GUI framework & DeiX Script**: compact tools for describing graphical interfaces and scripting.

### 🗂️ VFS — unified file system (`src/vfs.rs`)

All kernel subsystems access files through the VFS rather than talking to
partition drivers directly. One mount point, one permission check, one set
of error codes.

```text
VFS
├── /system    → EROFS, read-only
└── /userdata  → EXT2,  read/write
```

API: `read_file` / `write_file` / `mkdir` / `remove` / `stat` / `readdir` /
`exists`. Writing to `/system` returns `ReadOnly` without ever reaching the
disk. The `/system` image is read from disk once and cached in the heap.

### 🧬 Process manager (`src/process.rs`)

A single structure for everything that executes:

```text
Process
 ├── PID / UID / parent
 ├── AddressSpace (image, stack, heap)
 ├── file descriptor table + cwd
 ├── capabilities
 └── scheduler slot
```

`spawn()` creates the process record, `exec()` loads the image and creates a
scheduler task bound to the PID. Thread termination is reported back through
`notify_exit`, so a dead thread never leaves its process stuck in Running.

**MEX executes in Ring 3.** The program receives a pointer to the MexApi
table in RDI, but the pointers inside it do not point into the kernel — they
point to stubs on a shim page inside the program's region: every API call is
a `syscall` with number `0x4D58xxxx`, which the kernel dispatches back to the
real implementation. Pointers passed by the program are checked to lie within
its region; a program crash (page fault, invalid instruction) terminates the
process and is recorded in the log — the kernel keeps running.

**MEX format limitation.** `tools/mex.ld` links `.mex` binaries to the fixed
address `0x03000000`, and the image is not position-independent. A MEX
program is therefore loaded strictly at its link address, and running more
than one MEX process at a time is forbidden (`MEX_IN_USE`) — per-process
isolation for this format will only arrive together with PIC builds or true
per-process page mapping.

### 🚨 Unified error and diagnostics system (`src/diag/`)

```text
event (any subsystem, including pre-GUI)
   ↓
ring buffer (512 records, .bss, no heap on the critical path)
   ├── serial console (machine-readable key=value lines)
   ├── text console (during boot only)
   └── /userdata/log/{system,error,boot,dinit,kmod,panic}.log (deferred)
```

- **`DX-<SUBSYSTEM>-<NNNN>` codes** — a registry of 100+ codes with cause
  descriptions and recommended actions (`error show DX-NET-0007`).
- **Severity scale** TRACE…FATAL; TRACE/DEBUG are enabled with
  `diagnostics mode on`.
- **Kernel failure**: a report (CPU state, process, modules, mount points,
  RBP backtrace, last 256 events) is written as raw sectors at LBA 2048 —
  independent of file system state — and shown after reboot
  (the «ПРЕДЫДУЩИЙ СБОЙ СИСТЕМЫ» dialog, `panic last`).
- **Ring 3 cannot take down the kernel**: a user program exception
  terminates the process, is recorded with a code (e.g. `DX-MEM-0008`), and
  shows an APPLICATION CRASH dialog; the system keeps running.
- **Module quarantine**: if the previous session's failure happened inside a
  `.kmod` region, that module is not loaded on the next boot (`DX-KMD-0005`).
- **Aggregation**: a repeated identical failure within a 2 s window bumps the
  record's counter instead of adding new lines.
- **Cause chains**: an EXT2 error following an ATA failure references the
  root-cause record (`DX-DISK-*` → `DX-EXT-0006`).
- **Error Center** — a GUI application (Start menu): counters, a list, and a
  card with a copyable identifier like `DX-GFX-0008-20261006-083154`
  (date comes from the CMOS RTC driver, `src/rtc.rs`).

### 📜 DeiX Script language (`src/ds.rs`)
- A full interpreter with `if`/`else`/`fi` branching, `while`/`done` loops, and `fn`/`end` functions.
- Variable substitution (`$var`), system variables (`PATH`, `HOME`, `SHELL`), arithmetic (`expr $a + $b`), and comparison operators (`==`, `!=`, `<`, `>`, `<=`, `>=`).
- ext2 file system commands (`cat`, `write`, `append`, `mkdir`, `rm`, `ls`, `cd`, `pwd`) and an interactive REPL (`ds -i`).

### 🛠️ Freestanding C/C++ SDK MEX v1.2
- The `tools/include/deix/mex.h` header includes basic memory functions (`memset`, `memcpy`, `memmove`, `memcmp`) with the `weak` attribute for building C/C++ programs without `glibc`.
- The `mexcc.cpp` driver supports cascading resource lookup via `locate_resource()` (`mex.ld`, `mex_pack.py`, `include/`).

### 🏛️ Contents of the `/system` partition (EROFS, read-only)
- `kernel/kernel.bin` — kernel image.
- `etc/init.deix` — init script.
- `services/dinit.cfg` — Dinit service configuration.
- `media/audio/ui/*.dps` — shell sound assets.

Only real files go into the image. Placeholder `libdeix_*.so` and
`kmod/*.kmod` files are not created: `module.rs` expects a `.kmod` to have a
valid header and an executable body, so placeholder bytes could only ever
load as a corrupted module. A module that does not exist is not in the image.

Kernel modules (`.kmod`) are a separate mechanism: they are loaded from disk
through `module.rs` and show up in `lsmod`. Built-in kernel subsystems are
not loaded modules and do not appear in that list.

### ⚡ Boot speed optimizations
- `Ramboot`: the INT 13h transfer block size was increased to 127 sectors (half as many BIOS calls and mode switches, ~40% faster reads).
- `Bootchain`: direct-load reads of EROFS files (`/system/kernel/kernel.bin`) straight into memory with no intermediate unpacking.

### 🧪 End-to-end test suite (`tools/test_all_subsystems.py`)
- An automated script testing the partition map and C++ application cross-compilation.

---

## ⚡ Core kernel features

- **🦀 Pure Rust `#![no_std]`**, long-mode boot: `boot_sector` (MBR) → `stage2` (32-bit → long mode) → `kernel.bin` @ 0x100000.
- **🛡 Dinit (PID 1, Ring 0)** — supervisor for services, mount points, and auditing.
- **🚨 Security Monitor** — heuristic monitor for suspicious file activity (experimental).
- **🔊 Intel HDA + PC Speaker** — DMA 48 kHz / 16-bit stereo playback + PC Speaker PWM.
- **🖼️ DXLG & VBE** — VBE 32bpp graphics with a Cyrillic font.
- **🛡 Ring 3 + syscall/sysret** — hardware isolation for MEX programs (GDT + TSS); all other subsystems run in Ring 0.
- **🧵 Preemptive multitasking** — scheduler driven by the IRQ0 PIT interrupt (`threads list/test`).
- **🧾 Real EROFS** (magic `0xE0F5E1E2`, validated by `fsck.erofs`) on the `/system` partition.
- **🐧 Linux compatibility groundwork** — a parser/validator for static ELF64 and a subset of Linux ABI syscalls (`src/linux/`). Launching ELF binaries as processes is disabled in this release: the scheduler still executes code in Ring 0 only, and calling that a user process would be dishonest (`DX-ELF-0009`).
- **🌐 Networking** — RTL8139 + ARP + IPv4 + ICMP + UDP + TCP; a real DNS
  client (A records over UDP/53, TTL-aware cache) and an HTTP/1.1 client:
  redirects, Transfer-Encoding: chunked, gzip/DEFLATE decompression
  (RFC 1951/1952) with CRC-32 verification (`ifconfig`, `ping`, `dns`,
  GUI browser).
- **🔐 TLS 1.3** — our own client (RFC 8446): X25519 + HKDF-SHA256 +
  AES-128-GCM, written from scratch and checked against NIST/RFC test
  vectors; the browser opens `https://` sites. **Server certificates are
  verified**: X.509/DER parsing, RSA (PKCS#1 v1.5, PSS) and ECDSA
  (P-256/P-384) signatures, chain validation up to built-in roots
  (23 current CAs from the Mozilla bundle), validity dates via RTC,
  hostname via SAN, CertificateVerify signature.
- **📦 MEX applications** — DeiX's own binary format (`run`, `pkg`, `mexcc`).

---

## 🗂️ Partition map

| Partition | LBA | Sectors | FS | Purpose |
|---|---|---|---|---|
| `/system` | 4096 | 32768 | EROFS ro | System partition (kernel `kernel.bin`, libraries, `.kmod` modules, services) |
| `/userdata` | 36864 | 32768 | ext2 rw | User data, programs, packages |

Service areas outside partitions: LBA 0 — MBR; 1..2046 — reserved (stage2, ramboot and the kernel are files in `/system/boot` and `/system/kernel`);
2047 — `DEIXKIMG` boot map (extents of those files + kernel SHA-256, see `docs/BOOTMAP.md`);
2048..2111 — crash dump area (`DEIXPNIC`). All geometry sources are cross-checked by
`tools/check_partition_map.py` (runs in `build.sh` and CI).

---

## 💻 CLI commands

```
help about echo clear uptime color cpuid mem lang
ifconfig [ip|dns <ip>] arp ping dns <host> gpu [info|nvinfo|mode] sound [list|play|beep|mode|hda]
dinit [status|services|mounts|users|audit|security|stage|reload]
duil [run|calc] ds [script.dxs|-i|-c] taskmgr ls cat write mkdir rm pkg run install
useradd passwd whoami users encrypt crypt nvidia hal microcode logo linux profile lock
threads crash bugreport dmesg crashlog dev root reboot poweroff shutdown halt
ps kill kmod [load|list] lsmod
log [list|errors|warnings|critical|kernel|boot|dinit|kmod|flush|clear]
error [list|show DX-XXX-NNNN] panic [last|clear]
diagnostics [summary|mode on|mode off]
```

---

## ⚠️ Known limitations (v0.3.0-beta)

- Single CPU (no SMP); disks are ATA PIO only (no AHCI/NVMe), reads are slow (~1.4 ms/KiB in QEMU).
- Ring 3 is available to MEX programs only; one MEX process at a time
  (the format links to the fixed address `0x03000000`).
- Launching ELF processes is disabled — the loader and a subset of Linux
  syscalls exist, but the scheduler would execute them in Ring 0.
- Process `AddressSpace` is a bookkeeping structure, not full page-table isolation.
- The physical memory allocator is a bitmap (not buddy).
- `getrandom` is not a cryptographic RNG (jitter/rdtsc entropy with no strict guarantees).
- Disk encryption is a custom "LUKS-inspired" format (XTS-AES-256), incompatible with LUKS.
- The browser is a limited text-mode HTTP/HTTPS client. TLS 1.3 only
  (servers without it get an honest error). Certificates are verified
  against a chain up to 23 built-in Mozilla roots, but WITHOUT revocation
  (OCSP/CRL) or Certificate Transparency; sites whose root is outside the
  set get an honest error. HTTP/1.1 is GET-only with Connection: close
  (no keep-alive); the response body is capped at 1 MiB (2 MiB after gzip
  decompression) — a limit of the 16-MiB kernel heap; beyond that the page
  is honestly truncated. Rendering is text-only: script/style contents are
  skipped and HTML entities are decoded, but there is no CSS layout, no
  JavaScript, and no images.
- DNS — A records over UDP/53 only: no IPv6 (AAAA), DNSSEC, or TCP fallback
  for responses over 512 bytes; no IP fragment reassembly.
- NVIDIA: PCI hardware detection and info output, not an acceleration driver.
- No USB, no ACPI power management; audio is PC Speaker plus basic Intel HDA (tones/PCM).

---

## 🚀 Quick start

### Build
Requirements: `nasm`, `rustup nightly` with the `x86_64-unknown-none` target, `python3`, `qemu-system-x86_64`.

```bash
git clone https://github.com/Atimenka/DeiX.git
cd DeiX
./build.sh
```

### Run in QEMU

```bash
# Standard run with graphics and Intel HDA audio:
./run_full.sh

# Quick run (VGA):
./run.sh

# Boot from the ISO image:
./run_iso.sh
```

```bash
# Manual run with HDA audio and an RTL8139 network card:
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
  -m 512M -serial stdio \
  -netdev user,id=n0 -device rtl8139,netdev=n0 \
  -device intel-hda -device hda-duplex \
  -audiodev pa,id=snd0
```

---

## 🧪 Testing

```bash
python3 tools/test_all_subsystems.py    # end-to-end test of all subsystems and the toolchain
python3 tools/check_partition_map.py    # partition map consistency check (2 partitions)
python3 tools/make_logo.py assets/logo.png build/logo.dxlg 128
python3 tools/wav2dps.py assets/start.wav build/sounds/start.dps 8000
python3 tools/make_deix_fs.py build/deix_disk.img   # factory image build
```

---

Developed by **Atimenka** & AI. Released under the **GPL-3.0** License.
