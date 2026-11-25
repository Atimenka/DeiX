//! Host-side tests of the fixed-size IPC queue.
#![allow(dead_code)]
mod spinlock {
    pub struct IrqSpinLock<T>(std::sync::Mutex<T>);
    impl<T> IrqSpinLock<T> {
        pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
    }
}
#[path = "../src/process_ipc.rs"] mod process_ipc;
