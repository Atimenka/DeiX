//! Manifest parsing for Nano-Threads.
//!
//! Each Nano-Thread is described by a simple TOML-like manifest:
//!
//! ```toml
//! [process]
//! name = "fs-erofs-mount"
//! requires = ["drv-ata-reset", "mem-heap-init"]
//! provides = ["mount-erofs"]
//! priority = 50
//! timeout_ms = 2000
//! restart_policy = "on-failure"
//! max_restarts = 3
//! log_level = "debug"
//! ```
//!
//! No external crates — we parse line-by-line into [`ProcessManifest`].

use alloc::string::String;
use alloc::vec::Vec;

/// Restart policy for a Nano-Thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    Always,
    OnFailure,
    Never,
}

impl RestartPolicy {
    pub fn from_str(s: &str) -> Self {
        match s {
            "always" => RestartPolicy::Always,
            "never" => RestartPolicy::Never,
            _ => RestartPolicy::OnFailure,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RestartPolicy::Always => "always",
            RestartPolicy::OnFailure => "on-failure",
            RestartPolicy::Never => "never",
        }
    }
}

/// Log level for a Nano-Thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn from_str(s: &str) -> Self {
        match s {
            "trace" => LogLevel::Trace,
            "debug" => LogLevel::Debug,
            "warn" => LogLevel::Warn,
            "error" => LogLevel::Error,
            _ => LogLevel::Info,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

/// Priority tiers: lower number = earlier launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Critical, // 0-10 — must complete before anything else
    Early,    // 11-30 — boot-critical infrastructure
    Normal,   // 31-60 — standard services
    Late,     // 61-80 — user-space-ish
    Background, // 81-100 — nice-to-have
}

impl Priority {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0..=10 => Priority::Critical,
            11..=30 => Priority::Early,
            31..=60 => Priority::Normal,
            61..=80 => Priority::Late,
            _ => Priority::Background,
        }
    }

    pub fn value(&self) -> u8 {
        match self {
            Priority::Critical => 5,
            Priority::Early => 20,
            Priority::Normal => 50,
            Priority::Late => 70,
            Priority::Background => 90,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Priority::Critical => "critical",
            Priority::Early => "early",
            Priority::Normal => "normal",
            Priority::Late => "late",
            Priority::Background => "bg",
        }
    }
}

/// Description of a single Nano-Thread.
#[derive(Debug, Clone)]
pub struct ProcessManifest {
    /// Unique name, e.g. "fs-erofs-mount".
    pub name: String,
    /// Category prefix: drv, fs, sec, mem, net, log, sys, ui, svc.
    pub category: String,
    /// Names of processes that must complete before this one starts.
    pub requires: Vec<String>,
    /// Capabilities this process provides (used as DAG nodes).
    pub provides: Vec<String>,
    /// Launch priority (lower = earlier).
    pub priority: u8,
    /// Timeout in milliseconds (0 = no timeout).
    pub timeout_ms: u64,
    /// When to restart on failure.
    pub restart_policy: RestartPolicy,
    /// Max restart attempts.
    pub max_restarts: u32,
    /// Log verbosity.
    pub log_level: LogLevel,
    /// Whether this process is critical (cannot be degraded).
    pub critical: bool,
    /// Short human-readable description.
    pub description: String,
}

impl ProcessManifest {
    /// Parse a manifest from a string.
    pub fn parse(text: &str) -> Result<ProcessManifest, String> {
        let mut name = String::new();
        let mut requires: Vec<String> = Vec::new();
        let mut provides: Vec<String> = Vec::new();
        let mut priority: u8 = 50;
        let mut timeout_ms: u64 = 0;
        let mut restart_policy = RestartPolicy::OnFailure;
        let mut max_restarts: u32 = 3;
        let mut log_level = LogLevel::Info;
        let mut critical = false;
        let mut description = String::new();

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }

            // Split on first '='
            let eq = match line.find('=') {
                Some(i) => i,
                None => continue,
            };
            let key = line[..eq].trim();
            let value = line[eq + 1..].trim();

            match key {
                "name" => name = value.to_string(),
                "requires" => {
                    requires = parse_list(value);
                }
                "provides" => {
                    provides = parse_list(value);
                }
                "priority" => {
                    priority = parse_u8(value).ok_or("invalid priority")?;
                }
                "timeout_ms" => {
                    timeout_ms = parse_u64(value).ok_or("invalid timeout_ms")?;
                }
                "restart_policy" => {
                    restart_policy = RestartPolicy::from_str(value);
                }
                "max_restarts" => {
                    max_restarts = parse_u32(value).ok_or("invalid max_restarts")?;
                }
                "log_level" => {
                    log_level = LogLevel::from_str(value);
                }
                "critical" => {
                    critical = value.eq_ignore_ascii_case("true");
                }
                "description" => {
                    description = value.trim_matches('"').to_string();
                }
                _ => {} // ignore unknown keys
            }
        }

        if name.is_empty() {
            return Err("manifest missing 'name'".to_string());
        }

        // Derive category from name prefix (before first '-').
        let category = name.split('-').next().unwrap_or("misc").to_string();

        Ok(ProcessManifest {
            name,
            category,
            requires,
            provides,
            priority,
            timeout_ms,
            restart_policy,
            max_restarts,
            log_level,
            critical,
            description,
        })
    }

    pub fn priority_tier(&self) -> Priority {
        Priority::from_u8(self.priority)
    }
}

fn parse_list(s: &str) -> Vec<String> {
    let trimmed = s.trim_matches(|c| c == '[' || c == ']');
    trimmed
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_u8(s: &str) -> Option<u8> {
    s.trim().parse::<u8>().ok()
}

fn parse_u32(s: &str) -> Option<u32> {
    s.trim().parse::<u32>().ok()
}

fn parse_u64(s: &str) -> Option<u64> {
    s.trim().parse::<u64>().ok()
}
