# Dinit separation and NVIDIA bring-up: current state

This document is an engineering inventory, **not** a claim that either project
is complete. The current image continues to use the existing boot path.

## Dinit

`src/dinit/mod.rs` constructs and runs Dinit in kernel context. Its `pid` and
service `execution_ring` fields do not provide process isolation. There is no
separately built `/system/kernel/els/dinit/dinit.elf` or Dinit RPC ABI. Nano-Core
currently starts after Dinit and its orchestrator contains separate service
policy. `src/nanocore/ipc.rs` provides kernel-internal named queues, not
permission-checked process IPC.

`process::exec` only starts MEX, not ELF; MEX uses one fixed shared address
range. `usermode::make_region_user` modifies the active page tables and may
expose entire 2 MiB pages; it does not create a private address space. The
existing syscall path and global Ring-3 return state are not a multi-process
isolation boundary. Neither a PID nor loading an ELF alone solves this.

Before switching the boot path, implement and test private page tables and
per-task context switching, safe user-pointer copying and process-scoped
capabilities, a bounded permission-checked request/reply transport, an ELF
loader with strict mapping validation, and a supervisor-specific ABI. Core
must initialize independently and monitor Dinit without taking over policy;
Dinit must own configuration, dependencies, service supervision and sessions.
Bootstrap success must gate login. Recovery must reconcile existing service
identities before issuing starts. Tests must demonstrate privilege and memory
isolation, denied capabilities, failed bootstrap, crash/restart and no
duplicate services. Keep the existing path until the replacement actually
satisfies those checks; this inventory is not an implemented ABI.

The current service lifecycle implementation has host tests for restart
backoff and bounded retries. Those tests do not demonstrate isolation.

## NVIDIA / Optimus

`gpu nvinfo` uses PCI configuration reads to describe NVIDIA display devices,
including the BAR0 address/type when available. It does not size the BAR,
enable PCI memory decoding or bus mastering, read MMIO, wake a GPU, or submit
commands. Known GT 520M PCI IDs are matched as product identifiers only; they
do not prove the exact chip revision or panel routing.

Real acceleration requires a validated GPU-specific resource and power model,
safe MMIO mapping, GPU memory management, command submission, synchronization,
recovery and an appropriate rendering API. On an Optimus laptop the display
may be attached to the Intel GPU, so NVIDIA rendering and presentation are
separate problems. Firmware VBE is a software framebuffer, not NVIDIA
acceleration. No GPU register writes or experimental initialization are enabled
by this read-only groundwork. Real-hardware testing is still required.
