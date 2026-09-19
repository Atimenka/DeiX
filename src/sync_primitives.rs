//! Блокирующие примитивы синхронизации для потоков (Mutex, RwLock, Semaphore, Condvar).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use crate::spinlock::SpinLock;

pub struct Mutex {
    owner: AtomicU32,
    waiters: SpinLock<Vec<u32>>,
}

impl Mutex {
    pub const fn new() -> Self {
        Self {
            owner: AtomicU32::new(0),
            waiters: SpinLock::new(Vec::new()),
        }
    }

    pub fn lock(&self) {
        let cur_pid = crate::sched::current_pid();
        while self.owner.compare_exchange(0, cur_pid, Ordering::Acquire, Ordering::Relaxed).is_err() {
            {
                let mut w = self.waiters.lock();
                if !w.contains(&cur_pid) {
                    w.push(cur_pid);
                }
            }
            crate::sched::yield_now();
        }
    }

    pub fn try_lock(&self) -> bool {
        let cur_pid = crate::sched::current_pid();
        self.owner.compare_exchange(0, cur_pid, Ordering::Acquire, Ordering::Relaxed).is_ok()
    }

    pub fn unlock(&self) {
        let cur_pid = crate::sched::current_pid();
        if self.owner.compare_exchange(cur_pid, 0, Ordering::Release, Ordering::Relaxed).is_ok() {
            let mut w = self.waiters.lock();
            if let Some(next_pid) = w.pop() {
                crate::sched::set_state(next_pid, crate::sched::State::Ready);
            }
        }
    }
}

pub struct RwLock {
    readers: AtomicU32,
    writer: AtomicU32,
    waiters: SpinLock<Vec<u32>>,
}

impl RwLock {
    pub const fn new() -> Self {
        Self {
            readers: AtomicU32::new(0),
            writer: AtomicU32::new(0),
            waiters: SpinLock::new(Vec::new()),
        }
    }

    pub fn read_lock(&self) {
        while self.writer.load(Ordering::Relaxed) != 0 {
            crate::sched::yield_now();
        }
        self.readers.fetch_add(1, Ordering::Acquire);
    }

    pub fn read_unlock(&self) {
        self.readers.fetch_sub(1, Ordering::Release);
    }

    pub fn write_lock(&self) {
        let cur_pid = crate::sched::current_pid();
        while self.writer.compare_exchange(0, cur_pid, Ordering::Acquire, Ordering::Relaxed).is_err()
            || self.readers.load(Ordering::Relaxed) != 0
        {
            crate::sched::yield_now();
        }
    }

    pub fn write_unlock(&self) {
        let cur_pid = crate::sched::current_pid();
        self.writer.compare_exchange(cur_pid, 0, Ordering::Release, Ordering::Relaxed).ok();
    }
}

pub struct Semaphore {
    count: AtomicI32,
    waiters: SpinLock<Vec<u32>>,
}

impl Semaphore {
    pub const fn new(initial: i32) -> Self {
        Self {
            count: AtomicI32::new(initial),
            waiters: SpinLock::new(Vec::new()),
        }
    }

    pub fn wait(&self) {
        while self.count.fetch_sub(1, Ordering::Acquire) <= 0 {
            crate::sched::yield_now();
        }
    }

    pub fn post(&self) {
        self.count.fetch_add(1, Ordering::Release);
    }
}

pub struct Condvar {
    waiters: SpinLock<Vec<(u32, usize)>>,
}

impl Condvar {
    pub const fn new() -> Self {
        Self {
            waiters: SpinLock::new(Vec::new()),
        }
    }

    pub fn wait(&self, mutex: &Mutex) {
        let pid = crate::sched::current_pid();
        {
            let mut w = self.waiters.lock();
            w.push((pid, mutex as *const _ as usize));
        }
        mutex.unlock();
        crate::sched::yield_now();
        mutex.lock();
    }

    pub fn notify_one(&self) {
        let mut w = self.waiters.lock();
        if let Some((pid, _)) = w.pop() {
            crate::sched::set_state(pid, crate::sched::State::Ready);
        }
    }
}
