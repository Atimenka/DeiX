//! Host tests for the kernel-side authenticated-session snapshot.
#![allow(dead_code)]
mod spinlock {
    pub struct IrqSpinLock<T>(std::sync::Mutex<T>);
    impl<T> IrqSpinLock<T> {
        pub const fn new(v: T) -> Self { Self(std::sync::Mutex::new(v)) }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
    }
}
#[path = "../src/session_snapshot.rs"] mod session_snapshot;
