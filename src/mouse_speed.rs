//! Fixed-point scaling for relative pointing devices. Absolute pointers bypass it.
#[derive(Clone, Copy)]
pub struct MotionScaler { percent: u16, rem_x: i32, rem_y: i32 }
impl MotionScaler {
    pub const fn new() -> Self { Self { percent: 100, rem_x: 0, rem_y: 0 } }
    pub fn percent(&self) -> u16 { self.percent }
    pub fn set_percent(&mut self, percent: u16) {
        self.percent = percent.clamp(50, 300);
        self.rem_x = 0; self.rem_y = 0;
    }
    pub fn scale(&mut self, dx: i32, dy: i32) -> (i32, i32) {
        fn axis(delta: i32, percent: u16, remainder: &mut i32) -> i32 {
            let n = delta as i64 * percent as i64 + *remainder as i64;
            let out = (n / 100).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            *remainder = (n % 100) as i32;
            out
        }
        (axis(dx, self.percent, &mut self.rem_x), axis(dy, self.percent, &mut self.rem_y))
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn default_is_identity() { let mut s = MotionScaler::new(); assert_eq!(s.scale(10, -12), (10, -12)); }
    #[test] fn fractional_movements_and_signs_are_preserved() {
        let mut s = MotionScaler::new(); s.set_percent(50);
        assert_eq!(s.scale(1, -1), (0, 0)); assert_eq!(s.scale(1, -1), (1, -1));
        assert_eq!(s.scale(-1, 1), (0, 0)); assert_eq!(s.scale(-1, 1), (-1, 1));
    }
    #[test] fn speed_changes_clear_remainders_and_are_bounded() {
        let mut s = MotionScaler::new(); s.set_percent(50); s.scale(1, 0); s.set_percent(150);
        assert_eq!(s.scale(1, -1), (1, -1)); s.set_percent(999); assert_eq!(s.percent(), 300);
        assert_eq!(s.scale(2, -2), (6, -6));
    }
}
