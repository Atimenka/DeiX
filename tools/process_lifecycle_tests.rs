//! Host test for the real process table, with scheduler and I/O mocked.
#![allow(dead_code, unused_variables)]
extern crate alloc;
#[macro_export] macro_rules! println { ($($arg:tt)*) => {{}}; }
mod spinlock {
    pub struct SpinLock<T>(std::sync::Mutex<T>);
    pub type IrqSpinLock<T> = SpinLock<T>;
    impl<T> SpinLock<T> {
        pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
        pub fn try_lock(&self) -> Option<std::sync::MutexGuard<'_, T>> { self.0.try_lock().ok() }
    }
}
mod sync { pub fn without_interrupts<F: FnOnce() -> R, R>(f: F) -> R { f() } }
mod mm { pub mod address_space { pub struct AddressSpace; } }
mod linux { pub mod elf {
    pub struct Loaded { pub entry: u64, pub stack_top: u64 }
    pub fn load_private(_: &[u8], _: &mut crate::mm::address_space::AddressSpace) -> Result<Loaded, alloc::string::String> { Err("invalid".into()) }
} }
mod process_ipc { pub fn purge(_: u32) {} }
mod timer {
    std::thread_local! { static NOW: core::cell::Cell<u64> = const { core::cell::Cell::new(0) }; }
    pub fn uptime_ms() -> u64 { NOW.with(|n| n.get()) }
    pub fn set(value: u64) { NOW.with(|n| n.set(value)); }
}
mod sched {
    pub static TERMINATED: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
    pub fn new_user_address_space() -> Result<crate::mm::address_space::AddressSpace, ()> { Ok(crate::mm::address_space::AddressSpace) }
    pub fn spawn_private_user_pid(_: &str, _: u64, _: u64, _: crate::mm::address_space::AddressSpace, _: u32) -> Option<usize> { None }
    std::thread_local! { static CURRENT: core::cell::Cell<usize> = const { core::cell::Cell::new(0) }; }
    pub fn current_id() -> usize { CURRENT.with(|v| v.get()) }
    pub fn set_current(id: usize) { CURRENT.with(|v| v.set(id)); }
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
#[path = "../src/process_heartbeat.rs"] mod process_heartbeat;
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
    assert!(sched::TERMINATED.lock().unwrap().contains(&slot));
    let info = process::info(pid).unwrap();
    assert_eq!(info.state, process::ProcessState::Zombie);
    assert_eq!(info.exit_code, Some(9));
    assert_eq!(process::reap(pid).unwrap(), Some(9));
}

#[test]
fn private_exit_records_code_without_terminating_another_slot() {
    let req = process::SpawnRequest {
        name: "private-elf".into(), uid: 0, parent: 1, caps: 0,
        cwd: "/".into(), image_path: "/private.elf".into(),
    };
    let pid = process::spawn(&req).unwrap();
    process::mark_running(pid, 52).unwrap();
    process::notify_private_exit(52, -14);
    let info = process::info(pid).unwrap();
    assert_eq!(info.state, process::ProcessState::Zombie);
    assert_eq!(info.exit_code, Some(-14));
    // Duplicate notifications must not overwrite the first exit status.
    process::notify_private_exit(52, 0);
    assert_eq!(process::reap(pid).unwrap(), Some(-14));
}

#[test]
fn ipc_checks_sender_and_receiver_caps_and_running_state() {
    let sender_req = process::SpawnRequest {
        name: "sender".into(), uid: 0, parent: 1, caps: process::cap::IPC_SEND,
        cwd: "/".into(), image_path: "".into(),
    };
    let receiver_req = process::SpawnRequest { name: "receiver".into(), caps: process::cap::IPC_RECEIVE, ..sender_req.clone() };
    let sender = process::spawn(&sender_req).unwrap();
    let receiver = process::spawn(&receiver_req).unwrap();
    process::mark_running(sender, 0).unwrap();
    assert_eq!(process::ipc_sender_and_target(receiver), None);
    process::mark_running(receiver, 51).unwrap();
    assert_eq!(process::ipc_sender_and_target(receiver), Some(sender));
    assert_eq!(process::ipc_receiver(), None);
    process::kill(receiver, 9).unwrap();
    assert_eq!(process::ipc_sender_and_target(receiver), None);
    process::kill(sender, 0).unwrap();
    process::reap(sender).unwrap(); process::reap(receiver).unwrap();
}

#[test]
fn repeated_kill_and_slot_reuse_cannot_terminate_the_new_owner() {
    let req = process::SpawnRequest { name: "slot-reuse".into(), uid: 0, parent: 1,
        caps: 0, cwd: "/".into(), image_path: "".into() };
    let old = process::spawn(&req).unwrap();
    process::mark_running(old, 57).unwrap();
    process::notify_private_exit(57, 17);
    let new = process::spawn(&req).unwrap();
    process::mark_running(new, 57).unwrap();
    process::kill(old, 9).unwrap();
    assert_eq!(process::info(old).unwrap().exit_code, Some(17));
    assert_eq!(process::info(new).unwrap().state, process::ProcessState::Running);
    assert!(!sched::TERMINATED.lock().unwrap().contains(&57));
    // The normal task-exit path must resolve the new live owner, not the old zombie.
    process::notify_exit(57, 23);
    assert_eq!(process::info(new).unwrap().exit_code, Some(23));
    assert_eq!(process::reap(old).unwrap(), Some(17));
    assert_eq!(process::reap(new).unwrap(), Some(23));
}

#[test]
fn heartbeat_uses_current_task_capability_and_kernel_time() {
    use process_heartbeat::Health;
    let req = process::SpawnRequest { name: "heartbeat".into(), uid: 0, parent: 1,
        caps: process::cap::HEARTBEAT, cwd: "/".into(), image_path: "".into() };
    let pid = process::spawn(&req).unwrap();
    timer::set(100); process::mark_running(pid, 58).unwrap();
    assert_eq!(process::heartbeat_health(pid, 50), Some(Health::Starting));
    sched::set_current(59);
    assert!(!process::heartbeat_current());
    timer::set(151);
    assert_eq!(process::heartbeat_health(pid, 50), Some(Health::TimedOut));
    sched::set_current(58);
    assert!(process::heartbeat_current());
    assert_eq!(process::info(pid).unwrap().last_heartbeat_ms, Some(151));
    assert_eq!(process::heartbeat_health(pid, 50), Some(Health::Alive));
    process::kill(pid, 9).unwrap();
    assert!(!process::heartbeat_current());
    assert_eq!(process::heartbeat_health(pid, 50), None);
    process::reap(pid).unwrap();
}

#[test]
fn managed_identity_prevents_duplicates_and_survives_supervisor_recreation() {
    let req = process::SpawnRequest { name: "managed-identity-test".into(), uid: 0, parent: 1,
        caps: 7, cwd: "/".into(), image_path: "/system/svc.elf".into() };
    let pid = process::spawn_managed(&req).unwrap();
    assert_eq!(process::spawn_managed(&req), Err(process::ProcessError::AlreadyManaged(pid)));
    let created = process::managed_process(&req.name).unwrap();
    assert_eq!(created.state, process::ProcessState::Created);
    process::mark_running(pid, 60).unwrap();
    let recovered = process::managed_process(&req.name).unwrap();
    assert_eq!(recovered.pid, pid);
    assert_eq!(recovered.state, process::ProcessState::Running);
    assert_eq!(recovered.image_path, req.image_path);
    assert_eq!(recovered.caps, req.caps);
    process::kill(pid, 7).unwrap();
    assert!(process::managed_process(&req.name).is_none());
    let replacement = process::spawn_managed(&req).unwrap();
    assert_ne!(pid, replacement);
    process::kill(replacement, 0).unwrap();
    process::reap(pid).unwrap(); process::reap(replacement).unwrap();
}

#[test]
fn competing_service_reservations_have_exactly_one_winner() {
    let req = process::SpawnRequest { name: "managed-race-test".into(), uid: 0, parent: 1,
        caps: 0, cwd: "/".into(), image_path: "/system/svc.elf".into() };
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let b = barrier.clone(); let a = req.clone();
    let thread = std::thread::spawn(move || { b.wait(); process::spawn_managed(&a) });
    barrier.wait();
    let other = process::spawn_managed(&req);
    let first = thread.join().unwrap();
    let winner = match (first, other) {
        (Ok(pid), Err(process::ProcessError::AlreadyManaged(other))) |
        (Err(process::ProcessError::AlreadyManaged(other)), Ok(pid)) if pid == other => pid,
        pair => panic!("unexpected results: {:?}", pair),
    };
    process::kill(winner, 0).unwrap(); process::reap(winner).unwrap();
}
