//! Embedded manifests for all 50 Nano-Threads.
//!
//! Each manifest describes a Nano-Thread: its name, dependencies,
//! capabilities, priority, timeout, restart policy, and log level.
//! The orchestrator reads these manifests to build the DAG and launch
//! processes in parallel.
//!
//! Categories: drv (10), fs (8), sec (6), mem (4), net (4), log (6),
//!             sys (6), ui (4), svc (2) = 50 total.

use alloc::vec::Vec;
use crate::nanocore::manifest::ProcessManifest;

/// All 50 manifests as raw TOML strings.
/// Format: [process] section with key = value pairs.

pub const DRV_ATA_RESET: &str = r#"
[process]
name = "drv-ata-reset"
category = "drv"
description = "ATA controller reset"
priority = 1
timeout_ms = 500
restart_policy = "on-failure"
max_restarts = 2
log_level = "debug"
"#;

pub const DRV_ATA_READ: &str = r#"
[process]
name = "drv-ata-read"
category = "drv"
description = "ATA sector read"
requires = ["drv-ata-reset"]
priority = 10
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "debug"
"#;

pub const DRV_KBD_INIT: &str = r#"
[process]
name = "drv-kbd-init"
category = "drv"
description = "Keyboard initialization (IRQ1)"
requires = ["sys-irq-router"]
priority = 15
timeout_ms = 500
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const DRV_MOUSE_INIT: &str = r#"
[process]
name = "drv-mouse-init"
category = "drv"
description = "Mouse initialization (IRQ12)"
priority = 15
timeout_ms = 500
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const DRV_NET_RTL8139: &str = r#"
[process]
name = "drv-net-rtl8139"
category = "drv"
description = "RTL8139 network driver"
requires = ["drv-pci-enum", "net-stack-init"]
priority = 30
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "debug"
"#;

pub const DRV_VGA_MODE: &str = r#"
[process]
name = "drv-vga-mode"
category = "drv"
description = "VGA graphics mode setup"
requires = ["drv-pci-enum"]
priority = 20
timeout_ms = 500
restart_policy = "on-failure"
max_restarts = 1
log_level = "info"
"#;

pub const DRV_FB_WRITE: &str = r#"
[process]
name = "drv-fb-write"
category = "drv"
description = "Framebuffer base address setup"
requires = ["drv-vga-mode"]
priority = 25
timeout_ms = 500
restart_policy = "on-failure"
max_restarts = 1
log_level = "info"
"#;

pub const DRV_PCI_ENUM: &str = r#"
[process]
name = "drv-pci-enum"
category = "drv"
description = "PCI bus device enumeration"
priority = 5
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "debug"
"#;

pub const DRV_RTC_SYNC: &str = r#"
[process]
name = "drv-rtc-sync"
category = "drv"
description = "RTC time sync"
priority = 8
timeout_ms = 500
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const DRV_APIC_INIT: &str = r#"
[process]
name = "drv-apic-init"
category = "drv"
description = "APIC initialization"
priority = 3
timeout_ms = 500
restart_policy = "never"
max_restarts = 0
log_level = "debug"
"#;

pub const FS_EROFS_MOUNT: &str = r#"
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
"#;

pub const FS_EROFS_READ: &str = r#"
[process]
name = "fs-erofs-read"
category = "fs"
description = "Read file from EROFS"
requires = ["fs-erofs-mount"]
priority = 45
timeout_ms = 2000
restart_policy = "on-failure"
max_restarts = 3
log_level = "debug"
"#;

pub const FS_EXT2_MOUNT: &str = r#"
[process]
name = "fs-ext2-mount"
category = "fs"
description = "Mount ext2 at /home"
requires = ["fs-erofs-mount", "drv-ata-reset"]
provides = ["mount-ext2"]
priority = 42
timeout_ms = 2000
restart_policy = "on-failure"
max_restarts = 3
log_level = "debug"
"#;

pub const FS_VFS_ROUTER: &str = r#"
[process]
name = "fs-vfs-router"
category = "fs"
description = "VFS path routing"
requires = ["fs-erofs-mount"]
provides = ["vfs-route"]
priority = 35
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
critical = true
"#;

pub const FS_PATH_RESOLVE: &str = r#"
[process]
name = "fs-path-resolve"
category = "fs"
description = "Path resolution"
requires = ["fs-vfs-router"]
priority = 50
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const FS_STAT_WORKER: &str = r#"
[process]
name = "fs-stat-worker"
category = "fs"
description = "File metadata stats"
requires = ["fs-vfs-router"]
priority = 55
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const FS_DIR_ENUM: &str = r#"
[process]
name = "fs-dir-enum"
category = "fs"
description = "Directory enumeration"
requires = ["fs-vfs-router", "fs-path-resolve"]
priority = 58
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const FS_JOURNAL_REPLAY: &str = r#"
[process]
name = "fs-journal-replay"
category = "fs"
description = "ext2 journal replay"
requires = ["fs-ext2-mount"]
priority = 60
timeout_ms = 2000
restart_policy = "on-failure"
max_restarts = 2
log_level = "debug"
"#;

pub const SEC_SIG_VERIFY: &str = r#"
[process]
name = "sec-sig-verify"
category = "sec"
description = "Signature verification"
requires = ["fs-erofs-read"]
provides = ["sig-verified"]
priority = 25
timeout_ms = 5000
restart_policy = "never"
max_restarts = 0
log_level = "debug"
critical = true
"#;

pub const SEC_ENTROPY_POOL: &str = r#"
[process]
name = "sec-entropy-pool"
category = "sec"
description = "Entropy pool initialization"
priority = 5
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "debug"
"#;

pub const SEC_USER_AUTH: &str = r#"
[process]
name = "sec-user-auth"
category = "sec"
description = "User authentication"
requires = ["sec-sig-verify", "sec-entropy-pool"]
priority = 60
timeout_ms = 5000
restart_policy = "on-failure"
max_restarts = 3
log_level = "info"
"#;

pub const SEC_SESSION_CREATE: &str = r#"
[process]
name = "sec-session-create"
category = "sec"
description = "Session creation"
requires = ["sec-user-auth"]
priority = 65
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const SEC_AUDIT_LOG: &str = r#"
[process]
name = "sec-audit-log"
category = "sec"
description = "Audit logging"
priority = 10
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const SEC_SECCOMP_FILTER: &str = r#"
[process]
name = "sec-seccomp-filter"
category = "sec"
description = "Seccomp syscall filtering"
requires = ["sys-syscall-gate"]
priority = 30
timeout_ms = 500
restart_policy = "never"
max_restarts = 0
log_level = "debug"
critical = true
"#;

pub const MEM_HEAP_INIT: &str = r#"
[process]
name = "mem-heap-init"
category = "mem"
description = "Heap allocator initialization"
priority = 2
timeout_ms = 500
restart_policy = "never"
max_restarts = 0
log_level = "debug"
critical = true
"#;

pub const MEM_PAGE_ALLOC: &str = r#"
[process]
name = "mem-page-alloc"
category = "mem"
description = "Page allocation"
requires = ["mem-heap-init"]
priority = 15
timeout_ms = 500
restart_policy = "on-failure"
max_restarts = 2
log_level = "debug"
"#;

pub const MEM_ZERO_PAGE: &str = r#"
[process]
name = "mem-zero-page"
category = "mem"
description = "Zero page allocation"
requires = ["mem-heap-init"]
priority = 16
timeout_ms = 500
restart_policy = "on-failure"
max_restarts = 2
log_level = "trace"
"#;

pub const MEM_OOM_KILLER: &str = r#"
[process]
name = "mem-oom-killer"
category = "mem"
description = "OOM killer"
requires = ["mem-heap-init", "sys-sched-init"]
priority = 50
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 3
log_level = "info"
"#;

pub const NET_STACK_INIT: &str = r#"
[process]
name = "net-stack-init"
category = "net"
description = "Network stack initialization"
priority = 20
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "debug"
"#;

pub const NET_DHCP_CLIENT: &str = r#"
[process]
name = "net-dhcp-client"
category = "net"
description = "DHCP client"
requires = ["net-stack-init", "drv-net-rtl8139"]
priority = 40
timeout_ms = 5000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const NET_IP_ROUTE: &str = r#"
[process]
name = "net-ip-route"
category = "net"
description = "IP routing table"
requires = ["net-stack-init", "net-dhcp-client"]
priority = 45
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const NET_DNS_RESOLVE: &str = r#"
[process]
name = "net-dns-resolve"
category = "net"
description = "DNS resolution"
requires = ["net-ip-route"]
priority = 50
timeout_ms = 3000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const LOG_RING_INIT: &str = r#"
[process]
name = "log-ring-init"
category = "log"
description = "Log ring buffer initialization"
priority = 1
timeout_ms = 500
restart_policy = "never"
max_restarts = 0
log_level = "debug"
critical = true
"#;

pub const LOG_RING_PUSH: &str = r#"
[process]
name = "log-ring-push"
category = "log"
description = "Push event to log ring"
requires = ["log-ring-init"]
priority = 10
timeout_ms = 100
restart_policy = "on-failure"
max_restarts = 3
log_level = "trace"
"#;

pub const LOG_FLUSH_WORKER: &str = r#"
[process]
name = "log-flush-worker"
category = "log"
description = "Flush logs to disk"
requires = ["log-ring-init", "fs-ext2-mount"]
priority = 35
timeout_ms = 2000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const LOG_HEARTBEAT_MON: &str = r#"
[process]
name = "log-heartbeat-mon"
category = "log"
description = "Heartbeat monitor"
requires = ["sys-sched-init"]
priority = 12
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 3
log_level = "debug"
"#;

pub const LOG_TRACE_COLLECT: &str = r#"
[process]
name = "log-trace-collect"
category = "log"
description = "Trace collection"
requires = ["log-ring-init"]
priority = 55
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 1
log_level = "trace"
"#;

pub const LOG_CRASH_DUMP: &str = r#"
[process]
name = "log-crash-dump"
category = "log"
description = "Crash dump on panic"
priority = 5
timeout_ms = 5000
restart_policy = "never"
max_restarts = 0
log_level = "error"
"#;

pub const SYS_SCHED_INIT: &str = r#"
[process]
name = "sys-sched-init"
category = "sys"
description = "Scheduler initialization"
priority = 3
timeout_ms = 500
restart_policy = "never"
max_restarts = 0
log_level = "debug"
critical = true
"#;

pub const SYS_IRQ_ROUTER: &str = r#"
[process]
name = "sys-irq-router"
category = "sys"
description = "IRQ routing"
priority = 4
timeout_ms = 500
restart_policy = "never"
max_restarts = 0
log_level = "debug"
critical = true
"#;

pub const SYS_SYSCALL_GATE: &str = r#"
[process]
name = "sys-syscall-gate"
category = "sys"
description = "Syscall gate"
requires = ["sys-sched-init"]
priority = 10
timeout_ms = 500
restart_policy = "never"
max_restarts = 0
log_level = "debug"
critical = true
"#;

pub const SYS_TIME_SYNC: &str = r#"
[process]
name = "sys-time-sync"
category = "sys"
description = "Time synchronization"
requires = ["drv-rtc-sync", "net-dns-resolve"]
priority = 50
timeout_ms = 3000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const SYS_REBOOT_HANDLER: &str = r#"
[process]
name = "sys-reboot-handler"
category = "sys"
description = "Reboot handler"
priority = 80
timeout_ms = 500
restart_policy = "never"
max_restarts = 0
log_level = "info"
"#;

pub const SYS_SHUTDOWN_HANDLER: &str = r#"
[process]
name = "sys-shutdown-handler"
category = "sys"
description = "Shutdown handler"
priority = 80
timeout_ms = 500
restart_policy = "never"
max_restarts = 0
log_level = "info"
"#;

pub const UI_FB_INIT: &str = r#"
[process]
name = "ui-fb-init"
category = "ui"
description = "Framebuffer initialization for UI"
requires = ["drv-fb-write"]
priority = 30
timeout_ms = 500
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const UI_INPUT_ROUTE: &str = r#"
[process]
name = "ui-input-route"
category = "ui"
description = "Input routing"
requires = ["drv-kbd-init", "drv-mouse-init"]
priority = 40
timeout_ms = 500
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const UI_KEYMAP_LOAD: &str = r#"
[process]
name = "ui-keymap-load"
category = "ui"
description = "Keyboard layout loading"
requires = ["drv-kbd-init", "fs-erofs-read"]
priority = 45
timeout_ms = 500
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const UI_COMPOSITOR_PREP: &str = r#"
[process]
name = "ui-compositor-prep"
category = "ui"
description = "Compositor preparation"
requires = ["ui-fb-init", "ui-input-route"]
priority = 55
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

pub const SVC_INIT_BRIDGE: &str = r#"
[process]
name = "svc-init-bridge"
category = "svc"
description = "Bridge to Dinit supervisor"
requires = ["fs-vfs-router", "sys-sched-init", "sec-seccomp-filter"]
provides = ["dinit-bridge"]
priority = 70
timeout_ms = 2000
restart_policy = "on-failure"
max_restarts = 3
log_level = "debug"
"#;

pub const SVC_UDEV_EVENT: &str = r#"
[process]
name = "svc-udev-event"
category = "svc"
description = "Udev event handling"
requires = ["svc-init-bridge", "drv-pci-enum"]
priority = 75
timeout_ms = 1000
restart_policy = "on-failure"
max_restarts = 2
log_level = "info"
"#;

/// All 50 manifest strings.
pub const ALL_MANIFESTS: &[&str] = &[
    DRV_ATA_RESET, DRV_ATA_READ, DRV_KBD_INIT, DRV_MOUSE_INIT, DRV_NET_RTL8139,
    DRV_VGA_MODE, DRV_FB_WRITE, DRV_PCI_ENUM, DRV_RTC_SYNC, DRV_APIC_INIT,
    FS_EROFS_MOUNT, FS_EROFS_READ, FS_EXT2_MOUNT, FS_VFS_ROUTER, FS_PATH_RESOLVE,
    FS_STAT_WORKER, FS_DIR_ENUM, FS_JOURNAL_REPLAY,
    SEC_SIG_VERIFY, SEC_ENTROPY_POOL, SEC_USER_AUTH, SEC_SESSION_CREATE,
    SEC_AUDIT_LOG, SEC_SECCOMP_FILTER,
    MEM_HEAP_INIT, MEM_PAGE_ALLOC, MEM_ZERO_PAGE, MEM_OOM_KILLER,
    NET_STACK_INIT, NET_DHCP_CLIENT, NET_IP_ROUTE, NET_DNS_RESOLVE,
    LOG_RING_INIT, LOG_RING_PUSH, LOG_FLUSH_WORKER, LOG_HEARTBEAT_MON,
    LOG_TRACE_COLLECT, LOG_CRASH_DUMP,
    SYS_SCHED_INIT, SYS_IRQ_ROUTER, SYS_SYSCALL_GATE, SYS_TIME_SYNC,
    SYS_REBOOT_HANDLER, SYS_SHUTDOWN_HANDLER,
    UI_FB_INIT, UI_INPUT_ROUTE, UI_KEYMAP_LOAD, UI_COMPOSITOR_PREP,
    SVC_INIT_BRIDGE, SVC_UDEV_EVENT,
];

/// Parse all manifests into ProcessManifest structs.
pub fn parse_all() -> Result<Vec<ProcessManifest>, String> {
    let mut result = Vec::with_capacity(ALL_MANIFESTS.len());
    for manifest_str in ALL_MANIFESTS {
        let m = ProcessManifest::parse(manifest_str)
            .map_err(|e| alloc::format!("failed to parse {}: {}", manifest_str, e))?;
        result.push(m);
    }
    Ok(result)
}

/// Get the example manifest for fs-erofs-mount (for documentation).
pub fn example_fs_erofs_mount() -> &'static str {
    FS_EROFS_MOUNT
}
