# Dinit / Core audit (current implementation)

This records existing behaviour, not a declaration that the requested
separation exists. Dinit is still in the kernel and there is no Dinit binary,
private address space, authenticated process IPC or bootstrap gate.

## Boot and callers

* `src/lib.rs` starts usermode support and the scheduler, then calls
  `dinit::init()` directly and starts Nano-Core. Login starts unconditionally
  later in the same kernel entry path; the login result is registered directly
  through `DINIT.lock()`. Shutdown drops the same global object.
* `src/ui/desktop.rs` calls `dinit::tick()`; `src/cli.rs` and `src/ds.rs`
  call the Dinit CLI directly. `src/diag/boot.rs` tests the global object.
* `DINIT` in `src/dinit/mod.rs` is a kernel `SpinLock<Option<Dinit>>`;
  `pid = 1` is bookkeeping, not a scheduled user process.

## Existing responsibilities to preserve

* `src/dinit/mod.rs` owns the service table, autostart, supervision, retry,
  security event response, sessions, stage transitions and CLI commands.
  `src/dinit/service.rs`, `user.rs`, `namespace.rs`, `mount.rs`, and `audit.rs`
  hold the corresponding data structures and policy.
* `src/init_parser.rs` parses the built-in `INIT_DEIX_SCRIPT`; it uses the
  kernel Vault and diagnostics. Bootstrap and `dinit reload` use that constant.
  The `/system/etc/init.deix` created by `tools/make_deix_fs.py` is a different
  placeholder, so changing the consumer to read it without migrating the
  script would change current behaviour.
* `src/nanocore/orchestrator.rs` separately implements service dependency
  ordering, launch and restart policy for its built-in manifests; that policy
  must be accounted for rather than silently duplicated in the final design.
* The existing security monitor, mount validation, audit, CLI, and user-session
  behaviour are all part of the migration, not optional features to delete.

## Kernel interface and execution gaps

* `process::spawn/exec/info/kill/reap` are direct Rust calls, not a stable
  userspace ABI. `exec` rejects ELF; MEX uses one fixed shared region and one
  launch context. `ProcInfo` does not provide a stable service identity for
  restart reconciliation. Capabilities are recorded but the required scoped
  process-facing Dinit API is not implemented.
* `src/usermode.rs` changes the active page tables in place for user memory,
  and the legacy Ring-3 return state and syscall stack are global. The
  scheduler can now switch owned CR3 roots and launch a low-level CPL3 task
  with a task-specific kernel entry stack. The private ELF loader can populate these pages and a CI-only QEMU test
  reads a separately built probe from `/system/tests` and executes it at CPL3.
  This path is not wired into the process manager or Dinit. Its syscall surface
  only handles exit and a CI-only test call; it is not a Dinit ABI.
* Nano-Core IPC is an in-kernel named queue, not a bounded authenticated
  process request/reply interface. VFS, logs, timers, process state, session
  events and security monitoring are currently accessed as internal functions.
* The heartbeat trackers monitor Nano-Core services; they do not supervise an
  independent Dinit process. Current Dinit startup returns no bootstrap result
  and does not reconcile surviving services after Dinit failure.

## Required evidence before claiming completion

The image must contain the separately built executable at the requested ELS
path. Boot tests must establish that it actually executes in its own address
space, uses the documented kernel ABI, cannot access kernel or another
process's memory, and lacks unauthorized capabilities. Test missing/corrupt
binary and failed bootstrap, login gating, heartbeat timeout and crash,
restart with already-running services (no duplicate starts), configuration,
CLI, session and service lifecycle preservation. Validate core-only recovery
and normal boot in QEMU before using the image; physical ASUS testing remains
separate. Merely adding an ELS file, a PID, or an `execution_ring` field does
not satisfy these tests.

## Page-table construction work

`src/mm/address_space.rs` constructs owned page tables and separate user pages
in an unused PML4 slot, with inherited kernel mappings supervisor-only. Its
host tests exercise construction and allocation ownership, not CPU privilege
checks. The scheduler can retain and switch these roots. A CI-only QEMU boot
self-test runs two kernel-mode tasks at the same virtual address with different
backing pages, yielding between them, and verifies that the kernel root is
restored. A second QEMU test executes two CPL3 tasks using distinct pages and
checks that a third CPL3 task faults when reading kernel memory. These tests
do not cover all syscalls, devices, or scheduler interactions. NX is not
enabled and Dinit does not use these spaces. Slot reuse frees a finished task's
tables only after it is no longer current. This does not establish Dinit
isolation or a working user-process API.

## Private ELF probe

`load_private` maps validated static PIE load segments into owned private
pages, applies supported relative relocations and creates a zeroed stack.
`DEIX_TEST_ADDRESS_SPACE=1` adds a separately compiled test ELF to the image
and QEMU verifies execution from the system VFS in CPL3. This probe is not
Dinit, does not implement its policy or ABI, and is not included in normal
images. NX is not enabled; page permissions do not enforce non-executable data.
