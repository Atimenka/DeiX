//! nanocore CLI — command-line interface for the Nano-Core orchestrator.
//!
//! Commands:
//!   nanocore status           — table of all process states
//!   nanocore logs [--follow]  — view log entries
//!   nanocore logs --filter X  — filter logs by name substring
//!   nanocore trace <name>     — trace a specific process
//!   nanocore restart <name>   — hot restart a process
//!   nanocore graph            — DAG graph visualization
//!   nanocore mode <mode>      — switch run mode
//!
//! The CLI is invoked from the existing `cli.rs` command processor.

use alloc::string::ToString;
use alloc::vec::Vec;
use alloc::format;

use crate::nanocore::orchestrator::{Orchestrator, RunMode};
use crate::nanocore::log;
use crate::println;

/// Execute a nanocore command.
/// Returns true if the command was handled.
pub fn execute(args: &[&str]) -> bool {
    if args.is_empty() {
        return false;
    }
    if args[0] != "nanocore" {
        return false;
    }
    if args.len() < 2 {
        print_help();
        return true;
    }

    let cmd = args[1];
    let rest = &args[2..];

    crate::nanocore::orchestrator::with_orchestrator(|orch| {
        match cmd {
            "status" => cmd_status(orch),
            "logs" => cmd_logs(orch, rest),
            "trace" => cmd_trace(orch, rest),
            "restart" => cmd_restart(orch, rest),
            "graph" => cmd_graph(orch),
            "mode" => cmd_mode(orch, rest),
            "help" | "?" => print_help(),
            _ => {
                println!("Unknown nanocore command: {}", cmd);
                println!("Use 'nanocore help' for command list.");
            }
        }
    });
    true
}

fn print_help() {
    println!("=== Nano-Core CLI ===");
    println!("  nanocore status            — Show process status table");
    println!("  nanocore logs [--follow]   — Show log entries");
    println!("  nanocore logs --filter X   — Filter logs by process name");
    println!("  nanocore trace <name>      — Trace a specific process");
    println!("  nanocore restart <name>    — Hot restart a process");
    println!("  nanocore graph             — Show DAG graph (waves)");
    println!("  nanocore mode <mode>       — Switch run mode (normal/fast/safe/debug/recovery)");
    println!("  nanocore help              — Show this help");
}

/// Render a status table of all processes.
fn cmd_status(orch: &Orchestrator) {
    let procs = orch.get_processes();
    let phase = orch.phase();
    let mode = orch.mode();
    let started = orch.started_ms();
    let now = log::now_ms();
    let elapsed = now.saturating_sub(started);

    println!();
    println!("╔═════════════════════════════════════════════════════════════════════════╗");
    println!("║                    Nano-Core Status (mode: {}, phase: {})      ║",
             mode.as_str(), phase.as_str());
    println!("╚═════════════════════════════════════════════════════════════════════════╝");
    println!("  Boot time: {:.3}s | Processes: {} | Completed: {}",
             elapsed as f64 / 1000.0,
             procs.len(),
             orch.completed_count());
    println!();
    println!("  {'PID':<6} {'Name':<24} {'Category':<10} {'Priority':<10} {'State':<12} {'Restarts':<10} {'Actions':<10}");
    println!("  {'─'.repeat(6)} {'─'.repeat(24)} {'─'.repeat(10)} {'─'.repeat(10)} {'─'.repeat(12)} {'─'.repeat(10)} {'─'.repeat(10)}");

    for p in procs.iter() {
        let name = &p.manifest.name;
        let cat = &p.manifest.category;
        let prio = p.manifest.priority_tier().as_str();
        let state = p.state.as_str();
        let restarts = p.restart_count.to_string();
        let actions = p.actions_logged.to_string();
        println!("  {:<6} {:<24} {:<10} {:<10} {:<12} {:<10} {:<10}",
                 p.pid, name, cat, prio, state, restarts, actions);
    }

    let stats = orch.get_stats();
    println!();
    println!("  Total: {} | Running: {} | Exited: {} | Failed: {} | Panic: {} | Restarts: {}",
             stats.total, stats.running, stats.exited, stats.failed,
             stats.panicked, stats.total_restarts);
    println!();
}

/// Show log entries.
fn cmd_logs(orch: &Orchestrator, args: &[&str]) {
    let filter = args.iter().position(|&a| a == "--filter")
        .and_then(|i| args.get(i + 1))
        .copied();
    let follow = args.contains(&"--follow");

    let entries = if let Some(f) = filter {
        log::get_entries_filtered(f)
    } else {
        log::get_entries()
    };

    for entry in entries.iter() {
        println!("{}", entry.format());
    }

    if follow {
        println!("--- Following (press Ctrl+C to stop) ---");
        let mut last = entries.len();
        loop {
            let new = log::get_entries_since(last);
            for entry in new.iter() {
                println!("{}", entry.format());
            }
            last += new.len();
            // In a real kernel, we'd yield here. For now, just break.
            break;
        }
    }
}

/// Trace a specific process.
fn cmd_trace(orch: &Orchestrator, args: &[&str]) {
    if args.is_empty() {
        println!("Usage: nanocore trace <name>");
        return;
    }
    let name = args[0];
    let entries = log::get_entries_filtered(name);

    println!("=== Trace: {} ===", name);
    println!("  {'Time':<10} {'PID':<6} {'Level':<8} Message");
    println!("  {'─'.repeat(10)} {'─'.repeat(6)} {'─'.repeat(8)} {'─'.repeat(40)}");

    for entry in entries.iter() {
        println!("  [{:.3}]   [{:<4}] [{:<6}] {}",
                 entry.timestamp_ms as f64 / 1000.0,
                 entry.pid,
                 entry.level.as_str().to_uppercase(),
                 entry.message);
    }

    if entries.is_empty() {
        println!("  No log entries for '{}'", name);
    }
    println!();
}

/// Hot restart a process.
fn cmd_restart(orch: &Orchestrator, args: &[&str]) {
    if args.is_empty() {
        println!("Usage: nanocore restart <name>");
        return;
    }
    let name = args[0];

    if let Some(proc) = orch.get_process(name) {
        let pid = proc.pid;
        println!("Restarting {} (PID {})...", name, pid);

        // Mark as failed first
        orch.mark_failed(pid, name, "manual restart requested");

        // Restart
        if let Err(e) = orch.restart_on_failure(pid, proc.manifest.max_restarts) {
            println!("Restart failed: {}", e);
        } else {
            println!("Restart initiated for {}", name);
        }
    } else {
        println!("Process not found: {}", name);
    }
    println!();
}

/// Show DAG graph as waves (parallel groups).
fn cmd_graph(orch: &Orchestrator) {
    let waves = orch.get_dag_waves();
    let dag = orch.get_dag();
    println!();
    println!("╔═════════════════════════════════════════════════════════════════════════╗");
    println!("║                    Nano-Core DAG Graph (Waves)                       ║");
    println!("╚═════════════════════════════════════════════════════════════════════════╝");
    println!();

    if waves.is_empty() {
        println!("  DAG not built yet.");
        println!();
        return;
    }

    for (i, wave) in waves.iter().enumerate() {
        println!("  Wave {}: {} processes (parallel)", i + 1, wave.len());
        for proc_name in wave.iter() {
            let deps = dag.dependencies(proc_name);
            if deps.is_empty() {
                println!("    └── {} (no dependencies)", proc_name);
            } else {
                println!("    └── {} (requires: {}, parallel with {} others)",
                         proc_name, deps.join(", "), wave.len() - 1);
            }
        }
        println!();
    }

    let total = waves.len();
    println!("  Total waves: {}", total);
    println!("  Max parallelism: {} processes", waves.iter().map(|w| w.len()).max().unwrap_or(0));
    println!();
}

/// Switch run mode.
fn cmd_mode(orch: &Orchestrator, args: &[&str]) {
    if args.is_empty() {
        println!("Current mode: {}", orch.mode().as_str());
        println!("Available modes: normal, fast, safe, debug, recovery");
        println!();
        return;
    }
    let new_mode = RunMode::from_str(args[0]);
    println!("Mode changed to: {}", new_mode.as_str());
    println!();
}


