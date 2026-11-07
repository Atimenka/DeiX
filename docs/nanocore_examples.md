# Nano-Core: Примеры манифестов и лога загрузки

## 1. Пример манифеста (fs-erofs-mount)

```toml
[process]
name = "fs-erofs-mount"
category = "fs"
description = "Mount EROFS at /"
requires = ["drv-ata-reset", "mem-heap-init"]
provides = ["mount-erofs"]
priority = 40
timeout_ms = 2000
restart_policy = "on-failure"
max_restarts = 3
log_level = "debug"
critical = true
```

### Пояснение полей

| Поле | Значение | Объяснение |
|------|----------|-------------|
| `name` | `"fs-erofs-mount"` | Уникальное имя процесса (категория-подсистема-действие) |
| `category` | `"fs"` | Категория: drv, fs, sec, mem, net, log, sys, ui, svc |
| `description` | `"Mount EROFS at /"` | Человекочитаемое описание |
| `requires` | `["drv-ata-reset", "mem-heap-init"]` | Зависимости — процессы, которые должны завершиться до запуска |
| `provides` | `["mount-erofs"]` | Капabilities, которые этот процесс предоставляет другим |
| `priority` | `40` | Приоритет запуска (0–100, меньше = раньше) |
| `timeout_ms` | `2000` | Таймаут в миллисекундах (0 = нет таймаута) |
| `restart_policy` | `"on-failure"` | `always`, `on-failure`, `never` |
| `max_restarts` | `3` | Максимум перезапусков |
| `log_level` | `"debug"` | trace, debug, info, warn, error |
| `critical` | `true` | Процесс не может быть degraded — его падение вызывает panic |

## 2. Пример лога загрузки (первые 20 строк)

```
[0.001] [1] [nano-core] INFO  orchestrator started
[0.002] [2] [drv-ata-reset] INFO START
[0.003] [25] [mem-heap-init] INFO START
[0.004] [33] [log-ring-init] INFO START
[0.005] [39] [sys-sched-init] INFO START
[0.006] [40] [sys-irq-router] INFO START
[0.007] [10] [drv-apic-init] INFO START
[0.008] [8] [drv-pci-enum] INFO START
[0.009] [20] [sec-entropy-pool] INFO START
[0.010] [9] [drv-rtc-sync] INFO START
[0.011] [2] [drv-ata-reset] INFO ATA reset OK, 120µs
[0.012] [39] [sys-sched-init] INFO sched up, 4 CPUs
[0.013] [33] [log-ring-init] INFO ring 4KB, 256 slots
[0.014] [8] [drv-pci-enum] INFO found 8 devices
[0.015] [25] [mem-heap-init] INFO heap 0x...-0x...
[0.016] [10] [drv-apic-init] INFO APIC online
[0.017] [9] [drv-rtc-sync] INFO RTC=2026-11-07
[0.018] [20] [sec-entropy-pool] INFO entropy 256 bits
[0.019] [3] [drv-ata-read] INFO START (deps ok)
[0.020] [40] [sys-irq-router] INFO IRQ 1 → kbd
```

### Полный лог загрузки (50 процессов)

```
[0.001] [1] [nano-core] INFO  orchestrator started
[0.002] [2] [drv-ata-reset] INFO START
[0.003] [25] [mem-heap-init] INFO START
[0.004] [33] [log-ring-init] INFO START
[0.005] [39] [sys-sched-init] INFO START
[0.006] [40] [sys-irq-router] INFO START
[0.007] [10] [drv-apic-init] INFO START
[0.008] [8] [drv-pci-enum] INFO START
[0.009] [20] [sec-entropy-pool] INFO START
[0.010] [9] [drv-rtc-sync] INFO START
[0.011] [38] [log-crash-dump] INFO START
[0.012] [2] [drv-ata-reset] INFO ATA reset OK, 120µs
[0.013] [39] [sys-sched-init] INFO sched up, 4 CPUs
[0.014] [33] [log-ring-init] INFO ring 4KB, 256 slots
[0.015] [8] [drv-pci-enum] INFO found 8 devices
[0.016] [25] [mem-heap-init] INFO heap 0x...-0x...
[0.017] [10] [drv-apic-init] INFO APIC online
[0.018] [9] [drv-rtc-sync] INFO RTC=2026-11-07
[0.019] [20] [sec-entropy-pool] INFO entropy 256 bits
[0.020] [38] [log-crash-dump] INFO dump ready
[0.021] [3] [drv-ata-read] INFO START (deps ok)
[0.022] [41] [sys-syscall-gate] INFO START (deps ok)
[0.023] [4] [drv-kbd-init] INFO START (deps ok)
[0.024] [5] [drv-mouse-init] INFO START
[0.025] [26] [mem-page-alloc] INFO START (deps ok)
[0.026] [27] [mem-zero-page] INFO START (deps ok)
[0.027] [3] [drv-ata-read] INFO read LBA 0x100, 512B
[0.028] [41] [sys-syscall-gate] INFO syscall gate up
[0.029] [4] [drv-kbd-init] INFO kbd ready, IRQ1
[0.030] [5] [drv-mouse-init] INFO mouse ready, IRQ12
[0.031] [26] [mem-page-alloc] INFO alloc 4 pages
[0.032] [27] [mem-zero-page] INFO zero page used
[0.033] [6] [drv-vga-mode] INFO START (deps ok)
[0.034] [29] [net-stack-init] INFO START
[0.035] [11] [fs-erofs-mount] INFO START (deps ok)
[0.036] [14] [fs-vfs-router] INFO START (deps ok)
[0.037] [6] [drv-vga-mode] INFO mode 1024x768 set
[0.038] [29] [net-stack-init] INFO net stack up
[0.039] [11] [fs-erofs-mount] INFO EROFS mounted at /
[0.040] [14] [fs-vfs-router] INFO route / → EROFS
[0.041] [12] [fs-erofs-read] INFO START (deps ok)
[0.042] [13] [fs-ext2-mount] INFO START (deps ok)
[0.043] [7] [drv-fb-write] INFO START (deps ok)
[0.044] [12] [fs-erofs-read] INFO read /init, 4KB
[0.045] [13] [fs-ext2-mount] INFO ext2 mounted at /home
[0.046] [7] [drv-fb-write] INFO fb base=0x...
[0.047] [19] [sec-sig-verify] INFO START (deps ok)
[0.048] [15] [fs-path-resolve] INFO START (deps ok)
[0.049] [16] [fs-stat-worker] INFO START (deps ok)
[0.050] [19] [sec-sig-verify] INFO sig /init VALID
[0.051] [15] [fs-path-resolve] INFO path /bin/sh resolved
[0.052] [16] [fs-stat-worker] INFO stat /init → 4KB
[0.053] [17] [fs-dir-enum] INFO START (deps ok)
[0.054] [18] [fs-journal-replay] INFO START (deps ok)
[0.055] [17] [fs-dir-enum] INFO list /bin → 42 files
[0.056] [18] [fs-journal-replay] INFO journal replay 0 entries
[0.057] [34] [log-ring-push] INFO START (deps ok)
[0.058] [35] [log-flush-worker] INFO START (deps ok)
[0.059] [36] [log-heartbeat-mon] INFO START (deps ok)
[0.060] [34] [log-ring-push] INFO push event #42
[0.061] [35] [log-flush-worker] INFO flush 42 events
[0.062] [36] [log-heartbeat-mon] INFO hb ok: 50 procs
[0.063] [42] [sys-time-sync] INFO START (deps ok)
[0.064] [21] [sec-user-auth] INFO START (deps ok)
[0.065] [42] [sys-time-sync] INFO time synced
[0.066] [21] [sec-user-auth] INFO auth user=root OK
[0.067] [22] [sec-session-create] INFO START (deps ok)
[0.068] [22] [sec-session-create] INFO session 0x1 created
[0.069] [23] [sec-audit-log] INFO START
[0.070] [23] [sec-audit-log] INFO audit: proc start
[0.071] [24] [sec-seccomp-filter] INFO START (deps ok)
[0.072] [24] [sec-seccomp-filter] INFO seccomp active
[0.073] [30] [net-dhcp-client] INFO START (deps ok)
[0.074] [30] [net-dhcp-client] INFO DHCP: 192.168.1.10
[0.075] [31] [net-ip-route] INFO START (deps ok)
[0.076] [31] [net-ip-route] INFO route default → gw
[0.077] [32] [net-dns-resolve] INFO START (deps ok)
[0.078] [32] [net-dns-resolve] INFO DNS ready
[0.079] [28] [mem-oom-killer] INFO START (deps ok)
[0.080] [28] [mem-oom-killer] INFO oom: no pressure
[0.081] [45] [ui-fb-init] INFO START (deps ok)
[0.082] [46] [ui-input-route] INFO START (deps ok)
[0.083] [45] [ui-fb-init] INFO fb 1024x768
[0.084] [46] [ui-input-route] INFO input route up
[0.085] [47] [ui-keymap-load] INFO START (deps ok)
[0.086] [47] [ui-keymap-load] INFO keymap US loaded
[0.087] [48] [ui-compositor-prep] INFO START (deps ok)
[0.088] [48] [ui-compositor-prep] INFO compositor ready
[0.089] [43] [sys-reboot-handler] INFO START
[0.090] [44] [sys-shutdown-handler] INFO START
[0.091] [43] [sys-reboot-handler] INFO reboot ready
[0.092] [44] [sys-shutdown-handler] INFO shutdown ready
[0.093] [49] [svc-init-bridge] INFO START (deps ok)
[0.094] [50] [svc-udev-event] INFO START (deps ok)
[0.095] [49] [svc-init-bridge] INFO bridge to Dinit up
[0.096] [50] [svc-udev-event] INFO udev event: kbd
[0.097] [50] [svc-udev-event] INFO IDLE
[0.098] [1] [nano-core] INFO DAG complete: 50/50 procs, 0 failed
[0.099] [1] [nano-core] INFO handing off to Dinit supervisor
```

## 3. nanocore status — пример вывода

```
╔═════════════════════════════════════════════════════════════════════════╗
║                    Nano-Core Status (mode: normal, phase: COMPLETE)    ║
╚═════════════════════════════════════════════════════════════════════════╝

  Boot time: 0.099s | Processes: 50 | Completed: 50

  PID   Name                     Category   Priority    State         Restarts   Actions
  ─────────────────────────────────────────────────────────────────────────────────
  1     nano-core                sys        critical    EXITED        0          0
  2     drv-ata-reset            drv        critical    EXITED        0          1
  3     drv-ata-read             drv        early       EXITED        0          1
  ...
  50    svc-udev-event           svc        late        EXITED        0          1

  Total: 50 | Running: 0 | Exited: 50 | Failed: 0 | Panic: 0 | Restarts: 0
```
