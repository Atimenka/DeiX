//! Host test for the real process table, with scheduler and I/O mocked.
#![allow(dead_code, unused_variables)]
extern crate alloc;
#[macro_export] macro_rules! println { ($($arg:tt)*) => {{}}; }
mod spinlock {
    pub struct SpinLock<T>(std::sync::Mutex<T>);
    impl<T> SpinLock<T> {
        pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
        pub fn try_lock(&self) -> Option<std::sync::MutexGuard<'_, T>> { self.0.try_lock().ok() }
    }
}
mod timer { pub fn uptime_ms() -> u64 { 0 } }
mod sched {
    pub static TERMINATED: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
    pub fn current_id() -> usize { 0 }
    pub fn spawn_pid(_: &str, _: extern "C" fn(), _: u32) -> Option<usize> { None }
    pub fn terminate(id: usize) { TERMINATED.lock().unwrap().push(id); }
}
mod mex {
    pub fn is_mex_image(_: &[u8]) -> bool { false }
    pub fn load_address() -> u64 { 0 }
    pub fn load_into(_: &[u8], _: u64, _: u64) -> Result<u64, alloc::string::String> { Err("not supported".into()) }
    pub fn run_ring3(_: u64) -> i64 { 0 }
}
mod diag {
    pub struct ErrorCode(pub u32);
    pub enum Subsystem { Elf }
    impl ErrorCode {
        pub fn new(_: Subsystem, n: u16) -> Self { Self(n as u32) }
        pub fn as_string(&self) -> alloc::string::String { alloc::string::String::new() }
    }
    pub fn error(_: ErrorCode, _: &str) {}
}
mod vfs {
    pub struct Error;
    impl Error { pub fn message(&self) -> alloc::string::String { "not available".into() } }
    pub struct Stat { pub read_only: bool }
    pub fn stat(_: &str) -> Result<Stat, Error> { Err(Error) }
    pub fn read_file(_: &str) -> Result<alloc::vec::Vec<u8>, Error> { Err(Error) }
    pub fn write_file(_: &str, _: &[u8]) -> Result<(), Error> { Err(Error) }
}
#[path = "../src/process.rs"] mod process;

#[test]
fn kill_pid_terminates_its_scheduler_slot_not_pid() {
    let req = process::SpawnRequest {
        name: "service".into(), uid: 0, parent: 1, caps: 0,
        cwd: "/".into(), image_path: "/service.mex".into(),
    };
    let pid = process::spawn(&req).unwrap();
    let slot = 43;
    assert_ne!(pid as usize, slot);
    process::mark_running(pid, slot).unwrap();
    process::kill(pid, 9).unwrap();
    assert_eq!(&*sched::TERMINATED.lock().unwrap(), &[slot]);
    let info = process::info(pid).unwrap();
    assert_eq!(info.state, process::ProcessState::Zombie);
    assert_eq!(info.exit_code, Some(9));
    assert_eq!(process::reap(pid).unwrap(), Some(9));
}
