# Experimental private-process mailbox ABI v1

This is a mechanism under development, not Dinit separation. It is available
only to scheduler-managed private CPL3 ELF processes, not legacy MEX. The kernel
chooses capabilities at process creation; userspace cannot grant itself them.
Calls use x86-64 `syscall`: number in RAX, arguments in RDI, RSI, RDX, R10;
return is signed RAX. RCX and R11 are clobbered. Calls are non-blocking.

* `0x44580003`: return caller PID, or -1 if not a running process.
* `0x44580001`: send `(destination_pid, tag, word0, word1)`. Requires
  `IPC_SEND` and a running destination with `IPC_RECEIVE`. The sender PID is
  derived from the current scheduler slot. Returns 0, -1 for authorization or
  unavailable target, -22 for invalid PID, -11 for a full queue.
* `0x44580002`: receive `(out_pointer)`. Requires `IPC_RECEIVE`. On success
  writes 32 bytes: sender PID (little-endian u32), four zero bytes, tag,
  word0 and word1 (three little-endian u64 values). Returns 0, -1 for missing
  permission, -11 if empty, or -14 for a non-writable range. Validation covers
  the entire range before dequeue; invalid buffers do not consume messages.

The global queue holds 64 messages, at most eight per recipient. FIFO order is
maintained per recipient; excess is rejected, not silently dropped. Exit/kill
purges messages addressed to the terminated PID. Messages sent by a process
before its termination can still be received. PID allocation does not wrap.
Queue traffic is volatile and messages are not durable. Caps permit sending to
any opted-in receiver, not scoped service capabilities. This ABI has not yet
been used by Dinit, which remains inside the kernel; no bootstrap or watchdog
contract is implied.

## Boot configuration read

`0x44580004` reads the boot-time snapshot of `/system/etc/init.deix`:
arguments are `(out_pointer, offset, length)`. It requires `READ_BOOT_CONFIG`.
The kernel snapshots the file before scheduling starts, rejects directories and
files larger than 8192 bytes, and makes no filesystem calls from the syscall.
Each read is limited to 1024 bytes. The return value is the number of bytes
copied (zero at EOF), -1 for missing capability, -2 when the snapshot is
unavailable, -22 for an invalid length, or -14 for an invalid output buffer.
The complete output range is checked before copying. This is a byte transport;
the existing kernel Dinit still parses its embedded fallback. The snapshot does
not reflect later on-disk edits or implement reload.

## Process heartbeat

`0x44580005` records a heartbeat for the currently running private process.
It requires `HEARTBEAT`, takes no arguments and returns 0 or -1 on denial.
Neither PID nor time is supplied by userspace: the kernel derives the process
from its scheduler slot and samples monotonic uptime. The last observation is
available in the kernel's process snapshot. `heartbeat_health` classifies a
running process as starting, alive or timed out using a caller-specified grace
period; the deadline is inclusive. It does not make restart decisions. This
is not yet a Dinit watchdog, bootstrap gate, or recovery implementation.

## Managed service lookup

`0x44580006` looks up a kernel-reserved service identity. It takes the pointer
and length of a UTF-8 name in RDI and RSI, and requires `MANAGE_SERVICES`.
The name is copied from the caller's private pages after checking the entire
range; it may reside in read-only memory. Length must be 1–128 bytes. The
return value is the PID of a Created or Running managed process, -1 for missing
capability, -2 if no such process exists, -14 for an invalid user range, or
-22 for invalid length or UTF-8. This lookup does not launch or stop services,
and a returned process can exit immediately afterward; callers must recheck
its status. Names are assigned by trusted kernel code, not arbitrary user PIDs.

`0x44580007` reads a process status snapshot: RDI is a PID, RSI points to a
16-byte output buffer. It requires `MANAGE_SERVICES`. The output is
little-endian: PID (u32), state (u32: 1 Created, 2 Running, 3 Zombie), exit
code (i32), and exit-code-present (u32, 0 or 1). Returns 0 on success, -1
without the capability, -2 for an absent or reaped PID, -14 for an invalid
output range, and -22 if the PID does not fit u32. A snapshot can become
stale immediately after the call; it cannot be used as a guarantee of liveness.
