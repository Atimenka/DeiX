//! Nano-Thread process state management.
//!
//! Each Nano-Thread is a lightweight isolated process with its own
//! lifecycle: Pending → Starting → Running → Idle/End/Failed.
//!
//! State transitions are logged via the log ring for observability.

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::nanocore::manifest::{ProcessManifest, RestartPolicy};

/// Current state of a Nano-Thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Not yet started (dependencies not satisfied).
    Pending,
    /// Currently starting up.
    Starting,
    /// Running and healthy.
    Running,
    /// Waiting for a trigger or event.
    Waiting,
    /// Completed successfully.
    Exited,
    /// Failed and no more restarts.
    Failed,
    /// Failed but will be restarted.
    Restarting,
    /// Crashed critically (cannot be restarted).
    Panic,
}

impl ProcessState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProcessState::Pending => "PENDING",
            ProcessState::Starting => "STARTING",
            ProcessState::Running => "RUNNING",
            ProcessState::Waiting => "WAITING",
            ProcessState::Exited => "EXITED",
            ProcessState::Failed => "FAILED",
            ProcessState::Restarting => "RESTARTING",
            ProcessState::Panic => "PANIC",
        }
    }

    /// Is this process in a "healthy" state (not failed, not panicking)?
    pub fn is_healthy(&self) -> bool {
        matches!(self, ProcessState::Running | ProcessState::Waiting | ProcessState::Exited)
    }

    /// Is this process in a terminal state?
    pub fn is_terminal(&self) -> bool {
        matches!(self, ProcessState::Exited | ProcessState::Failed | ProcessState::Panic)
    }
}

/// A live Nano-Thread process.
#[derive(Clone)]
pub struct NanoThread {
    pub manifest: ProcessManifest,
    pub state: ProcessState,
    pub pid: u32,
    pub restart_count: u32,
    pub last_heartbeat: u64,
    pub heartbeat_missed: u32,
    pub actions_logged: u32,
    pub start_time_ms: u64,
    pub exit_time_ms: Option<u64>,
    pub error: Option<String>,
}

impl NanoThread {
    pub fn new(manifest: ProcessManifest, pid: u32, now_ms: u64) -> Self {
        NanoThread {
            manifest,
            state: ProcessState::Pending,
            pid,
            restart_count: 0,
            last_heartbeat: now_ms,
            heartbeat_missed: 0,
            actions_logged: 0,
            start_time_ms: now_ms,
            exit_time_ms: None,
            error: None,
        }
    }

    pub fn start(&mut self, now_ms: u64) {
        self.state = ProcessState::Starting;
        self.start_time_ms = now_ms;
        self.last_heartbeat = now_ms;
        self.heartbeat_missed = 0;
    }

    pub fn mark_running(&mut self, now_ms: u64) {
        self.state = ProcessState::Running;
        self.last_heartbeat = now_ms;
        self.heartbeat_missed = 0;
    }

    pub fn mark_waiting(&mut self, now_ms: u64) {
        self.state = ProcessState::Waiting;
        self.last_heartbeat = now_ms;
    }

    pub fn mark_exited(&mut self, now_ms: u64) {
        self.state = ProcessState::Exited;
        self.exit_time_ms = Some(now_ms);
    }

    pub fn mark_failed(&mut self, now_ms: u64, error: &str) {
        self.state = ProcessState::Failed;
        self.exit_time_ms = Some(now_ms);
        self.error = Some(error.to_string());
    }

    pub fn mark_restarting(&mut self, now_ms: u64) {
        self.state = ProcessState::Restarting;
        self.restart_count += 1;
        self.heartbeat_missed = 0;
        self.last_heartbeat = now_ms;
        self.error = None;
    }

    pub fn mark_panic(&mut self, now_ms: u64, error: &str) {
        self.state = ProcessState::Panic;
        self.exit_time_ms = Some(now_ms);
        self.error = Some(error.to_string());
    }

    pub fn heartbeat(&mut self, now_ms: u64) {
        self.last_heartbeat = now_ms;
        self.heartbeat_missed = 0;
    }

    pub fn check_heartbeat(&mut self, now_ms: u64, timeout_ms: u64) -> bool {
        if now_ms >= self.last_heartbeat + timeout_ms {
            self.heartbeat_missed += 1;
            false
        } else {
            true
        }
    }

    /// Should this process be restarted?
    pub fn should_restart(&self) -> bool {
        match self.manifest.restart_policy {
            RestartPolicy::Always => true,
            RestartPolicy::Never => false,
            RestartPolicy::OnFailure => {
                self.state == ProcessState::Failed && self.restart_count < self.manifest.max_restarts
            }
        }
    }

    /// Is this process critical (cannot be degraded)?
    pub fn is_critical(&self) -> bool {
        self.manifest.critical
    }

    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    pub fn is_healthy(&self) -> bool {
        self.state.is_healthy()
    }
}

/// Aggregate stats for all Nano-Threads.
#[derive(Debug, Clone)]
pub struct ProcessStats {
    pub total: usize,
    pub running: usize,
    pub pending: usize,
    pub exited: usize,
    pub failed: usize,
    pub panicked: usize,
    pub restarting: usize,
    pub waiting: usize,
    pub starting: usize,
    pub total_restarts: u32,
    pub total_actions: u32,
}

/// Compute stats from a slice of processes.
pub fn compute_stats(procs: &[&NanoThread]) -> ProcessStats {
    let mut stats = ProcessStats {
        total: procs.len(),
        running: 0,
        pending: 0,
        exited: 0,
        failed: 0,
        panicked: 0,
        restarting: 0,
        waiting: 0,
        starting: 0,
        total_restarts: 0,
        total_actions: 0,
    };
    for p in procs {
        match p.state {
            ProcessState::Running => stats.running += 1,
            ProcessState::Pending => stats.pending += 1,
            ProcessState::Exited => stats.exited += 1,
            ProcessState::Failed => stats.failed += 1,
            ProcessState::Panic => stats.panicked += 1,
            ProcessState::Restarting => stats.restarting += 1,
            ProcessState::Waiting => stats.waiting += 1,
            ProcessState::Starting => stats.starting += 1,
        }
        stats.total_restarts += p.restart_count;
        stats.total_actions += p.actions_logged;
    }
    stats
}
