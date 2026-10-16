# DeiX executable file format — `.mex` (DeiX EXecutable)

Specification version: 1.1

## What's new in v1.1

- 4 networking functions added to MexApi:
  - `ping(ip_ptr, ip_len, timeout_ms) -> i64`
  - `get_mac(mac_out) -> i64`
  - `get_ip(ip_out) -> i64`
  - `arp_resolve(ip_ptr, ip_len, mac_out) -> i64`
- Backward compatible with v1.0: old programs keep working
- Header versioning: `version_minor = 1` (was 0)

## Why a custom format instead of ELF/PE

- `.mex` is a flat binary with no relocations, loaded at a fixed address
  and entered as an ordinary function that receives a pointer to the
  system API table. No parser complexity, no dynamic linking.
- The format reflects DeiX's actual loader capabilities: the image is not
  position-independent, so exactly one MEX process may run at a time
  (`MEX_IN_USE`). Per-process isolation for this format will only arrive
  together with PIC builds or true per-process page mapping.

## Execution model

- A program is x86-64 machine code compiled/assembled without
  position-independent code (non-PIC), expecting to be loaded at the fixed
  address `MEX_LOAD_ADDR = 0x03000000` (`tools/mex.ld`).
- The entry point is an ordinary function with this signature
  (System V AMD64 ABI):
  ```c
  int64_t mex_main(const MexApi* api);
  ```
- **The program executes in Ring 3.** It receives a pointer to the MexApi
  table in RDI, but the function pointers inside it do not point into the
  kernel — they point to stubs on a shim page inside the program's region:
  every API call is a `syscall` with number `0x4D58xxxx`, which the kernel
  dispatches back to the real implementation. Pointers passed by the
  program are validated to lie within its region. A program crash
  (page fault, invalid instruction) terminates the process and is logged —
  the kernel keeps running.

## File format (binary, little-endian)

```
Offset     Size     Field             Description
0x00       4        magic             Bytes 'M','E','X','1' (0x3158454D LE)
0x04       2        version_major     Currently 1
0x06       2        version_minor     1 (v1.1), 0 (v1.0)
0x08       4        header_size       Header size in bytes (32)
0x0C       4        entry_offset      Entry point offset from the body start
0x10       4        body_size         Program body size in bytes
0x14       4        bss_size          Extra memory zeroed after the body
0x18       8        reserved          Reserved (0)
0x20       ...      body              Machine code + data (body_size bytes)
```

## System API (`MexApi`) — v1.1

The program detects available fields via `version_minor` in the `.mex`
header (see `src/mex.rs` for the authoritative definition):

```rust
#[repr(C)]
pub struct MexApi {
    // --- v1.0 (offset 0x00–0x28) ---
    pub print:         extern "C" fn(ptr: *const u8, len: usize),
    pub read_char:     extern "C" fn() -> u8,
    pub try_read_char: extern "C" fn() -> i32,
    pub uptime_ms:     extern "C" fn() -> u64,
    pub read_file:     extern "C" fn(name_ptr, name_len, out_ptr, out_cap) -> i64,
    pub write_file:    extern "C" fn(name_ptr, name_len, data_ptr, data_len) -> i64,

    // --- v1.1 (offset 0x30–0x48) ---
    pub ping:          extern "C" fn(ip_ptr: *const u8, ip_len: usize, timeout_ms: u64) -> i64,
    pub get_mac:       extern "C" fn(mac_out: *mut u8) -> i64,
    pub get_ip:        extern "C" fn(ip_out: *mut u8) -> i64,
    pub arp_resolve:   extern "C" fn(ip_ptr: *const u8, ip_len: usize, mac_out: *mut u8) -> i64,
}
```

### New functions in v1.1

| Function | Description |
|---|---|
| `ping(ip, 4, timeout_ms)` | Sends an ICMP echo. Returns the RTT in ms, or -1 (error/timeout). |
| `get_mac(buf)` | Writes the 6-byte MAC address into `buf`. Returns 0 or -1. |
| `get_ip(buf)` | Writes the 4-byte IPv4 address into `buf`. Returns 0 or -1. |
| `arp_resolve(ip, 4, mac)` | Looks up the MAC for an IP in the ARP cache. Returns 0 (found) or -1. |

## Kernel modules (`.kmod`)

A separate mechanism from `.mex`: module files are loaded from disk by
`src/module.rs` and show up in `lsmod`. The current factory image ships no
`.kmod` files — the networking, crypto, and graphics subsystems are built
into the kernel and are not loaded modules. Placeholder module files are
never created: `module.rs` expects a valid header and an executable body.

`.kmod` format:
```
Offset    Size    Field
0x00      4       magic "KMOD" (0x444F4D4B)
0x04      2       version_major
0x06      2       version_minor
0x08      4       header_size (44)
0x0C      4       init_offset
0x10      4       body_size
0x14      4       bss_size
0x18      4       name_len
0x1C      28      name (ASCII, zero-padded)
0x38      ...     body (flat machine code)
```

Module entry point:
```c
int64_t kmod_init(const KernelApi* api);
```

A module registers its services through KernelApi and returns 0 on success.

## Limitations (honestly)

- **Not position-independent.** One program instance at a time
  (`MEX_IN_USE`); the image loads only at `0x03000000`.
- **Isolation is per-region, not per-address-space.** Ring 3 plus pointer
  validation against the program's region; there are no separate page
  tables per process.
- Built with `nasm -f bin` + `python3 tools/mex_pack.py`, or in C/C++ with
  `mexcc` (see `tools/`).
