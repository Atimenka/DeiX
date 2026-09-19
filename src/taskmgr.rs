//! Диспетчер задач (Task Manager) DeiX OS.

use crate::sched::{self, Priority, State};

pub fn cmd_taskmgr(arg: &str) {
    let mut parts = arg.trim().split_whitespace();
    let sub = parts.next().unwrap_or("list");

    match sub {
        "" | "list" => print_table(),
        "top" => live_top(),
        "info" => {
            if let Some(pid_str) = parts.next() {
                if let Ok(pid) = pid_str.parse::<u32>() {
                    show_details(pid);
                } else {
                    crate::println!("  [taskmgr] Ошибка: некорректный PID");
                }
            } else {
                crate::println!("  Использование: taskmgr info <pid>");
            }
        }
        "kill" => {
            if let Some(pid_str) = parts.next() {
                if let Ok(pid) = pid_str.parse::<u32>() {
                    sched::terminate_by_pid(pid);
                    crate::println!("  [taskmgr] Задача PID {} завершена", pid);
                } else {
                    crate::println!("  [taskmgr] Ошибка: некорректный PID");
                }
            } else {
                crate::println!("  Использование: taskmgr kill <pid>");
            }
        }
        "renice" => {
            if let Some(pid_str) = parts.next() {
                if let (Ok(pid), Some(prio_str)) = (pid_str.parse::<u32>(), parts.next()) {
                    let prio = match prio_str.to_lowercase().as_str() {
                        "realtime" | "rt" => Priority::Realtime,
                        "high" | "h" => Priority::High,
                        "idle" | "i" => Priority::Idle,
                        _ => Priority::Normal,
                    };
                    if sched::set_priority(pid, prio) {
                        crate::println!("  [taskmgr] Установлен приоритет {:?} для PID {}", prio, pid);
                    } else {
                        crate::println!("  [taskmgr] Задача PID {} не найдена", pid);
                    }
                } else {
                    crate::println!("  Использование: taskmgr renice <pid> <realtime|high|normal|idle>");
                }
            } else {
                crate::println!("  Использование: taskmgr renice <pid> <pri>");
            }
        }
        "suspend" => {
            if let Some(pid_str) = parts.next() {
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if sched::set_state(pid, State::Suspended) {
                        crate::println!("  [taskmgr] Задача PID {} приостановлена", pid);
                    } else {
                        crate::println!("  [taskmgr] Задача PID {} не найдена", pid);
                    }
                }
            } else {
                crate::println!("  Использование: taskmgr suspend <pid>");
            }
        }
        "resume" => {
            if let Some(pid_str) = parts.next() {
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if sched::set_state(pid, State::Ready) {
                        crate::println!("  [taskmgr] Задача PID {} возобновлена", pid);
                    } else {
                        crate::println!("  [taskmgr] Задача PID {} не найдена", pid);
                    }
                }
            } else {
                crate::println!("  Использование: taskmgr resume <pid>");
            }
        }
        _ => {
            crate::println!("  Использование: taskmgr [list|top|info <pid>|kill <pid>|renice <pid> <pri>|suspend <pid>|resume <pid>]");
        }
    }
}

fn print_table() {
    let now = crate::timer::uptime_ms();
    let tasks = sched::list_info();

    crate::println!("=== ДИСПЕТЧЕР ЗАДАЧ (TASKMGR) DeiX OS ===");
    crate::println!("{:<5} {:<5} {:<16} {:<10} {:<12} {:<10} {:<10}",
        "PID", "PPID", "NAME", "PRIORITY", "STATE", "TICKS", "SWITCHES");
    crate::println!("-----------------------------------------------------------------------");

    for t in tasks {
        let state_str = match t.state {
            State::Ready => "Running",
            State::Sleeping(_) => "Sleeping",
            State::Finished => "Finished",
            State::Suspended => "Suspended",
            State::Empty => "Empty",
        };
        let switches = t.switches_vol + t.switches_invol;
        crate::println!("{:<5} {:<5} {:<16} {:<10} {:<12} {:<10} {:<10}",
            t.pid, t.ppid, t.name, t.priority.as_str(), state_str, t.cpu_ticks, switches);
    }
    crate::println!("-----------------------------------------------------------------------");
    crate::println!("Всего переключений контекста: {} | Аптайм: {} сек", sched::switch_count(), now / 1000);
}

fn live_top() {
    crate::println!("=== LIVE TOP (10 секунд наблюдения) ===");
    for _ in 0..5 {
        print_table();
        sched::sleep_ms(2000);
    }
}

fn show_details(pid: u32) {
    if let Some(info) = sched::task_info(pid) {
        crate::println!("=== ДЕТАЛИ ЗАДАЧИ PID {} ===", info.pid);
        crate::println!(" Имя:              {}", info.name);
        crate::println!(" PPID родителя:    {}", info.ppid);
        crate::println!(" Владелец процесса: {}", info.owner_pid);
        crate::println!(" Состояние:        {:?}", info.state);
        crate::println!(" Приоритет:        {:?}", info.priority);
        crate::println!(" Всего тиков CPU:  {}", info.cpu_ticks);
        crate::println!(" Тики ядро / user: {} / {}", info.kernel_ticks, info.user_ticks);
        crate::println!(" Переключения (vol/invol): {} / {}", info.switches_vol, info.switches_invol);
        crate::println!(" Время старта:     {} мс", info.start_ms);
        crate::println!(" Код выхода:       {}", info.exit_code);
        crate::println!(" Флаг зависания:   {}", info.hung);
    } else {
        crate::println!("  [taskmgr] Задача PID {} не найдена", pid);
    }
}
