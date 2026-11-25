//! Host-side tests of the real software renderer, without hardware access.
#![allow(dead_code)]
extern crate alloc;
#[macro_export] macro_rules! serial_println { ($($arg:tt)*) => {}; }
mod vbe {
    #[derive(Clone, Copy)]
    pub struct Framebuffer { pub addr: usize, pub width: u32, pub height: u32, pub bpp: u16, pub pitch: usize }
    pub const BPP_32: u16 = 32;
    pub fn bios_framebuffer() -> Option<Framebuffer> { None }
    pub fn set_mode(_: &(), _: u32, _: u32, _: u16) -> Option<Framebuffer> { None }
}
mod gpu { pub struct Info { pub supports_bochs_vbe: bool, pub device: () } pub fn detect() -> Option<Info> { None } }
mod mouse { pub fn set_screen_size(_: u32, _: u32) {} }
mod font { pub fn read_glyph(_: u8) -> [u8; 16] { [255; 16] } }
mod sync { pub fn without_interrupts<F: FnOnce() -> R, R>(f: F) -> R { f() } }
mod spinlock {
    pub struct SpinLock<T>(std::sync::Mutex<T>);
    impl<T> SpinLock<T> {
        pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
        pub fn try_lock(&self) -> Option<std::sync::MutexGuard<'_, T>> { self.0.try_lock().ok() }
    }
}
#[path = "../src/cp866.rs"] mod cp866;
#[path = "../src/ui/surface.rs"] pub mod surface;
#[path = "../src/ui/text.rs"] pub mod text;
mod ui { pub use crate::surface; pub use crate::text; }
#[path = "../src/renderer.rs"] mod renderer;
use renderer::{Color, Rect, Renderer};
fn renderer() -> Renderer { Renderer::new(vbe::Framebuffer { addr: 0, width: 40, height: 32, bpp: 32, pitch: 160 }) }
fn check_bounds(r: &Renderer, rect: Rect) {
    for y in 0..32 { for x in 0..40 {
        assert_eq!(r.back_buffer[y * 40 + x] != 0, rect.contains(x as i32, y as i32), "{x},{y}");
    }}
}
#[test] fn clipped_rectangles_and_alpha() {
    for alpha in [100, 255] {
        let mut r = renderer(); r.set_clip(Rect::new(5, 7, 13, 9));
        r.fill_rect_alpha(-5, -5, 80, 80, Color::WHITE, alpha);
        check_bounds(&r, Rect::new(5, 7, 13, 9));
    }
}
#[test] fn clipped_glyph_fast_and_partial_paths() {
    let mut r = renderer(); r.set_clip(Rect::new(5, 7, 10, 9));
    r.draw_text(2, 4, "ABCD", Color::WHITE, None);
    check_bounds(&r, Rect::new(5, 7, 10, 9));
}
#[test] fn spans_lines_and_circles_do_not_leak() {
    let mut r = renderer(); r.set_clip(Rect::new(5, 7, 13, 9));
    r.fill_fade_v(0, 0, 40, 32, Color::WHITE, 255, 255);
    r.draw_hline(0, 3, 40, Color::WHITE);
    r.fill_circle(0, 0, 30, Color::WHITE);
    check_bounds(&r, Rect::new(5, 7, 13, 9));
}
#[test] fn empty_and_offscreen_clip_then_restore() {
    let mut r = renderer(); let old = r.set_clip(Rect::new(-30, -30, 10, 10));
    r.fill_rect(0, 0, 40, 32, Color::WHITE);
    assert!(r.back_buffer.iter().all(|p| *p == 0));
    r.set_clip(old); r.fill_rect(0, 0, 40, 32, Color::WHITE);
    check_bounds(&r, Rect::new(0, 0, 40, 32));
}
#[test] fn blur_and_shading_stay_inside_clip() {
    let mut r = renderer(); r.fill_rect(0, 0, 40, 32, Color::WHITE);
    let before = r.back_buffer.clone(); r.set_clip(Rect::new(5, 7, 13, 9));
    r.shade_rect(0, 0, 40, 32, 255); r.apply_blur_rect(0, 0, 40, 32, 2);
    for y in 0..32 { for x in 0..40 {
        if !Rect::new(5, 7, 13, 9).contains(x as i32, y as i32) { assert_eq!(r.back_buffer[y * 40 + x], before[y * 40 + x]); }
    }}
}
