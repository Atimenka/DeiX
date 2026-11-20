//! UI-драйвер: композитор окон и графическая оболочка DeiX OS.

pub mod appicons;
pub mod apps;
pub mod config;
pub mod desktop;
pub mod effects;
pub mod metrics;
pub mod notify;
pub mod surface;
pub mod taskbar;
pub mod text;
pub mod theme;
pub mod wallpaper;
pub mod widgets;
pub mod window;

pub use desktop::run_desktop_session;

/// Статус графической подсистемы: активен ли программный композитор
/// (framebuffer установлен, рендерер работает). FPS не сообщается:
/// рабочий стол перерисовывается по damage-событиям, а не с фиксированной
/// частотой, и измерителя кадров нет — раньше здесь лежала статическая
/// заглушка `active: true, 30 FPS`, которая врала даже в текстовом режиме.
pub fn compositor_status() -> bool {
    crate::renderer::has_active_framebuffer()
}

