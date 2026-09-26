//! Отрисовка приложения Диспетчер задач (Task Manager)

use crate::renderer::{Color, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::Window;
use alloc::format;

pub fn draw_task_manager(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    selected_pid: Option<usize>,
    _status_msg: Option<&String>,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    r.draw_text(w.x + 12, content_y + 8, "SYSTEM METRICS & PROCESSES", theme.accent, None);

    let allocated = crate::allocator::allocated_heap_bytes();
    let total = crate::allocator::total_heap_bytes();
    let ram_percent = if total > 0 { (allocated * 100) / total } else { 0 };
    let cpu_load = 18usize;
    let disk_io = 12usize;
    let wifi_rate = 128usize;

    r.draw_text(w.x + 12, content_y + 28, &format!("CPU Load: {:>2}%", cpu_load), theme.text_primary, None);
    r.fill_rounded_rect(w.x + 130, content_y + 28, 100, 10, 2, theme.titlebar_inactive);
    r.fill_rounded_rect(w.x + 130, content_y + 28, cpu_load as u32, 10, 2, theme.accent);

    let ram_str = format!("RAM: {} KB / {} KB ({}%)", allocated / 1024, total / 1024, ram_percent);
    r.draw_text(w.x + 240, content_y + 28, &ram_str, theme.text_primary, None);

    let net_str = format!("Disk I/O: {}% | WiFi eth0: {} KB/s", disk_io, wifi_rate);
    r.draw_text(w.x + 12, content_y + 44, &net_str, theme.text_secondary, None);

    let table_y = content_y + 68;
    r.fill_rect(w.x + 8, table_y, w.width - 16, 20, theme.titlebar_active);
    r.draw_text(w.x + 16, table_y + 2, "PID   NAME           STATE       MEM", theme.text_primary, None);

    let procs = [
        (1, "kernel_idle", "Running", "128 KB"),
        (2, "gfx_compositor", "Active", "512 KB"),
        (3, "net_stack_eth0", "Sleeping", "256 KB"),
        (4, "deix_shell_gui", "Active", "1024 KB"),
    ];

    let mut ty = table_y + 22;
    for (pid, name, state, mem) in procs.iter() {
        let is_sel = selected_pid == Some(*pid);
        let bg = if is_sel { theme.titlebar_active } else { theme.window_bg };
        r.fill_rect(w.x + 8, ty, w.width - 16, 18, bg);

        let row = format!("{:<5} {:<14} {:<11} {:<8}", pid, name, state, mem);
        let fg = if is_sel { theme.accent } else { theme.text_primary };
        r.draw_text(w.x + 16, ty + 2, &row, fg, None);
        ty += 20;
    }

    r.fill_rounded_rect(w.x + w.width as i32 - 110, content_y + w.height as i32 - 32, 100, 24, 4, Color::RED);
    r.draw_text(w.x + w.width as i32 - 100, content_y + w.height as i32 - 28, "Kill Task", Color::WHITE, None);
}
