//! Host tests for the actual bounded service request queue and validation.
#![allow(dead_code, unused_variables)]
extern crate alloc;
mod spinlock {
    pub struct IrqSpinLock<T>(std::sync::Mutex<T>);
    impl<T> IrqSpinLock<T> {
        pub const fn new(v: T) -> Self { Self(std::sync::Mutex::new(v)) }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
    }
}
mod sched {
    pub fn spawn(_: &str, _: extern "C" fn()) -> Option<usize> { None }
    pub fn sleep_ms(_: u64) {}
}
mod process {
    use alloc::string::String;
    pub mod cap { pub const MANAGE_SERVICES: u64 = 1; pub const READ_BOOT_CONFIG: u64 = 2; pub const WRITE_LOG: u64 = 4; }
    pub enum ProcessError { AlreadyManaged(u32), Other }
    pub struct SpawnRequest { pub name: String, pub uid: u32, pub parent: u32, pub caps: u64, pub cwd: String, pub image_path: String }
    pub struct ManagedProcess { pub pid: u32 }
    pub fn pid_has_capability(_: u32, _: u64) -> bool { false }
    pub fn pid_can_delegate(_: u32, _: u64) -> bool { false }
    pub fn spawn_managed(_: &SpawnRequest) -> Result<u32, ProcessError> { Err(ProcessError::Other) }
    pub fn exec(_: u32) -> Result<(), ()> { Err(()) }
    pub fn kill(_: u32, _: i32) -> Result<(), ()> { Err(()) }
    pub fn reap(_: u32) -> Result<Option<i32>, ()> { Err(()) }
    pub fn managed_process(_: &str) -> Option<ManagedProcess> { None }
}
mod boot_config { pub fn init() -> bool { false } }
mod diag {
    pub mod record { pub const MESSAGE_LEN: usize = 96; }
    pub struct ErrorCode;
    pub const NONE: ErrorCode = ErrorCode;
    pub enum Severity { Info }
    pub enum Action { None }
    pub mod log {
        pub fn log_ctx(_: super::Severity, _: super::ErrorCode, _: super::Action, _: &str, _: u32, _: u32, _: u8, _: u64) -> Option<u32> { None }
    }
}
#[path = "../src/service_rpc.rs"] mod service_rpc;
