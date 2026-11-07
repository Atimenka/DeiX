//! Heartbeat monitoring for Nano-Threads.
//!
//! Each Nano-Thread sends a heartbeat every 500ms. If 3 beats are missed,
//! the process is restarted (up to max_restarts). If 3 restarts fail,
//! the process is marked degraded. Critical processes cannot be degraded —
//! their failure triggers a Nano-Core panic.
//!
//! Heartbeat is checked by the orchestrator in its idle loop.

use crate::nanocore::manifest::LogLevel;

/// Heartbeat interval in milliseconds.
pub const HEARTBEAT_INTERVAL_MS: u64 = 500;
/// Number of missed beats before restart.
pub const HEARTBEAT_MAX_MISSED: u32 = 3;

/// Tracks heartbeat state for a single process.
#[derive(Debug, Clone)]
pub struct HeartbeatTracker {
    pub last_beat_ms: u64,
    pub missed_count: u32,
    pub is_alive: bool,
}

impl HeartbeatTracker {
    pub fn new(now_ms: u64) -> Self {
        HeartbeatTracker {
            last_beat_ms: now_ms,
            missed_count: 0,
            is_alive: true,
        }
    }

    pub fn beat(&mut self, now_ms: u64) {
        self.last_beat_ms = now_ms;
        self.missed_count = 0;
        self.is_alive = true;
    }

    /// Check heartbeat, return true if process is still alive.
    pub fn check(&mut self, now_ms: u64) -> bool {
        if now_ms >= self.last_beat_ms + HEARTBEAT_INTERVAL_MS {
            self.missed_count += 1;
            if self.missed_count >= HEARTBEAT_MAX_MISSED {
                self.is_alive = false;
                return false;
            }
        }
        true
    }

    pub fn should_restart(&self) -> bool {
        !self.is_alive
    }
}

/// Result of a heartbeat check.
pub enum HeartbeatResult {
    /// Process is healthy.
    Healthy,
    /// Process missed heartbeats, should be restarted.
    Restart,
    /// Process is critically dead, cannot be restarted.
    Critical,
}

/// Check heartbeat for a critical process.
pub fn check_critical(
    tracker: &mut HeartbeatTracker,
    pid: u32,
    name: &str,
    now_ms: u64,
) -> HeartbeatResult {
    if tracker.check(now_ms) {
        return HeartbeatResult::Healthy;
    }
    // Critical process is dead — panic.
    crate::nanocore::log::log_error(pid, name, "CRITICAL: heartbeat lost, panic");
    HeartbeatResult::Critical
}

/// Check heartbeat for a non-critical process.
pub fn check_normal(
    tracker: &mut HeartbeatTracker,
    pid: u32,
    name: &str,
    now_ms: u64,
) -> HeartbeatResult {
    if tracker.check(now_ms) {
        return HeartbeatResult::Healthy;
    }
    crate::nanocore::log::log_error(
        pid,
        name,
        alloc::format!("heartbeat lost ({} missed)", tracker.missed_count),
    );
    HeartbeatResult::Restart
}

/// Send a heartbeat (called by each Nano-Thread).
pub fn send_beat(pid: u32, name: &str, now_ms: u64) {
    crate::nanocore::log::log(pid, name, LogLevel::Trace, "heartbeat");
}
