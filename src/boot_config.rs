//! Bounded, explicitly refreshable snapshot of /system/etc/init.deix.
//! The kernel supplies bytes, not parsing or supervisor policy, to authorized
//! private processes. Dinit still uses the legacy in-kernel path today.
use crate::spinlock::IrqSpinLock;
const MAX_SIZE: usize = 8192;
pub const MAX_READ: usize = 1024;
struct Snapshot { bytes: [u8; MAX_SIZE], len: usize, valid: bool, generation: u64 }
impl Snapshot {
    const fn new() -> Self { Self { bytes: [0; MAX_SIZE], len: 0, valid: false, generation: 0 } }
    fn set(&mut self, bytes: &[u8]) -> bool {
        // Keep the last good snapshot if a refresh is too large.
        if bytes.len() > MAX_SIZE || self.generation == i64::MAX as u64 { return false; }
        self.bytes[..bytes.len()].copy_from_slice(bytes);
        self.len = bytes.len();
        self.valid = true;
        self.generation += 1;
        true
    }
    fn chunk(&self, offset: usize, len: usize) -> Option<&[u8]> {
        if !self.valid || len > MAX_READ { return None; }
        let start = offset.min(self.len);
        let end = start.saturating_add(len).min(self.len);
        Some(&self.bytes[start..end])
    }
}
static SNAPSHOT: IrqSpinLock<Snapshot> = IrqSpinLock::new(Snapshot::new());
/// Load at boot or on the service worker; never perform VFS I/O in a syscall.
pub fn init() -> bool {
    const PATH: &str = "/system/etc/init.deix";
    let Ok(stat) = crate::vfs::stat(PATH) else { return false; };
    if stat.is_dir || stat.size > MAX_SIZE as u64 { return false; }
    let Ok(bytes) = crate::vfs::read_file(PATH) else { return false; };
    SNAPSHOT.lock().set(&bytes)
}
/// -2 means unavailable, -22 invalid length, -14 invalid output buffer.
pub fn read_to_user(address: u64, offset: u64, length: u64) -> i64 {
    let Ok(length) = usize::try_from(length) else { return -22; };
    if length > MAX_READ { return -22; }
    let Ok(offset) = usize::try_from(offset) else { return -22; };
    let snapshot = SNAPSHOT.lock();
    let Some(bytes) = snapshot.chunk(offset, length) else { return -2; };
    if !crate::sched::current_copy_to_user(address, bytes) { return -14; }
    bytes.len() as i64
}
/// Nonzero generation, or -2 when no snapshot is present. Never wraps.
pub fn generation() -> i64 {
    let snapshot = SNAPSHOT.lock();
    if snapshot.valid { snapshot.generation as i64 } else { -2 }
}
/// Reject a stale generation before touching the output buffer; holding the
/// lock through copy makes each returned chunk belong to the given generation.
pub fn read_generation_to_user(address: u64, offset: u64, length: u64, generation: u64) -> i64 {
    let Ok(length) = usize::try_from(length) else { return -22; };
    if length > MAX_READ { return -22; }
    let Ok(offset) = usize::try_from(offset) else { return -22; };
    let snapshot = SNAPSHOT.lock();
    if !snapshot.valid { return -2; }
    if generation != snapshot.generation { return -116; }
    let Some(bytes) = snapshot.chunk(offset, length) else { return -2; };
    if !crate::sched::current_copy_to_user(address, bytes) { return -14; }
    bytes.len() as i64
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_is_bounded_and_handles_offsets() {
        let mut s = Snapshot::new();
        assert!(s.chunk(0, 1).is_none());
        assert!(s.set(b"abcdef"));
        assert_eq!(s.generation, 1);
        assert_eq!(s.chunk(2, 3), Some(&b"cde"[..]));
        assert_eq!(s.chunk(usize::MAX, 5), Some(&b""[..]));
        assert!(s.chunk(0, MAX_READ + 1).is_none());
        assert!(!s.set(&[0; MAX_SIZE + 1]));
        assert_eq!(s.chunk(0, 6), Some(&b"abcdef"[..]));
        assert_eq!(s.generation, 1);
        assert!(s.set(b"new"));
        assert_eq!(s.generation, 2);
        s.generation = i64::MAX as u64;
        assert!(!s.set(b"overflow"));
        assert_eq!(s.chunk(0, 3), Some(&b"new"[..]));
    }
}
