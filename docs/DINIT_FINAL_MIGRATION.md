# Dinit ELF: remaining migration and acceptance checklist

This is a plan for the final migration, **not a claim that Dinit has been
separated**. The running Dinit and Nano-Core orchestration policy are still in
Ring 0. There is currently no `/system/kernel/els/dinit/dinit.elf`, no separate
Dinit bootstrap, no Dinit watchdog, and no login gate.

## Mechanisms available now

* A static PIE ELF can be loaded from VFS into a private address space and run
  at CPL3. The process manager records exit and fault status. NX is not enabled;
  writable pages are currently executable, so isolation is incomplete.
* Private processes have a restricted ABI (`PROCESS_IPC_V1.md`): authenticated
  bounded mailboxes, boot-config snapshot read, heartbeat, managed service
  lookup, status, and non-blocking start/stop. A kernel worker performs VFS
  operations. Starts currently allow only `.elf` under `/system/services/`,
  normally create children with zero capabilities, and cannot run legacy MEX.
  A separate explicit grant supports a restricted subset of the manager's
  capabilities (IPC, heartbeat and boot-config read), but not full legacy
  service authority.
* The process table can reserve stable service identities. The existing in-kernel
  Dinit can reattach a surviving, matching managed process. A QEMU-only test
  checks this with a real CPL3 service; this is not Dinit restart recovery.
* `system/etc/init.deix` is the source for both the system-image file and the
  current kernel fallback. The private ABI provides a boot-time snapshot; it
  does not implement live reload.

## Work required before the switch

1. Define the complete versioned Dinit/Core protocol and its capability model:
   service spawn grants and image allowlists, process exit/reap, VFS/config
   reload, time and sleep, device probes, mount validation, kernel event stream,
   security actions, audit/logs, session/login notifications, and CLI requests.
   Give only explicitly scoped rights to Dinit and each service. Preserve the
   existing format and semantics rather than quietly dropping features.
2. Implement a userspace runtime and separately build a real static PIE
   `/system/kernel/els/dinit/dinit.elf`. Port actual policy from
   `src/dinit/{mod,service,user,namespace,mount,audit}.rs` and
   `src/nanocore/{dag,manifests,orchestrator}.rs`: config parsing, dependency
   ordering, service startup/stop/restart/backoff, security event decisions,
   logs, sessions and CLI. Built-in kernel subsystem mechanisms may remain in
   Core, but policy and mutable Dinit supervisor state must not remain there.
   Do not treat a launcher, heartbeat loop, copied file, or PID field as a
   working supervisor.
3. Introduce a Core-owned minimal supervisor lifecycle: launch Dinit in its
   own address space, grant its scoped capabilities, require an authenticated
   bootstrap-ready result and heartbeat, and gate login/user sessions until
   bootstrap succeeds. Core must initialize and offer a bounded recovery path
   even when the ELF is absent, corrupt, exits, stalls or fails bootstrap.
4. On Dinit failure, stop admitting new sessions, apply bounded restart and
   backoff, relaunch the executable and reconcile managed service identities
   with actual process state. Adopt only matching image and authority, do not
   duplicate already-running services, and reconcile stopped/crashed ones using
   Dinit policy. Specify behavior for requests in flight and stale IPC replies.
5. Replace **all** direct kernel `dinit::` callers and the global DINIT object
   with the protocol: boot, desktop tick, CLI, diagnostics, shutdown, session
   registration and security events. Account for Nano-Core's independent
   boot/dependency policy rather than leaving it duplicated in the kernel.
   Remove the old implementation only after equivalent behavior is tested.
6. Complete memory protection (including NX and validated user-copy paths),
   prevent capability escalation and ensure no shared kernel/user writable
   mappings. Audit syscall context, IRQ locks, task and page-table lifetime,
   bounded queues, cancellation and denial-of-service limits.

## Required evidence for completion

* Build/image tests inspect the exact ELS path and verify the ELF is executed
  in CPL3 with its own CR3, cannot access kernel/other process memory, and
  cannot invoke unauthorized operations. Check missing/corrupt ELF and absent
  permissions fail closed without falsely reporting a successful bootstrap.
* QEMU tests cover normal bootstrap and login, failed bootstrap and login gate,
  Dinit crash and heartbeat timeout, bounded restart, Core-only recovery, and
  reconciliation with a running service (no duplicate starts), a stopped
  service, and a failed dependency. Include in-flight requests, queue saturation,
  stale replies, and failures while loading a service.
* Regression tests cover init parsing/reload, service lifecycle and backoff,
  dependency ordering, audit/security events, session/CLI behavior, boot chain,
  filesystem image format and recovery after restart.
* Physical ASUS input/display and actual NVIDIA acceleration remain separate
  hardware validation/development tasks; QEMU or a framebuffer does not prove
  they work or that GPU acceleration exists. A test image is copied as a file
  onto the existing Ventoy stick; it must never require reformatting it.
