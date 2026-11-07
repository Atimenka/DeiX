//! Nano-Core Orchestrator — the "conductor" of the boot process.
//!
//! The orchestrator is a lightweight (~50 KB) process loaded by the
//! boot chain. It does NOT do any actual work itself — it reads the
//! DAG manifests, spawns Nano-Threads in parallel, monitors their
//! heartbeats, restarts failed processes, and logs every event.
//!
//! Key APIs:
//!   - spawn_process(name, caps) -> Result<Pid>
//!   - monitor_heartbeat(pid) -> bool
//!   - restart_on_failure(pid, max) -> Result<()>
//!   - build_dag(dir) -> Dag
//!   - log_start(name), log_action(name, detail), log_end(name)
//!
//! The orchestrator runs in the kernel's idle loop, checking for
//! processes that are ready to start and processes that need
//! heartbeat checks.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::nanocore::dag::Dag;
use crate::nanocore::manifest::{LogLevel, ProcessManifest};
use crate::nanocore::process::{NanoThread, ProcessState};
use crate::nanocore::heartbeat::HeartbeatTracker;
use crate::nanocore::log;
use crate::spinlock::SpinLock;

/// Max Nano-Threads in the system.
pub const MAX_THREADS: usize = 128;

/// The orchestrator's process ID.
pub const ORCHESTRATOR_PID: u32 = 1;

/// Current run mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Normal,   // All 50 processes
    Fast,     // Only critical (~15)
    Safe,     // No UI and net (~35)
    Debug,    // All + verbose logs
    Recovery, // Only fs-read + log + sys
}

impl RunMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "fast" => RunMode::Fast,
            "safe" => RunMode::Safe,
            "debug" => RunMode::Debug,
            "recovery" => RunMode::Recovery,
            _ => RunMode::Normal,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RunMode::Normal => "normal",
            RunMode::Fast => "fast",
            RunMode::Safe => "safe",
            RunMode::Debug => "debug",
            RunMode::Recovery => "recovery",
        }
    }
}

/// Current boot phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootPhase {
    Initializing,
    Launching,
    Monitoring,
    Complete,
    Panic,
}

impl BootPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            BootPhase::Initializing => "INITIALIZING",
            BootPhase::Launching => "LAUNCHING",
            BootPhase::Monitoring => "MONITORING",
            BootPhase::Complete => "COMPLETE",
            BootPhase::Panic => "PANIC",
        }
    }
}

/// The Nano-Core Orchestrator.
pub struct Orchestrator {
    processes: SpinLock<Vec<NanoThread>>,
    dag: SpinLock<Dag>,
    heartbeats: SpinLock<BTreeMap<u32, HeartbeatTracker>>,
    phase: SpinLock<BootPhase>,
    mode: RunMode,
    next_pid: SpinLock<u32>,
    started_ms: u64,
    completed_count: SpinLock<u32>,
    total_count: SpinLock<u32>,
}

impl Orchestrator {
    pub const fn new() -> Self {
        Orchestrator {
            processes: SpinLock::new(Vec::new()),
            dag: SpinLock::new(Dag::new()),
            heartbeats: SpinLock::new(BTreeMap::new()),
            phase: SpinLock::new(BootPhase::Initializing),
            mode: RunMode::Normal,
            next_pid: SpinLock::new(2), // PID 1 is orchestrator itself
            started_ms: 0,
            completed_count: SpinLock::new(0),
            total_count: SpinLock::new(0),
        }
    }

    pub fn set_mode(&mut self, mode: RunMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> RunMode {
        self.mode
    }

    pub fn phase(&self) -> BootPhase {
        *self.phase.lock()
    }

    pub fn set_phase(&self, phase: BootPhase) {
        *self.phase.lock() = phase;
    }

    pub fn started_ms(&self) -> u64 {
        self.started_ms
    }

    /// Initialize the orchestrator with all manifests.
    pub fn init(&mut self, manifests: &[ProcessManifest]) {
        self.started_ms = log::now_ms();
        log::set_boot_time(self.started_ms);

        // Build DAG
        let mut dag = self.dag.lock();
        for m in manifests {
            dag.add_node(&m.name, &m.requires);
        }
        drop(dag);

        // Create process entries
        let mut procs = self.processes.lock();
        procs.reserve(manifests.len());
        let mut next_pid = self.next_pid.lock();
        for m in manifests {
            if !self.should_run(m) {
                continue;
            }
            let pid = *next_pid;
            *next_pid += 1;
            let thread = NanoThread::new(m.clone(), pid, self.started_ms);
            procs.push(thread);
        }
        let total = procs.len() as u32;
        *self.total_count.lock() = total;

        // Initialize heartbeat trackers
        let mut hbs = self.heartbeats.lock();
        for p in procs.iter() {
            hbs.insert(p.pid, HeartbeatTracker::new(self.started_ms));
        }

        self.set_phase(BootPhase::Launching);
        log::log(
            ORCHESTRATOR_PID,
            "nano-core",
            LogLevel::Info,
            &alloc::format!(
                "orchestrator started ({} procs, mode={}, dag_nodes={})",
                total, self.mode.as_str(), self.dag.lock().nodes().len()
            ),
        );
    }

    /// Check if a process should run in the current mode.
    fn should_run(&self, m: &ProcessManifest) -> bool {
        match self.mode {
            RunMode::Normal => true,
            RunMode::Debug => true,
            RunMode::Fast => {
                // Only critical + early processes
                m.priority <= 30
            }
            RunMode::Safe => {
                // No UI and net
                m.category != "ui" && m.category != "net"
            }
            RunMode::Recovery => {
                // Only fs-read + log + sys
                m.category == "fs" || m.category == "log" || m.category == "sys"
            }
        }
    }

    /// Build the DAG from a directory of manifests (or embedded manifests).
    pub fn build_dag(&mut self, manifests: &[ProcessManifest]) -> Result<(), String> {
        let mut dag = self.dag.lock();
        for m in manifests {
            dag.add_node(&m.name, &m.requires);
        }
        Ok(())
    }

    /// Spawn a process by name.
    pub fn spawn_process(&self, name: &str) -> Result<u32, String> {
        let mut procs = self.processes.lock();
        let mut next_pid = self.next_pid.lock();
        let now_ms = log::now_ms();

        for p in procs.iter_mut() {
            if p.manifest.name == name && p.state == ProcessState::Pending {
                p.start(now_ms);
                let pid = p.pid;
                // Initialize heartbeat
                {
                    let mut hbs = self.heartbeats.lock();
                    hbs.insert(pid, HeartbeatTracker::new(now_ms));
                }
                log::log_start(pid, name);
                log::log_action(pid, name, "spawned");
                return Ok(pid);
            }
        }

        // Not found — spawn a new process from manifest
        let manifests = crate::nanocore::manifests::parse_all()
            .map_err(|e| alloc::format!("parse manifests: {}", e))?;
        for m in &manifests {
            if m.name == name {
                let pid = *next_pid;
                *next_pid += 1;
                let mut thread = NanoThread::new(m.clone(), pid, now_ms);
                thread.start(now_ms);
                procs.push(thread);
                {
                    let mut hbs = self.heartbeats.lock();
                    hbs.insert(pid, HeartbeatTracker::new(now_ms));
                }
                log::log_start(pid, name);
                log::log_action(pid, name, "spawned");
                return Ok(pid);
            }
        }

        Err(alloc::format!("process not found: {}", name))
    }

    /// Spawn a process with specific capabilities.
    pub fn spawn_process_with_caps(
        &self,
        name: &str,
        caps: &[&str],
    ) -> Result<u32, String> {
        // For now, caps are informational. In a real implementation,
        // they would be passed to seccomp.
        let _ = caps;
        self.spawn_process(name)
    }

    /// Monitor heartbeat for a process.
    pub fn monitor_heartbeat(&self, pid: u32) -> bool {
        let now_ms = log::now_ms();
        let mut hbs = self.heartbeats.lock();
        match hbs.get_mut(&pid) {
            Some(hb) => hb.check(now_ms),
            None => false,
        }
    }

    /// Check all heartbeats and restart failed processes.
    pub fn check_all_heartbeats(&self) {
        let now_ms = log::now_ms();
        let mut to_restart = Vec::new();
        let mut critical_fail = false;

        // Check heartbeats
        {
            let mut hbs = self.heartbeats.lock();
            let mut procs = self.processes.lock();
            for p in procs.iter_mut() {
                if p.state != ProcessState::Running && p.state != ProcessState::Waiting {
                    continue;
                }
                if let Some(hb) = hbs.get_mut(&p.pid) {
                    if !hb.check(now_ms) {
                        if p.is_critical() {
                            log::log_error(
                                p.pid,
                                &p.manifest.name,
                                "CRITICAL: heartbeat lost",
                            );
                            critical_fail = true;
                        } else {
                            log::log_error(
                                p.pid,
                                &p.manifest.name,
                                &alloc::format!("heartbeat lost ({} missed)", hb.missed_count),
                            );
                            to_restart.push(p.pid);
                        }
                    }
                }
            }
        }

        if critical_fail {
            self.set_phase(BootPhase::Panic);
            log::log_error(
                ORCHESTRATOR_PID,
                "nano-core",
                "PANIC: critical process heartbeat lost",
            );
            return;
        }

        // Restart non-critical failed processes
        for pid in to_restart {
            self.restart_on_failure(pid, 3);
        }
    }

    /// Restart a failed process.
    pub fn restart_on_failure(&self, pid: u32, max: u32) -> Result<(), String> {
        let mut procs = self.processes.lock();
        let now_ms = log::now_ms();

        for p in procs.iter_mut() {
            if p.pid == pid {
                if p.restart_count >= max {
                    log::log_error(
                        p.pid,
                        &p.manifest.name,
                        &alloc::format!("restart limit {} reached", max),
                    );
                    return Err(alloc::format!(
                        "restart limit {} reached for {}",
                        max, p.manifest.name
                    ));
                }
                p.mark_restarting(now_ms);
                log::log_action(p.pid, &p.manifest.name, "RESTARTING");
                // Reset heartbeat
                {
                    let mut hbs = self.heartbeats.lock();
                    hbs.insert(pid, HeartbeatTracker::new(now_ms));
                }
                return Ok(());
            }
        }
        Err(alloc::format!("process not found: {}", pid))
    }

    /// Mark a process as running.
    pub fn mark_running(&self, pid: u32, name: &str, detail: &str) {
        let now_ms = log::now_ms();
        let mut procs = self.processes.lock();
        for p in procs.iter_mut() {
            if p.pid == pid {
                p.mark_running(now_ms);
                log::log_action(pid, name, detail);
                break;
            }
        }
    }

    /// Mark a process as exited.
    pub fn mark_exited(&self, pid: u32, name: &str, detail: &str) {
        let now_ms = log::now_ms();
        let mut procs = self.processes.lock();
        let mut completed = false;
        for p in procs.iter_mut() {
            if p.pid == pid {
                p.mark_exited(now_ms);
                log::log_end(pid, name, detail);
                *self.completed_count.lock() += 1;
                completed = true;
                break;
            }
        }
        if completed && self.all_done() {
            self.set_phase(BootPhase::Complete);
            log::log(
                ORCHESTRATOR_PID,
                "nano-core",
                LogLevel::Info,
                &alloc::format!(
                    "DAG complete: {}/{} procs",
                    self.completed_count.lock(),
                    self.total_count.lock()
                ),
            );
            log::log(
                ORCHESTRATOR_PID,
                "nano-core",
                LogLevel::Info,
                "handing off to Dinit supervisor",
            );
        }
    }

    /// Mark a process as failed.
    pub fn mark_failed(&self, pid: u32, name: &str, error: &str) {
        let now_ms = log::now_ms();
        let mut procs = self.processes.lock();
        for p in procs.iter_mut() {
            if p.pid == pid {
                p.mark_failed(now_ms, error);
                log::log_error(pid, name, error);
                // Check if critical and degraded
                if p.is_critical() && !p.should_restart() {
                    self.set_phase(BootPhase::Panic);
                    log::log_error(
                        ORCHESTRATOR_PID,
                        "nano-core",
                        "PANIC: critical process failed, cannot degrade",
                    );
                }
                break;
            }
        }
    }

    /// Check if all processes are done.
    pub fn all_done(&self) -> bool {
        let procs = self.processes.lock();
        let total = *self.total_count.lock();
        *self.completed_count.lock() >= total && !procs.is_empty()
    }

    /// Get completed count.
    pub fn completed_count(&self) -> u32 {
        *self.completed_count.lock()
    }

    /// Get total count.
    pub fn total_count(&self) -> u32 {
        *self.total_count.lock()
    }

    /// Get a process by name.
    pub fn get_process(&self, name: &str) -> Option<NanoThread> {
        let procs = self.processes.lock();
        procs.iter().find(|p| p.manifest.name == name).cloned()
    }

    /// Get all processes.
    pub fn get_processes(&self) -> Vec<NanoThread> {
        self.processes.lock().iter().cloned().collect()
    }

    /// Get process stats.
    pub fn get_stats(&self) -> crate::nanocore::process::ProcessStats {
        let procs = self.processes.lock();
        let refs: Vec<&NanoThread> = procs.iter().collect();
        crate::nanocore::process::compute_stats(&refs)
    }

    /// Get the DAG.
    pub fn get_dag(&self) -> Dag {
        self.dag.lock().clone()
    }

    /// Get the DAG waves (parallel groups).
    pub fn get_dag_waves(&self) -> Vec<Vec<String>> {
        self.dag.lock().waves()
    }

    /// Run the orchestrator's main loop step.
    /// Called from the kernel idle loop.
    pub fn step(&self) {
        // 1. Check heartbeats
        if self.phase() == BootPhase::Monitoring || self.phase() == BootPhase::Launching {
            self.check_all_heartbeats();
        }
    }

    /// Launch ready processes.
    pub fn launch_ready(&self) {
        let now_ms = log::now_ms();
        let completed: Vec<&str> = {
            let procs = self.processes.lock();
            procs
                .iter()
                .filter(|p| p.state == ProcessState::Exited)
                .map(|p| p.manifest.name.as_str())
                .collect()
        };

        let ready = self.dag.lock().ready_set(&completed);
        for name in ready {
            if let Ok(pid) = self.spawn_process(&name) {
                // Mark as running immediately (in real impl, process would
                // signal when it's actually running).
                self.mark_running(pid, &name, "deps ok, launched");
            }
        }

        // If all are launched, switch to monitoring
        if self.phase() == BootPhase::Launching {
            let procs = self.processes.lock();
            let all_launched = procs.iter().all(|p| {
                p.state == ProcessState::Running
                    || p.state == ProcessState::Exited
                    || p.state == ProcessState::Waiting
                    || p.state == ProcessState::Failed
                    || p.state == ProcessState::Panic
            });
            drop(procs);
            if all_launched {
                self.set_phase(BootPhase::Monitoring);
                log::log(
                    ORCHESTRATOR_PID,
                    "nano-core",
                    LogLevel::Info,
                    "all processes launched, entering monitoring",
                );
            }
        }
    }

    /// Run the full boot sequence.
    pub fn boot(&mut self) {
        self.set_phase(BootPhase::Initializing);

        // Parse manifests
        let manifests = match crate::nanocore::manifests::parse_all() {
            Ok(m) => m,
            Err(e) => {
                log::log_error(ORCHESTRATOR_PID, "nano-core", &alloc::format!("manifest parse error: {}", e));
                self.set_phase(BootPhase::Panic);
                return;
            }
        };

        // Initialize
        self.init(&manifests);

        // Launch ready processes in waves
        loop {
            if self.phase() == BootPhase::Panic || self.phase() == BootPhase::Complete {
                break;
            }
            self.launch_ready();
            // In a real kernel, we'd yield to the scheduler here.
            // For now, we just iterate.
            if self.phase() == BootPhase::Monitoring {
                break;
            }
        }

        // Log boot summary
        let stats = self.get_stats();
        log::log(
            ORCHESTRATOR_PID,
            "nano-core",
            LogLevel::Info,
            &alloc::format!(
                "boot summary: {}/{} running, {} exited, {} failed, {} restarts",
                stats.running, stats.total, stats.exited, stats.failed, stats.total_restarts
            ),
        );
    }
}

impl core::default::Default for Orchestrator {
    fn default() -> Self {
        Self::new()
    }
}

/// Global orchestrator instance.
pub static ORCHESTRATOR: SpinLock<Option<Orchestrator>> = SpinLock::new(None);

/// Initialize the global orchestrator.
pub fn init_global() {
    let mut guard = ORCHESTRATOR.lock();
    if guard.is_none() {
        *guard = Some(Orchestrator::new());
    }
}

/// Get a reference to the global orchestrator inside a closure.
/// If not initialized, the closure is called with a fresh instance.
pub fn with_orchestrator<F: FnOnce(&Orchestrator)>(f: F) {
    let guard = ORCHESTRATOR.lock();
    match guard.as_ref() {
        Some(orch) => f(orch),
        None => {
            // Not initialized — show a message
            crate::println!("Nano-Core not initialized.");
        }
    }
}

/// Get a mutable reference to the global orchestrator.
pub fn with_orchestrator_mut<F: FnOnce(&mut Orchestrator)>(f: F) {
    let mut guard = ORCHESTRATOR.lock();
    if let Some(ref mut orch) = *guard {
        f(orch);
    } else {
        crate::println!("Nano-Core not initialized.");
    }
}
