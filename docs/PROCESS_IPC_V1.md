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
the existing kernel Dinit still parses its embedded fallback. The snapshot changes only after an authorized asynchronous reload.

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

## Experimental asynchronous service operations

These calls require `MANAGE_SERVICES` and run only in private ELF tasks. They
submit bounded requests to a kernel worker so that VFS and process creation do
not execute inside the interrupt-disabled syscall handler. The worker provides
mechanisms only: dependency ordering, restart and reconciliation are policy for
Dinit. This interface is experimental and does not yet replace the existing
in-kernel Dinit calls.

* `0x44580008`: submit start `(name_ptr, name_len, path_ptr, path_len)`.
  Returns a positive request token, or a negative errno. Names are UTF-8,
  1–128 bytes, without control characters or slashes. Images must be static PIE
  `.elf` files below `/system/services/`; traversal, empty components and
  control characters are rejected. CI-only builds also permit `/system/tests/`.
* `0x44580009`: submit stop `(name_ptr, name_len, expected_pid)`. It only stops
  the managed process whose identity and PID both match; it cannot stop the
  caller. It does not set a restart policy.
* `0x4458000a`: poll `(token, result_ptr)`. The result is eight bytes:
  little-endian signed i32 status and u32 PID. Returns 0 and consumes a completed
  result, -11 if pending, -2 for an unknown token, or -14 for an invalid output
  range. Only the submitting PID can poll. Validating the output precedes
  consumption. Each caller may retain four requests; the global limit is 16.

Submission can return -1 for missing permission, -5 if no worker is available,
-11 if full, -14 for invalid memory, -22 for invalid arguments or -75 if tokens
are exhausted. Completion status is 0 on success, -17 for an already-reserved
identity (PID returned), -2 for an absent/mismatched stop target, -5 on image
load failure, -12 on allocation/table failure, or -1 if the caller is no longer
authorized. The original `0x44580008` starts children with zero capabilities. Failed starts are killed and reaped. Requests are
purged when a caller exits; in-flight operations may already have taken effect,
so a replacement supervisor must reconcile actual process state rather than
assuming cancellation. The worker rechecks the caller before and after a start.

### Explicit capability delegation

`0x4458000b` submits a start with arguments `(name_ptr, name_len, path_ptr,
path_len, capability_mask)` in RDI, RSI, RDX, R10 and R8. It has the same
request/result semantics as `0x44580008`. A nonzero grant additionally requires
`DELEGATE_SERVICES`, must be a subset of the caller's own capabilities, and
may contain only `IPC_SEND`, `IPC_RECEIVE`, `HEARTBEAT` and
`READ_BOOT_CONFIG`, and `WRITE_LOG`. In particular, service-management, delegation and install
rights cannot be delegated. The kernel checks authority at submission, before
execution and after the image starts. This limited whitelist covers only
currently implemented private-process interfaces; it does not yet provide the
full authority needed to preserve every legacy service.

`0x4458000c` returns the current process's parent PID derived from the process
table, or -1 if the current task has no running process record. Userspace
cannot set its own parent PID through this interface.

### Configuration snapshot reload

`0x4458000d` submits an asynchronous reload of `/system/etc/init.deix` and
returns a request token. It requires both `MANAGE_SERVICES` and
`READ_BOOT_CONFIG`; completion is retrieved through `0x4458000a`. A worker
performs the VFS access outside syscall context. Status 0 means the bounded
snapshot was replaced; -5 means reading or size validation failed, in which
case the last valid snapshot remains available. This does not itself parse or
apply configuration: that is the future Dinit process's responsibility.

### Diagnostic log submission

`0x4458000e` accepts `(message_ptr, byte_len)` and queues an informational
entry in the existing kernel diagnostic ring. `WRITE_LOG` is required; messages
must be nonempty UTF-8, at most 96 bytes and contain no control characters.
The kernel attributes the entry to the caller PID; identical records from
different PIDs are not aggregated together. Poll the returned token with
`0x4458000a` (also available to `WRITE_LOG` holders); a successful result has
the diagnostic event ID in the result's second field. Queue quotas and purge on
caller exit apply. This supplies log writing, not Dinit's full log policy,
query API, or durable audit guarantees.

### Managed identity metadata

`0x4458000f` takes `(name_ptr, name_len, out_ptr)`, requires
`MANAGE_SERVICES`, and atomically snapshots a currently Created or Running
managed identity under the process-table lock. Its fixed 280-byte little-endian
output is PID, state, UID and path length (four u32s), capability mask (u64),
and 256 bytes containing the exact UTF-8 image path followed by zeros. State
values are 1 Created and 2 Running. It returns 0, -1 for missing capability,
-2 if the identity is absent, -14 for invalid user memory, -22 for invalid name
length or UTF-8, and -75 if the stored path exceeds 256 bytes. No allocation,
truncation or filesystem I/O occurs in the syscall.

A replacement manager can compare the recorded path, UID and capabilities to
its desired policy before adopting a process. This is a point-in-time snapshot:
the process can exit immediately afterwards; compare PID again for subsequent
actions. A pathname is not a cryptographic identity of executable bytes, and
this interface does not independently authenticate a changed disk image, list
all identities, or implement adoption, recovery or restart policy.

### Monotonic time

`0x44580010` returns kernel monotonic uptime in milliseconds, saturated at
`i64::MAX`, for any running private process; it requires no extra capability.
It returns -1 if no running process corresponds to the calling task. This is
not a wall clock and does not provide sleeping or scheduling policy.

### Consistent multi-chunk configuration reads

`0x44580011` returns the current positive snapshot generation (or -2 when no
snapshot exists); `0x44580012` reads `(out_ptr, offset, length, generation)`.
Both require `READ_BOOT_CONFIG`. The latter has the same bounds and errors as
`0x44580004`, and returns -116 without copying if a refresh replaced the
snapshot since the caller obtained its generation. Each chunk's generation
check and copy are atomic against refresh; clients must discard **all** earlier
chunks and retry on -116. Generations increase on each successful refresh,
including an identical file, never wrap, and a failed refresh preserves the
last good snapshot and generation. The older read call does not offer this
consistency guarantee.

### Authenticated local session snapshot

`0x44580013` takes an output pointer and requires the non-delegable
`READ_SESSION` capability. After successful local login, the trusted kernel
login flow publishes a point-in-time 48-byte record: UID and username length
(two little-endian u32s), login uptime in milliseconds (u64), then 32 bytes of
UTF-8 username, zero-padded. Return values are 0, -1 without permission, -2
before login, -14 for invalid output memory and -75 if the authenticated name
cannot be represented without truncation. No password, hash or token is exposed.
The kernel assigns UID as in the existing login flow; it is not supplied by the
private process. This is a single local-login snapshot, not a session event
stream, logout API, authorization gate, policy implementation or Dinit session
ownership. Current UI has no corresponding logout path. A replacement manager
must still handle future session lifecycle and reconciliation explicitly.

### Finished managed processes

`0x44580015` enumerates unreaped managed zombies using `(after_pid, out_ptr)`
and requires `MANAGE_SERVICES`. The next PID strictly greater than the cursor
is returned in a 144-byte little-endian record: PID and UTF-8 name length (u32),
exit code (i32), four reserved zero bytes, and a zero-padded 128-byte name.
Return values are 0, -1 for missing permission, -2 if no later zombie exists,
-14 for invalid output memory, and -22 if the cursor does not fit u32. Ordinary
unmanaged processes are not listed. Enumeration is not a stable snapshot:
processes can exit or be reaped between calls, so rescan from zero to discover
late exits with lower PIDs. Names can recur after an earlier instance exits;
PIDs never repeat.

`0x44580014` reaps `(name_ptr, name_len, expected_pid, exit_code_ptr)`, also
requiring `MANAGE_SERVICES`. The kernel checks identity, PID and zombie state
and removes that exact process under one lock. It returns 0 with the i32 exit
code, -1 without permission, -2 for an absent or mismatched identity/PID, -11
for a still-live instance, -14 for invalid memory, -22 for invalid input, or
-5 for a zombie lacking exit status. Output memory is checked before removal.
This provides bounded discovery and cleanup, not restart policy or a reliable
event stream; another authorized manager may reap an entry first.

### Observation of managed heartbeats

`0x44580016` takes `(pid, timeout_ms, out_ptr)` and requires
`MANAGE_SERVICES`. The target must be a running managed process with the
`HEARTBEAT` capability; the caller cannot supply or modify heartbeat times.
The 24-byte little-endian result contains health (u32: 1 starting, 2 alive,
3 timed out), last-heartbeat-present (u32), last heartbeat uptime (u64, zero
if absent), and the kernel's sampled uptime (u64). Timeout is evaluated as
elapsed strictly greater than the supplied interval. Return values: 0, -1
without permission, -2 for an absent/non-running/unmanaged PID, -95 when the
process has no heartbeat, -14 for an invalid output range and -22 for an
invalid PID. The snapshot can become stale immediately; deciding whether and
when to restart remains supervisor policy. This interface is not the Dinit
watchdog.

### Private service image enforcement

Service worker starts always use the private ELF execution path. A legacy MEX
image is rejected by content as well as by extension; renaming a MEX file to
`.elf` cannot select the legacy shared mapping. A failed image start is killed
and reaped and completes with -5. Legacy non-service MEX behavior is unchanged.

### Live managed process discovery

`0x44580017` returns the next live managed identity after a PID cursor. It
requires `MANAGE_SERVICES` and takes `(after_pid, out_ptr)`. The 416-byte
little-endian record consists of PID, state (1 Created / 2 Running), UID, name
length, image-path length and flags (six u32s), capabilities (u64), a 128-byte
zero-padded identity and a 256-byte zero-padded image path. Bit 0 of flags means
the legacy path exceeded the limit and was omitted; in that case path length
is zero and a supervisor must not adopt based on this record. Other flags are
zero. Returns 0, -1 for missing permission, -2 at the end, -14 for invalid
output memory, or -22 for an out-of-range cursor. No allocation or VFS I/O
occurs in the syscall.

The list is not a transaction: processes may start or exit between calls, and a
Created process may finish loading or fail. PIDs do not repeat. After a pass,
a supervisor must rescan as needed and recheck identity, state and authority
before acting. The separately enumerated zombie list covers exited processes;
this discovery interface is not itself recovery, adoption, or a guarantee that
path contents match previously executed bytes.
