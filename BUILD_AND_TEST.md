# DeiX 0.3.0-beta — build and test

This document describes the current state of the branch. Everything listed
here matches the code; known limitations are collected at the end.

## 1. Dependencies

```bash
# Rust nightly + bare-metal target (nightly is required: abi_x86_interrupt,
# alloc_error_handler, naked-asm)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
    -y --default-toolchain nightly --profile minimal
source "$HOME/.cargo/env"
rustup target add x86_64-unknown-none

# Assembler, emulator, EROFS utilities
sudo pacman -S --needed nasm qemu-system-x86 erofs-utils   # Arch
# sudo apt install -y nasm qemu-system-x86 erofs-utils     # Debian/Ubuntu
```

`erofs-utils` is optional: `tools/make_deix_fs.py` can build EROFS itself
in pure Python. With `mkfs.erofs` the image is more compact, and
`fsck.erofs` lets you verify it with a third-party tool.

## 2. Build

```bash
bash build.sh
```

Result: `build/deix_disk.img` (10 MiB) and the release artifacts
`build/deix-0.3.0-beta-x86_64.img` + `build/SHA256SUMS`.

Steps: partition map check (`tools/check_partition_map.py`) -> nasm
stage2/ramboot -> cargo build --release -> objcopy kernel.bin -> MBR ->
rebuild of stage2/ramboot with the real kernel size and LBA -> image
assembly -> partition formatting (`tools/make_deix_fs.py`: EROFS
`/system`, ext2 `/userdata`, `DEIXKIMG` kernel descriptor).

Kernel only, no image: `cargo build --release`.

## 3. Disk layout

| Area | LBA | Sectors | Contents |
|---|---|---|---|
| MBR | 0 | 1 | boot sector + partition table |
| boot | 1..2046 | ≤2046 | stage2 + ramboot + raw kernel.bin |
| `DEIXKIMG` | 2047 | 1 | kernel descriptor: LBA, size, SHA-256 |
| panic dump | 2048 | 64 | crash dump (`DEIXPNIC`), outside any FS |
| `/system` | 4096 | 8704 | EROFS, read-only |
| `/userdata` | 12800 | 5632 | ext2, read/write |

Booting uses the raw kernel (MBR -> stage2 -> kernel.bin at LBA 1..2046);
`/system/kernel/kernel.bin` is a copy of the same file inside EROFS. After
startup the kernel compares both copies by size and SHA-256 via the
`DEIXKIMG` descriptor; a mismatch is recorded as `DX-KRN-0013`.

The single source of truth for geometry is `src/partition_map.rs`.
`tools/check_partition_map.py` checks `tools/make_deix_fs.py`,
`src/install.rs`, `src/ext2.rs`, `boot/boot_sector.asm`, and
`src/diag/panic.rs` against it; it runs in `build.sh` and CI.

## 4. Running

`./run.sh` and `./run_debug.sh` already enable networking (slirp + RTL8139)
and audio. Manually:

```bash
# Headless, all output to the terminal
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
    -m 256M -display none -serial stdio -no-reboot

# With a VGA window and networking (RTL8139)
qemu-system-x86_64 -drive file=build/deix_disk.img,format=raw,if=ide \
    -m 256M -serial stdio -netdev user,id=n0 -device rtl8139,netdev=n0
```

On first boot the system asks you to create an account (which is also the
XTS-AES-256 disk encryption key). After that — the CLI; `help` lists the
commands.

Successful-boot marker in the serial log:
`=== DeiX boot: serial debug active ===`, then
`DeiX 0.3.0-beta x86_64 - mini kernel booted successfully!`.

## 5. Tests

```bash
python3 tools/check_partition_map.py    # partition map check across all sources
python3 tools/test_all_subsystems.py    # partition map + MEX toolchain (C/C++)
python3 tools/qemu_scenario_reboot.py   # factory -> login -> reboot x2 (persistence)
python3 tools/qemu_install_test.py      # install onto a second disk + boot from it
```

Verifying EROFS with a third-party tool:

```bash
dd if=build/deix_disk.img of=/tmp/system.img bs=512 skip=4096 count=8704
dump.erofs /tmp/system.img | head -5     # -> Filesystem magic number: 0xE0F5E1E2
fsck.erofs --extract=/tmp/out /tmp/system.img && ls -R /tmp/out
```

CLI commands for manual checks:

| Command | What it does |
|---|---|
| `threads list` | scheduler tasks and the context-switch counter |
| `dinit status` | services: built-ins (Ring 0) and processes |
| `ifconfig` / `ping` | networking (needs `-device rtl8139`) |
| `dns <host>` | a real DNS query (A record over UDP/53) |
| `gpu info` | GPU detection |
| `error list` / `panic last` | diagnostics log and the last crash dump |
| `install` | install the system onto a second disk (ATA Slave) |

### Internet in QEMU (user-mode networking, slirp)

The guest gets a fixed configuration: IP `10.0.2.15/24`, gateway
`10.0.2.2`, DNS `10.0.2.3` (defaults in `src/net/mod.rs`, changeable with
`ifconfig <ip>` / `ifconfig dns <ip>`). Checking real internet access:

```
dns example.com          # A record through the host's slirp resolver
ping 10.0.2.2            # ICMP to the slirp gateway
gpu mode                 # then open https://example.com in the browser
```

The browser (GUI, "Web Browser") resolves names via DNS, opens a TCP
connection, and performs an HTTP/1.1 GET with redirect following, chunked
decoding, and gzip decompression (`src/net/inflate.rs`); `https://` works
through our own TLS 1.3 client (`src/net/tls.rs`) with certificate
verification (`src/net/x509.rs`: chain up to 23 Mozilla roots, validity
dates via RTC, SAN hostname, CertificateVerify). Honest limits: TLS 1.3
only (no 1.2), certificate revocation (OCSP/CRL) is not checked. To test:
`https://example.com`, `https://www.google.com` (response body capped at
1 MiB, 2 MiB after gzip decompression; beyond that the page is honestly
truncated). Signature verification on an emulated CPU takes noticeable
time — an RSA-4096 and two ECDSA checks can add 1-3 seconds to page load.
Outbound ICMP does not pass through slirp — `ping 8.8.8.8` will not get a
reply; that is a QEMU limitation, not the network stack's.

## 6. Memory map (important when making changes)

```
0x00100000  kernel code+data (up to 1 MiB — KERNEL_SECTORS=2048 limit)
0x00300000  kernel .bss (16 MiB heap, ends ~0x1360000)
0x01400000  Ring 3 (user program code and stack)
0x03000000  MEX program region (Ring 3, fixed link address)
```

Do not place anything inside the `.bss` range — an early version of Ring 3
wrote its code right over the kernel heap.

## 7. UI sounds (PC speaker)

Effects live in `assets/*.wav` and are not baked into kernel.bin:

1. **build.sh [2e/8]** — `tools/wav2dps.py` converts WAV to DPS1 (8 kHz, u8, mono).
2. **make_deix_fs.py** — puts `*.dps` into EROFS as `/system/media/audio/ui/`.
3. **Kernel (src/sound.rs)** — reads DPS through the VFS (the `/system`
   image is cached) and plays it via PWM on the PC speaker.

```
sound            # list of effects, [ok]/[--] — whether the file is in the image
sound start      # play (start error lowbat fullbat usbcon usbdisc)
sound beep 880 200
```

PWM playback is blocking (effects are capped at 10 s); on real hardware
port writes are slower, so sound is roughly half the pitch/twice the
length compared to QEMU.

## 8. Known limitations

- Single CPU; disks are ATA PIO only (~1.4 ms/KiB in QEMU) — avoid large
  reads on interactive paths.
- Ring 3 is MEX-only; one MEX process at a time (fixed link address
  `0x03000000`).
- Launching ELF processes is disabled (`DX-ELF-0009`): the loader and a
  subset of Linux syscalls exist, but execution would happen in Ring 0.
- `AddressSpace` is a bookkeeping structure, with no per-process page
  tables (except the MEX region).
- The physical page allocator is a bitmap, not buddy.
- `getrandom` is not a cryptographic RNG.
- Disk encryption is a custom "LUKS-inspired" format (XTS-AES-256),
  incompatible with LUKS tooling.
- DNS — A records over UDP/53 only (no AAAA/DNSSEC/TCP fallback); the IP
  stack does not reassemble fragmented packets.
- HTTP client — HTTP/1.1, GET only + Connection: close (no keep-alive);
  chunked and gzip are supported. https:// uses TLS 1.3 with X.509 chain
  verification up to 23 built-in Mozilla roots (no OCSP/CRL or Certificate
  Transparency); TLS 1.2 is not supported.
- No USB, SMP, AHCI/NVMe, or ACPI power management.

Key files: `src/partition_map.rs` (geometry), `src/bootchain.rs`
(kernel image cross-check), `src/vfs.rs` (unified FS), `src/diag/` (errors
and crash dumps), `src/dinit/` (PID 1), `tools/make_deix_fs.py` (image).
