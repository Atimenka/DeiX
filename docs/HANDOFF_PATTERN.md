# DeiX OS: Handoff Pattern (Передача Эстафеты)

## Architecture Overview

DeiX OS implements a **handoff pattern** where control is transferred from the Nano-Core Orchestrator to the Dinit Supervisor (PID 1) during the boot process.

### Phase 1: Orchestrator = PID 1 (Early Boot)

**Who is in charge:** Nano-Core Orchestrator

**What it does:**
- Boots hardware drivers (drv-ata, drv-kbd, drv-mouse)
- Mounts filesystems (EROFS, EXT2)
- Initializes logging (log-ring)
- Starts security monitoring (sec-sig)
- Manages early boot processes as children

**Process hierarchy:**
```
Orchestrator (PID 1)
├── drv-ata (child)
├── fs-erofs (child)
├── log-ring (child)
├── sec-sig (child)
├── sched-tick (child)
└── ... (other early processes)
```

**Failure handling:** If any child process crashes, Orchestrator reaps it (wait()) and restarts it.

### Phase 2: Birth of Dinit (The Handoff Moment)

**What happens:**
1. Orchestrator loads Dinit binary from disk
2. Calls `fork()` + `execve("/sbin/dinit")`
3. New process is born with temporary PID (e.g., 42)
4. Orchestrator calls special system call: `sys_change_pid_1(new_pid)`

**The magic:** PID namespace root is transferred.

### Phase 3: Dinit = PID 1 (Late Boot)

**Who is in charge:** Dinit Supervisor

**What happens:**
1. Dinit becomes PID 1 (root of PID namespace)
2. Orchestrator is demoted to PID 0 (or background daemon)
3. Orchestrator becomes a child of Dinit
4. Dinit takes ownership of user space services

**New process hierarchy:**
```
Dinit (PID 1)
├── Nano-Core Orchestrator (PID 0, child)
│   ├── drv-ata (grandchild)
│   ├── fs-erofs (grandchild)
│   └── ... (early boot processes)
├── CLI (child)
├── Desktop UI (child)
├── Network services (child)
└── ... (user space services)
```

## Why This Pattern is Powerful

### 1. Security
- If Orchestrator has vulnerability, attacker is sandboxed
- Orchestrator only manages hardware, not user space
- Dinit (standard service manager) doesn't touch hardware directly

### 2. Clean Separation
- Orchestrator: Early boot, hardware, drivers
- Dinit: Late boot, user space, services
- Each component does what it's designed for

### 3. Robustness
- If Dinit crashes, hardware drivers (under Orchestrator) keep running
- System survives Dinit failure
- Orchestrator continues managing hotplug (USB device insertion, etc.)

### 4. Hotplug Support
- Dinit asks Orchestrator: "New USB device, deploy driver from DAG"
- Orchestrator handles dynamic driver loading
- Dinit doesn't need to know about hardware details

## Arrows on the Diagram

**Forward arrow (Orchestrator → Dinit):**
```
Orchestrator → Dinit: "starts late stage (PID 1)"
```

**Backward arrow (Dinit → Orchestrator):**
```
Dinit → Orchestrator: "takes ownership / supervises"
```

This shows the **democratic handoff**: Orchestrator voluntarily steps down, Dinit takes control, but Orchestrator remains as a specialized service.

## What Happens if Handoff Fails

If PID namespace transfer doesn't work correctly:
1. Drivers become orphans (zombie processes)
2. System loses connection to hardware
3. User space can't start
4. System hangs or crashes

**Mitigation:** Orchestrator must verify all early boot processes are still alive after handoff. If not, it must re-attach them or restart them.

## Implementation in DeiX OS

### Current State (Partial)
- Nano-Core Orchestrator exists (src/nanocore/orchestrator.rs)
- Dinit Supervisor exists (src/dinit/mod.rs)
- Both are initialized during boot
- Handoff is implicit (not explicitly implemented)

### Future Enhancements
1. Add explicit `sys_change_pid_1()` system call
2. Implement process tree re-parenting
3. Add handoff verification
4. Add hotplug communication channel (Dinit → Orchestrator)

## Key Files

- `src/nanocore/orchestrator.rs` - Nano-Core Orchestrator
- `src/dinit/mod.rs` - Dinit Supervisor
- `src/process.rs` - Process management
- `src/sched.rs` - Scheduler
- `src/lib.rs` - kernel_main() boot sequence

## Diagram Reference

See `arch_02_nanocore_kernel.png` for the complete diagram showing:
- Orchestrator as early boot manager
- Dinit as late boot supervisor
- The handoff arrow between them

---

**Summary:** The handoff pattern makes DeiX OS modular, secure, and robust. Orchestrator is the early boot champion, Dinit is the late boot champion. They work together but have clear responsibilities.
