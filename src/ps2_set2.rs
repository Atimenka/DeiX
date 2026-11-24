//! Set-2 scan-code decoding for controllers without hardware translation.
//!
//! The output uses the set-1 make/break codes consumed by `keyboard.rs`.
//! No keystrokes are stored after each call.
#[derive(Default)]
pub struct Decoder { extended: bool, released: bool, pause_remaining: u8 }

impl Decoder {
    pub const fn new() -> Self { Self { extended: false, released: false, pause_remaining: 0 } }

    pub fn feed(&mut self, code: u8) -> Option<(bool, u8)> {
        if self.pause_remaining != 0 { self.pause_remaining -= 1; return None; }
        match code {
            0xe1 => { self.extended = false; self.released = false; self.pause_remaining = 7; return None; },
            0xe0 => { self.extended = true; return None; },
            0xf0 => { self.released = true; return None; },
            _ => {}
        }
        let extended = self.extended;
        let released = self.released;
        self.extended = false;
        self.released = false;
        let translated = if extended {
            match code {
                0x75 => 0x48, 0x72 => 0x50, 0x6b => 0x4b, 0x74 => 0x4d,
                0x1f => 0x5b, 0x27 => 0x5c, 0x5a => 0x1c,
                _ => return None,
            }
        } else {
            match code {
                0x76=>0x01, 0x16=>0x02, 0x1e=>0x03, 0x26=>0x04, 0x25=>0x05,
                0x2e=>0x06, 0x36=>0x07, 0x3d=>0x08, 0x3e=>0x09, 0x46=>0x0a,
                0x45=>0x0b, 0x4e=>0x0c, 0x55=>0x0d, 0x66=>0x0e, 0x0d=>0x0f,
                0x15=>0x10, 0x1d=>0x11, 0x24=>0x12, 0x2d=>0x13, 0x2c=>0x14,
                0x35=>0x15, 0x3c=>0x16, 0x43=>0x17, 0x44=>0x18, 0x4d=>0x19,
                0x54=>0x1a, 0x5b=>0x1b, 0x5a=>0x1c, 0x14=>0x1d, 0x1c=>0x1e,
                0x1b=>0x1f, 0x23=>0x20, 0x2b=>0x21, 0x34=>0x22, 0x33=>0x23,
                0x3b=>0x24, 0x42=>0x25, 0x4b=>0x26, 0x4c=>0x27, 0x52=>0x28,
                0x0e=>0x29, 0x12=>0x2a, 0x5d=>0x2b, 0x1a=>0x2c, 0x22=>0x2d,
                0x21=>0x2e, 0x2a=>0x2f, 0x32=>0x30, 0x31=>0x31, 0x3a=>0x32,
                0x41=>0x33, 0x49=>0x34, 0x4a=>0x35, 0x59=>0x36, 0x11=>0x38,
                0x29=>0x39,
                _ => return None,
            }
        };
        Some((extended, translated | if released { 0x80 } else { 0 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn characters_and_shift_releases() {
        let mut d = Decoder::new();
        assert_eq!(d.feed(0x12), Some((false, 0x2a)));
        assert_eq!(d.feed(0x1c), Some((false, 0x1e)));
        assert_eq!(d.feed(0xf0), None);
        assert_eq!(d.feed(0x1c), Some((false, 0x9e)));
        assert_eq!(d.feed(0xf0), None);
        assert_eq!(d.feed(0x12), Some((false, 0xaa)));
        assert_eq!(d.feed(0x5a), Some((false, 0x1c)));
    }
    #[test]
    fn extended_break_and_unknown_do_not_stick() {
        let mut d = Decoder::new();
        assert_eq!(d.feed(0xe0), None);
        assert_eq!(d.feed(0x75), Some((true, 0x48)));
        assert_eq!(d.feed(0xe0), None);
        assert_eq!(d.feed(0xf0), None);
        assert_eq!(d.feed(0x75), Some((true, 0xc8)));
        assert_eq!(d.feed(0xe0), None);
        assert_eq!(d.feed(0x7f), None);
        assert_eq!(d.feed(0x1c), Some((false, 0x1e)));
    }
    #[test]
    fn pause_sequence_is_not_mistaken_for_keys() {
        let mut d = Decoder::new();
        for c in [0xe1, 0x14, 0x77, 0xe1, 0xf0, 0x14, 0xf0, 0x77] {
            assert_eq!(d.feed(c), None);
        }
        assert_eq!(d.feed(0x1c), Some((false, 0x1e)));
    }
}
