//! US HID keyboard usage translation into DeiX's existing character queue.
//! Each interface tracks its own modifier and caps-lock state.
use crate::usb2::hid::BootKeyboard;
use alloc::vec::Vec;

pub enum Input { Byte(u8), Escape, Super }
pub struct Keyboard { boot: BootKeyboard, caps: bool }
impl Keyboard {
    pub fn new() -> Self { Self { boot: BootKeyboard::new(), caps: false } }
    pub fn report(&mut self, report: &[u8], out: &mut Vec<Input>) -> Result<(), ()> {
        let mut changes = Vec::new();
        self.boot.update(report, &mut changes).map_err(|_| ())?;
        let shift = self.boot.modifiers() & 0x22 != 0;
        let ctrl_or_alt = self.boot.modifiers() & 0x55 != 0;
        for change in changes {
            if !change.pressed { continue; }
            match change.usage {
                0x39 => self.caps = !self.caps,
                0xe3 | 0xe7 => out.push(Input::Super),
                0x29 => out.push(Input::Escape),
                // Do not type modified shortcuts as plain text.
                u if !ctrl_or_alt => if let Some(ch) = map_key(u, shift, self.caps) { out.push(Input::Byte(ch)); },
                _ => {}
            }
        }
        Ok(())
    }
}
fn map_key(usage: u8, shift: bool, caps: bool) -> Option<u8> {
    match usage {
        0x04..=0x1d => Some(if shift ^ caps { b'A' + usage - 4 } else { b'a' + usage - 4 }),
        0x1e..=0x27 => {
            let i = (usage - 0x1e) as usize;
            Some(if shift { b"!@#$%^&*()"[i] } else { b"1234567890"[i] })
        }
        0x28 | 0x58 => Some(b'\n'), 0x2a => Some(8), 0x2b => Some(b'\t'), 0x2c => Some(b' '),
        0x2d..=0x38 => {
            let i = (usage - 0x2d) as usize;
            Some(if shift { b"_+{}|~:\"~<>?"[i] } else { b"-=[]\\#;'`,./"[i] })
        }
        0x4f => Some(0x14), 0x50 => Some(0x13), 0x51 => Some(0x12), 0x52 => Some(0x11),
        _ => None,
    }
}
#[cfg(test)] mod tests {
    use super::*;
    fn bytes(v: Vec<Input>) -> Vec<u8> { v.into_iter().filter_map(|e| if let Input::Byte(b) = e {Some(b)} else {None}).collect() }
    #[test] fn reports_emit_new_presses_once_and_apply_modifiers() {
        let mut k = Keyboard::new(); let mut out = Vec::new();
        k.report(&[0,0,4,0,0,0,0,0], &mut out).unwrap(); assert_eq!(bytes(out), b"a");
        let mut out = Vec::new(); k.report(&[0,0,4,0,0,0,0,0], &mut out).unwrap(); assert!(out.is_empty());
        k.report(&[0,0,0,0,0,0,0,0], &mut out).unwrap();
        k.report(&[2,0,4,0,0,0,0,0], &mut out).unwrap(); assert_eq!(bytes(out), b"A");
    }
    #[test] fn caps_and_shift_and_rollover() {
        let mut k = Keyboard::new(); let mut out = Vec::new();
        k.report(&[0,0,0x39,0,0,0,0,0], &mut out).unwrap();
        k.report(&[0,0,4,0,0,0,0,0], &mut out).unwrap(); assert_eq!(bytes(out), b"A");
        let mut out = Vec::new(); assert!(k.report(&[0,0,1,1,1,1,1,1], &mut out).is_err());
        k.report(&[2,0,5,0,0,0,0,0], &mut out).unwrap(); assert_eq!(bytes(out), b"b");
    }
    #[test] fn punctuation_navigation_and_control() {
        assert_eq!(map_key(0x2d, true, false), Some(b'_'));
        assert_eq!(map_key(0x33, false, false), Some(b';'));
        assert_eq!(map_key(0x38, true, false), Some(b'?'));
        assert_eq!(map_key(0x52, false, false), Some(0x11));
        let mut k = Keyboard::new(); let mut out = Vec::new();
        k.report(&[1,0,4,0,0,0,0,0], &mut out).unwrap(); assert!(out.is_empty());
    }
}
