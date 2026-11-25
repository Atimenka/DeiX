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
