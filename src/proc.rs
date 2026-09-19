//! Модель процессов и потоков DeiX OS.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use crate::spinlock::SpinLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThreadState {
    Ready,
    Running,
    Sleeping,
    Blocked,
    Finished,
}

pub struct Process {
    pub pid: u32,
    pub pml4: u64,
    pub threads: Vec<u32>,
    pub parent: u32,
    pub uid: u32,
    pub cwd: String,
    pub exit_code: Option<i64>,
}

pub struct Thread {
    pub tid: u32,
    pub owner_pid: u32,
    pub rsp: u64,
    pub stack: Option<Box<[u8]>>,
    pub tls_base: u64,
    pub state: ThreadState,
}

pub struct ProcessTable {
    pub processes: Vec<Process>,
}

impl ProcessTable {
    pub fn new() -> Self {
        Self {
            processes: Vec::new(),
        }
    }

    pub fn create_process(&mut self, parent: u32, uid: u32, cwd: &str) -> u32 {
        let pid = (self.processes.len() as u32) + 100;
        self.processes.push(Process {
            pid,
            pml4: 0x100000,
            threads: Vec::new(),
            parent,
            uid,
            cwd: String::from(cwd),
            exit_code: None,
        });
        pid
    }
}

pub static PROCESS_TABLE: SpinLock<ProcessTable> = SpinLock::new(ProcessTable {
    processes: Vec::new(),
});
