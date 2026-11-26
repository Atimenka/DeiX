//! Authenticated local login snapshot, not session policy or authentication.
//! Only trusted kernel login code can publish; private processes may only read.
use crate::spinlock::IrqSpinLock;

pub const NAME_MAX: usize = 32;
pub const WIRE_SIZE: usize = 48;
#[derive(Clone, Copy)]
struct Session {
    active: bool,
    representable: bool,
    uid: u32,
    login_ms: u64,
    name_len: usize,
    name: [u8; NAME_MAX],
}
impl Session {
    const fn new() -> Self { Self { active: false, representable: false, uid: 0, login_ms: 0, name_len: 0, name: [0; NAME_MAX] } }
    fn publish(&mut self, uid: u32, name: &str, now: u64) -> bool {
        let mut next = Self::new();
        next.active = true;
        next.uid = uid;
        next.login_ms = now;
        let bytes = name.as_bytes();
        if !bytes.is_empty() && bytes.len() <= NAME_MAX {
            next.representable = true;
            next.name_len = bytes.len();
            next.name[..bytes.len()].copy_from_slice(bytes);
        }
        *self = next; // Never expose an old login if the new name is too long.
        self.representable
    }
    fn wire(&self) -> Result<[u8; WIRE_SIZE], i32> {
        if !self.active { return Err(-2); }
        if !self.representable { return Err(-75); }
        let mut out = [0u8; WIRE_SIZE];
        out[0..4].copy_from_slice(&self.uid.to_le_bytes());
        out[4..8].copy_from_slice(&(self.name_len as u32).to_le_bytes());
        out[8..16].copy_from_slice(&self.login_ms.to_le_bytes());
        out[16..].copy_from_slice(&self.name);
        Ok(out)
    }
}
static SESSION: IrqSpinLock<Session> = IrqSpinLock::new(Session::new());
/// Caller must invoke only after the existing login flow authenticated the user.
pub fn publish_authenticated(uid: u32, name: &str, now: u64) -> bool {
    SESSION.lock().publish(uid, name, now)
}
/// One point-in-time snapshot. No password, credential hash or session token.
pub fn snapshot() -> Result<[u8; WIRE_SIZE], i32> { SESSION.lock().wire() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_login_and_valid_login_are_distinct() {
        let mut s = Session::new();
        assert_eq!(s.wire(), Err(-2));
        assert!(s.publish(1000, "alice", 123));
        let out = s.wire().unwrap();
        assert_eq!(&out[0..4], &1000u32.to_le_bytes());
        assert_eq!(&out[4..8], &5u32.to_le_bytes());
        assert_eq!(&out[8..16], &123u64.to_le_bytes());
        assert_eq!(&out[16..21], b"alice");
        assert!(out[21..].iter().all(|x| *x == 0));
    }
    #[test] fn long_or_empty_name_never_leaks_a_previous_login() {
        let mut s = Session::new();
        assert!(s.publish(0, "root", 1));
        assert!(!s.publish(1000, "123456789012345678901234567890123", 2));
        assert_eq!(s.wire(), Err(-75));
        assert!(!s.publish(1000, "", 3));
        assert_eq!(s.wire(), Err(-75));
        assert!(s.publish(0, "root", 4));
        assert_eq!(&s.wire().unwrap()[16..20], b"root");
    }
}
