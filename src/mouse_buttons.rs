//! Tracks buttons independently across PS/2 and up to 32 USB interfaces.
//! Unplugging a device must not leave a button held, or release another device.
pub struct Buttons { ps2: bool, allocated: u32, pressed: u32 }
impl Buttons {
    pub const fn new() -> Self { Self { ps2: false, allocated: 0, pressed: 0 } }
    pub fn set_ps2(&mut self, down: bool) { self.ps2 = down; }
    pub fn allocate(&mut self) -> Option<u8> {
        let free = !self.allocated;
        if free == 0 { return None; }
        let id = free.trailing_zeros() as u8;
        self.allocated |= 1u32 << id;
        Some(id)
    }
    pub fn set_usb(&mut self, id: u8, down: bool) {
        if id >= 32 { return; }
        let bit = 1u32 << id;
        if self.allocated & bit == 0 { return; }
        if down { self.pressed |= bit; } else { self.pressed &= !bit; }
    }
    pub fn release(&mut self, id: u8) {
        if id >= 32 { return; }
        let bit = 1u32 << id;
        self.allocated &= !bit;
        self.pressed &= !bit;
    }
    pub fn down(&self) -> bool { self.ps2 || self.pressed != 0 }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn multiple_devices_do_not_release_each_other() {
        let mut b = Buttons::new(); let a = b.allocate().unwrap(); let c = b.allocate().unwrap();
        b.set_usb(a, true); b.set_usb(c, true); b.set_usb(a, false); assert!(b.down());
        b.release(c); assert!(!b.down()); b.set_ps2(true); b.set_usb(a, true); b.release(a);
        assert!(b.down()); b.set_ps2(false); assert!(!b.down());
    }
    #[test] fn unplug_releases_and_reuses_the_slot() {
        let mut b = Buttons::new(); let id = b.allocate().unwrap(); b.set_usb(id, true);
        b.release(id); assert!(!b.down()); assert_eq!(b.allocate(), Some(id)); assert!(!b.down());
    }
    #[test] fn bounded_slots_and_unknown_source() {
        let mut b = Buttons::new(); for _ in 0..32 { assert!(b.allocate().is_some()); }
        assert!(b.allocate().is_none()); b.set_usb(32, true); b.release(255); assert!(!b.down());
        b.release(5); b.set_usb(5, true); assert!(!b.down()); assert_eq!(b.allocate(), Some(5));
    }
}
