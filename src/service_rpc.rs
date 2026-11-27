//! Non-blocking, bounded mechanism for private service managers. The worker
//! performs filesystem/process operations outside the syscall entry path.
//! The caller owns restart, dependency and session policy; this is not Dinit.
use crate::spinlock::IrqSpinLock;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, Ordering};

const CAPACITY: usize = 16;
const PER_CALLER: usize = 4;
pub const MAX_NAME: usize = 128;
pub const MAX_PATH: usize = 256;
#[derive(Clone, Copy, PartialEq, Eq)]
enum State { Pending, Working, Complete }
#[derive(Clone, Copy)]
enum Action { Start, Stop, ReloadConfig, WriteLog }
#[derive(Clone, Copy)]
struct Request {
    token: u64, caller: u32, state: State, action: Action,
    name: [u8; MAX_NAME], name_len: usize,
    path: [u8; MAX_PATH], path_len: usize,
    target: u32, grant: u64, result: i32, result_pid: u32,
}
struct Queue { slots: [Option<Request>; CAPACITY], next: u64 }
impl Queue {
    const fn new() -> Self { Self { slots: [None; CAPACITY], next: 1 } }
    fn submit(&mut self, mut req: Request) -> Result<u64, i32> {
        if self.slots.iter().flatten().filter(|r| r.caller == req.caller).count() >= PER_CALLER { return Err(-11); }
        let Some(slot) = self.slots.iter_mut().find(|s| s.is_none()) else { return Err(-11); };
        // Keep tokens positive when cast to the signed ABI; never recycle them.
        if self.next > i64::MAX as u64 { return Err(-75); }
        req.token = self.next; self.next += 1;
        *slot = Some(req);
        Ok(req.token)
    }
    fn claim(&mut self) -> Option<Request> {
        let req = self.slots.iter_mut().flatten().find(|r| r.state == State::Pending)?;
        req.state = State::Working;
        Some(*req)
    }
    fn complete(&mut self, token: u64, result: i32, pid: u32) {
        if let Some(req) = self.slots.iter_mut().flatten().find(|r| r.token == token && r.state == State::Working) {
            req.result = result; req.result_pid = pid; req.state = State::Complete;
        }
    }
    fn take(&mut self, caller: u32, token: u64) -> Result<(i32, u32), i32> {
        let Some(slot) = self.slots.iter_mut().find(|s| s.as_ref().map_or(false, |r| r.token == token && r.caller == caller)) else { return Err(-2); };
        let req = slot.as_ref().unwrap();
        if req.state != State::Complete { return Err(-11); }
        let result = (req.result, req.result_pid);
        *slot = None;
        Ok(result)
    }
    fn purge(&mut self, caller: u32) {
        for slot in &mut self.slots {
            if slot.as_ref().map_or(false, |r| r.caller == caller) { *slot = None; }
        }
    }
}
static QUEUE: IrqSpinLock<Queue> = IrqSpinLock::new(Queue::new());
static WORKER_ACTIVE: AtomicBool = AtomicBool::new(false);

fn valid_name(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.len() <= MAX_NAME && core::str::from_utf8(bytes).map_or(false, |name|
        !name.trim().is_empty() && !name.chars().any(|c| c.is_control() || c == '/' || c == '\\'))
}
fn valid_path(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > MAX_PATH { return false; }
    let Ok(path) = core::str::from_utf8(bytes) else { return false; };
    let base_ok = path.starts_with("/system/services/") ||
        (option_env!("DEIX_TEST_ADDRESS_SPACE") == Some("1") && path.starts_with("/system/tests/"));
    base_ok && path.ends_with(".elf") && !path.contains("//") && !path.contains('\\')
        && !path.chars().any(|c| c.is_control())
        && !path.split('/').any(|component| component == "." || component == "..")
}
fn request(caller: u32, action: Action, name: &[u8], path: &[u8], target: u32, grant: u64) -> Result<u64, i32> {
    if !valid_name(name) || (matches!(action, Action::Start) && !valid_path(path)) { return Err(-22); }
    if !WORKER_ACTIVE.load(Ordering::Acquire) { return Err(-5); }
    let mut req = Request { token: 0, caller, state: State::Pending, action,
        name: [0; MAX_NAME], name_len: name.len(), path: [0; MAX_PATH], path_len: path.len(),
        target, grant, result: 0, result_pid: 0 };
    req.name[..name.len()].copy_from_slice(name);
    req.path[..path.len()].copy_from_slice(path);
    QUEUE.lock().submit(req)
}
pub fn submit_start(caller: u32, name: &[u8], path: &[u8]) -> Result<u64, i32> {
    request(caller, Action::Start, name, path, 0, 0)
}
pub fn submit_start_delegated(caller: u32, name: &[u8], path: &[u8], grant: u64) -> Result<u64, i32> {
    if !crate::process::pid_can_delegate(caller, grant) { return Err(-1); }
    request(caller, Action::Start, name, path, 0, grant)
}
pub fn submit_stop(caller: u32, name: &[u8], target: u32) -> Result<u64, i32> {
    if target == 0 || target == caller { return Err(-22); }
    request(caller, Action::Stop, name, &[], target, 0)
}
/// Reload reads the actual disk file on the worker, not in syscall context.
pub fn submit_config_reload(caller: u32) -> Result<u64, i32> {
    if !WORKER_ACTIVE.load(Ordering::Acquire) { return Err(-5); }
    if !crate::process::pid_has_capability(caller, crate::process::cap::MANAGE_SERVICES | crate::process::cap::READ_BOOT_CONFIG) { return Err(-1); }
    QUEUE.lock().submit(Request { token: 0, caller, state: State::Pending, action: Action::ReloadConfig,
        name: [0; MAX_NAME], name_len: 0, path: [0; MAX_PATH], path_len: 0,
        target: 0, grant: 0, result: 0, result_pid: 0 })
}
fn valid_log(message: &[u8]) -> bool {
    !message.is_empty() && message.len() <= crate::diag::record::MESSAGE_LEN
        && core::str::from_utf8(message).map_or(false, |text| !text.chars().any(|c| c.is_control()))
}
/// Log messages are bounded UTF-8, cannot inject control characters, and are
/// emitted by the worker with the authenticated caller PID as context.
pub fn submit_log(caller: u32, message: &[u8]) -> Result<u64, i32> {
    if !WORKER_ACTIVE.load(Ordering::Acquire) { return Err(-5); }
    if !crate::process::pid_has_capability(caller, crate::process::cap::WRITE_LOG) { return Err(-1); }
    if !valid_log(message) { return Err(-22); }
    let mut req = Request { token: 0, caller, state: State::Pending, action: Action::WriteLog,
        name: [0; MAX_NAME], name_len: 0, path: [0; MAX_PATH], path_len: message.len(),
        target: 0, grant: 0, result: 0, result_pid: 0 };
    req.path[..message.len()].copy_from_slice(message);
    QUEUE.lock().submit(req)
}
pub fn take(caller: u32, token: u64) -> Result<(i32, u32), i32> { QUEUE.lock().take(caller, token) }
pub fn purge(caller: u32) { QUEUE.lock().purge(caller); }

/// Starts one background task at boot; absence is reported, never simulated.
pub fn start_worker() -> bool {
    if WORKER_ACTIVE.load(Ordering::Acquire) { return true; }
    let started = crate::sched::spawn("service-rpc", worker).is_some();
    WORKER_ACTIVE.store(started, Ordering::Release);
    started
}
extern "C" fn worker() {
    loop {
        let request = QUEUE.lock().claim();
        if let Some(req) = request {
            let (result, pid) = execute(req);
            QUEUE.lock().complete(req.token, result, pid);
        } else { crate::sched::sleep_ms(10); }
    }
}
fn execute(req: Request) -> (i32, u32) {
    use crate::process::{self, ProcessError};
    let needed = if matches!(req.action, Action::WriteLog) { process::cap::WRITE_LOG } else { process::cap::MANAGE_SERVICES };
    if !process::pid_has_capability(req.caller, needed) { return (-1, 0); }
    let Ok(name) = core::str::from_utf8(&req.name[..req.name_len]) else { return (-22, 0); };
    match req.action {
        Action::WriteLog => {
            let Ok(text) = core::str::from_utf8(&req.path[..req.path_len]) else { return (-22, 0); };
            let event = crate::diag::log::log_ctx(crate::diag::Severity::Info, crate::diag::NONE,
                crate::diag::Action::None, text, req.caller, 0, 0, 0);
            match event { Some(id) => (0, id), None => (-5, 0) }
        }
        Action::ReloadConfig => {
            if !process::pid_has_capability(req.caller, process::cap::READ_BOOT_CONFIG) { return (-1, 0); }
            if crate::boot_config::init() { (0, 0) } else { (-5, 0) }
        }
        Action::Start => {
            let Ok(path) = core::str::from_utf8(&req.path[..req.path_len]) else { return (-22, 0); };
            // Recheck at execution time as well as submission time.
            if !process::pid_can_delegate(req.caller, req.grant) { return (-1, 0); }
            let spawn = process::SpawnRequest { name: String::from(name), uid: 0, parent: req.caller,
                caps: req.grant, cwd: String::from("/userdata"), image_path: String::from(path) };
            let pid = match process::spawn_managed(&spawn) {
                Ok(pid) => pid,
                Err(ProcessError::AlreadyManaged(pid)) => return (-17, pid),
                Err(_) => return (-12, 0),
            };
            if process::exec_private(pid).is_err() { let _ = process::kill(pid, -1); let _ = process::reap(pid); return (-5, 0); }
            // If the submitter died while the filesystem operation was in
            // progress, do not leave a newly started service orphaned.
            if !process::pid_has_capability(req.caller, process::cap::MANAGE_SERVICES)
                || !process::pid_can_delegate(req.caller, req.grant) {
                let _ = process::kill(pid, -1); let _ = process::reap(pid); return (-1, 0);
            }
            (0, pid)
        }
        Action::Stop => {
            let Some(managed) = process::managed_process(name) else { return (-2, 0); };
            if managed.pid != req.target || managed.pid == req.caller { return (-2, 0); }
            if process::kill(req.target, 0).is_err() { return (-2, 0); }
            let _ = process::reap(req.target);
            (0, req.target)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(caller: u32) -> Request { Request { token: 0, caller, state: State::Pending, action: Action::Stop,
        name: [0; MAX_NAME], name_len: 0, path: [0; MAX_PATH], path_len: 0, target: 3, grant: 0, result: 0, result_pid: 0 } }
    #[test] fn queue_enforces_ownership_backpressure_and_one_shot_completion() {
        let mut q = Queue::new();
        let a = q.submit(sample(1)).unwrap();
        assert_eq!(q.take(2, a), Err(-2)); assert_eq!(q.take(1, a), Err(-11));
        for _ in 1..PER_CALLER { q.submit(sample(1)).unwrap(); }
        assert_eq!(q.submit(sample(1)), Err(-11));
        let claimed = q.claim().unwrap(); assert_eq!(claimed.token, a);
        q.complete(a, -5, 7);
        assert_eq!(q.take(1, a), Ok((-5, 7))); assert_eq!(q.take(1, a), Err(-2));
        q.purge(1); assert!(q.slots.iter().all(Option::is_none));
    }
    #[test] fn validates_log_messages() {
        assert!(valid_log("служба запущена".as_bytes()));
        assert!(!valid_log(b""));
        assert!(!valid_log(b"first\nforged"));
        assert!(!valid_log(&[0xff]));
        assert!(!valid_log(&[b'x'; 97]));
    }
    #[test] fn validates_service_names_and_paths() {
        assert!(valid_name(b"network")); assert!(!valid_name(b"  "));
        assert!(!valid_name(b"x/y")); assert!(!valid_name(&[0xff]));
        assert!(valid_path(b"/system/services/net.elf"));
        for path in [b"/system/services/../kernel/x.elf".as_slice(), b"/system/services//a.elf", b"/userdata/a.elf", b"/system/services/a.mex"] {
            assert!(!valid_path(path));
        }
    }
}
