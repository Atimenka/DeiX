//! Nano-Core CLI — встроенные команды для управления нано-корой.
//!
//! Подключается в CLI ядра как команда `nc`:
//! - `nc` или `nc status` — статус нано-кора (фаза, DAG, heartbeat'и);
//! - `nc boot` — ручной перезапуск boot-цепочки;
//! - `nc tick` — один шаг monitor-цикла (проверка heartbeat'ов);
//! - `nc dag` — отрисовка DAG (последовательность волн);
//! - `nc log` — последние записи журнала нано-кора;
//! - `nc restart <name>` — перезапуск службы;
//! - `nc stop` — остановка нано-кора.

use crate::nanocore::{heartbeat, ipc, log, orchestrator};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Разбить строку аргументов на слова (whitespace-separated).
fn split_words(text: &str) -> Vec<String> {
    text.split_whitespace().map(String::from).collect()
}

/// Командная строка: `nc` с подкомандами.
pub fn run(args: &str) {
    let words = split_words(args);
    let sub = words.first().map(String::as_str).unwrap_or("status");
    match sub {
        "status" => cmd_status(),
        "boot" => cmd_boot(),
        "tick" => cmd_tick(),
        "dag" => cmd_dag(),
        "log" => cmd_log(),
        "ipc" => cmd_ipc(),
        "restart" => {
            let name = words.get(1).map(String::as_str).unwrap_or("");
            cmd_restart(name);
        }
        "stop" => cmd_stop(),
        other => {
            crate::println!("{}", String::from("unknown nc subcommand"));
            print_usage();
            let _ = other;
        }
    }
}

fn cmd_status() {
    let orch = orchestrator::ORCHESTRATOR.lock();
    crate::println!("=== Nano-Core status ===");
    crate::println!("{}", orchestrator::status_line());
    crate::println!("DAG stages: {}", orch.dag.len());
    crate::println!("DAG waves: {}", orch.dag.wave_count());

    crate::println!("Services:");
    for job in &orch.jobs {
        let status = if job.running {
            String::from("running")
        } else if let Some(reason) = &job.stop_reason {
            format!("stopped ({})", reason)
        } else {
            String::from("idle")
        };
        crate::println!(
            "  {:<14} spawn={:<28} pid={:<6} {}",
            job.manifest.name, job.manifest.spawn, job.pid, status
        );
    }

    crate::println!("Heartbeats:");
    let beats = heartbeat::HEARTBEATS.lock();
    for hb in beats.beats.values() {
        let state = if hb.alive { "alive" } else { "DEAD" };
        crate::println!(
            "  {:<14} last={}ms deadline={}ms fails={} restarts={} {}",
            hb.name,
            hb.last_beat_ms,
            hb.deadline_ms,
            hb.failures,
            hb.restarts,
            state
        );
    }
    crate::println!("=== end ===");
}

fn cmd_boot() {
    crate::println!("[nanocore] manual boot chain restart...");
    orchestrator::boot_chain();
    crate::println!("[nanocore] boot chain done");
}

fn cmd_tick() {
    let n = orchestrator::run_monitor_tick();
    crate::println!("[nanocore] monitor tick done, {} services restarted", n);
}

fn cmd_dag() {
    let orch = orchestrator::ORCHESTRATOR.lock();
    crate::println!("=== Nano-Core DAG ===");
    if orch.dag.is_empty() {
        crate::println!("(DAG пусто — службы ещё не зарегистрированы)");
        crate::println!("=== end ===");
        return;
    }
    for stage in orch.dag.stages() {
        let flag = if stage.ok { "OK" } else { "--" };
        let state = if stage.ok {
            "done"
        } else if stage.started_ms != 0 {
            "running"
        } else {
            "pending"
        };
        crate::println!(
            "  [{}] {:<14} wave={} deps={} state={}",
            flag,
            stage.name,
            stage.wave,
            stage.deps.join(", "),
            state
        );
    }
    crate::println!("=== end ===");
}

fn cmd_ipc() {
    crate::println!("=== Nano-Core IPC channels ===");
    let names = ipc::channel_names();
    if names.is_empty() {
        crate::println!("(каналов нет)");
        crate::println!("=== end ===");
        return;
    }
    for name in &names {
        let pending = ipc::pending(name);
        let empty = ipc::is_empty(name);
        crate::println!("  {:<14} pending={} empty={}", name, pending, empty);
        if let Some(msg) = ipc::last(name) {
            crate::println!("    last @{}ms: {}", msg.at_ms, msg.as_str());
        }
    }
    crate::println!("=== end ===");
}

fn cmd_log() {
    crate::println!("=== Nano-Core log (last 32) ===");
    for entry in log::recent().iter().rev().take(32) {
        crate::println!(
            "  [{}] t={}ms {}",
            entry.level.as_str(),
            entry.at_ms,
            entry.text
        );
    }
    crate::println!("=== end ===");
}

fn cmd_restart(name: &str) {
    if name.is_empty() {
        crate::println!("usage: nc restart <service-name>");
        return;
    }
    if orchestrator::restart_service(name) {
        crate::println!("[nanocore] service {} restarted", name);
    } else {
        crate::println!("[nanocore] service {} restart failed", name);
    }
}

fn cmd_stop() {
    crate::nanocore::stop();
    crate::println!("[nanocore] stopped");
}

fn print_usage() {
    crate::println!("");
    crate::println!("nc [subcommand] — Nano-Core control");
    crate::println!("  status        — состояние служб и heartbeat'ов");
    crate::println!("  boot          — запустить boot-цепочку DAG");
    crate::println!("  tick          — шаг heartbeat-мониторинга");
    crate::println!("  dag           — DAG зависимостей (по волнам)");
    crate::println!("  log           — последние записи журнала");
    crate::println!("  restart <s>   — перезапустить службу <s>");
    crate::println!("  stop          — остановить нано-кору");
}
