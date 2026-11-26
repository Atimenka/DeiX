//! Host tests for the bounded, immutable boot-config snapshot.
#![allow(dead_code)]
extern crate alloc;
mod spinlock {
    pub struct IrqSpinLock<T>(std::sync::Mutex<T>);
    impl<T> IrqSpinLock<T> {
        pub const fn new(v: T) -> Self { Self(std::sync::Mutex::new(v)) }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
    }
}
mod vfs {
    pub struct Stat { pub is_dir: bool, pub size: u64 }
    pub fn stat(_: &str) -> Result<Stat, ()> { Err(()) }
    pub fn read_file(_: &str) -> Result<alloc::vec::Vec<u8>, ()> { Err(()) }
}
mod sched { pub fn current_copy_to_user(_: u64, _: &[u8]) -> bool { false } }
#[path = "../src/boot_config.rs"] mod boot_config;
