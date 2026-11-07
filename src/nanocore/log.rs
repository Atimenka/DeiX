//! Nano-Core logging system.
//!
//! Every Nano-Thread logs its actions via this module. Logs go through:
//!   1. log-ring-push — in-memory ring buffer (fast, no I/O)
//!   2. log-flush-worker — async flush to disk (/var/log/nanocore.log)
//!   3. Console output — visible during boot via serial/VGA
//!
//! Format: [timestamp_ms] [pid] [name] [level] message
//!
//! The ring buffer is a fixed-size pre-allocated array (no heap allocs
//! on the critical path). Flush to disk happens from the orchestrator's
//! idle loop, not from interrupt context.

use crate::nanocore::manifest::LogLevel;
use crate::spinlock::IrqSpinLock;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

/// Max log entries in the ring buffer.
pub const RING_CAPACITY: usize = 1024;
/// Max message length per log entry.
pub const MAX_MSG_LEN: usize = 256;

/// A single log entry.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp_ms: u64,
    pub pid: u32,
    pub name: String,
    pub level: LogLevel,
    pub message: String,
}

impl LogEntry {
    /// Format as a single-line string: [ts] [pid] [name] [LEVEL] message
    pub fn format(&self) -> String {
        alloc::format!(
            "[{:.3}] [{}] [{}] {} {}",
            self.timestamp_ms as f64 / 1000.0,
            self.pid,
            self.name,
            self.level.as_str().to_uppercase(),
            self.message
        )
    }
}

/// Entry type for the ring buffer (fixed-size, no String for speed).
#[derive(Debug, Clone, Copy)]
struct RingEntry {
    timestamp_ms: u64,
    pid: u32,
    name: [u8; 32],
    name_len: u8,
    level: u8,
    msg: [u8; MAX_MSG_LEN],
    msg_len: u16,
    valid: bool,
}

impl RingEntry {
    const fn empty() -> Self {
        RingEntry {
            timestamp_ms: 0,
            pid: 0,
            name: [0u8; 32],
            name_len: 0,
            level: 0,
            msg: [0u8; MAX_MSG_LEN],
            msg_len: 0,
            valid: false,
        }
    }
}

/// Ring buffer for log entries.
struct LogRing {
    slots: [RingEntry; RING_CAPACITY],
    head: usize,
    filled: usize,
}

impl LogRing {
    const fn new() -> Self {
        LogRing {
            slots: [const { RingEntry::empty() }; RING_CAPACITY],
            head: 0,
            filled: 0,
        }
    }

    fn push(&mut self, entry: LogEntry) {
        let slot = self.head % RING_CAPACITY;
        let e = &mut self.slots[slot];
        e.timestamp_ms = entry.timestamp_ms;
        e.pid = entry.pid;
        let name_bytes = entry.name.as_bytes();
        let copy_len = name_bytes.len().min(31);
        e.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        e.name_len = copy_len as u8;
        e.level = entry.level as u8;
        let msg_bytes = entry.message.as_bytes();
        let msg_copy = msg_bytes.len().min(MAX_MSG_LEN - 1);
        e.msg[..msg_copy].copy_from_slice(&msg_bytes[..msg_copy]);
        e.msg_len = msg_copy as u16;
        e.valid = true;
        self.head = (self.head + 1) % RING_CAPACITY;
        if self.filled < RING_CAPACITY {
            self.filled += 1;
        }
    }

    fn get(&self, idx: usize) -> Option<LogEntry> {
        if idx >= self.filled {
            return None;
        }
        let slot = (self.head - self.filled + idx) % RING_CAPACITY;
        let e = &self.slots[slot];
        if !e.valid {
            return None;
        }
        let name = String::from_utf8_lossy(&e.name[..e.name_len as usize]).to_string();
        let message = String::from_utf8_lossy(&e.msg[..e.msg_len as usize]).to_string();
        Some(LogEntry {
            timestamp_ms: e.timestamp_ms,
            pid: e.pid,
            name,
            level: LogLevel::from_u8(e.level),
            message,
        })
    }

    fn iter(&self) -> impl Iterator<Item = LogEntry> + '_ {
        (0..self.filled).filter_map(move |i| self.get(i))
    }
}

/// Global log ring buffer.
static LOG_RING: IrqSpinLock<LogRing> = IrqSpinLock::new(LogRing::new());

/// Current boot timestamp (ms since boot) — set by orchestrator.
static BOOT_START_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Set the boot start time.
pub fn set_boot_time(ms: u64) {
    BOOT_START_MS.store(ms, core::sync::atomic::Ordering::Relaxed);
}

/// Get current time relative to boot start.
pub fn now_ms() -> u64 {
    crate::timer::uptime_ms()
}

/// Log a message.
pub fn log(pid: u32, name: &str, level: LogLevel, msg: &str) {
    let entry = LogEntry {
        timestamp_ms: now_ms(),
        pid,
        name: name.to_string(),
        level,
        message: msg.to_string(),
    };
    LOG_RING.lock().push(entry.clone());
    // Also output to serial for debugging.
    crate::serial_println!("{}", entry.format());
}

/// Convenience: log START.
pub fn log_start(pid: u32, name: &str) {
    log(pid, name, LogLevel::Info, "START");
}

/// Convenience: log an action.
pub fn log_action(pid: u32, name: &str, detail: &str) {
    log(pid, name, LogLevel::Debug, detail);
}

/// Convenience: log END/IDLE.
pub fn log_end(pid: u32, name: &str, state: &str) {
    log(pid, name, LogLevel::Info, state);
}

/// Log an error.
pub fn log_error(pid: u32, name: &str, detail: &str) {
    log(pid, name, LogLevel::Error, detail);
}

/// Get all log entries (for nanocore logs command).
pub fn get_entries() -> Vec<LogEntry> {
    LOG_RING.lock().iter().collect()
}

/// Get entries filtered by name substring.
pub fn get_entries_filtered(filter: &str) -> Vec<LogEntry> {
    let ring = LOG_RING.lock();
    let entries: Vec<LogEntry> = ring.iter().filter(|e| e.name.contains(filter)).collect();
    entries
}

/// Get entries since a given index (for --follow).
pub fn get_entries_since(idx: usize) -> Vec<LogEntry> {
    let ring = LOG_RING.lock();
    ring.iter().skip(idx).collect()
}

/// Total entries logged.
pub fn total_entries() -> usize {
    LOG_RING.lock().filled
}

/// Reset the log ring (for recovery mode).
pub fn reset() {
    LOG_RING.lock().head = 0;
    LOG_RING.lock().filled = 0;
    for i in 0..RING_CAPACITY {
        LOG_RING.lock().slots[i] = RingEntry::empty();
    }
}
