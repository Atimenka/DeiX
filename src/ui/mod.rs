//! UI-драйвер: композитор окон и графическая оболочка DeiX OS.

pub mod apps;
pub mod desktop;
pub mod metrics;
pub mod surface;
pub mod taskbar;
pub mod theme;
pub mod window;

#[allow(unused_imports)]
pub use apps::*;
#[allow(unused_imports)]
pub use desktop::*;
#[allow(unused_imports)]
pub use metrics::*;
#[allow(unused_imports)]
pub use surface::*;
#[allow(unused_imports)]
pub use taskbar::*;
#[allow(unused_imports)]
pub use theme::*;
#[allow(unused_imports)]
pub use window::*;

pub struct KernelGuiCompositorService {
    pub active: bool,
    pub frame_rate: u32,
}

static COMPOSITOR_SERVICE: crate::spinlock::SpinLock<KernelGuiCompositorService> =
    crate::spinlock::SpinLock::new(KernelGuiCompositorService {
        active: true,
        frame_rate: 30,
    });

pub fn compositor_status() -> (bool, u32) {
    let c = COMPOSITOR_SERVICE.lock();
    (c.active, c.frame_rate)
}
