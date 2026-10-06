//! Отрисовка приложения Диспетчер задач (Task Manager)
//!
//! Все числа берутся из реальных счётчиков ядра:
//! * RAM — `allocator::allocated_heap_bytes` / `total_heap_bytes`;
//! * процессы и доля CPU — `sched::list()` (тики PIT на задачу);
//! * диск — `ata::io_counters()`;
//! * сеть — `rtl8139::traffic_counters()`.
//!
//! Скорости (сект/с, байт/с) считаются как приращение счётчика между
//! двумя отрисовками кадра, делённое на прошедшее время.

use crate::renderer::{Color, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::Window;
use alloc::format;
use alloc::string::String;

/// Предыдущий снимок счётчиков для расчёта скоростей.
struct Sample {
    at_ms: u64,
    sectors_read: u64,
    sectors_written: u64,
    rx_bytes: u64,
    tx_bytes: u64,
}

static mut LAST_SAMPLE: Option<Sample> = None;

/// Мгновенные скорости, вычисленные по двум снимкам счётчиков.
struct Rates {
    sectors_per_sec: u64,
    net_bytes_per_sec: u64,
}

fn sample_rates() -> Rates {
    let now = crate::timer::uptime_ms();
    let (rd, wr) = crate::ata::io_counters();
    let (_rxp, rxb, _txp, txb) = crate::rtl8139::traffic_counters();

    let rates = unsafe {
        match &*(&raw const LAST_SAMPLE) {
            Some(prev) => {
                let dt = now.saturating_sub(prev.at_ms);
                if dt == 0 {
                    Rates {
                        sectors_per_sec: 0,
                        net_bytes_per_sec: 0,
                    }
                } else {
                    let d_sectors = rd
                        .saturating_sub(prev.sectors_read)
                        + wr.saturating_sub(prev.sectors_written);
                    let d_net =
                        rxb.saturating_sub(prev.rx_bytes) + txb.saturating_sub(prev.tx_bytes);
                    Rates {
                        sectors_per_sec: d_sectors * 1000 / dt,
                        net_bytes_per_sec: d_net * 1000 / dt,
                    }
                }
            }
            None => Rates {
                sectors_per_sec: 0,
                net_bytes_per_sec: 0,
            },
        }
    };

    unsafe {
        LAST_SAMPLE = Some(Sample {
            at_ms: now,
            sectors_read: rd,
            sectors_written: wr,
            rx_bytes: rxb,
            tx_bytes: txb,
        });
    }

    rates
}

pub fn draw_task_manager(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    selected_pid: Option<usize>,
    status_msg: Option<&String>,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    r.draw_text(w.x + 12, content_y + 8, "SYSTEM METRICS & PROCESSES", theme.accent, None);

    let allocated = crate::allocator::allocated_heap_bytes();
    let total = crate::allocator::total_heap_bytes();
    let ram_percent = if total > 0 { (allocated * 100) / total } else { 0 };

    let tasks = crate::sched::list();
    let ticks_total = crate::sched::ticks_total();
    let rates = sample_rates();

    // Доля CPU самой загруженной задачи — реальное время на процессоре,
    // посчитанное планировщиком в тиках PIT.
    let busiest = tasks.iter().max_by_key(|t| t.runtime_ticks);
    let (busy_name, busy_percent) = match busiest {
        Some(t) if ticks_total > 0 => (
            t.name.clone(),
            ((t.runtime_ticks * 100) / ticks_total as u64) as usize,
        ),
        _ => (String::from("-"), 0usize),
    };

    let cpu_str = format!("CPU top: {:>2}%  ({})", busy_percent, busiest_name_short(&busy_name));
    r.draw_text(w.x + 12, content_y + 28, &cpu_str, theme.text_primary, None);
    r.fill_rounded_rect(w.x + 190, content_y + 28, 100, 10, 2, theme.titlebar_inactive);
    r.fill_rounded_rect(w.x + 190, content_y + 28, busy_percent.min(100) as u32, 10, 2, theme.accent);

    let ram_str = format!("RAM: {} KB / {} KB ({}%)", allocated / 1024, total / 1024, ram_percent);
    r.draw_text(w.x + 300, content_y + 28, &ram_str, theme.text_primary, None);

    let (rd, wr) = crate::ata::io_counters();
    let (_rxp, rxb, _txp, txb) = crate::rtl8139::traffic_counters();
    let io_str = format!(
        "Disk: {} sect/s (R:{} W:{})   eth0: {} B/s (rx {} / tx {})",
        rates.sectors_per_sec,
        rd,
        wr,
        rates.net_bytes_per_sec,
        rxb,
        txb
    );
    r.draw_text(w.x + 12, content_y + 44, &io_str, theme.text_secondary, None);

    let table_y = content_y + 68;
    r.fill_rect(w.x + 8, table_y, w.width - 16, 20, theme.titlebar_active);
    r.draw_text(w.x + 16, table_y + 2, "PID   NAME           STATE       CPU%   TICKS", theme.text_primary, None);

    let mut ty = table_y + 22;
    let mut shown = 0usize;
    for t in tasks.iter() {
        if ty + 18 > content_y + w.height as i32 - 36 {
            break;
        }
        let is_sel = selected_pid == Some(t.id);
        let bg = if is_sel { theme.titlebar_active } else { theme.window_bg };
        r.fill_rect(w.x + 8, ty, w.width - 16, 18, bg);

        let cpu_pct = if ticks_total > 0 {
            (t.runtime_ticks * 100) / ticks_total as u64
        } else {
            0
        };
        let row = format!(
            "{:<5} {:<14} {:<11} {:>4}  {:>6}",
            t.pid,
            name_short(&t.name),
            crate::sched::state_str(t.state),
            cpu_pct,
            t.runtime_ticks
        );
        let fg = if is_sel { theme.accent } else { theme.text_primary };
        r.draw_text(w.x + 16, ty + 2, &row, fg, None);
        ty += 20;
        shown += 1;
    }

    let footer = format!(
        "{} of {} tasks shown   ctx switches: {}   pit ticks: {}",
        shown,
        tasks.len(),
        crate::sched::switch_count(),
        ticks_total
    );
    r.draw_text(w.x + 12, content_y + w.height as i32 - 34, &footer, theme.text_secondary, None);

    // Результат последнего действия (например, завершения процесса).
    if let Some(msg) = status_msg {
        r.draw_text(w.x + 12, content_y + w.height as i32 - 16, msg, theme.accent, None);
    }

    r.fill_rounded_rect(w.x + w.width as i32 - 110, content_y + w.height as i32 - 32, 100, 24, 4, Color::RED);
    r.draw_text(w.x + w.width as i32 - 100, content_y + w.height as i32 - 28, "Kill Task", Color::WHITE, None);
}

fn name_short(name: &str) -> String {
    if name.len() > 14 {
        format!("{}..", &name[..12])
    } else {
        String::from(name)
    }
}

fn busiest_name_short(name: &str) -> String {
    if name.len() > 20 {
        format!("{}..", &name[..18])
    } else {
        String::from(name)
    }
}
