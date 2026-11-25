//! Host tests for the in-kernel Nano-Core IPC implementation.
extern crate alloc;
mod timer { pub fn uptime_ms() -> u64 { 0 } }
mod spinlock {
    pub struct SpinLock<T>(std::sync::Mutex<T>);
    impl<T> SpinLock<T> {
        pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
    }
}
#[path = "../src/nanocore/ipc.rs"] mod ipc;
